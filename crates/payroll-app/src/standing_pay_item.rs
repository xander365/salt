//! `StandingPayItem` (issue #79, parent #70 §0): an Earning or Deduction an
//! Employment carries with an effective-from date, which
//! [`crate::create_ordinary_payroll_run`] proposes on its own for every new
//! Ordinary run. This module owns creating and ending one; the proposal
//! itself lives in `payroll_run.rs`, reading [`standing_pay_lines_in_force`]
//! under the same transaction and Employer lock that run creation already
//! takes.

use chrono::NaiveDate;
use payroll::{
    EarningInstruction, EmployerId, EmploymentId, Money, VoluntaryDeductionInstruction,
    validate_effective_from_is_a_period_start,
};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::ids::app_id;
use crate::payroll_run::PayLineInstruction;

app_id! {
    /// `payroll-app`'s own id (§4.1): a native UUID, minted only by
    /// [`create_standing_pay_item`], where the row itself is inserted.
    StandingPayItemId
}

/// What a `StandingPayItem` may hold — narrower than [`PayLineInstruction`]
/// by design: `EarningInstruction::Overtime` is hours for one period, never
/// a recurring fact, so it is not constructible here at all rather than
/// refused at a write boundary (§0's own words for `StandingPayItem`, "An
/// Earning or Deduction").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandingPayItemInstruction {
    TaxableAllowance {
        amount: Money,
        label: Option<payroll::EarningLabel>,
    },
    MedicalAidPremium(Money),
}

impl StandingPayItemInstruction {
    fn into_pay_line_instruction(self) -> PayLineInstruction {
        match self {
            Self::TaxableAllowance { amount, label } => {
                PayLineInstruction::Earning(EarningInstruction::TaxableAllowance { amount, label })
            }
            Self::MedicalAidPremium(amount) => PayLineInstruction::Deduction(
                VoluntaryDeductionInstruction::MedicalAidPremium(amount),
            ),
        }
    }
}

/// Records a `StandingPayItem` in force on `employment_id` from
/// `effective_from`. Every new Ordinary run whose PayPeriod ends on or after
/// that date proposes it (§0) — this call itself proposes nothing, since
/// only run creation holds the Employer lock a consistent proposal needs.
///
/// `effective_from` must be one of the Employer's own `PayPeriod` start
/// dates — the same demand INV-014 makes of a `CompensationTerms` start and
/// the coverage-start guard makes of a `SaltCoverageStart` — checked in Rust
/// before the row is written, using the pure crate's own
/// `validate_effective_from_is_a_period_start` so the refusal is worded
/// exactly as it already is for those two facts.
pub async fn create_standing_pay_item(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    instruction: StandingPayItemInstruction,
    effective_from: NaiveDate,
    created_by: &str,
) -> Result<StandingPayItemId, PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // Locks the governing Employer row `FOR SHARE`, the same guard
    // `declare_unsupported_deduction_status` takes against a concurrent
    // `change_pay_schedule` stranding this `effective_from` outside the
    // schedule it was validated against.
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // Excludes a concurrent `void_employment`, the same lock
    // `declare_unsupported_deduction_status` takes before writing its own
    // effective-dated fact.
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

    let pay_line_instruction = instruction.into_pay_line_instruction();
    let pay_line_json =
        serde_json::to_value(&pay_line_instruction).expect("PayLineInstruction always serializes");

    let id: String = sqlx::query_scalar(
        "INSERT INTO standing_pay_item
            (employment_id, pay_line_json, effective_from, created_by)
         VALUES ($1, $2, $3, $4)
         RETURNING id::text",
    )
    .bind(employment_id.as_str())
    .bind(&pay_line_json)
    .bind(effective_from)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    let standing_pay_item_id = StandingPayItemId::new(id);

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: created_by,
            action_type: ActionType::StandingPayItemCreated,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: Some(serde_json::json!({
                "standing_pay_item_id": standing_pay_item_id.as_str(),
                "pay_line_json": pay_line_json,
                "effective_from": effective_from,
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(standing_pay_item_id)
}

/// Ends `standing_pay_item_id`: no future Ordinary run proposes it again.
/// Recorded by `ended_at`/`ended_by`/`ended_reason`, never a delete — a run
/// already proposed from this item still names it, and a historical
/// proposal must always have something to point at.
///
/// Demands a non-empty `reason`, refused before anything is read, the same
/// discipline every other reasoned act in this crate applies
/// (`RemovalReasonCannotBeEmpty`, `ReversalReasonCannotBeEmpty`).
pub async fn end_standing_pay_item(
    db: &SaltDatabase,
    standing_pay_item_id: &StandingPayItemId,
    reason: &str,
    ended_by: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::StandingPayItemEndReasonCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    type ItemRow = (String, String, Option<chrono::DateTime<chrono::Utc>>);
    let row: Option<ItemRow> = sqlx::query_as(
        "SELECT standing_pay_item.employment_id, employment.employer_id, standing_pay_item.ended_at
         FROM standing_pay_item
         JOIN employment ON employment.id = standing_pay_item.employment_id
         WHERE standing_pay_item.id = $1::uuid
         FOR UPDATE OF standing_pay_item",
    )
    .bind(standing_pay_item_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((employment_id, employer_id, ended_at)) = row else {
        return Err(PayrollAppError::StandingPayItemNotFound(
            standing_pay_item_id.clone(),
        ));
    };
    if ended_at.is_some() {
        return Err(PayrollAppError::StandingPayItemAlreadyEnded(
            standing_pay_item_id.clone(),
        ));
    }

    sqlx::query(
        "UPDATE standing_pay_item
         SET ended_at = now(), ended_by = $2, ended_reason = $3
         WHERE id = $1::uuid",
    )
    .bind(standing_pay_item_id.as_str())
    .bind(ended_by)
    .bind(reason)
    .execute(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &EmployerId::new(employer_id),
            actor: ended_by,
            action_type: ActionType::StandingPayItemEnded,
            target_type: "employment",
            target_id: &employment_id,
            context: Some(serde_json::json!({
                "standing_pay_item_id": standing_pay_item_id.as_str(),
                "reason": reason,
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// The `StandingPayItem`s in force on `employment_id` as of `as_of` — every
/// row whose `effective_from` is on or before it and which has not been
/// ended — each carrying the exact `pay_line_json` a proposed line copies
/// verbatim. `as_of` is always a run's `PayPeriod` end (§0, ADR-0005,
/// ADR-0007): the one selector every effective-dated fact in the product
/// uses, `PayeTable`, `SscRuleset` and `TaxYear` included.
///
/// Ordered by `effective_from` then `created_at` then `id`, so a run's
/// proposed lines land in creation order rather than the random order
/// `id`'s own UUID would otherwise sort ties into.
pub(crate) async fn standing_pay_lines_in_force(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employment_id: &EmploymentId,
    as_of: NaiveDate,
) -> Result<Vec<(StandingPayItemId, serde_json::Value, NaiveDate)>, PayrollAppError> {
    let rows: Vec<(String, serde_json::Value, NaiveDate)> = sqlx::query_as(
        "SELECT id::text, pay_line_json, effective_from
         FROM standing_pay_item
         WHERE employment_id = $1 AND effective_from <= $2 AND ended_at IS NULL
         ORDER BY effective_from, created_at, id",
    )
    .bind(employment_id.as_str())
    .bind(as_of)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, pay_line_json, effective_from)| {
            (StandingPayItemId::new(id), pay_line_json, effective_from)
        })
        .collect())
}
