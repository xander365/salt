//! `RecordOpeningBalance` (§4.5, ADR-0014, §12).
//!
//! Recording only, and never automatic: an `OpeningBalance` is an
//! affirmative payroll fact or it is nothing (ADR-0014). Nothing here is
//! called from `create_employment`, and nothing else in this crate writes
//! this table. Frozen once the Employment's first finalization in that
//! TaxYear has happened (ADR-0013, issue #33) — checked before any of the
//! four guards below, since a frozen row refuses regardless of what it is
//! being asked to change to.

use chrono::NaiveDate;
use payroll::{EmploymentId, Money, PayPeriod, PaySchedule, PayrollError, TaxYear};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::freeze::employment_has_a_finalization_in;

/// Records the `OpeningBalance` for one (Employment, TaxYear), or replaces
/// whichever one is already there.
///
/// Four guards (§4.5, ADR-0014), all checked in Rust — using the pure
/// crate's `PaySchedule` — before the row is written:
///
/// 1. `salt_coverage_start` must itself be a `PayPeriod` **end** date the
///    Employer's `PaySchedule` generates — the same demand INV-014 makes of
///    a `CompensationTerms` **start**.
/// 2. It must fall inside `tax_year` — restated as a schema `CHECK` in
///    migration 0022, because every later reader treats the boundary as a
///    date inside the row's own `TaxYear` (§7, §8).
/// 3. It must be on or after the Employment's first payable `PayPeriod` end
///    in `tax_year`: Salt cannot claim to have replaced a system for
///    periods in which the Employment did not exist.
/// 4. Non-zero prior figures are refused when guard 3 holds with equality —
///    an empty covered span, which is a contradiction on the face of the
///    row. Zero figures over a non-empty span are ordinary (unpaid leave,
///    nil PAYE) and are accepted.
pub async fn record_opening_balance(
    pool: &PgPool,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
    salt_coverage_start: NaiveDate,
    prior_taxable_remuneration: Money,
    prior_paye: Money,
    created_by: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = pool.begin().await?;

    // The Employer row is locked first, and `FOR SHARE` is what makes the
    // `SaltCoverageStart` guard below hold: `change_pay_schedule` takes `FOR
    // UPDATE` on that row and reads this table looking for a boundary its
    // new schedule would strand, so without the lock a schedule change and
    // this write neither conflict nor see each other, and both commit —
    // storing a SaltCoverageStart that falls mid-period, the single thing
    // user story 11 exists to prevent.
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // `FOR UPDATE OF employment` holds the row against a concurrent
    // `void_employment`, so a void committing between this read and the
    // insert cannot leave an OpeningBalance recorded against a now-voided
    // Employment.
    //
    // `FOR UPDATE` rather than `FOR SHARE` because this use case writes a
    // *frozen* fact: `finalize_payroll_run` takes `FOR SHARE` on its
    // members' `employment` rows before it reads any master data, and two
    // `FOR SHARE` locks do not conflict. Only the stronger lock makes the
    // freeze check below hold — otherwise a finalization committing between
    // the check and the upsert would leave an `OpeningBalance` edited after
    // the finalization that re-reads it.
    let employment: Option<(bool, NaiveDate)> =
        sqlx::query_as("SELECT is_void, start_date FROM employment WHERE id = $1 FOR UPDATE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let (is_void, employment_start_date) =
        employment.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    // ADR-0013: OpeningBalance is re-read into every later period's
    // YearToDateContext, so once this Employment's first FinalizedPayroll in
    // `tax_year` exists, changing it here would re-price every already-paid
    // future period while the frozen snapshots kept showing the old figure.
    // `finalized_payroll` never loses a row, so this check counts a
    // reversed FinalizedPayroll exactly as a live one.
    if employment_has_a_finalization_in(&mut tx, employment_id, tax_year).await? {
        return Err(PayrollAppError::OpeningBalanceFrozenByFinalization {
            employment_id: employment_id.clone(),
            tax_year,
        });
    }

    let coverage_period = period_containing(schedule, salt_coverage_start)?;
    if coverage_period.end() != salt_coverage_start {
        return Err(PayrollAppError::SaltCoverageStartNotAPeriodEnd {
            salt_coverage_start,
        });
    }

    if TaxYear::for_period_end(salt_coverage_start) != tax_year {
        return Err(PayrollAppError::SaltCoverageStartOutsideTaxYear {
            salt_coverage_start,
            tax_year,
        });
    }

    let first_payable_period_end =
        first_payable_period_end(schedule, tax_year, employment_start_date)?;
    if salt_coverage_start < first_payable_period_end {
        return Err(
            PayrollAppError::SaltCoverageStartBeforeEmploymentIsPayable {
                salt_coverage_start,
                first_payable_period_end,
            },
        );
    }

    let covered_span_is_empty = salt_coverage_start == first_payable_period_end;
    if covered_span_is_empty
        && (prior_taxable_remuneration != Money::ZERO || prior_paye != Money::ZERO)
    {
        return Err(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start,
            },
        );
    }

    // `xmax = 0` is true only for the row version this statement itself
    // inserted; an update leaves the prior version's `xmax` set. That is
    // what distinguishes a first record from a changed one, in the same
    // round trip as the write.
    let inserted: bool = sqlx::query_scalar(
        "INSERT INTO opening_balance
            (employment_id, tax_year, first_salt_period_end,
             prior_taxable_remuneration, prior_paye, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (employment_id, tax_year) DO UPDATE
         SET first_salt_period_end = EXCLUDED.first_salt_period_end,
             prior_taxable_remuneration = EXCLUDED.prior_taxable_remuneration,
             prior_paye = EXCLUDED.prior_paye,
             created_at = now(),
             created_by = EXCLUDED.created_by
         RETURNING (xmax = 0)",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .bind(salt_coverage_start)
    .bind(prior_taxable_remuneration.cents())
    .bind(prior_paye.cents())
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: created_by,
            action_type: if inserted {
                ActionType::OpeningBalanceCreated
            } else {
                ActionType::OpeningBalanceChanged
            },
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

fn period_containing(schedule: PaySchedule, date: NaiveDate) -> Result<PayPeriod, PayrollAppError> {
    schedule
        .period_containing(date)
        .ok_or(PayrollError::PayScheduleOutsideRepresentableCalendar { date })
        .map_err(PayrollAppError::from)
}

/// The Employment's first payable `PayPeriod` end in `tax_year` (§4.5 guard
/// 3): the later of the period containing the Employment's own start date
/// and `tax_year`'s own first period — whichever leaves fewer periods for
/// this Employment to have existed through. A continuing employee whose
/// Employment predates `tax_year` gets that TaxYear's own first period; a
/// mid-year joiner gets the period their start date actually falls in.
fn first_payable_period_end(
    schedule: PaySchedule,
    tax_year: TaxYear,
    employment_start_date: NaiveDate,
) -> Result<NaiveDate, PayrollAppError> {
    let employment_first_period_end = period_containing(schedule, employment_start_date)?.end();

    // Every TaxYear's own first period ends in its own March (ADR-0005), so
    // 1 March of `tax_year`'s starting year always lands inside it. Guard 2
    // has already matched that starting year to a real `NaiveDate`'s own
    // year, so this construction fails only at the very edge of the
    // representable calendar — where it is refused rather than panicked on,
    // exactly as `period_containing` refuses there.
    let march_first = NaiveDate::from_ymd_opt(tax_year.starting_year(), 3, 1).ok_or(
        PayrollError::PayScheduleOutsideRepresentableCalendar {
            date: employment_start_date,
        },
    )?;
    let tax_year_first_period_end = period_containing(schedule, march_first)?.end();

    Ok(employment_first_period_end.max(tax_year_first_period_end))
}
