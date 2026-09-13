//! The three routes issue #79 adds (parent #70 §D-10): `GET` and `POST
//! .../employments/{em}/standing-pay-items` and `POST
//! .../standing-pay-items/{s}/end`. An Operator records a standing taxable
//! allowance or a standing medical aid premium on an Employment from a
//! PayPeriod start, sees every item it has ever carried, and ends one with a
//! reason. Every new Ordinary run then proposes the items in force — that is
//! `payroll_app::create_ordinary_payroll_run`'s own work, not these routes'.
//!
//! Like [`crate::employment_facts`], each handler authorizes through
//! [`AuthorizedEmployerContext`], confirms the Employment belongs to that
//! Employer before any use case runs (ADR-0017), validates transport shape
//! only, and never validates a payroll rule itself: an effective-from date
//! that is not a PayPeriod start is refused by `payroll_app`, under the same
//! `effective_from_not_a_period_start` code `compensation-terms` answers.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::{DateTime, NaiveDate, Utc};
use payroll::Money;
use payroll_app::{StandingPayItem, StandingPayItemInstruction};
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::{RecordedResponse, authorized_employment};
use crate::error::ApiError;
use crate::payroll_runs::parse_label;
use crate::state::AppState;

/// A standing item's instruction on the wire, tagged by `kind` — the same
/// spellings a run's pay lines use, so a standing allowance and the line it
/// proposes read identically:
///
/// - `{"kind": "taxableAllowance", "amountCents": 50000, "label": "standby"}`
/// - `{"kind": "medicalAidPremium", "amountCents": 75000}`
///
/// Overtime has no spelling here: it is hours for one period, never a
/// standing fact.
///
/// `Deserialize` since issue #80: `payroll_runs.rs`'s override handler reads
/// the request body's `"line"` field straight into this same type, rather
/// than a parallel request-only enum — one shape, read and written both
/// ways.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum StandingPayItemInstructionDto {
    #[serde(rename_all = "camelCase")]
    TaxableAllowance {
        amount_cents: i64,
        label: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    MedicalAidPremium { amount_cents: i64 },
}

/// `pub(crate)` since issue #80: `payroll_runs.rs`'s override handler parses
/// the same `{"kind": ..., ...}` shape for the line it overrides a standing
/// item to, and must refuse an overtime `kind` (D14 has no standing
/// spelling for it) exactly as this route already does.
pub(crate) fn parse_instruction(
    instruction: StandingPayItemInstructionDto,
) -> Result<StandingPayItemInstruction, ApiError> {
    match instruction {
        StandingPayItemInstructionDto::TaxableAllowance {
            amount_cents,
            label,
        } => Ok(StandingPayItemInstruction::TaxableAllowance {
            amount: Money::from_cents(amount_cents).map_err(|_| ApiError::malformed_request())?,
            label: parse_label(label)?,
        }),
        StandingPayItemInstructionDto::MedicalAidPremium { amount_cents } => {
            Ok(StandingPayItemInstruction::MedicalAidPremium(
                Money::from_cents(amount_cents).map_err(|_| ApiError::malformed_request())?,
            ))
        }
    }
}

/// `pub(crate)` since issue #80: `payroll_runs.rs` reuses this to render a
/// `StandingItemProposal`'s own instruction — the change banner, the refresh
/// report, and a standing pay line's `standingPayLine` all show the item's
/// current instruction in exactly this shape.
pub(crate) fn instruction_to_dto(
    instruction: StandingPayItemInstruction,
) -> StandingPayItemInstructionDto {
    match instruction {
        StandingPayItemInstruction::TaxableAllowance { amount, label } => {
            StandingPayItemInstructionDto::TaxableAllowance {
                amount_cents: amount.cents(),
                label: Some(label.as_str().to_string()),
            }
        }
        StandingPayItemInstruction::MedicalAidPremium(amount) => {
            StandingPayItemInstructionDto::MedicalAidPremium {
                amount_cents: amount.cents(),
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StandingPayItemEndingDto {
    ended_at: DateTime<Utc>,
    ended_by: String,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StandingPayItemDto {
    standing_pay_item_id: String,
    #[serde(flatten)]
    instruction: StandingPayItemInstructionDto,
    effective_from: NaiveDate,
    created_at: DateTime<Utc>,
    created_by: String,
    /// `null` while the item is in force.
    ended: Option<StandingPayItemEndingDto>,
}

fn item_to_dto(item: StandingPayItem) -> StandingPayItemDto {
    StandingPayItemDto {
        standing_pay_item_id: item.id.to_string(),
        instruction: instruction_to_dto(item.instruction),
        effective_from: item.effective_from,
        created_at: item.created_at,
        created_by: item.created_by,
        ended: item.ended.map(|ended| StandingPayItemEndingDto {
            ended_at: ended.ended_at,
            ended_by: ended.ended_by,
            reason: ended.reason,
        }),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StandingPayItemsResponse {
    standing_pay_items: Vec<StandingPayItemDto>,
}

/// `GET /api/employers/{e}/employments/{em}/standing-pay-items`: every item
/// the Employment has carried, ended ones included.
pub(crate) async fn list_standing_pay_items(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
) -> Result<Json<StandingPayItemsResponse>, ApiError> {
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;

    let items = payroll_app::list_standing_pay_items(state.db(), &employment_id).await?;

    Ok(Json(StandingPayItemsResponse {
        standing_pay_items: items.into_iter().map(item_to_dto).collect(),
    }))
}

/// The create body: one instruction, tagged by `kind`, with its
/// `effectiveFrom` beside it. Spelled as its own tagged enum rather than a
/// struct flattening [`StandingPayItemInstructionDto`]: `#[serde(flatten)]`
/// deserializes through a buffer that can hold floats, which this workspace
/// refuses anywhere near money.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum CreateStandingPayItemRequest {
    #[serde(rename_all = "camelCase")]
    TaxableAllowance {
        effective_from: NaiveDate,
        amount_cents: i64,
        label: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    MedicalAidPremium {
        effective_from: NaiveDate,
        amount_cents: i64,
    },
}

impl CreateStandingPayItemRequest {
    fn into_parts(self) -> (NaiveDate, StandingPayItemInstructionDto) {
        match self {
            Self::TaxableAllowance {
                effective_from,
                amount_cents,
                label,
            } => (
                effective_from,
                StandingPayItemInstructionDto::TaxableAllowance {
                    amount_cents,
                    label,
                },
            ),
            Self::MedicalAidPremium {
                effective_from,
                amount_cents,
            } => (
                effective_from,
                StandingPayItemInstructionDto::MedicalAidPremium { amount_cents },
            ),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedStandingPayItemResponse {
    standing_pay_item_id: String,
}

/// `POST /api/employers/{e}/employments/{em}/standing-pay-items`. An
/// allowance must carry a non-blank label, the same demand `PUT .../pay-lines`
/// makes of every new allowance line (§D-2).
pub(crate) async fn create_standing_pay_item(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
    body: Result<Json<CreateStandingPayItemRequest>, JsonRejection>,
) -> Result<Json<CreatedStandingPayItemResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;
    let (effective_from, instruction) = request.into_parts();
    let instruction = parse_instruction(instruction)?;

    let standing_pay_item_id = payroll_app::create_standing_pay_item(
        state.db(),
        &employment_id,
        instruction,
        effective_from,
        &context.actor(),
    )
    .await?;

    Ok(Json(CreatedStandingPayItemResponse {
        standing_pay_item_id: standing_pay_item_id.to_string(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EndStandingPayItemRequest {
    #[serde(default)]
    reason: String,
}

/// `POST /api/employers/{e}/employments/{em}/standing-pay-items/{s}/end`.
/// A missing `reason` is read as empty so the refusal is the use case's own
/// `standing_pay_item_end_reason_cannot_be_empty`, not a malformed request.
pub(crate) async fn end_standing_pay_item(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id, standing_pay_item_id)): Path<(String, String, String)>,
    body: Result<Json<EndStandingPayItemRequest>, JsonRejection>,
) -> Result<Json<RecordedResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;

    payroll_app::end_standing_pay_item(
        state.db(),
        &employment_id,
        &standing_pay_item_id,
        &request.reason,
        &context.actor(),
    )
    .await?;

    Ok(Json(RecordedResponse {}))
}
