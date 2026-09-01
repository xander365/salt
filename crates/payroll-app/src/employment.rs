//! `CreateEmployment`, `VoidEmployment`, and reading an Employment back as
//! the `EmploymentSnapshot` the pure crate accepts (§4.0, §4.3, §12).

use chrono::NaiveDate;
use payroll::{
    CompensationTerms, EmployerId, EmploymentId, EmploymentSnapshot, Money, PersonId,
    PersonReference,
};
use sqlx::{Acquire, Postgres};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::ids::new_id;

/// Records a new Employment against an Employer and a Person.
///
/// Two refusals are domain refusals rather than database errors, because
/// both state a fact about the Employment being described: an `end_date`
/// before the `start_date`, and an Employer that does not exist. The
/// Employer is checked inside the insert itself rather than by a preceding
/// `SELECT`, so there is no window between the two statements.
pub async fn create_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person_id: &PersonId,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    created_by: &str,
) -> Result<EmploymentId, PayrollAppError> {
    let pool = db.pool();
    if let Some(end_date) = end_date
        && end_date < start_date
    {
        return Err(PayrollAppError::EmploymentEndsBeforeItStarts {
            start_date,
            end_date,
        });
    }

    let id = EmploymentId::new(new_id());

    let inserted = sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, end_date, created_by)
         SELECT $1, $2, $3, $4, $5, $6
         WHERE EXISTS (SELECT 1 FROM employer WHERE id = $2)",
    )
    .bind(id.as_str())
    .bind(employer_id.as_str())
    .bind(person_id.as_str())
    .bind(start_date)
    .bind(end_date)
    .bind(created_by)
    .execute(pool)
    .await?;

    if inserted.rows_affected() == 0 {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    }

    Ok(id)
}

/// Marks an Employment void: never a physical delete (§4.3 — "delete when
/// nothing references it" is a racing check against a moving target). Writes
/// the `EmploymentVoided` ActionLog entry in the same transaction as the
/// void itself, so a committed void with no audit entry is not a reachable
/// state (§10).
///
/// Voiding an already-void Employment is refused rather than repeated. The
/// void has already happened, and a second `EmploymentVoided` entry would
/// record an act that did not — in a log no role may afterwards correct.
pub async fn void_employment(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    actor: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // The `is_void = FALSE` predicate is what makes two concurrent voids
    // resolve to one: the second blocks on the first's row lock, then
    // matches nothing.
    let employer_id: Option<String> = sqlx::query_scalar(
        "UPDATE employment SET is_void = TRUE
         WHERE id = $1 AND is_void = FALSE
         RETURNING employer_id",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;

    let Some(employer_id) = employer_id else {
        let exists: Option<bool> =
            sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1")
                .bind(employment_id.as_str())
                .fetch_optional(&mut *tx)
                .await?;
        return Err(match exists {
            Some(_) => PayrollAppError::EmploymentIsVoid(employment_id.clone()),
            None => PayrollAppError::EmploymentNotFound(employment_id.clone()),
        });
    };
    let employer_id = EmployerId::new(employer_id);

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor,
            action_type: ActionType::EmploymentVoided,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Reads an Employment back as an `EmploymentSnapshot`, together with
/// whichever `CompensationTerms` row is in force `as_of` that date: the one
/// with the latest `effective_from` on or before it. Its `effective_until`
/// is derived, never stored (§4.4) — the day before the next row's
/// `effective_from`, or `None` when no later row exists yet.
///
/// One statement, not three. The row in force and the row that ends it are
/// two halves of one fact, so reading them separately would let a
/// `CompensationTerms` row written in between produce a snapshot whose
/// `effective_until` never described the row it was attached to.
///
/// A void Employment is refused: this snapshot is a calculation input, and
/// §4.3 keeps a voided Employment out of every payroll.
///
/// Public entry point over the opaque [`SaltDatabase`] handle. The
/// generic connection-taking implementation is `get_employment_snapshot_on`,
/// used internally by callers — `calculate.rs` among them — that already
/// hold a transaction and need this read on that same connection.
pub async fn get_employment_snapshot(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentSnapshot, PayrollAppError> {
    get_employment_snapshot_on(db.pool(), employment_id, as_of).await
}

/// Takes anything a connection can be acquired from — a `&PgPool` for a
/// standalone read, or a `&mut Transaction` so a caller assembling several
/// facts at once reads them all on the one connection, inside its own
/// transaction and under whatever lock it already holds.
pub(crate) async fn get_employment_snapshot_on<'a>(
    conn: impl Acquire<'a, Database = Postgres>,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentSnapshot, PayrollAppError> {
    let mut conn = conn.acquire().await?;

    type SnapshotRow = (
        String,
        String,
        NaiveDate,
        Option<NaiveDate>,
        bool,
        Option<NaiveDate>,
        Option<i64>,
        Option<NaiveDate>,
    );

    let row: Option<SnapshotRow> = sqlx::query_as(
        "SELECT employment.employer_id,
                employment.person_id,
                employment.start_date,
                employment.end_date,
                employment.is_void,
                in_force.effective_from,
                in_force.basic_pay,
                (SELECT MIN(later.effective_from)
                 FROM compensation_terms later
                 WHERE later.employment_id = employment.id
                   AND later.effective_from > in_force.effective_from)
         FROM employment
         LEFT JOIN LATERAL (
             SELECT effective_from, basic_pay
             FROM compensation_terms
             WHERE employment_id = employment.id AND effective_from <= $2
             ORDER BY effective_from DESC
             LIMIT 1
         ) AS in_force ON TRUE
         WHERE employment.id = $1",
    )
    .bind(employment_id.as_str())
    .bind(as_of)
    .fetch_optional(&mut *conn)
    .await?;

    let (employer_id, person_id, start_date, end_date, is_void, effective_from, basic_pay, next) =
        row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;

    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    let (effective_from, basic_pay_cents) = effective_from
        .zip(basic_pay)
        .ok_or_else(|| PayrollAppError::NoCompensationTermsInForce(employment_id.clone()))?;

    let effective_until = next.map(|next| {
        next.pred_opt()
            .expect("a later effective_from is never the earliest representable date")
    });

    let basic_pay = Money::from_cents(basic_pay_cents).expect(
        "compensation_terms.basic_pay CHECK: the column holds no negative amount, so it is a Money",
    );
    let compensation_terms = CompensationTerms::new(effective_from, effective_until, basic_pay)
        .expect("effective_until, when present, is a later effective_from minus one day");

    Ok(EmploymentSnapshot::new(
        employment_id.clone(),
        EmployerId::new(employer_id),
        PersonReference::new(PersonId::new(person_id)),
        start_date,
        end_date,
        compensation_terms,
    ))
}
