//! `DeclareUnsupportedDeductionStatus` and the read that resolves which
//! declaration governs a period (§4.5c, §12).

use chrono::NaiveDate;
use payroll::{
    EmploymentId, PayPeriod, UnsupportedDeductionKinds, UnsupportedDeductionStatus,
    validate_effective_from_is_a_period_start,
};
use sqlx::{Acquire, PgPool, Postgres};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::freeze::{
    diverging_periods_json, live_finalized_periods_in_span, require_acknowledgement_of,
};

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
///
/// Demands a non-empty `reason` (refused before anything is touched) and
/// returns every Live finalized `PayPeriod` this declaration now diverges
/// from (§6.5), which `acknowledged_diverging_periods` must name exactly —
/// the same acknowledgement `correct_compensation_terms` requires, refused
/// with [`PayrollAppError::MasterDataDivergenceNotAcknowledged`] carrying
/// the list, and pass `&[]` where nothing diverges. The list is every period
/// in `[effective_from, next_effective_from)` that already has a live
/// `FinalizedPayroll`, `next_effective_from` being whichever later row
/// already exists for this Employment, or open-ended when none does. That span is exactly what this write is about to govern,
/// whether the row at `effective_from` is new or already there: a first
/// declaration takes a span away from whatever answered `Unknown` or an
/// earlier row before it, and a later change takes it from itself. Computed
/// *before* the write, on the same connection, so the read is consistent
/// with the value about to replace it.
pub async fn declare_unsupported_deduction_status(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    status: UnsupportedDeductionStatus,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    declared_by: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::UnsupportedDeductionDeclarationReasonCannotBeEmpty);
    }

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

    // This exclusive Employment lock holds the row against a concurrent
    // `void_employment` and serializes effective-dated master-data writes.
    // A sibling declaration must not change the span after it is read for
    // this correction's divergence list.
    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR UPDATE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let is_void =
        is_void.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    validate_effective_from_is_a_period_start(schedule, effective_from)?;

    // `FOR UPDATE` holds the row (if any) against a concurrent correction of
    // the same fact, and is also this call's "before" read — `None` for a
    // first declaration at this `effective_from`, `Some` for a later change.
    let before: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT status, kinds FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from = $2
         FOR UPDATE",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .fetch_optional(&mut *tx)
    .await?;
    let before_json = before.map(|(status, kinds)| {
        serde_json::json!({
            "status": status,
            "kinds": kinds,
        })
    });

    // The same "next row's effective_from" derivation `get_employment_snapshot`
    // and `get_unsupported_deduction_status` make: whatever this write
    // decides, it governs `effective_from` up to whichever later row already
    // exists, or open-ended. Read before the write below, so it names
    // exactly the span this call is about to take over — whether from an
    // earlier row's tail (a first declaration) or from itself (a change).
    let next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT MIN(effective_from) FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from > $2",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .fetch_one(&mut *tx)
    .await?;

    let diverging_periods =
        live_finalized_periods_in_span(&mut tx, employment_id, effective_from, next_effective_from)
            .await?;

    // Checked against the list this transaction has just computed, under the
    // Employment lock, and before the write below, so a refusal leaves the
    // declaration exactly as it was.
    require_acknowledgement_of(
        employment_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

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
    .bind(kinds.clone())
    .bind(declared_by)
    .execute(&mut *tx)
    .await?;

    // Unlike `PriorEmployment`, the ActionType catalogue names only one act
    // for this declaration (`UnsupportedDeductionStatusCorrected`): every
    // write against a period — first declaration or later change — is a
    // correction to what governs that period, not a distinct kind of act.
    // The entry existing is also the record of the acknowledgement, since
    // the call is refused above unless the caller named exactly this list.
    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: declared_by,
            action_type: ActionType::UnsupportedDeductionStatusCorrected,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: Some(serde_json::json!({
                "reason": reason,
                "before": before_json,
                "after": {
                    "effective_from": effective_from,
                    "status": db_status,
                    "kinds": kinds,
                },
                "diverging_live_finalized_periods": diverging_periods_json(&diverging_periods),
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(diverging_periods)
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
