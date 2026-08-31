//! `DeclareUnsupportedDeductionStatus` and the read that resolves which
//! declaration governs a period (§4.5c, §12).

use chrono::NaiveDate;
use payroll::{
    EmploymentId, UnsupportedDeductionKinds, UnsupportedDeductionStatus,
    validate_effective_from_is_a_period_start,
};
use sqlx::{Acquire, PgPool, Postgres};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;

/// Records the `UnsupportedDeductionDeclaration` in force from
/// `effective_from`, or replaces whichever one already governs that exact
/// date. Effective-dated, unlike `PriorEmployment` (§4.5b): this genuinely
/// changes mid-year — an employee joining a provident fund in August is
/// exactly the case §4.5c names.
///
/// `effective_from` must be one of the Employer's own `PayPeriod` start
/// dates (INV-014's shape, applied here so exactly one row ever governs a
/// period), checked in Rust before the row is written rather than left to
/// database arithmetic.
///
/// `status` must be `ConfirmedNone` or `Present`: the row itself is
/// two-valued, and `Unknown` is what no row in force at a period end
/// already means (§4.5c), so it is refused here rather than written. An
/// empty `kinds` collection is not representable in
/// `UnsupportedDeductionKinds`, so `Present` can never collapse into
/// `ConfirmedNone` by accident.
///
/// This writes no pre-finalization gate: `calculate` already refuses
/// `Unknown`, so a period with no declaration in force can never reach
/// `Calculated` (§4.5c).
pub async fn declare_unsupported_deduction_status(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    status: UnsupportedDeductionStatus,
    declared_by: &str,
) -> Result<(), PayrollAppError> {
    let (db_status, kinds) = match status {
        UnsupportedDeductionStatus::Unknown => {
            return Err(PayrollAppError::UnsupportedDeductionDeclarationCannotBeUnknown);
        }
        UnsupportedDeductionStatus::ConfirmedNone => ("confirmed_none", None),
        UnsupportedDeductionStatus::Present(kinds) => (
            "present",
            Some(
                serde_json::to_value(&kinds)
                    .expect("UnsupportedDeductionKinds serializes to a JSON array"),
            ),
        ),
    };

    let mut tx = pool.begin().await?;

    // The Employer row is locked first, and `FOR SHARE` is what makes the
    // `effective_from` guard below hold: `change_pay_schedule` takes `FOR
    // UPDATE` on that row and reads this table looking for an
    // `effective_from` its new schedule would strand, so without the lock a
    // schedule change and this write neither conflict nor see each other,
    // and both commit — leaving a declaration governing no period the
    // schedule generates.
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // `FOR SHARE OF employment` holds the row against a concurrent
    // `void_employment`, so a void committing between this read and the
    // insert cannot leave a declaration recorded against a now-voided
    // Employment.
    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR SHARE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let is_void =
        is_void.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    validate_effective_from_is_a_period_start(schedule, effective_from)?;

    sqlx::query(
        "INSERT INTO unsupported_deduction_declaration
            (employment_id, effective_from, status, kinds, declared_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (employment_id, effective_from) DO UPDATE
         SET status = EXCLUDED.status,
             kinds = EXCLUDED.kinds,
             declared_at = now(),
             declared_by = EXCLUDED.declared_by",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .bind(db_status)
    .bind(kinds)
    .bind(declared_by)
    .execute(&mut *tx)
    .await?;

    // Unlike `PriorEmployment`, the ActionType catalogue names only one act
    // for this declaration (`UnsupportedDeductionStatusCorrected`): every
    // write against a period — first declaration or later change — is a
    // correction to what governs that period, not a distinct kind of act.
    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: declared_by,
            action_type: ActionType::UnsupportedDeductionStatusCorrected,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Reads back the `UnsupportedDeductionStatus` **in force** at `as_of`:
/// the row with the latest `effective_from` on or before that date.
///
/// `as_of` is a `PayPeriod` end date at every real call site, and because
/// every `effective_from` is pinned to a `PayPeriod` start, exactly one
/// row governs any given period (§4.5c) — the resolution is a `LIMIT 1`
/// rather than an overlap search.
///
/// **No row in force means `Unknown`** (§4.5c), returned as the pure
/// crate's own third value for the same reason `get_prior_employment`
/// does: one spelling of silence, never two.
///
/// A missing or void Employment is refused rather than answered
/// `Unknown`, exactly as in `get_prior_employment`.
///
/// Takes anything a connection can be acquired from — a `&PgPool` for a
/// standalone read, or a `&mut Transaction` so a caller assembling several
/// facts at once reads them all on the one connection, inside its own
/// transaction and under whatever lock it already holds.
pub async fn get_unsupported_deduction_status<'a>(
    conn: impl Acquire<'a, Database = Postgres>,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<UnsupportedDeductionStatus, PayrollAppError> {
    let mut conn = conn.acquire().await?;

    type DeclarationRow = (bool, Option<String>, Option<serde_json::Value>);

    let row: Option<DeclarationRow> = sqlx::query_as(
        "SELECT employment.is_void,
                in_force.status,
                in_force.kinds
         FROM employment
         LEFT JOIN LATERAL (
             SELECT status, kinds
             FROM unsupported_deduction_declaration
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

    let (is_void, status, kinds) =
        row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    let Some(status) = status else {
        return Ok(UnsupportedDeductionStatus::Unknown);
    };

    Ok(match status.as_str() {
        "confirmed_none" => UnsupportedDeductionStatus::ConfirmedNone,
        "present" => {
            let kinds = kinds
                .expect("unsupported_deduction_declaration CHECK: a 'present' row names kinds");
            // The CHECK guarantees a non-empty JSON array; it cannot
            // guarantee every element is one of the four kinds this build
            // knows, so a row written outside Rust is a stored-data fault
            // and not a domain refusal.
            let kinds: UnsupportedDeductionKinds =
                serde_json::from_value(kinds).map_err(|err| {
                    PayrollAppError::Database(format!(
                        "unsupported_deduction_declaration.kinds is not an UnsupportedDeductionKinds: {err}"
                    ))
                })?;
            UnsupportedDeductionStatus::Present(kinds)
        }
        other => unreachable!(
            "unsupported_deduction_declaration.status CHECK admits only 'confirmed_none' and 'present', not {other}"
        ),
    })
}
