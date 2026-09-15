//! The Payslip renderer (issue #82, parent #70, ADR-0021): a pure function
//! from a plain, already-frozen [`PayslipInput`] to PDF bytes. It reads no
//! database — every value it prints was decided by
//! [`payroll_app::get_payslip_data`] before this module ever runs — and it
//! lives in `salt-server`, never in `payroll-app`, so the pure calculator
//! crate stays free of a PDF dependency (ADR-0018's own reasoning applied
//! one crate over).
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
//! | Hours and remuneration categories | Each earning line's own kind: `BasicPay` prints as the wage basis line above; `TaxableAllowance` prints its label; `Overtime` prints its hours and multiplier beside its free-text label |
//! | Gross, deductions, net | The frozen `PayrollFigures` |
//! | Provenance | `SaltVersion` and `PayslipTemplateVersion`, both frozen |
//!
//! **`Q-OPEN-7` (unverified, stated rather than settled):** whether
//! Annexure 1 requires overtime's hour *category* to be named, not merely
//! multiplied. This renderer assumes it does not settle that question by
//! printing only a multiplier — every overtime line prints **both** the
//! multiplier (`OvertimeMultiplier`, the classification) and the free-text
//! label an Operator typed, so a named category is on the page whenever one
//! was given, without this renderer inventing a category where none was.
//!
//! INV-001 bans `f32`/`f64` workspace-wide so money can never silently
//! become inexact — but this module's own money is always `i64` cents,
//! exactly as everywhere else in Salt (`format_money` below does its
//! rounding and grouping in integer arithmetic). The float type this module
//! *does* use is `printpdf`'s own coordinate system: `Pt`/`Mm` are `f32`
//! newtypes baked into that crate's public API, for page geometry and font
//! metrics that were never money to begin with. Disallowing `f32` here
//! would not protect INV-001; it would just force every coordinate through
//! an `as f32` cast at the `printpdf` call site instead of at the top of
//! this file, which hides the exception rather than naming it once.
#![allow(clippy::disallowed_types, clippy::float_arithmetic)]

use chrono::NaiveDate;
use printpdf::{
    Color, FontId, Line, LinePoint, Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage,
    PdfSaveOptions, Point, Pt, Rgb, TextItem,
};

/// The one template version this build knows how to render. Kept as a
/// constant, rather than trusting the caller's string, so a typo in the
/// `match` below cannot silently fall through to the wrong layout.
pub const STANDARD_V1: &str = "standard-v1";

/// DejaVu Sans, embedded (ADR-0021: "embed the font file in the
/// repository", never a system font). `crates/salt-server/assets/fonts/`
/// carries its own `LICENSE.txt`. One weight only — hierarchy on the page
/// comes from size and spacing, not a second embedded font, so ADR-0021's
/// "every font it needs, kept alive forever" stays a promise about one file.
static DEJAVU_SANS: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

/// One printed pay line: a label, an optional detail (overtime's hours and
/// multiplier), and the amount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipLine {
    pub label: String,
    pub detail: Option<String>,
    pub amount_cents: i64,
}

/// What a reversed Payslip must say (CONTEXT.md's own `Reversal` entry):
/// the reason, and — once a Correction has taken its place — the
/// replacement's own id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipReversedNotice {
    pub reason: String,
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
    match input.payslip_template_version.as_str() {
        STANDARD_V1 => Ok(render_standard_v1(input)),
        other => Err(PayslipRenderError::UnknownTemplateVersion(
            other.to_string(),
        )),
    }
}

const PAGE_WIDTH_MM: f32 = 210.0;
const PAGE_HEIGHT_MM: f32 = 297.0;
const MARGIN_MM: f32 = 18.0;
const CONTENT_RIGHT_MM: f32 = PAGE_WIDTH_MM - MARGIN_MM;
const BOTTOM_LIMIT_MM: f32 = PAGE_HEIGHT_MM - MARGIN_MM;

const BODY_SIZE: f32 = 10.0;
const SMALL_SIZE: f32 = 8.5;
const HEADING_SIZE: f32 = 12.0;
const TITLE_SIZE: f32 = 16.0;

/// Builds one page's `Op` list at a time, tracking a top-down cursor in mm
/// and closing/reopening the PDF text object around anything that is not
/// text (a rule) — a raw PDF content stream cannot paint a path while a
/// `BT`/`ET` text object is open.
struct Layout<'a> {
    font: &'a ParsedFont,
    font_id: FontId,
    pages: Vec<PdfPage>,
    ops: Vec<Op>,
    in_text: bool,
    y_mm: f32,
}

impl<'a> Layout<'a> {
    fn new(font: &'a ParsedFont, font_id: FontId) -> Self {
        Self {
            font,
            font_id,
            pages: Vec::new(),
            ops: Vec::new(),
            in_text: false,
            y_mm: MARGIN_MM,
        }
    }

    fn pdf_y(&self) -> f32 {
        PAGE_HEIGHT_MM - self.y_mm
    }

    fn ensure_text(&mut self) {
        if !self.in_text {
            self.ops.push(Op::StartTextSection);
            self.in_text = true;
        }
    }

    fn end_text(&mut self) {
        if self.in_text {
            self.ops.push(Op::EndTextSection);
            self.in_text = false;
        }
    }

    /// Starts a fresh page if fewer than `needed_mm` remain above the
    /// bottom margin. A Payslip's own line count is small and bounded (at
    /// most a handful of earnings and deductions), so this is reached only
    /// by an unusually long one — never silently, and never by truncating
    /// a line rather than carrying it onto a new page.
    fn ensure_room(&mut self, needed_mm: f32) {
        if self.y_mm + needed_mm > BOTTOM_LIMIT_MM {
            self.new_page();
        }
    }

    fn new_page(&mut self) {
        self.end_text();
        let ops = std::mem::take(&mut self.ops);
        self.pages
            .push(PdfPage::new(Mm(PAGE_WIDTH_MM), Mm(PAGE_HEIGHT_MM), ops));
        self.y_mm = MARGIN_MM;
    }

    fn text_width_mm(&self, text: &str, size_pt: f32) -> f32 {
        let units_per_em = self.font.pdf_font_metrics.units_per_em.max(1) as f32;
        let total_units: f32 = text
            .chars()
            .map(|ch| {
                let gid = self.font.lookup_glyph_index(ch as u32).unwrap_or(0);
                self.font.get_horizontal_advance(gid) as f32
            })
            .sum();
        let width_pt = total_units / units_per_em * size_pt;
        width_pt / 72.0 * 25.4
    }

    fn set_font(&mut self, size_pt: f32) {
        self.ops.push(Op::SetFont {
            font: PdfFontHandle::External(self.font_id.clone()),
            size: Pt(size_pt),
        });
    }

    /// Draws `text` left-aligned at `x_mm` on the current line, at the
    /// current cursor `y`. Does not advance the cursor — callers advance
    /// explicitly, since a row often draws a label on the left and an
    /// amount on the right at the same `y`.
    ///
    /// Closes and reopens the text object around every single placement,
    /// deliberately: a raw PDF `Td` moves *relative to the current line's*
    /// text matrix, not to the page origin, so leaving one text object open
    /// across several `SetTextCursor`/`ShowText` pairs would make each
    /// absolute `(x_mm, y_mm)` this module computes land at the *sum* of
    /// every position before it. A fresh `BT` resets the text matrix to
    /// identity, which is what makes the very next `Td` land exactly where
    /// this layout says it should.
    fn text_at(&mut self, x_mm: f32, size_pt: f32, text: &str) {
        self.end_text();
        self.ensure_text();
        self.set_font(size_pt);
        self.ops.push(Op::SetTextCursor {
            pos: Point::new(Mm(x_mm), Mm(self.pdf_y())),
        });
        self.ops.push(Op::ShowText {
            items: vec![TextItem::Text(text.to_string())],
        });
    }

    /// As [`Self::text_at`], right-aligned so `text`'s own right edge lands
    /// on `right_edge_mm` — used for every money column.
    fn text_right_at(&mut self, right_edge_mm: f32, size_pt: f32, text: &str) {
        let width = self.text_width_mm(text, size_pt);
        self.text_at(right_edge_mm - width, size_pt, text);
    }

    fn rule(&mut self) {
        self.end_text();
        self.ops.push(Op::SetOutlineThickness { pt: Pt(0.6) });
        self.ops.push(Op::SetOutlineColor {
            col: Color::Rgb(Rgb {
                r: 0.55,
                g: 0.55,
                b: 0.55,
                icc_profile: None,
            }),
        });
        self.ops.push(Op::DrawLine {
            line: Line {
                points: vec![
                    LinePoint {
                        p: Point::new(Mm(MARGIN_MM), Mm(self.pdf_y())),
                        bezier: false,
                    },
                    LinePoint {
                        p: Point::new(Mm(CONTENT_RIGHT_MM), Mm(self.pdf_y())),
                        bezier: false,
                    },
                ],
                is_closed: false,
            },
        });
    }

    fn advance(&mut self, mm: f32) {
        self.y_mm += mm;
    }

    fn finish(mut self) -> Vec<PdfPage> {
        self.end_text();
        if !self.ops.is_empty() || self.pages.is_empty() {
            let ops = std::mem::take(&mut self.ops);
            self.pages
                .push(PdfPage::new(Mm(PAGE_WIDTH_MM), Mm(PAGE_HEIGHT_MM), ops));
        }
        self.pages
    }
}

/// `N$ 12,345.67` — cents formatted with a thousands separator, the
/// convention Namibian currency figures already use elsewhere on a Salt
/// screen. Never a locale-sensitive formatter: the figure a payslip prints
/// must read the same however this process is configured.
fn format_money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let whole = cents.unsigned_abs() / 100;
    let fraction = cents.unsigned_abs() % 100;
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    format!("N$ {sign}{grouped}.{fraction:02}")
}

fn format_date(date: NaiveDate) -> String {
    date.format("%d %b %Y").to_string()
}

fn row(layout: &mut Layout, label: &str, detail: Option<&str>, amount_cents: i64) {
    layout.ensure_room(6.0);
    layout.text_at(MARGIN_MM, BODY_SIZE, label);
    if let Some(detail) = detail {
        layout.text_right_at(CONTENT_RIGHT_MM - 32.0, SMALL_SIZE, detail);
    }
    layout.text_right_at(CONTENT_RIGHT_MM, BODY_SIZE, &format_money(amount_cents));
    layout.advance(6.0);
}

fn total_row(layout: &mut Layout, label: &str, amount_cents: i64) {
    layout.ensure_room(7.0);
    layout.text_at(MARGIN_MM, BODY_SIZE, label);
    layout.text_right_at(CONTENT_RIGHT_MM, BODY_SIZE, &format_money(amount_cents));
    layout.advance(7.0);
}

fn heading(layout: &mut Layout, text: &str) {
    layout.ensure_room(9.0);
    layout.text_at(MARGIN_MM, HEADING_SIZE, text);
    layout.advance(5.5);
    layout.rule();
    layout.advance(3.5);
}

fn render_standard_v1(input: &PayslipInput) -> Vec<u8> {
    let mut warnings = Vec::new();
    let font = ParsedFont::from_bytes(DEJAVU_SANS, 0, &mut warnings)
        .expect("DejaVuSans.ttf is a well-formed embedded font — see assets/fonts/LICENSE.txt");

    let mut doc = PdfDocument::new("Payslip");
    let font_id = doc.add_font(&font);

    let mut layout = Layout::new(&font, font_id);

    layout.text_at(MARGIN_MM, TITLE_SIZE, "PAYSLIP");
    layout.text_right_at(
        CONTENT_RIGHT_MM,
        SMALL_SIZE,
        &format!("Ref: {}", input.finalized_payroll_id),
    );
    layout.advance(9.0);

    if let Some(reversed) = &input.reversed {
        layout.text_at(MARGIN_MM, BODY_SIZE, "THIS PAYROLL HAS BEEN REVERSED");
        layout.advance(5.0);
        layout.text_at(
            MARGIN_MM,
            SMALL_SIZE,
            &format!("Reason: {}", reversed.reason),
        );
        layout.advance(4.5);
        if let Some(replacement_id) = &reversed.replacement_id {
            layout.text_at(
                MARGIN_MM,
                SMALL_SIZE,
                &format!("Replaced by finalized payroll {replacement_id}"),
            );
            layout.advance(4.5);
        }
        layout.advance(2.0);
    }
    if let Some(replaces) = &input.replaces {
        layout.text_at(
            MARGIN_MM,
            BODY_SIZE,
            &format!("This is a Replacement for finalized payroll {replaces}"),
        );
        layout.advance(7.0);
    }

    heading(&mut layout, "Employer");
    layout.text_at(MARGIN_MM, BODY_SIZE, &input.employer_registered_name);
    layout.advance(5.0);
    for line in &input.employer_address_lines {
        layout.text_at(MARGIN_MM, SMALL_SIZE, line);
        layout.advance(4.2);
    }
    if let Some(tax_number) = &input.employer_income_tax_number {
        layout.text_at(
            MARGIN_MM,
            SMALL_SIZE,
            &format!("Income Tax No: {tax_number}"),
        );
        layout.advance(4.2);
    }
    if let Some(ssc_number) = &input.employer_social_security_number {
        layout.text_at(
            MARGIN_MM,
            SMALL_SIZE,
            &format!("Social Security No: {ssc_number}"),
        );
        layout.advance(4.2);
    }
    layout.advance(3.0);

    heading(&mut layout, "Employee");
    layout.text_at(MARGIN_MM, BODY_SIZE, &input.employee_full_name);
    layout.advance(5.0);
    if let Some(identity_number) = &input.employee_identity_number {
        layout.text_at(MARGIN_MM, SMALL_SIZE, &format!("ID No: {identity_number}"));
        layout.advance(4.2);
    }
    for line in &input.employee_address_lines {
        layout.text_at(MARGIN_MM, SMALL_SIZE, line);
        layout.advance(4.2);
    }
    layout.advance(3.0);

    heading(&mut layout, "Pay Period");
    layout.text_at(
        MARGIN_MM,
        BODY_SIZE,
        &format!(
            "{} to {}",
            format_date(input.period_start),
            format_date(input.period_end)
        ),
    );
    layout.text_right_at(
        CONTENT_RIGHT_MM,
        BODY_SIZE,
        &format!("Pay date: {}", format_date(input.pay_date)),
    );
    layout.advance(9.0);

    heading(&mut layout, "Earnings");
    for line in &input.earnings {
        row(
            &mut layout,
            &line.label,
            line.detail.as_deref(),
            line.amount_cents,
        );
    }
    layout.advance(1.5);
    total_row(&mut layout, "Gross Pay", input.gross_pay_cents);
    layout.advance(4.0);

    heading(&mut layout, "Deductions");
    for line in &input.deductions {
        row(
            &mut layout,
            &line.label,
            line.detail.as_deref(),
            line.amount_cents,
        );
    }
    layout.advance(1.5);
    total_row(
        &mut layout,
        "Total Deductions",
        input.total_deductions_cents,
    );
    layout.advance(4.0);

    layout.rule();
    layout.advance(3.5);
    total_row(&mut layout, "Net Pay", input.net_pay_cents);
    layout.advance(9.0);

    layout.text_at(
        MARGIN_MM,
        SMALL_SIZE,
        &format!(
            "Salt version {}  ·  Payslip template {}",
            input.salt_version, input.payslip_template_version
        ),
    );
    layout.advance(4.2);
    layout.text_at(
        MARGIN_MM,
        SMALL_SIZE,
        "Rendered on demand from Salt's frozen payroll record. Not stored.",
    );

    let pages = layout.finish();
    doc.with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use printpdf::{PdfParseOptions, PdfWarnMsg};

    fn a_line(label: &str, amount_cents: i64) -> PayslipLine {
        PayslipLine {
            label: label.to_string(),
            detail: None,
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

    /// Extracts every `Op::ShowText` string on every page, in order — the
    /// mechanism Deep Instructions demand: "verify by extracting the
    /// rendered values and comparing them, never by hashing bytes".
    fn rendered_text(bytes: &[u8]) -> String {
        let mut warnings: Vec<PdfWarnMsg> = Vec::new();
        let doc = PdfDocument::parse(bytes, &PdfParseOptions::default(), &mut warnings)
            .expect("render_payslip must always produce a parseable PDF");
        let mut text = String::new();
        for page in &doc.pages {
            for op in &page.ops {
                if let Op::ShowText { items } = op {
                    for item in items {
                        match item {
                            TextItem::Text(s) => text.push_str(s),
                            // External fonts round-trip as glyph ids, each
                            // carrying the character it decodes to via the
                            // embedded ToUnicode CMap — this is where that
                            // text actually comes back out.
                            TextItem::GlyphIds(codepoints) => {
                                for codepoint in codepoints {
                                    if let Some(cid) = &codepoint.cid {
                                        text.push_str(cid);
                                    }
                                }
                            }
                            TextItem::Offset(_) => {}
                        }
                    }
                    text.push(' ');
                }
            }
        }
        text
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
        assert!(!bytes.is_empty());
    }

    #[test]
    fn every_frozen_particular_and_figure_is_printed() {
        let bytes = render_payslip(&minimal_input()).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("Acme Corp (Pty) Ltd"), "{text}");
        assert!(text.contains("Ada Lovelace"), "{text}");
        assert!(text.contains("80012345678"), "{text}");
        assert!(text.contains("12345678"), "{text}");
        assert!(text.contains("N$ 15,000.00"), "{text}");
        assert!(text.contains("N$ 1,200.00"), "{text}");
        assert!(text.contains("N$ 13,755.00"), "{text}");
        assert!(text.contains("0.1.0+abcdef1"), "{text}");
        assert!(text.contains(STANDARD_V1), "{text}");
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
            amount_cents: 124_615,
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("Sunday overtime"), "{text}");
        assert!(text.contains("12.00 hrs @ 1.5x"), "{text}");
    }

    #[test]
    fn a_reversed_payroll_prints_its_reason_and_replacement() {
        let mut input = minimal_input();
        input.reversed = Some(PayslipReversedNotice {
            reason: "March salary was wrong".to_string(),
            replacement_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("REVERSED"), "{text}");
        assert!(text.contains("March salary was wrong"), "{text}");
        assert!(
            text.contains("22222222-2222-2222-2222-222222222222"),
            "{text}"
        );
    }

    #[test]
    fn a_replacement_payroll_names_what_it_replaces() {
        let mut input = minimal_input();
        input.replaces = Some("33333333-3333-3333-3333-333333333333".to_string());

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(
            text.contains("33333333-3333-3333-3333-333333333333"),
            "{text}"
        );
    }

    /// The stability contract itself (ADR-0021): re-rendering the same
    /// frozen input twice must print the same content. Compared as
    /// extracted text, never as bytes (Deep Instructions).
    #[test]
    fn rendering_the_same_input_twice_prints_identical_content() {
        let input = minimal_input();

        let first = rendered_text(&render_payslip(&input).unwrap());
        let second = rendered_text(&render_payslip(&input).unwrap());

        assert_eq!(first, second);
    }

    /// A frozen `EmployerParticulars` change and a frozen `PersonParticulars`
    /// change are exactly what ADR-0021 says a re-render must survive
    /// unchanged for any *other* record — proven properly at the
    /// `payroll_app::get_payslip_data` level, since that is where "frozen"
    /// is enforced. Here, the narrower claim: two different frozen inputs
    /// print two different documents, so this renderer is not silently
    /// ignoring the particulars it was handed.
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

    #[test]
    fn format_money_groups_thousands_and_keeps_two_decimal_places() {
        assert_eq!(format_money(0), "N$ 0.00");
        assert_eq!(format_money(1), "N$ 0.01");
        assert_eq!(format_money(150_000), "N$ 1,500.00");
        assert_eq!(format_money(123_456_789), "N$ 1,234,567.89");
    }
}
