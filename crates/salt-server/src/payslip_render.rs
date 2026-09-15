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
#[cfg(any(test, feature = "test-support"))]
use printpdf::{PdfParseOptions, PdfWarnMsg};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipReversedNotice {
    pub reason: String,
    pub reversed_at: NaiveDate,
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

/// The line height a single wrapped line of body-sized text advances the
/// cursor by — the same spacing an address line already used before
/// wrapping existed.
const LINE_HEIGHT_MM: f32 = 4.2;
/// As [`LINE_HEIGHT_MM`], for a wrapped line of small-sized text (a
/// provenance note, a reversal reason continuation).
const SMALL_LINE_HEIGHT_MM: f32 = 3.8;
const ROW_LINE_HEIGHT_MM: f32 = 4.6;
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

    /// Greedy word-wrap: every returned line's rendered width is at most
    /// `max_width_mm`, except a single word that alone exceeds it — printed
    /// on its own line rather than split, since a payslip never hyphenates a
    /// name or an address. Always returns at least one line (empty for
    /// empty input), so a caller never has to special-case "nothing to
    /// wrap".
    fn wrap(&self, text: &str, size_pt: f32, max_width_mm: f32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if current.is_empty() || self.text_width_mm(&candidate, size_pt) <= max_width_mm {
                current = candidate;
            } else {
                lines.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// Draws `text` word-wrapped to `max_width_mm`, left-aligned at `x_mm`,
    /// one line at a time — each line pays for its own room via
    /// [`Self::ensure_room`] before it is drawn, so a long name or address
    /// flows onto a new page rather than clipping off the bottom of this
    /// one.
    fn text_wrapped(&mut self, x_mm: f32, max_width_mm: f32, size_pt: f32, text: &str) {
        for line in self.wrap(text, size_pt, max_width_mm) {
            self.ensure_room(LINE_HEIGHT_MM);
            self.text_at(x_mm, size_pt, &line);
            self.advance(LINE_HEIGHT_MM);
        }
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

/// `pub(crate)`: `payslip.rs`'s own `to_render_input` reuses this to format
/// a pay line's `standing_effective_from` inside a provenance note the same
/// way this module formats every other date, rather than keeping a second,
/// driftable copy of "how a Payslip spells a date".
pub(crate) fn format_date(date: NaiveDate) -> String {
    date.format("%d %b %Y").to_string()
}

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
    let reserved = COLUMN_GAP_MM
        + AMOUNT_COLUMN_WIDTH_MM
        + if detail.is_some() {
            DETAIL_COLUMN_WIDTH_MM
        } else {
            0.0
        };
    let label_max_width = (CONTENT_RIGHT_MM - MARGIN_MM - reserved).max(MIN_LABEL_WIDTH_MM);
    let label_lines = layout.wrap(label, BODY_SIZE, label_max_width);
    let provenance_lines = provenance
        .map(|text| layout.wrap(text, SMALL_SIZE, CONTENT_RIGHT_MM - MARGIN_MM))
        .unwrap_or_default();

    let needed = label_lines.len() as f32 * ROW_LINE_HEIGHT_MM
        + provenance_lines.len() as f32 * SMALL_LINE_HEIGHT_MM
        + 1.5;
    layout.ensure_room(needed);

    for (index, line) in label_lines.iter().enumerate() {
        layout.text_at(MARGIN_MM, BODY_SIZE, line);
        if index == 0 {
            if let Some(detail) = detail {
                layout.text_right_at(
                    CONTENT_RIGHT_MM - AMOUNT_COLUMN_WIDTH_MM,
                    SMALL_SIZE,
                    detail,
                );
            }
            layout.text_right_at(CONTENT_RIGHT_MM, BODY_SIZE, &format_money(amount_cents));
        }
        layout.advance(ROW_LINE_HEIGHT_MM);
    }
    for line in &provenance_lines {
        layout.text_at(MARGIN_MM, SMALL_SIZE, line);
        layout.advance(SMALL_LINE_HEIGHT_MM);
    }
    layout.advance(1.5);
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

    let content_width = CONTENT_RIGHT_MM - MARGIN_MM;

    if let Some(reversed) = &input.reversed {
        layout.text_at(MARGIN_MM, BODY_SIZE, "THIS PAYROLL HAS BEEN REVERSED");
        layout.advance(5.0);
        layout.text_wrapped(
            MARGIN_MM,
            content_width,
            SMALL_SIZE,
            &format!("Reason: {}", reversed.reason),
        );
        layout.text_at(
            MARGIN_MM,
            SMALL_SIZE,
            &format!("Reversed on: {}", format_date(reversed.reversed_at)),
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
        layout.text_wrapped(
            MARGIN_MM,
            content_width,
            BODY_SIZE,
            &format!("This is a Replacement for finalized payroll {replaces}"),
        );
        layout.advance(2.8);
    }

    heading(&mut layout, "Employer");
    layout.text_wrapped(
        MARGIN_MM,
        content_width,
        BODY_SIZE,
        &input.employer_registered_name,
    );
    layout.advance(0.8);
    for line in &input.employer_address_lines {
        layout.text_wrapped(MARGIN_MM, content_width, SMALL_SIZE, line);
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
    layout.text_wrapped(
        MARGIN_MM,
        content_width,
        BODY_SIZE,
        &input.employee_full_name,
    );
    layout.advance(0.8);
    if let Some(identity_number) = &input.employee_identity_number {
        layout.text_at(MARGIN_MM, SMALL_SIZE, &format!("ID No: {identity_number}"));
        layout.advance(4.2);
    }
    for line in &input.employee_address_lines {
        layout.text_wrapped(MARGIN_MM, content_width, SMALL_SIZE, line);
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
            line.provenance.as_deref(),
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
            line.provenance.as_deref(),
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

/// Extracts every printed text run from a rendered PDF, in order — Deep
/// Instructions' own "verify by extracting the rendered values and comparing
/// them, never by hashing bytes", shared here so this module's own unit
/// tests and `salt-server`'s HTTP integration tests
/// (`tests/payslip.rs`) read one PDF the same way rather than each keeping
/// its own copy of this parser (issue #82 review). `cfg`-gated exactly like
/// `router.rs`'s own `test-support` feature: `test` for this crate's own
/// unit tests, `feature = "test-support"` for another crate's integration
/// test binary that depends on this one as a dev-dependency with that
/// feature enabled.
#[cfg(any(test, feature = "test-support"))]
pub fn rendered_text(bytes: &[u8]) -> String {
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

/// One placed text run: the page index it was drawn on, its `(x_mm, y_mm)`
/// origin (as [`Layout::text_at`] placed it — bottom-left, PDF coordinates)
/// and the text itself.
///
/// Extracted the same way [`rendered_text`] is, by parsing the PDF back —
/// never by inspecting `Layout`'s own state, which the renderer never
/// exposes. This is what lets a test check *placement* — that a wrapped
/// label's own text never overlaps the detail or amount beside it, that a
/// wrapped line never lands past the page's own width — the thing
/// `rendered_text`'s flat string alone cannot show, since flattening every
/// `ShowText` into one string cannot tell two overlapping lines from two
/// lines stacked cleanly one above the other.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq)]
pub struct TextPlacement {
    pub page: usize,
    pub x_mm: f32,
    pub y_mm: f32,
    pub text: String,
}

/// Pairs every `Op::SetTextCursor` with the `Op::ShowText` that immediately
/// follows it. Safe to rely on positionally because [`Layout::text_at`]
/// closes and reopens the PDF text object around *every single placement*
/// (its own doc comment explains why): each `ShowText` in the op stream is
/// always preceded directly by exactly one `SetTextCursor` naming where it
/// landed.
#[cfg(any(test, feature = "test-support"))]
pub fn text_placements(bytes: &[u8]) -> Vec<TextPlacement> {
    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    let doc = PdfDocument::parse(bytes, &PdfParseOptions::default(), &mut warnings)
        .expect("render_payslip must always produce a parseable PDF");
    let mut placements = Vec::new();
    for (page_index, page) in doc.pages.iter().enumerate() {
        let mut cursor: Option<(f32, f32)> = None;
        for op in &page.ops {
            match op {
                Op::SetTextCursor { pos } => {
                    cursor = Some((pos.x.0, pos.y.0));
                }
                Op::ShowText { items } => {
                    let mut text = String::new();
                    for item in items {
                        match item {
                            TextItem::Text(s) => text.push_str(s),
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
                    if let Some((x_pt, y_pt)) = cursor {
                        placements.push(TextPlacement {
                            page: page_index,
                            x_mm: x_pt / 72.0 * 25.4,
                            y_mm: y_pt / 72.0 * 25.4,
                            text,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    placements
}

#[cfg(test)]
mod tests {
    use super::*;

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
            provenance: None,
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
            reversed_at: NaiveDate::from_ymd_opt(2026, 5, 12).unwrap(),
            replacement_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("REVERSED"), "{text}");
        assert!(text.contains("March salary was wrong"), "{text}");
        assert!(text.contains("12 May 2026"), "{text}");
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

    fn a_layout(font: &ParsedFont) -> Layout<'_> {
        let mut doc = PdfDocument::new("test");
        let font_id = doc.add_font(font);
        Layout::new(font, font_id)
    }

    fn dejavu() -> ParsedFont {
        let mut warnings = Vec::new();
        ParsedFont::from_bytes(DEJAVU_SANS, 0, &mut warnings)
            .expect("DejaVuSans.ttf is a well-formed embedded font")
    }

    /// The width-aware layout's own load-bearing property (issue #82
    /// review): every line [`Layout::wrap`] returns fits the budget it was
    /// given — checked against `text_width_mm`, the same glyph-metric
    /// calculation `Layout::text_at`'s callers already trust for right
    /// alignment, so this is the layout-calculation-level test the review
    /// asks for, not a guess at how many characters fit. Extracted text
    /// alone (`rendered_text`) cannot show this: two placed lines of
    /// different widths look identical once flattened into one string.
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

    /// A single word that alone exceeds the budget is printed whole rather
    /// than split mid-word — a payslip never hyphenates a name.
    #[test]
    fn wrap_never_splits_a_single_word_even_when_it_exceeds_the_budget() {
        let font = dejavu();
        let layout = a_layout(&font);

        let text = "Supercalifragilisticexpialidocious";
        let lines = layout.wrap(text, BODY_SIZE, 5.0);

        assert_eq!(lines, vec![text.to_string()]);
    }

    /// The property [`row`] exists to guarantee: a label long enough to wrap
    /// still never lets its detail or its amount land on top of it, or on
    /// top of each other — checked against the actual `(x_mm, y_mm)` each
    /// piece of text was placed at, paired straight off `Layout`'s own op
    /// stream (`Layout::text_at`'s doc comment is why a `SetTextCursor`
    /// always immediately precedes the `ShowText` it positions). This is
    /// exactly the placement-level test Deep Instructions ask for: flat
    /// extracted text cannot tell "beside" from "on top of".
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

        let mut placements: Vec<(f32, f32, String)> = Vec::new();
        let mut cursor: Option<(f32, f32)> = None;
        for op in &layout.ops {
            match op {
                Op::SetTextCursor { pos } => cursor = Some((pos.x.0, pos.y.0)),
                Op::ShowText { items } => {
                    let mut text = String::new();
                    for item in items {
                        if let TextItem::Text(s) = item {
                            text.push_str(s);
                        }
                    }
                    if let Some((x_pt, y_pt)) = cursor {
                        placements.push((x_pt / 72.0 * 25.4, y_pt / 72.0 * 25.4, text));
                    }
                }
                _ => {}
            }
        }

        // The label wrapped: at least two lines share the row's own left
        // margin, at two different heights.
        let label_lines: Vec<&(f32, f32, String)> = placements
            .iter()
            .filter(|(x, _, _)| (*x - MARGIN_MM).abs() < 0.01)
            .collect();
        assert!(label_lines.len() >= 2, "{placements:?}");

        let (label_x, label_y, label_text) = label_lines[0];
        let label_right_edge = label_x + layout.text_width_mm(label_text, BODY_SIZE);

        // The detail and the amount both sit on the label's first line —
        // never on a wrapped continuation line — and neither starts before
        // the label (or, for the amount, the detail) it sits beside ends.
        let same_line: Vec<&(f32, f32, String)> = placements
            .iter()
            .filter(|(_, y, _)| (*y - label_y).abs() < 0.01)
            .collect();
        let (detail_x, _, detail_text) = same_line
            .iter()
            .find(|(_, _, text)| text.contains("hrs @"))
            .expect("the detail is drawn on the label's first line");
        let (amount_x, _, _) = same_line
            .iter()
            .find(|(_, _, text)| text.contains("N$"))
            .expect("the amount is drawn on the label's first line");

        assert!(
            *detail_x >= label_right_edge - 0.01,
            "detail at {detail_x} overlaps the label ending at {label_right_edge}: {placements:?}"
        );
        let detail_right_edge = detail_x + layout.text_width_mm(detail_text, SMALL_SIZE);
        assert!(
            *amount_x >= detail_right_edge - 0.01,
            "amount at {amount_x} overlaps the detail ending at {detail_right_edge}: {placements:?}"
        );
    }

    /// A pay line's frozen provenance (issue #82 review) prints beneath its
    /// label — `to_render_input`'s own job is building this sentence;
    /// `render_payslip`'s job, proven here, is only ever to print it
    /// verbatim.
    #[test]
    fn a_pay_line_with_provenance_prints_it_beneath_the_line() {
        let mut input = minimal_input();
        input.earnings.push(PayslipLine {
            label: "Standby allowance".to_string(),
            detail: None,
            provenance: Some("Standing since 01 Mar 2026".to_string()),
            amount_cents: 50_000,
        });

        let bytes = render_payslip(&input).unwrap();
        let text = rendered_text(&bytes);

        assert!(text.contains("Standby allowance"), "{text}");
        assert!(text.contains("Standing since 01 Mar 2026"), "{text}");
    }

    /// A long employer registered name wraps onto more than one line rather
    /// than running past the page's own right margin — proven against real
    /// rendered PDF bytes, through the same `text_placements` this crate's
    /// own HTTP integration tests use.
    #[test]
    fn a_long_employer_name_wraps_instead_of_overflowing_the_page() {
        let mut input = minimal_input();
        input.employer_registered_name =
            "Extraordinarily Long Registered Employer Trading Name (Proprietary) Limited \
             Incorporated In The Republic"
                .to_string();

        let bytes = render_payslip(&input).unwrap();
        let placements = text_placements(&bytes);

        let name_lines: Vec<&TextPlacement> = placements
            .iter()
            .filter(|p| p.text.contains("Extraordinarily") || p.text.contains("Republic"))
            .collect();
        assert!(name_lines.len() >= 2, "{placements:?}");

        let font = dejavu();
        let layout = a_layout(&font);
        let content_width = CONTENT_RIGHT_MM - MARGIN_MM;
        for placement in &name_lines {
            assert!(
                layout.text_width_mm(&placement.text, BODY_SIZE) <= content_width + 0.01,
                "{placement:?} is wider than the {content_width}mm page content width"
            );
        }
    }
}
