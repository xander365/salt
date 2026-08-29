//! `CreateEmployment`, `VoidEmployment`, and reading an Employment back as
//! the `EmploymentSnapshot` the pure crate accepts (§4.0, §4.3, §12).

use chrono::NaiveDate;
use payroll::{
    CompensationTerms, EmployerId, EmploymentId, EmploymentSnapshot, Money, PersonId,
    PersonReference,
};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::error::PayrollAppError;
use crate::ids::new_id;

/// Records a new Employment against an Employer and a Person.
pub async fn create_employment(
    pool: &PgPool,
    employer_id: &EmployerId,
    person_id: &PersonId,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    created_by: &str,
) -> Result<EmploymentId, PayrollAppError> {
    let id = EmploymentId::new(new_id());

    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, end_date, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id.as_str())
    .bind(employer_id.as_str())
    .bind(person_id.as_str())
    .bind(start_date)
    .bind(end_date)
    .bind(created_by)
    .execute(pool)
    .await?;

    Ok(id)
}

/// Marks an Employment void: never a physical delete (§4.3 — "delete when
/// nothing references it" is a racing check against a moving target). Writes
/// the `EmploymentVoided` ActionLog entry in the same transaction as the
/// void itself, so a committed void with no audit entry is not a reachable
/// state (§10).
pub async fn void_employment(
    pool: &PgPool,
    employment_id: &EmploymentId,
    actor: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = pool.begin().await?;

    let employer_id: Option<String> = sqlx::query_scalar(
        "UPDATE employment SET is_void = TRUE WHERE id = $1 RETURNING employer_id",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let employer_id =
        employer_id.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
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
pub async fn get_employment_snapshot(
    pool: &PgPool,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<EmploymentSnapshot, PayrollAppError> {
    let employment_row: Option<(String, String, NaiveDate, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT employer_id, person_id, start_date, end_date FROM employment WHERE id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_optional(pool)
    .await?;
    let (employer_id, person_id, start_date, end_date) =
        employment_row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;

    let terms_row: Option<(NaiveDate, i64)> = sqlx::query_as(
        "SELECT effective_from, basic_pay FROM compensation_terms
         WHERE employment_id = $1 AND effective_from <= $2
         ORDER BY effective_from DESC
         LIMIT 1",
    )
    .bind(employment_id.as_str())
    .bind(as_of)
    .fetch_optional(pool)
    .await?;
    let (effective_from, basic_pay_cents) = terms_row
        .ok_or_else(|| PayrollAppError::NoCompensationTermsInForce(employment_id.clone()))?;

    let next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT MIN(effective_from) FROM compensation_terms
         WHERE employment_id = $1 AND effective_from > $2",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .fetch_one(pool)
    .await?;
    let effective_until = next_effective_from.map(|next| {
        next.pred_opt()
            .expect("a later effective_from is never the earliest representable date")
    });

    let basic_pay = Money::from_cents(basic_pay_cents).expect(
        "compensation_terms.basic_pay was stored from a Money value, so it is never negative",
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
