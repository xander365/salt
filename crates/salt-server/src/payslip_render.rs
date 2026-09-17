//! The Payslip renderer (issue #82, parent #70, ADR-0021): a pure function
//! from a plain, already-frozen [`PayslipInput`] to PDF bytes. It reads no
//! database — every value it prints was decided by
//! [`payroll_app::get_payslip_data`] before this module ever runs — and it
//! lives in `salt-server`, never in `payroll-app`, so the pure calculator
//! crate stays free of a PDF dependency (ADR-0018's own reasoning applied
//! one crate over).
//!
//! The low-level page cursor, font, money/date formatting and the
//! text-extraction test parser all live in [`crate::pdf_layout`] — shared
//! with `run_outputs_render` (issue #83). What stays here is specific to a
//! Payslip: its own pay-line row shape (label, detail, provenance, amount)
//! and its A4-portrait page layout.
//!
//! # The stability contract is content, not bytes (ADR-0021)
//!
//! [`render_payslip`] dispatches on `payslip_template_version` — today only
//! `"standard-v1"` exists, matching [`payroll_app::PAYSLIP_TEMPLATE_VERSION`]
//! — and each version's renderer, once shipped, is never edited again: a
//! layout change ships as a new version and a new `match` arm, so a
//! `FinalizedPayroll` frozen under `"standard-v1"` keeps rendering under
//! `"standard-v1"` for the life of the record, however many later versions
//! exist. What is *not* promised is that the PDF byte stream itself is
//! identical between two renders of the same input, or between two builds
//! of Salt: font subsetting, `printpdf`'s own internal ids and compression
//! are free to vary. Only the printed content — figures, labels, frozen
//! particulars, provenance — is the promise, which is why this module's own
//! tests extract text back out of the PDF ops rather than comparing bytes.
//!
//! # Field-to-source mapping (Deep Instructions, issue #82)
//!
//! The Labour General Regulations (regulation 3, Annexure 1) require more
//! than gross, deductions and net. Every field this renderer prints and
//! where it comes from:
//!
//! | Printed | Source |
//! |---|---|
//! | Employer registered name, address, tax/SSC numbers | `FinalizedEmployerParticulars` (frozen) |
//! | Employee full name, identity number, address | `FinalizedPersonParticulars` (frozen) |
//! | Pay period, pay date | `FinalizedPayroll`'s own frozen period and its `PayrollRun`'s pay date |
//! | Wage basis | The `BasicPay` earning line itself — Salt has one wage basis, a salaried amount per period; there is no separate hourly-paid Employment kind to distinguish (CONTEXT.md's own `DerivedHourlyRate` entry) |
//! | Hours and remuneration categories | Each earning line's own kind: `BasicPay` prints as the wage basis line above; `TaxableAllowance` prints its label; `Overtime` prints its hours and multiplier beside its free-text label. **Not printed:** an Employment's `OrdinaryHours` — `standard-v1` shows overtime hours only, which is recorded here as a gap rather than claimed as compliant |
//! | Reversal and replacement | The `Reversal` row's reason and instant, and the lineage columns — never inferred |
//! | Gross, deductions, net | The frozen `PayrollFigures` |
//! | Provenance | `SaltVersion` and `PayslipTemplateVersion`, both frozen |
//!
//! **`Q-OPEN-7` (live and unverified — an assumption, not a settled
//! answer):** whether Annexure 1 requires overtime's hour *category* to be
//! named, not merely multiplied. Rather than pick one reading, every
//! overtime line prints **both** the multiplier (`OvertimeMultiplier`, the
//! classification) and the free-text label an Operator typed, so a named
//! category is on the page whenever one was given, and this renderer never
//! invents one where none was.
//!
//! INV-001 bans `f32`/`f64` workspace-wide so money can never silently
//! become inexact — but this module's own money is always `i64` cents,
//! exactly as everywhere else in Salt (`pdf_layout::format_money` does its
//! rounding and grouping in integer arithmetic). The float type this module
//! *does* use is `printpdf`'s own coordinate system via
//! [`crate::pdf_layout::Layout`]: `Pt`/`Mm` are `f32` newtypes baked into
//! that crate's public API, for page geometry and font metrics that were
//! never money to begin with.
#![allow(clippy::disallowed_types, clippy::float_arithmetic)]

use chrono::{DateTime, NaiveDate};
use printpdf::{PdfDocument, PdfSaveOptions};

pub(crate) use crate::pdf_layout::format_date;
use crate::pdf_layout::{
    A4_PORTRAIT_MM, BODY_SIZE, Layout, ROW_LINE_HEIGHT_MM, SMALL_SIZE, TITLE_SIZE, format_instant,
    format_money, heading, load_font, total_row,
};

/// The one template version this build knows how to render. Kept as a
/// constant, rather than trusting the caller's string, so a typo in the
/// `match` below cannot silently fall through to the wrong layout.
pub const STANDARD_V1: &str = "standard-v1";

/// One printed pay line: a label, an optional detail (overtime's hours and
/// multiplier), an optional provenance note (issue #82 review: standing,
/// one-off, or copied-from-reversed-snapshot, plus any override reason —
/// read verbatim from the frozen `pay_line_provenance_json`, never
/// reconstructed from a current `StandingPayItem`), and the amount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipLine {
    pub label: String,
    pub detail: Option<String>,
    pub provenance: Option<String>,
    pub amount_cents: i64,
}

/// What a reversed Payslip must say (CONTEXT.md's own `Reversal` entry):
/// the reason, when it happened, and — once a Correction has taken its
/// place — the replacement's own id.
///
/// `reversed_at` is the stored instant itself, printed with its time and an
/// explicit `UTC`: Salt records no business time zone, so reducing it to a
/// bare date would silently print the day before for a reversal made just
/// after local midnight in Windhoek.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipReversedNotice {
    pub reason: String,
    pub reversed_at: DateTime<chrono::Utc>,
    pub replacement_id: Option<String>,
}

/// Everything [`render_payslip`] needs, entirely plain data: no database
/// handle, no `payroll_app` type. `salt-server`'s `payslip.rs` builds one of
/// these from `payroll_app::PayslipData` — this struct is the seam Deep
/// Instructions describe as "a plain frozen-data struct".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipInput {
    pub finalized_payroll_id: String,
    pub employer_registered_name: String,
    pub employer_address_lines: Vec<String>,
    pub employer_income_tax_number: Option<String>,
    pub employer_social_security_number: Option<String>,
    pub employee_full_name: String,
    pub employee_identity_number: Option<String>,
    pub employee_address_lines: Vec<String>,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub pay_date: NaiveDate,
    pub salt_version: String,
    pub payslip_template_version: String,
    pub earnings: Vec<PayslipLine>,
    pub deductions: Vec<PayslipLine>,
    pub gross_pay_cents: i64,
    pub total_deductions_cents: i64,
    pub net_pay_cents: i64,
    /// Present when this `FinalizedPayroll` is itself a Replacement: the id
    /// it replaces.
    pub replaces: Option<String>,
    /// Present when this `FinalizedPayroll` has been reversed.
    pub reversed: Option<PayslipReversedNotice>,
}

/// Why [`render_payslip`] refused. The only case today is a
/// `payslip_template_version` this build has no renderer for — which
/// `payroll_app::get_payslip_data` never produces itself (it only ever
/// freezes [`payroll_app::PAYSLIP_TEMPLATE_VERSION`]), so reaching this in
/// production means a row written by a future build this one predates. Never
/// a caller mistake, so `salt-server`'s handler answers it as an internal
/// error, not a 404 or a 422.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayslipRenderError {
    UnknownTemplateVersion(String),
}

impl std::fmt::Display for PayslipRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTemplateVersion(version) => {
                write!(
                    f,
                    "no Payslip renderer exists for template version {version:?}"
                )
            }
        }
    }
}

/// Renders `input` to PDF bytes, dispatching on `payslip_template_version`
/// (ADR-0021). The one arm today is [`STANDARD_V1`]; a later template
/// version adds an arm beside it and never edits this one — a
/// `FinalizedPayroll` frozen under `"standard-v1"` must keep rendering
/// exactly as `render_standard_v1` renders it, for the life of the record.
pub fn render_payslip(input: &PayslipInput) -> Result<Vec<u8>, PayslipRenderError> {
    render_payslips(std::slice::from_ref(input))
}

/// As [`render_payslip`], for every payslip in a run (issue #83): one PDF,
/// each payslip starting on a fresh page, in the order `inputs` is given —
/// callers pass member order (`employment.id`), never sorted here. Each
/// input still dispatches on its own `payslip_template_version`, so a run
/// whose rows span two template versions renders every row under its own
/// frozen layout.
///
/// An empty `inputs` still returns a valid, parseable PDF — one page saying
/// no payslips were finalized in this run — never a 404: that refusal
/// belongs to whether the run itself exists and has finalized, which the
/// caller has already checked before calling this.
pub fn render_payslips(inputs: &[PayslipInput]) -> Result<Vec<u8>, PayslipRenderError> {
    let font = load_font();

    let mut doc = PdfDocument::new("Payslip");
    let font_id = doc.add_font(&font);
    let mut layout = Layout::new(&font, font_id, A4_PORTRAIT_MM, MARGIN_MM);

    if inputs.is_empty() {
        let margin = layout.margin_mm();
        layout.text_at(margin, TITLE_SIZE, "PAYSLIPS");
        layout.advance(9.0);
        layout.paragraph(BODY_SIZE, "No payslips were finalized in this run.");
    }

    for (index, input) in inputs.iter().enumerate() {
        match input.payslip_template_version.as_str() {
            STANDARD_V1 => {
                if index > 0 {
                    layout.new_page();
                }
                draw_standard_v1(&mut layout, input);
            }
            other => {
                return Err(PayslipRenderError::UnknownTemplateVersion(
                    other.to_string(),
                ));
            }
        }
    }

    let pages = layout.finish();
    Ok(doc
        .with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new()))
}

const MARGIN_MM: f32 = 18.0;

/// Reserved on the right of a pay-line row for its money column, so a
/// wrapped label can never grow into the figure it belongs beside.
const AMOUNT_COLUMN_WIDTH_MM: f32 = 30.0;
/// Reserved beside [`AMOUNT_COLUMN_WIDTH_MM`] when a row also carries a
/// detail column (overtime's hours and multiplier).
const DETAIL_COLUMN_WIDTH_MM: f32 = 30.0;
/// The gap kept between a row's label text and whatever reserved column sits
/// to its right, so a label that exactly fills its budget never visually
/// touches the figure beside it.
const COLUMN_GAP_MM: f32 = 4.0;
/// A label column narrower than this is refused in favour of simply letting
/// the label run into the reserved columns — an unreachably small page
/// width, never real content, since every Payslip page is this module's own
/// fixed A4 size.
const MIN_LABEL_WIDTH_MM: f32 = 20.0;

/// One earning or deduction row: a label (word-wrapped so it can never grow
/// into the detail or amount columns), an optional detail (overtime's hours
/// and multiplier, printed only beside the label's first line), an optional
/// provenance note (issue #82 review, printed full-width beneath the label),
/// and the amount.
///
/// The row's total height is computed before anything is drawn, and
/// [`Layout::ensure_room`] is called exactly once for the whole row — never
/// per line — so a wrapped label can never straddle a page break with its
/// detail or amount left behind on the page above it.
fn row(
    layout: &mut Layout,
    label: &str,
    detail: Option<&str>,
    provenance: Option<&str>,
    amount_cents: i64,
) {
    let margin = layout.margin_mm();
    let content_right = layout.content_right_mm();

    // Each reserved column is at least its nominal width, and wider when
    // the text it actually holds needs more: a figure or a detail that
    // outgrows its column pushes the label's budget left, never over the
    // label.
    let amount = format_money(amount_cents);
    let amount_column = AMOUNT_COLUMN_WIDTH_MM.max(layout.text_width_mm(&amount, BODY_SIZE));
    let detail_column = detail.map_or(0.0, |detail| {
        DETAIL_COLUMN_WIDTH_MM.max(layout.text_width_mm(detail, SMALL_SIZE) + COLUMN_GAP_MM)
    });
    let reserved = COLUMN_GAP_MM + amount_column + detail_column;
    let label_max_width = (content_right - margin - reserved).max(MIN_LABEL_WIDTH_MM);
    let label_lines = layout.wrap(label, BODY_SIZE, label_max_width);
    let provenance_lines = provenance
        .map(|text| layout.wrap(text, SMALL_SIZE, content_right - margin))
        .unwrap_or_default();

    let needed =
        label_lines.len() as f32 * ROW_LINE_HEIGHT_MM + provenance_lines.len() as f32 * 3.8 + 1.5;
    layout.ensure_room(needed);

    for (index, line) in label_lines.iter().enumerate() {
        layout.text_at(margin, BODY_SIZE, line);
        if index == 0 {
            if let Some(detail) = detail {
                layout.text_right_at(
                    content_right - amount_column - COLUMN_GAP_MM,
                    SMALL_SIZE,
                    detail,
                );
            }
            layout.text_right_at(content_right, BODY_SIZE, &amount);
        }
        layout.advance(ROW_LINE_HEIGHT_MM);
    }
    for line in &provenance_lines {
        layout.text_at(margin, SMALL_SIZE, line);
        layout.advance(3.8);
    }
    layout.advance(1.5);
}

/// Draws one Payslip's full content into `layout`, starting at its current
/// cursor — never opening a document or a font, and never calling
/// [`Layout::finish`], so [`render_payslips`] can call this once per input
/// into one shared document. The single-payslip visible content this draws
/// is exactly what `render_standard_v1` always drew.
fn draw_standard_v1(layout: &mut Layout, input: &PayslipInput) {
    let margin = layout.margin_mm();
    let content_right = layout.content_right_mm();

    layout.text_at(margin, TITLE_SIZE, "PAYSLIP");
    layout.text_right_at(
        content_right,
        SMALL_SIZE,
        &format!("Ref: {}", input.finalized_payroll_id),
    );
    layout.advance(9.0);

    if let Some(reversed) = &input.reversed {
        layout.paragraph(BODY_SIZE, "REVERSED");
        layout.advance(0.8);
        layout.paragraph(
            SMALL_SIZE,
            &format!("Reversed on: {}", format_instant(reversed.reversed_at)),
        );
        layout.paragraph(SMALL_SIZE, &format!("Reason: {}", reversed.reason));
        match &reversed.replacement_id {
            Some(replacement_id) => layout.paragraph(
                SMALL_SIZE,
                &format!("Replaced by finalized payroll {replacement_id}"),
            ),
            None => layout.paragraph(SMALL_SIZE, "No replacement has been finalized."),
        }
        layout.advance(2.0);
    }
    if let Some(replaces) = &input.replaces {
        layout.paragraph(BODY_SIZE, "REPLACEMENT");
        layout.advance(0.8);
        layout.paragraph(
            SMALL_SIZE,
            &format!("This payslip replaces finalized payroll {replaces}"),
        );
        layout.advance(2.0);
    }

    heading(layout, "Employer");
    layout.paragraph(BODY_SIZE, &input.employer_registered_name);
    layout.advance(0.8);
    for line in &input.employer_address_lines {
        layout.paragraph(SMALL_SIZE, line);
    }
    if let Some(tax_number) = &input.employer_income_tax_number {
        layout.paragraph(SMALL_SIZE, &format!("Income Tax No: {tax_number}"));
    }
    if let Some(ssc_number) = &input.employer_social_security_number {
        layout.paragraph(SMALL_SIZE, &format!("Social Security No: {ssc_number}"));
    }
    layout.advance(3.0);

    heading(layout, "Employee");
    layout.paragraph(BODY_SIZE, &input.employee_full_name);
    layout.advance(0.8);
    if let Some(identity_number) = &input.employee_identity_number {
        layout.paragraph(SMALL_SIZE, &format!("ID No: {identity_number}"));
    }
    for line in &input.employee_address_lines {
        layout.paragraph(SMALL_SIZE, line);
    }
    layout.advance(3.0);

    heading(layout, "Pay Period");
    layout.paragraph(
        BODY_SIZE,
        &format!(
            "{} to {}",
            format_date(input.period_start),
            format_date(input.period_end)
        ),
    );
    layout.paragraph(
        BODY_SIZE,
        &format!("Pay date: {}", format_date(input.pay_date)),
    );
    layout.advance(4.8);

    heading(layout, "Earnings");
    for line in &input.earnings {
        row(
            layout,
            &line.label,
            line.detail.as_deref(),
            line.provenance.as_deref(),
            line.amount_cents,
        );
    }
    layout.advance(1.5);
    total_row(layout, "Gross Pay", input.gross_pay_cents);
    layout.advance(4.0);

    heading(layout, "Deductions");
    for line in &input.deductions {
        row(
            layout,
            &line.label,
            line.detail.as_deref(),
            line.provenance.as_deref(),
            line.amount_cents,
        );
    }
    layout.advance(1.5);
    total_row(layout, "Total Deductions", input.total_deductions_cents);
    layout.advance(4.0);

    // The rule and the Net Pay beneath it move to a new page together.
    layout.ensure_room(3.5 + 7.0);
    layout.rule();
    layout.advance(3.5);
    total_row(layout, "Net Pay", input.net_pay_cents);
    layout.advance(9.0);

    layout.paragraph(
        SMALL_SIZE,
        &format!(
            "Salt version {}  ·  Payslip template {}",
            input.salt_version, input.payslip_template_version
        ),
    );
    layout.paragraph(
        SMALL_SIZE,
        "Rendered on demand from Salt's frozen payroll record. Not stored.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_layout::{TextPlacement, placements_in_ops, rendered_text, text_placements};
    use printpdf::ParsedFont;

    fn a_line(label: &str, amount_cents: i64) -> PayslipLine {
        PayslipLine {
            label: label.to_string(),
            detail: None,
            provenance: None,
            amount_cents,
        }
    }

    fn minimal_input() -> PayslipInput {
        PayslipInput {
            finalized_payroll_id: "11111111-1111-1111-1111-111111111111".to_string(),
            employer_registered_name: "Acme Corp (Pty) Ltd".to_string(),
            employer_address_lines: vec!["1 Independence Ave".to_string(), "Windhoek".to_string()],
            employer_income_tax_number: Some("12345678".to_string()),
            employer_social_security_number: None,
            employee_full_name: "Ada Lovelace".to_string(),
            employee_identity_number: Some("80012345678".to_string()),
            employee_address_lines: vec!["2 Fidel Castro St, Swakopmund".to_string()],
            period_start: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            pay_date: NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
            salt_version: "0.1.0+abcdef1".to_string(),
            payslip_template_version: STANDARD_V1.to_string(),
            earnings: vec![a_line("Basic Pay", 1_500_000)],
            deductions: vec![a_line("PAYE", 120_000), a_line("Social Security", 4_500)],
            gross_pay_cents: 1_500_000,
            total_deductions_cents: 124_500,
            net_pay_cents: 1_375_500,
            replaces: None,
            reversed: None,
        }
    }

    fn a_layout(font: &ParsedFont) -> Layout<'_> {
        let mut doc = PdfDocument::new("test");
        let font_id = doc.add_font(font);
        Layout::new(font, font_id, A4_PORTRAIT_MM, MARGIN_MM)
    }

    fn dejavu() -> ParsedFont {
        load_font()
    }

    #[test]
    fn an_unknown_template_version_is_refused_rather_than_guessed_at() {
        let mut input = minimal_input();
        input.payslip_template_version = "some-future-v7".to_string();

        assert_eq!(
            render_payslip(&input),
            Err(PayslipRenderError::UnknownTemplateVersion(
                "some-future-v7".to_string()
            ))
        );
    }

    #[test]
    fn standard_v1_produces_a_parseable_pdf_starting_with_the_pdf_magic_bytes() {
        let bytes = render_payslip(&minimal_input()).unwrap();

        assert!(bytes.starts_with(b"%PDF"));
        assert!(!text_placements(&bytes).is_empty());
    }

    #[test]
    fn every_frozen_particular_and_figure_is_printed() {
        let mut input = minimal_input();
        input.employer_social_security_number = Some("SSC-998877".to_string());
        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        for expected in [
            "Acme Corp (Pty) Ltd",
            "1 Independence Ave",
            "Windhoek",
            "Income Tax No: 12345678",
            "Social Security No: SSC-998877",
            "Ada Lovelace",
            "ID No: 80012345678",
            "2 Fidel Castro St, Swakopmund",
            "01 Mar 2026 to 31 Mar 2026",
            "Pay date: 05 Apr 2026",
            "Basic Pay",
            "N$ 15,000.00",
            "PAYE",
            "N$ 1,200.00",
            "Social Security",
            "N$ 45.00",
            "N$ 1,245.00",
            "N$ 13,755.00",
            "0.1.0+abcdef1",
            STANDARD_V1,
        ] {
            assert!(text.contains(expected), "{expected:?} missing from {text}");
        }
        assert!(!text.contains("REVERSED"), "{text}");
        assert!(!text.contains("REPLACEMENT"), "{text}");
    }

    /// Q-OPEN-7 (Deep Instructions): the multiplier and the free-text label
    /// both print — this renderer never claims a named category the label
    /// did not actually give it.
    #[test]
    fn an_overtime_line_prints_both_its_detail_and_its_label() {
        let mut input = minimal_input();
        input.earnings.push(PayslipLine {
            label: "Sunday overtime".to_string(),
            detail: Some("12.00 hrs @ 1.5x".to_string()),
            provenance: None,
            amount_cents: 124_615,
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("Sunday overtime"), "{text}");
        assert!(text.contains("12.00 hrs @ 1.5x"), "{text}");
    }

    #[test]
    fn a_reversed_payroll_prints_its_date_reason_and_replacement() {
        let mut input = minimal_input();
        input.reversed = Some(PayslipReversedNotice {
            reason: "March salary was wrong".to_string(),
            reversed_at: "2026-05-12T22:30:00Z".parse().unwrap(),
            replacement_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("REVERSED"), "{text}");
        assert!(text.contains("Reason: March salary was wrong"), "{text}");
        // The instant is printed with its zone, never truncated to a date
        // that could be the wrong local day.
        assert!(
            text.contains("Reversed on: 12 May 2026, 22:30 UTC"),
            "{text}"
        );
        assert!(
            text.contains("Replaced by finalized payroll 22222222-2222-2222-2222-222222222222"),
            "{text}"
        );
    }

    /// A Reversal is complete on its own (CONTEXT.md's `Replacement`
    /// entry): the payslip says plainly that nothing replaced it yet,
    /// rather than leaving the reader to guess.
    #[test]
    fn a_reversed_payroll_with_no_replacement_says_so() {
        let mut input = minimal_input();
        input.reversed = Some(PayslipReversedNotice {
            reason: "Paid in error".to_string(),
            reversed_at: "2026-05-12T08:00:00Z".parse().unwrap(),
            replacement_id: None,
        });

        let text = rendered_text(&render_payslip(&input).unwrap());

        assert!(text.contains("REVERSED"), "{text}");
        assert!(
            text.contains("No replacement has been finalized."),
            "{text}"
        );
        assert!(!text.contains("Replaced by"), "{text}");
    }

    #[test]
    fn a_replacement_payroll_is_marked_and_names_what_it_replaces() {
        let mut input = minimal_input();
        input.replaces = Some("33333333-3333-3333-3333-333333333333".to_string());

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("REPLACEMENT"), "{text}");
        assert!(
            text.contains(
                "This payslip replaces finalized payroll 33333333-3333-3333-3333-333333333333"
            ),
            "{text}"
        );
        assert!(!text.contains("REVERSED"), "{text}");
    }

    /// The stability contract itself (ADR-0021): re-rendering the same
    /// frozen input twice must print the same content in the same places.
    /// Compared as extracted placements, never as bytes (Deep Instructions).
    #[test]
    fn rendering_the_same_input_twice_prints_identical_content() {
        let input = minimal_input();

        let first = text_placements(&render_payslip(&input).unwrap());
        let second = text_placements(&render_payslip(&input).unwrap());

        assert_eq!(first, second);
    }

    /// The narrower renderer-level claim behind content stability: two
    /// different frozen inputs print two different documents, so this
    /// renderer is not silently ignoring the particulars it was handed. The
    /// "a later correction never reaches an issued payslip" half is proven
    /// end to end in `tests/payslip.rs`, where "frozen" is enforced.
    #[test]
    fn a_different_frozen_input_prints_different_content() {
        let mut changed = minimal_input();
        changed.employer_registered_name = "Acme Holdings".to_string();
        changed.employee_full_name = "Ada King, Countess of Lovelace".to_string();

        let original_text = rendered_text(&render_payslip(&minimal_input()).unwrap());
        let changed_text = rendered_text(&render_payslip(&changed).unwrap());

        assert!(changed_text.contains("Acme Holdings"));
        assert!(!original_text.contains("Acme Holdings"));
        assert!(changed_text.contains("Ada King, Countess of Lovelace"));
    }

    /// The width-aware layout's own load-bearing property (issue #82
    /// review): every line [`Layout::wrap`] returns fits the budget it was
    /// given, checked against the same glyph metrics right alignment uses.
    #[test]
    fn wrap_never_returns_a_line_wider_than_the_budget_it_was_given() {
        let font = dejavu();
        let layout = a_layout(&font);

        let text = "The quick brown fox jumps over the lazy dog, again and again, until \
                     this sentence is unmistakably longer than any reasonable column width.";
        let max_width_mm = 60.0;

        let lines = layout.wrap(text, BODY_SIZE, max_width_mm);

        assert!(lines.len() > 1, "{lines:?}");
        for line in &lines {
            assert!(
                layout.text_width_mm(line, BODY_SIZE) <= max_width_mm + 0.01,
                "{line:?} is wider than the {max_width_mm}mm budget"
            );
        }
        // Every word survives the wrap, in the original order — wrapping
        // must never drop or duplicate content to make it fit.
        assert_eq!(lines.join(" "), text);
    }

    /// The property [`row`] exists to guarantee: a label long enough to wrap
    /// still never lets its detail or its amount land on top of it, or on
    /// top of each other — checked against the actual `(x_mm, y_mm)` each
    /// piece of text was placed at.
    #[test]
    fn a_row_with_a_wrapped_label_never_places_its_amount_over_its_label() {
        let font = dejavu();
        let mut layout = a_layout(&font);

        row(
            &mut layout,
            "An allowance label long enough that it must wrap onto a second line of this row",
            Some("12.00 hrs @ 1.5x"),
            Some("Standing since 01 Mar 2026"),
            124_615,
        );

        let placements = placements_in_ops(0, layout_ops(&layout));

        // The label wrapped: at least two body-sized lines share the row's
        // own left margin, at two different heights.
        let label_lines: Vec<&TextPlacement> = placements
            .iter()
            .filter(|p| (p.x_mm - MARGIN_MM).abs() < 0.01 && p.size_pt == BODY_SIZE)
            .collect();
        assert!(label_lines.len() >= 2, "{placements:?}");

        let label = label_lines[0];
        let label_right_edge = label.x_mm + layout.text_width_mm(&label.text, BODY_SIZE);

        let same_line: Vec<&TextPlacement> = placements
            .iter()
            .filter(|p| (p.y_mm - label.y_mm).abs() < 0.01)
            .collect();
        let detail = same_line
            .iter()
            .find(|p| p.text.contains("hrs @"))
            .expect("the detail is drawn on the label's first line");
        let amount = same_line
            .iter()
            .find(|p| p.text.contains("N$"))
            .expect("the amount is drawn on the label's first line");

        assert!(
            detail.x_mm >= label_right_edge,
            "detail overlaps the label ending at {label_right_edge}: {placements:?}"
        );
        let detail_right_edge = detail.x_mm + layout.text_width_mm(&detail.text, SMALL_SIZE);
        assert!(
            amount.x_mm >= detail_right_edge,
            "amount overlaps the detail ending at {detail_right_edge}: {placements:?}"
        );
    }

    /// A pay line's frozen provenance (issue #82 review) prints beneath its
    /// label, verbatim.
    #[test]
    fn a_pay_line_with_provenance_prints_it_beneath_the_line() {
        let mut input = minimal_input();
        input.earnings.push(PayslipLine {
            label: "Standby allowance".to_string(),
            detail: None,
            provenance: Some("Standing since 01 Mar 2026".to_string()),
            amount_cents: 50_000,
        });

        let placements = text_placements(&render_payslip(&input).unwrap());

        let label = placements
            .iter()
            .find(|p| p.text == "Standby allowance")
            .expect("the label is printed");
        let provenance = placements
            .iter()
            .find(|p| p.text == "Standing since 01 Mar 2026")
            .expect("the provenance is printed");
        assert_eq!(provenance.page, label.page);
        assert!(provenance.y_mm < label.y_mm, "{placements:?}");
    }

    /// Every frozen string a Payslip prints at its most extreme at once:
    /// long unbroken words, long names and addresses, a long reversal
    /// reason, very large figures and enough pay lines to need a second
    /// page. Checked against the real rendered PDF: no text run starts left
    /// of the margin, ends past the right margin, or sits below the bottom
    /// margin, and no two runs on the same baseline of the same page
    /// overlap. This is the placement-level test extracted text alone
    /// cannot be.
    #[test]
    fn nothing_on_an_extreme_payslip_leaves_the_page_or_overlaps() {
        let long_word = "Unbrokenreferencewithoutanyspaces".repeat(5);
        let mut input = minimal_input();
        input.employer_registered_name =
            format!("Extraordinarily Long Registered Employer Trading Name {long_word}");
        input.employer_address_lines = vec![format!("Plot {long_word}"); 3];
        input.employee_full_name = format!("Ada {long_word} Lovelace");
        input.employee_address_lines = vec![format!("Erf {long_word}"); 3];
        input.reversed = Some(PayslipReversedNotice {
            reason: format!("A long reason {long_word} and then some more words to wrap"),
            reversed_at: "2026-05-12T08:00:00Z".parse().unwrap(),
            replacement_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
        });
        input.replaces = Some("33333333-3333-3333-3333-333333333333".to_string());
        input.earnings = (0..40)
            .map(|index| PayslipLine {
                label: format!(
                    "Allowance {index} with a label that is long enough to wrap {long_word}"
                ),
                detail: Some(format!("{index}.00 hrs @ 1.5x")),
                provenance: Some(format!(
                    "Standing since 01 Mar 2026 — changed for this run: {long_word}"
                )),
                amount_cents: 99_999_999_999,
            })
            .collect();
        input.gross_pay_cents = 9_999_999_999_999;
        input.net_pay_cents = 9_999_999_999_999;

        let placements = text_placements(&render_payslip(&input).unwrap());
        let font = dejavu();
        let layout = a_layout(&font);
        let content_right = layout.content_right_mm();

        assert!(
            placements.iter().any(|p| p.page > 0),
            "the fixture must be long enough to need a second page"
        );
        let mut runs = Vec::new();
        for placement in &placements {
            let left = placement.x_mm;
            let right = left + layout.text_width_mm(&placement.text, placement.size_pt);
            assert!(
                left >= MARGIN_MM - 0.01,
                "{placement:?} starts left of the margin"
            );
            assert!(
                right <= content_right + 0.01,
                "{placement:?} ends at {right}mm, past the right margin"
            );
            assert!(
                placement.y_mm >= MARGIN_MM - 0.01,
                "{placement:?} sits below the bottom margin"
            );
            runs.push((placement, left, right));
        }
        for (index, (a, a_left, a_right)) in runs.iter().enumerate() {
            for (b, b_left, b_right) in &runs[index + 1..] {
                if a.page == b.page && (a.y_mm - b.y_mm).abs() < 0.01 {
                    assert!(
                        a_right <= b_left || b_right <= a_left,
                        "{a:?} overlaps {b:?}"
                    );
                }
            }
        }

        // Nothing was lost to make it fit: every allowance amount printed.
        let text = rendered_text(&render_payslip(&input).unwrap());
        assert_eq!(text.matches("N$ 999,999,999.99").count(), 40, "{text}");
    }

    /// Test-only accessor: `row`'s placement test needs the raw ops of an
    /// in-progress (unfinished) [`Layout`] — [`Layout::finish`] consumes
    /// `self` to produce pages, which would end the very row under test.
    fn layout_ops<'a>(layout: &'a Layout<'_>) -> &'a [printpdf::Op] {
        layout.test_only_ops()
    }
}
