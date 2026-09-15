//! `StandingPayItem` (issue #79, parent #70 §0): an Earning or Deduction an
//! Employment carries with an effective-from date, which
//! [`crate::create_ordinary_payroll_run`] proposes on its own for every new
//! Ordinary run. This module owns creating and ending one; the proposal
//! itself lives in `payroll_run.rs`, reading [`standing_pay_lines_in_force`]
//! under the same transaction and Employer lock that run creation already
//! takes.

use chrono::{DateTime, NaiveDate, Utc};
use payroll::{
    EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money,
    VoluntaryDeductionInstruction, validate_effective_from_is_a_period_start,
};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::ids::app_id;
use crate::payroll_run::{PayLineInstruction, refuse_out_of_scope_earning_label};

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
///
/// A standing allowance always carries its label. `EarningInstruction`'s
/// label is optional only so a version-1 snapshot reads back honestly as
/// "unlabelled" (§D-2); a new item has no such history, and an unlabelled
/// proposal would be a line the worksheet refuses to resave until someone
/// types a label onto it — which would silently turn it one-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandingPayItemInstruction {
    TaxableAllowance { amount: Money, label: EarningLabel },
    MedicalAidPremium(Money),
}

impl StandingPayItemInstruction {
    /// Reads a stored `pay_line_json` back. Only this module writes that
    /// column, and only from this type, so any other shape is a broken
    /// invariant rather than a refusal.
    ///
    /// `pub(crate)` since issue #80: `run_override.rs`'s change signal reads
    /// a `StandingPayItem`'s own `pay_line_json` too, to build the
    /// `StandingItemProposal` a run's stored line is diffed against.
    pub(crate) fn from_pay_line_json(pay_line_json: serde_json::Value) -> Self {
        let instruction: PayLineInstruction = serde_json::from_value(pay_line_json)
            .expect("standing_pay_item.pay_line_json is always a PayLineInstruction");
        match instruction {
            PayLineInstruction::Earning(EarningInstruction::TaxableAllowance {
                amount,
                label: Some(label),
            }) => Self::TaxableAllowance { amount, label },
            PayLineInstruction::Deduction(VoluntaryDeductionInstruction::MedicalAidPremium(
                amount,
            )) => Self::MedicalAidPremium(amount),
            other => panic!("standing_pay_item.pay_line_json holds {other:?}, not a standing kind"),
        }
    }

    /// `pub(crate)` since issue #80: `run_override.rs`'s override use case
    /// turns an operator-stated instruction into the `PayLineInstruction` a
    /// pay line stores.
    pub(crate) fn into_pay_line_instruction(self) -> PayLineInstruction {
        match self {
            Self::TaxableAllowance { amount, label } => {
                PayLineInstruction::Earning(EarningInstruction::TaxableAllowance {
                    amount,
                    label: Some(label),
                })
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
///
/// A medical aid premium of zero is refused before anything is read: it
/// withholds nothing (§D-4), and `set_run_pay_lines` refuses the same line,
/// so proposing one would leave every run it lands on unsaveable until an
/// Operator deleted it by hand.
pub async fn create_standing_pay_item(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    instruction: StandingPayItemInstruction,
    effective_from: NaiveDate,
    created_by: &str,
) -> Result<StandingPayItemId, PayrollAppError> {
    if instruction == StandingPayItemInstruction::MedicalAidPremium(Money::ZERO) {
        return Err(PayrollAppError::StandingMedicalAidPremiumIsZero);
    }
    if let StandingPayItemInstruction::TaxableAllowance { label, .. } = &instruction {
        refuse_out_of_scope_earning_label(Some(label))?;
    }

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

/// Ends `standing_pay_item_id` on `employment_id`: no future Ordinary run
/// proposes it again.
/// Recorded by `ended_at`/`ended_by`/`ended_reason`, never a delete — a run
/// already proposed from this item still names it, and a historical
/// proposal must always have something to point at.
///
/// Demands a non-empty `reason`, refused before anything is read, the same
/// discipline every other reasoned act in this crate applies
/// (`RemovalReasonCannotBeEmpty`, `ReversalReasonCannotBeEmpty`).
///
/// An item that exists but belongs to a different Employment is refused
/// exactly like one that does not exist (ADR-0017): a caller holding one
/// Employment's authorization learns nothing about another's items. So is an
/// id that is not a UUID at all — it cannot name a row, and casting it in
/// SQL would answer a database error rather than a refusal.
///
/// Takes the id as `&str`, like
/// [`crate::verify_payroll_run_belongs_to_employer`]: this is the boundary
/// where a caller's id, read off an HTTP path, becomes the wrapped type.
pub async fn end_standing_pay_item(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    standing_pay_item_id: &str,
    reason: &str,
    ended_by: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::StandingPayItemEndReasonCannotBeEmpty);
    }
    let standing_pay_item_id = StandingPayItemId::new(standing_pay_item_id);
    if uuid::Uuid::parse_str(standing_pay_item_id.as_str()).is_err() {
        return Err(PayrollAppError::StandingPayItemNotFound(
            standing_pay_item_id,
        ));
    }

    let mut tx = db.pool().begin().await?;

    // Lock the same Employer row an Ordinary run holds `FOR UPDATE` while it
    // snapshots membership and StandingPayItems. Without this lock, an end
    // could commit between that snapshot and the run's commit, leaving a
    // newly-created run proposing an item already ended by the time it is
    // visible. The common lock makes either order a complete stated fact.
    type ItemRow = (String, Option<DateTime<Utc>>);
    let row: Option<ItemRow> = sqlx::query_as(
        "SELECT employment.employer_id, standing_pay_item.ended_at
         FROM standing_pay_item
         JOIN employment ON employment.id = standing_pay_item.employment_id
         JOIN employer ON employer.id = employment.employer_id
         WHERE standing_pay_item.id = $1::uuid AND standing_pay_item.employment_id = $2
         FOR UPDATE OF standing_pay_item, employer",
    )
    .bind(standing_pay_item_id.as_str())
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((employer_id, ended_at)) = row else {
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
            target_id: employment_id.as_str(),
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

/// How a `StandingPayItem` was ended: when, by whom and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingPayItemEnding {
    pub ended_at: DateTime<Utc>,
    pub ended_by: String,
    pub reason: String,
}

/// One `StandingPayItem` as the Employment screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingPayItem {
    pub id: StandingPayItemId,
    pub instruction: StandingPayItemInstruction,
    pub effective_from: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    /// `None` while the item is still in force from `effective_from`.
    pub ended: Option<StandingPayItemEnding>,
}

/// Every `StandingPayItem` `employment_id` has ever carried, ended ones
/// included — an ended item is history a past proposal still points at, not
/// something to hide. In the order a run proposes them: `effective_from`,
/// then `created_at`, then `id`.
///
/// Takes no `EmployerId`: like every Employment-scoped use case here, the
/// caller proves the Employment is theirs first
/// ([`crate::verify_employment_belongs_to_employer`]).
pub async fn list_standing_pay_items(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
) -> Result<Vec<StandingPayItem>, PayrollAppError> {
    type Row = (
        String,
        serde_json::Value,
        NaiveDate,
        DateTime<Utc>,
        String,
        Option<DateTime<Utc>>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id::text, pay_line_json, effective_from, created_at, created_by,
                ended_at, ended_by, ended_reason
         FROM standing_pay_item
         WHERE employment_id = $1
         ORDER BY effective_from, created_at, id",
    )
    .bind(employment_id.as_str())
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                pay_line_json,
                effective_from,
                created_at,
                created_by,
                ended_at,
                ended_by,
                reason,
            )| {
                let ended = ended_at.map(|ended_at| StandingPayItemEnding {
                    ended_at,
                    ended_by: ended_by.expect("an ended StandingPayItem names who ended it"),
                    reason: reason.expect("an ended StandingPayItem states why"),
                });
                StandingPayItem {
                    id: StandingPayItemId::new(id),
                    instruction: StandingPayItemInstruction::from_pay_line_json(pay_line_json),
                    effective_from,
                    created_at,
                    created_by,
                    ended,
                }
            },
        )
        .collect())
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
///
/// Takes a concrete `&mut PgConnection` rather than a `Transaction`, since
/// issue #80: `get_payroll_run_detail`'s change signal reads this on its own
/// read transaction, not the write transaction run creation uses, and a
/// caller passes `&mut **tx` either way.
pub(crate) async fn standing_pay_lines_in_force(
    conn: &mut sqlx::PgConnection,
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
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, pay_line_json, effective_from)| {
            (StandingPayItemId::new(id), pay_line_json, effective_from)
        })
        .collect())
}
