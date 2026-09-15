//! `GET /api/employers/{e}/finalized-payroll/{f}/payslip` (issue #82,
//! parent #70): an Operator downloads one employee's Payslip as a real PDF
//! from the finalized run screen. Rendered on demand, in Rust, and never
//! stored — the response carries no cache header that would let anything
//! keep a copy, and there is no document table, object store or cache for
//! this handler to have written to in the first place.
//!
//! This handler is the one seam between `payroll_app::PayslipData` (the
//! frozen read model) and [`crate::payslip_render::PayslipInput`] (the pure
//! renderer's own plain struct, Deep Instructions issue #82) — everything
//! below `to_render_input` is pure mapping, no I/O.
//!
//! Scoped by [`AuthorizedEmployerContext`] exactly as
//! [`crate::finalized_payroll::get_finalized_payroll`] is: a
//! `finalized_payroll_id` belonging to another Employer reads back as the
//! same refusal an unknown one gets (ADR-0017), and either role may
//! download a payslip — this is not an Owner-only route.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use payroll::{Deduction, Earning, EmployerId, StatutoryDeduction, VoluntaryDeduction};
use payroll_app::{FinalizedEmployerParticulars, FinalizedPersonParticulars, PayslipData};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::error::ApiError;
use crate::payslip_render::{PayslipInput, PayslipLine, PayslipReversedNotice, render_payslip};
use crate::state::AppState;

/// `GET /api/employers/{e}/finalized-payroll/{f}/payslip`. Refused exactly
/// as [`crate::finalized_payroll::get_finalized_payroll`] is when the id is
/// unknown or belongs to another Employer, and additionally refused (409,
/// `payslip_particulars_not_frozen`) when this row predates issue #73 and
/// never froze what a Payslip demands — `?` below converts both through
/// `payroll_error`'s own exhaustive mapping.
pub(crate) async fn get_finalized_payroll_payslip(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, finalized_payroll_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let data =
        payroll_app::get_payslip_data(state.db(), &employer_id, &finalized_payroll_id).await?;

    let filename = format!("payslip-{}.pdf", data.id);
    let input = to_render_input(data);
    let bytes = render_payslip(&input)
        .map_err(|err| ApiError::internal(format!("payslip render refused: {err}")))?;

    Ok(pdf_response(bytes, &filename))
}

/// Builds the PDF response: `Content-Type: application/pdf`, an attachment
/// `Content-Disposition` naming the file, and `Cache-Control: no-store` —
/// the acceptance criterion that the response itself is never cacheable, on
/// top of `salt-server` simply never writing the bytes anywhere.
fn pdf_response(bytes: Vec<u8>, filename: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pdf"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let disposition = format!("attachment; filename=\"{filename}\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        // `filename` is built from `data.id`, a UUID this crate itself
        // parsed out of the database — never attacker-controlled input —
        // so this is always a valid header value.
        HeaderValue::from_str(&disposition)
            .expect("a UUID-derived filename is always a valid header value"),
    );
    response
}

fn to_render_input(data: PayslipData) -> PayslipInput {
    let figures = data.figures;
    PayslipInput {
        finalized_payroll_id: data.id.to_string(),
        employer_registered_name: data.employer_particulars.registered_name.clone(),
        employer_address_lines: employer_address_lines(&data.employer_particulars),
        employer_income_tax_number: data.employer_particulars.income_tax_number.clone(),
        employer_social_security_number: data.employer_particulars.social_security_number,
        employee_full_name: data.person_particulars.full_name.clone(),
        employee_identity_number: data.person_particulars.identity_number.clone(),
        employee_address_lines: person_address_lines(&data.person_particulars),
        period_start: data.period.start(),
        period_end: data.period.end(),
        pay_date: data.pay_date,
        salt_version: data.salt_version,
        payslip_template_version: data.payslip_template_version,
        earnings: data.earning_lines.iter().map(earning_line).collect(),
        deductions: data.deductions.iter().map(deduction_line).collect(),
        gross_pay_cents: figures.gross.cents(),
        total_deductions_cents: figures.total_deductions.cents(),
        net_pay_cents: figures.net_pay.cents(),
        replaces: data.replaces.map(|id| id.to_string()),
        reversed: data.reversal.map(|reversal| PayslipReversedNotice {
            reason: reversal.reason,
            replacement_id: reversal.replacement_id.map(|id| id.to_string()),
        }),
    }
}

/// `EmployerParticulars`' own required `address_line1`/`city` plus its
/// optional `address_line2`/`postal_code`, laid out the way a printed
/// address block reads: street line(s), then city and postal code together.
fn employer_address_lines(particulars: &FinalizedEmployerParticulars) -> Vec<String> {
    let mut lines = vec![particulars.address_line1.clone()];
    if let Some(line2) = &particulars.address_line2 {
        lines.push(line2.clone());
    }
    let mut city_line = particulars.city.clone();
    if let Some(postal_code) = &particulars.postal_code {
        city_line.push_str(", ");
        city_line.push_str(postal_code);
    }
    lines.push(city_line);
    lines
}

/// As [`employer_address_lines`], but every field is optional on
/// `PersonParticulars` (a Person's address may never have been recorded at
/// all) — each line is included only when it has something to show.
fn person_address_lines(particulars: &FinalizedPersonParticulars) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(line1) = &particulars.address_line1 {
        lines.push(line1.clone());
    }
    if let Some(line2) = &particulars.address_line2 {
        lines.push(line2.clone());
    }
    let mut city_line = particulars.city.clone().unwrap_or_default();
    if let Some(postal_code) = &particulars.postal_code {
        if !city_line.is_empty() {
            city_line.push_str(", ");
        }
        city_line.push_str(postal_code);
    }
    if !city_line.is_empty() {
        lines.push(city_line);
    }
    lines
}

fn format_hours(hours: rust_decimal::Decimal) -> String {
    hours.round_dp(2).to_string()
}

fn format_multiplier(multiplier: rust_decimal::Decimal) -> String {
    multiplier.normalize().to_string()
}

/// One frozen `Earning` line to its printed form (Deep Instructions,
/// `Q-OPEN-7`): `BasicPay` is the wage-basis line, an allowance prints its
/// own label, and overtime prints **both** its multiplier (the
/// classification) and its free-text label — never only one.
fn earning_line(earning: &Earning) -> PayslipLine {
    match earning {
        Earning::BasicPay(amount) => PayslipLine {
            label: "Basic Pay".to_string(),
            detail: None,
            amount_cents: amount.cents(),
        },
        Earning::TaxableAllowance { amount, label } => PayslipLine {
            label: label
                .as_ref()
                .map(|label| label.as_str().to_string())
                .unwrap_or_else(|| "Allowance".to_string()),
            detail: None,
            amount_cents: amount.cents(),
        },
        Earning::Overtime {
            amount,
            trace,
            label,
        } => PayslipLine {
            label: label
                .as_ref()
                .map(|label| label.as_str().to_string())
                .unwrap_or_else(|| "Overtime".to_string()),
            detail: Some(format!(
                "{} hrs @ {}x",
                format_hours(trace.hours.as_decimal()),
                format_multiplier(trace.multiplier.as_decimal())
            )),
            amount_cents: amount.cents(),
        },
    }
}

fn deduction_line(deduction: &Deduction) -> PayslipLine {
    let (label, amount) = match deduction {
        Deduction::Statutory(StatutoryDeduction::PAYE(amount)) => ("PAYE", *amount),
        Deduction::Statutory(StatutoryDeduction::SocialSecurity(amount)) => {
            ("Social Security", *amount)
        }
        Deduction::Voluntary(VoluntaryDeduction::MedicalAidPremium { amount, .. }) => {
            ("Medical Aid Premium", *amount)
        }
    };
    PayslipLine {
        label: label.to_string(),
        detail: None,
        amount_cents: amount.cents(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use payroll::{EarningLabel, Money, OvertimeHours, OvertimeMultiplier};

    fn money(cents: i64) -> Money {
        Money::from_cents(cents).unwrap()
    }

    /// `DerivedHourlyRate::new` is `pub(crate)` to `payroll` (its numerator
    /// and denominator are only ever derived, never typed in) — its public
    /// `Deserialize` is the one legitimate way another crate's test builds
    /// one, the same "exact fraction" wire shape `earning.rs`'s own tests
    /// round-trip.
    fn derived_hourly_rate(numerator: i64, denominator: i64) -> payroll::DerivedHourlyRate {
        serde_json::from_value(serde_json::json!({
            "numerator": numerator.to_string(),
            "denominator": denominator.to_string(),
        }))
        .unwrap()
    }

    fn employer_particulars() -> FinalizedEmployerParticulars {
        FinalizedEmployerParticulars {
            registered_name: "Acme Corp (Pty) Ltd".to_string(),
            address_line1: "1 Independence Ave".to_string(),
            address_line2: None,
            city: "Windhoek".to_string(),
            postal_code: Some("10001".to_string()),
            income_tax_number: Some("12345678".to_string()),
            social_security_number: None,
        }
    }

    fn person_particulars() -> FinalizedPersonParticulars {
        FinalizedPersonParticulars {
            full_name: "Ada Lovelace".to_string(),
            identity_number: Some("80012345678".to_string()),
            address_line1: None,
            address_line2: None,
            city: None,
            postal_code: None,
        }
    }

    /// A person whose address was never recorded prints no blank address
    /// lines at all — `person_address_lines` must never manufacture a line
    /// out of two `None`s.
    #[test]
    fn a_person_with_no_recorded_address_prints_no_address_lines() {
        let lines = person_address_lines(&person_particulars());
        assert_eq!(lines, Vec::<String>::new());
    }

    #[test]
    fn a_partial_person_address_still_prints_the_city_and_postal_code_together() {
        let mut particulars = person_particulars();
        particulars.city = Some("Swakopmund".to_string());
        particulars.postal_code = Some("9000".to_string());

        let lines = person_address_lines(&particulars);

        assert_eq!(lines, vec!["Swakopmund, 9000".to_string()]);
    }

    #[test]
    fn the_employer_address_always_carries_its_required_line1_and_city() {
        let lines = employer_address_lines(&employer_particulars());

        assert_eq!(
            lines,
            vec![
                "1 Independence Ave".to_string(),
                "Windhoek, 10001".to_string(),
            ]
        );
    }

    /// Overtime prints both halves Q-OPEN-7 leaves open: the multiplier
    /// (the classification) and the free-text label — never only one.
    #[test]
    fn an_overtime_line_carries_both_its_label_and_its_hours_and_multiplier_detail() {
        let overtime = Earning::Overtime {
            amount: money(124_615),
            trace: payroll::OvertimeTrace {
                basic_pay: money(1_200_000),
                ordinary_hours: payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2))
                    .unwrap(),
                months_per_year: rust_decimal::Decimal::new(12, 0),
                weeks_per_year: rust_decimal::Decimal::new(52, 0),
                derived_hourly_rate: derived_hourly_rate(900, 13),
                hours: OvertimeHours::new(rust_decimal::Decimal::new(1200, 2)).unwrap(),
                multiplier: OvertimeMultiplier::OneAndAHalf,
                policy: payroll::SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR,
            },
            label: Some(EarningLabel::new("Sunday overtime").unwrap()),
        };

        let line = earning_line(&overtime);

        assert_eq!(line.label, "Sunday overtime");
        assert_eq!(line.detail.as_deref(), Some("12.00 hrs @ 1.5x"));
    }

    /// An unlabelled overtime line still prints the multiplier detail, and
    /// falls back to the classification's own name rather than a blank
    /// label.
    #[test]
    fn an_unlabelled_overtime_line_still_prints_its_hours_and_multiplier() {
        let overtime = Earning::Overtime {
            amount: money(166_154),
            trace: payroll::OvertimeTrace {
                basic_pay: money(1_200_000),
                ordinary_hours: payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2))
                    .unwrap(),
                months_per_year: rust_decimal::Decimal::new(12, 0),
                weeks_per_year: rust_decimal::Decimal::new(52, 0),
                derived_hourly_rate: derived_hourly_rate(900, 13),
                hours: OvertimeHours::new(rust_decimal::Decimal::new(1200, 2)).unwrap(),
                multiplier: OvertimeMultiplier::Double,
                policy: payroll::SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR,
            },
            label: None,
        };

        let line = earning_line(&overtime);

        assert_eq!(line.label, "Overtime");
        assert_eq!(line.detail.as_deref(), Some("12.00 hrs @ 2x"));
    }

    // `to_render_input`'s handling of `id`, `replaces` and `reversal` is
    // proved end to end instead, in `crates/salt-server/tests/payslip.rs`:
    // `PayslipData`'s own `FinalizedPayrollId` fields have no public
    // constructor (issue #82 leans on that — an id is only ever minted by
    // `finalize_payroll_run`, never fabricated), so a real one can only be
    // obtained by actually finalizing a payroll through the public API.
}
