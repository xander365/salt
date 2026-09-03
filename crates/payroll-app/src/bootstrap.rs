//! `Bootstrap` (issue #48, parent #38 Spec 1 of 3): creates the first
//! Operator, the first Employer and its Owner `EmployerMembership` in one
//! transaction, so a fresh database is reachable without a signup page.
//! Bootstrap's Owner membership is the only membership that comes into
//! existence outside tests in Specs 1-3 (§0.39) — this module is that one
//! path, and it ships no HTTP route.
//!
//! Refuses as a whole, before any of the three inserts, once any Operator
//! already exists (§0.2). This is a first-run command, not an
//! administrative back door: it gains no `--force` flag and no "add another
//! Operator" mode.

use chrono::Duration;
use payroll::{DayOfMonth, EmployerId, PaySchedule, PeriodEndDay};

use crate::database::SaltDatabase;
use crate::employer::insert_employer;
use crate::error::PayrollAppError;
use crate::membership::{MembershipRole, insert_employer_membership};
use crate::operator::{OperatorId, insert_operator, is_lock_not_available};

/// How long [`bootstrap`] waits for its `LOCK TABLE operator IN EXCLUSIVE
/// MODE` before refusing. `EXCLUSIVE` conflicts with every lock mode except
/// `ACCESS SHARE`, and a lock request that is waiting queues *ahead* of the
/// requests made after it — so an unbounded wait here would not merely stall
/// this command, it would stall every sign-in and every session lookup of a
/// running Salt server behind it, for as long as the conflicting writer took.
/// Bounding the wait turns that into one refused command. Generous next to
/// the tens of milliseconds a competing bootstrap's own transaction costs, so
/// the legitimate race two concurrent first-runs create never reaches it.
const LOCK_WAIT_TIMEOUT: Duration = Duration::seconds(5);

/// The day of month a bootstrapped Employer's `PaySchedule` ends its periods
/// on, in the shape a caller assembles from raw, unvalidated input — a bare
/// `u8` rather than [`payroll::DayOfMonth`]. `salt-server`'s binary reaches
/// `payroll` only indirectly, through this crate (its own manifest carries
/// no dependency on it), so its CLI layer needs a way to name "a fixed day"
/// that does not require constructing a `payroll` type. [`bootstrap`] is
/// what turns a `Day` carrying an out-of-range value into
/// [`PayrollAppError::BootstrapPeriodEndDayInvalid`] rather than a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapPeriodEndDay {
    /// A fixed day. Valid only for 1..=28 — [`bootstrap`] is what checks
    /// that range, not this type's constructor, because a caller assembling
    /// this from a CLI flag has no `Result` to hand back yet.
    Day(u8),
    /// The last day of the month, whatever it is.
    LastDayOfMonth,
}

/// What [`bootstrap`] created: the ids of the first Operator and Employer,
/// bound together by the Owner `EmployerMembership` the same transaction
/// also wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOutcome {
    pub operator_id: OperatorId,
    pub employer_id: EmployerId,
}

/// Creates the first Operator, the first Employer and its Owner
/// `EmployerMembership` in one transaction (§0.2). `email`, `display_name`
/// and `password` become the Operator's own credential — the same
/// validation [`crate::create_operator`] applies, since this calls the very
/// same insert. `employer_name` and `period_end_day` become the Employer's
/// name and `PaySchedule`, validated the same way
/// [`crate::create_employer`] validates them.
///
/// `--period-end-day` is validated here, not defaulted or guessed: ADR-0005
/// makes a PaySchedule the thing every period boundary is generated from,
/// and it cannot be changed mid-tax-year, so a wrong one bootstrapped here
/// is not a display bug.
///
/// `LOCK TABLE operator IN EXCLUSIVE MODE` is taken first and held for the
/// whole transaction, before the existence check below ever runs. Without
/// it, two concurrent callers could both find zero existing Operators
/// before either commits its own insert, and both would succeed — exactly
/// the race the acceptance criteria refuse. `EXCLUSIVE` conflicts with
/// itself (it is not merely a read lock), so a second bootstrap blocks on
/// the first's lock until that transaction commits or rolls back, and then
/// re-reads a database that already holds the first's Operator row —
/// PostgreSQL's read-committed default is enough once the two are
/// serialized this way; no higher isolation level is needed. The wait for
/// that lock is bounded by `LOCK_WAIT_TIMEOUT` and answers
/// [`PayrollAppError::BootstrapOperatorTableBusy`] when it elapses, so this
/// command can never queue ahead of a running server's own reads of
/// `operator`.
pub async fn bootstrap(
    db: &SaltDatabase,
    email: &str,
    display_name: &str,
    password: &str,
    employer_name: &str,
    period_end_day: BootstrapPeriodEndDay,
) -> Result<BootstrapOutcome, PayrollAppError> {
    // Checked before any transaction is opened: a malformed period-end-day
    // is a fact about the caller's input, not the database, and refusing it
    // here means a bad value never even acquires the table lock below.
    let period_end_day = period_end_day_from_arg(period_end_day)?;
    let pay_schedule = PaySchedule::new(period_end_day);

    let mut tx = db.pool().begin().await?;

    // `SET LOCAL`, so the bound is scoped to this transaction and released
    // with it rather than left on a pooled connection for the next use case
    // to inherit. The value is a compile-time constant of this module, never
    // a caller's, which is why it is formatted into the statement — `SET`
    // takes no bind parameters. Same discipline as
    // `verify_operator_credential`'s own row-lock wait.
    sqlx::query(&format!(
        "SET LOCAL lock_timeout = {}",
        LOCK_WAIT_TIMEOUT.num_milliseconds()
    ))
    .execute(&mut *tx)
    .await?;

    // `operator` is a fixed, hard-coded table name, never caller input, so
    // this is the same discipline the statement above applies to a locking
    // clause PostgreSQL does not accept a bind parameter for.
    let locked = sqlx::query("LOCK TABLE operator IN EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await;
    match locked {
        Ok(_) => {}
        // Something else is writing `operator`, which a database with no
        // Operator in it has nothing to do. Refusing names that rather than
        // holding the queue open, and leaves nothing behind either way.
        Err(err) if is_lock_not_available(&err) => {
            return Err(PayrollAppError::BootstrapOperatorTableBusy);
        }
        Err(err) => return Err(err.into()),
    }

    let any_operator: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM operator LIMIT 1")
        .fetch_optional(&mut *tx)
        .await?;
    if any_operator.is_some() {
        return Err(PayrollAppError::BootstrapOperatorAlreadyExists);
    }

    let operator_id = insert_operator(&mut tx, email, display_name, password).await?;
    // ADR-0019: an Operator actor is always written as `operator:<id>`,
    // never a bare id or a free-text name. The Operator bootstrap creates is
    // also the one who is, in effect, doing the creating.
    let created_by = format!("operator:{operator_id}");
    let employer_id = insert_employer(&mut tx, employer_name, pay_schedule, &created_by).await?;
    insert_employer_membership(&mut tx, &operator_id, &employer_id, MembershipRole::Owner).await?;

    tx.commit().await?;

    Ok(BootstrapOutcome {
        operator_id,
        employer_id,
    })
}

fn period_end_day_from_arg(arg: BootstrapPeriodEndDay) -> Result<PeriodEndDay, PayrollAppError> {
    match arg {
        BootstrapPeriodEndDay::Day(day) => DayOfMonth::new(day)
            .map(PeriodEndDay::Day)
            .map_err(|_| PayrollAppError::BootstrapPeriodEndDayInvalid { day }),
        BootstrapPeriodEndDay::LastDayOfMonth => Ok(PeriodEndDay::LastDayOfMonth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_in_range_becomes_a_fixed_period_end_day() {
        assert_eq!(
            period_end_day_from_arg(BootstrapPeriodEndDay::Day(25)),
            Ok(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
        );
    }

    #[test]
    fn last_day_of_month_passes_through_unchanged() {
        assert_eq!(
            period_end_day_from_arg(BootstrapPeriodEndDay::LastDayOfMonth),
            Ok(PeriodEndDay::LastDayOfMonth)
        );
    }

    #[test]
    fn a_day_outside_1_to_28_is_refused() {
        assert_eq!(
            period_end_day_from_arg(BootstrapPeriodEndDay::Day(29)),
            Err(PayrollAppError::BootstrapPeriodEndDayInvalid { day: 29 })
        );
        assert_eq!(
            period_end_day_from_arg(BootstrapPeriodEndDay::Day(0)),
            Err(PayrollAppError::BootstrapPeriodEndDayInvalid { day: 0 })
        );
    }
}
