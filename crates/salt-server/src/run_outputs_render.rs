//! The Register and Payment Summary PDF renderers (issue #83, parent #70
//! §D-9, §D-10) — pure functions from plain, already-read input structs to
//! PDF bytes, the same seam [`crate::payslip_render`] draws between
//! `payroll_app` and the page: `salt-server`'s `run_outputs.rs` builds a
//! [`RegisterPdfInput`]/[`PaymentSummaryPdfInput`] from
//! `payroll_app::PayrollRegister`/`PaymentSummary` before either renderer
//! here ever runs. No `payroll_app` type appears below.
//!
//! **Unlike a Payslip, these are not versioned templates.** A Payslip is an
//! issued statutory document (ADR-0021): its layout is frozen per
//! `payslip_template_version` because a *past* payslip must keep rendering
//! exactly as it did when it was issued. The Register and Payment Summary
//! are views over live data, generated fresh from the frozen figures every
//! time they are requested — there is no "version" to freeze, and no
//! `render_register`/`render_payment_summary` call ever fails: whatever
//! shape a future review gives this layout, it renders however this build
//! renders it, today.
//!
//! Both use the low-level page cursor, font and money/date formatting
//! [`crate::pdf_layout`] already shares with `payslip_render`. What's here
//! is specific to these two documents: the Register's own multi-column
//! table (landscape — README.md's own "landscape for the register,
//! portrait for the summary") and the Payment Summary's plain name/net-pay
//! rows (portrait).
#![allow(clippy::disallowed_types, clippy::float_arithmetic)]

use chrono::NaiveDate;
use printpdf::{PdfDocument, PdfSaveOptions};

use crate::pdf_layout::{
    A4_LANDSCAPE_MM, A4_PORTRAIT_MM, BODY_SIZE, Layout, SMALL_SIZE, TITLE_SIZE, format_date,
    format_money, heading, line_height_mm, load_font, total_row,
};

/// The ten money fields §D-9's own column list names, in that order — the
/// same fields [`crate::pdf_layout`]-free of any one document's own layout
/// decisions, this module's own plain-data mirror of `payroll_app`'s
/// `PayrollFigures`/`PayrollRegisterTotals`. `taxable_remuneration` is
/// deliberately absent: it feeds PAYE's own calculation but is not one of
/// the columns README.md's Register spec asks this table to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterFiguresInput {
    pub basic_pay_cents: i64,
    pub taxable_allowances_cents: i64,
    pub overtime_cents: i64,
    pub gross_cents: i64,
    pub paye_cents: i64,
    pub employee_social_security_cents: i64,
    pub medical_aid_premium_cents: i64,
    pub total_deductions_cents: i64,
    pub net_pay_cents: i64,
    pub employer_social_security_cents: i64,
}

const MONEY_HEADERS: [&str; 10] = [
    "Basic Pay",
    "Allowances",
    "Overtime",
    "Gross",
    "PAYE",
    "Employee SSC",
    "Medical Aid",
    "Total Ded.",
    "Net Pay",
    "Employer SSC",
];

fn money_values(figures: &RegisterFiguresInput) -> [i64; 10] {
    [
        figures.basic_pay_cents,
        figures.taxable_allowances_cents,
        figures.overtime_cents,
        figures.gross_cents,
        figures.paye_cents,
        figures.employee_social_security_cents,
        figures.medical_aid_premium_cents,
        figures.total_deductions_cents,
        figures.net_pay_cents,
        figures.employer_social_security_cents,
    ]
}

/// Whether one [`RegisterPdfRow`] is still Live, or has been Reversed — the
/// same distinction `payroll_app::FinalizedPayrollLiveness` carries,
/// restated as plain data so this renderer needs no `payroll_app` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterPdfLiveness {
    Live,
    Reversed {
        reason: String,
        replaced_by: Option<String>,
    },
}

/// One row of a printed [`RegisterPdfInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterPdfRow {
    pub full_name: String,
    pub figures: RegisterFiguresInput,
    pub liveness: RegisterPdfLiveness,
    /// Present when this row is itself a Replacement: the id it replaces.
    pub replaces: Option<String>,
}

/// Everything [`render_register`] needs, entirely plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterPdfInput {
    pub payroll_run_id: String,
    /// "Ordinary" or "Correction" — printed beside the period, since no
    /// frozen employer name is available to a Register (README.md: it
    /// prints the run's own period, pay date and kind instead).
    pub run_kind_label: String,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub pay_date: NaiveDate,
    pub rows: Vec<RegisterPdfRow>,
    pub total_as_finalized: RegisterFiguresInput,
    pub total_still_live: RegisterFiguresInput,
}

/// One row of a printed [`PaymentSummaryPdfInput`]: a Live record only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSummaryPdfRow {
    pub full_name: String,
    pub net_pay_cents: i64,
    /// Present when this row is itself a Replacement (issue #83 acceptance
    /// criterion 4): its own note prints beneath the row.
    pub replaces: Option<String>,
}

/// Everything [`render_payment_summary`] needs, entirely plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSummaryPdfInput {
    pub payroll_run_id: String,
    /// Whether this run is a Correction — the one extra sentence README.md
    /// asks a Correction's own summary to add.
    pub is_correction: bool,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub pay_date: NaiveDate,
    pub rows: Vec<PaymentSummaryPdfRow>,
    pub excluded_reversed_count: usize,
    pub total_net_pay_cents: i64,
}

const TABLE_FONT: f32 = 7.0;
const REGISTER_MARGIN_MM: f32 = 14.0;
const COLUMN_PADDING_MM: f32 = 2.5;
const MIN_MONEY_COLUMN_MM: f32 = 14.0;
const ROW_NUM_COLUMN_MM: f32 = 8.0;
const NAME_COLUMN_MIN_MM: f32 = 26.0;
const NAME_COLUMN_MAX_MM: f32 = 44.0;
const STATUS_GAP_MM: f32 = 2.0;

/// The table's own column geometry, computed once from every string it will
/// ever need to print (every data row plus both total rows) before any row
/// is drawn — so every money column is exactly wide enough for its widest
/// value, and two rows can never disagree about where a column starts.
struct TableColumns {
    row_num_x: f32,
    name_x: f32,
    name_width: f32,
    money_right_edges: [f32; 10],
    status_x: f32,
    status_width: f32,
}

fn table_columns(
    layout: &Layout,
    names: &[&str],
    figures: &[&RegisterFiguresInput],
) -> TableColumns {
    let margin = layout.margin_mm();
    let content_right = layout.content_right_mm();

    let row_num_width = ROW_NUM_COLUMN_MM;

    let name_natural = names
        .iter()
        .map(|name| layout.text_width_mm(name, TABLE_FONT))
        .fold(layout.text_width_mm("Name", TABLE_FONT), f32::max);
    let name_width =
        (name_natural + COLUMN_PADDING_MM).clamp(NAME_COLUMN_MIN_MM, NAME_COLUMN_MAX_MM);

    let mut money_width = [MIN_MONEY_COLUMN_MM; 10];
    for (index, header) in MONEY_HEADERS.iter().enumerate() {
        let mut widest = layout.text_width_mm(header, TABLE_FONT);
        for row_figures in figures {
            let value = money_values(row_figures)[index];
            widest = widest.max(layout.text_width_mm(&format_money(value), TABLE_FONT));
        }
        money_width[index] = (widest + COLUMN_PADDING_MM).max(MIN_MONEY_COLUMN_MM);
    }

    let mut x = margin + row_num_width;
    let name_x = x;
    x += name_width;
    let mut money_right_edges = [0.0; 10];
    for (index, width) in money_width.iter().enumerate() {
        x += width;
        money_right_edges[index] = x;
    }
    // A small explicit buffer, never zero: a right-aligned money value's
    // rendered right edge and this column's own start would otherwise sit
    // at the exact same coordinate, and PDF's own limited numeric precision
    // (values round-trip through a fixed-point text encoding, not the exact
    // `f32` this module computed them in) can then place the two labels a
    // hair's width apart — enough to read as "overlapping" to a test that
    // checks real placements, though never enough for a human eye to see.
    let status_x = x + STATUS_GAP_MM;
    let status_width = (content_right - status_x).max(20.0);

    TableColumns {
        row_num_x: margin,
        name_x,
        name_width,
        money_right_edges,
        status_x,
        status_width,
    }
}

fn draw_table_header(layout: &mut Layout, columns: &TableColumns) {
    layout.text_at(columns.row_num_x, TABLE_FONT, "#");
    layout.text_at(columns.name_x, TABLE_FONT, "Name");
    for (header, right_edge) in MONEY_HEADERS.iter().zip(columns.money_right_edges) {
        layout.text_right_at(right_edge, TABLE_FONT, header);
    }
    layout.text_at(columns.status_x, TABLE_FONT, "Status");
    layout.advance(line_height_mm(TABLE_FONT));
    layout.rule();
    layout.advance(1.5);
}

/// Draws one table row: a row number (blank for a total row), a name, ten
/// right-aligned money cells (blank when `figures` is `None`) and a status
/// cell. Both the name and the status wrap within their own column width,
/// and the row's height is computed from the taller of the two before
/// anything is drawn — the same "measure once, draw once" discipline
/// `payslip_render::row` uses, so a wrapped cell can never straddle a page
/// break with the rest of its row left behind. Pagination redraws the table
/// header on the fresh page, since a reader turning to page two of a long
/// Register still needs to know which column is which.
fn draw_table_row(
    layout: &mut Layout,
    columns: &TableColumns,
    row_number: Option<usize>,
    name: &str,
    figures: Option<&RegisterFiguresInput>,
    status: &str,
) {
    let name_lines = layout.wrap(
        name,
        TABLE_FONT,
        (columns.name_width - COLUMN_PADDING_MM).max(1.0),
    );
    let status_lines = layout.wrap(
        status,
        TABLE_FONT,
        (columns.status_width - COLUMN_PADDING_MM).max(1.0),
    );
    let line_height = line_height_mm(TABLE_FONT);
    let lines = name_lines.len().max(status_lines.len()).max(1);
    let needed = lines as f32 * line_height + 1.0;

    if layout.remaining_mm() < needed {
        layout.new_page();
        draw_table_header(layout, columns);
    }

    for index in 0..lines {
        if index == 0 {
            if let Some(row_number) = row_number {
                layout.text_at(columns.row_num_x, TABLE_FONT, &row_number.to_string());
            }
            if let Some(figures) = figures {
                for (value, right_edge) in
                    money_values(figures).iter().zip(columns.money_right_edges)
                {
                    layout.text_right_at(right_edge, TABLE_FONT, &format_money(*value));
                }
            }
        }
        if let Some(line) = name_lines.get(index) {
            layout.text_at(columns.name_x, TABLE_FONT, line);
        }
        if let Some(line) = status_lines.get(index) {
            layout.text_at(columns.status_x, TABLE_FONT, line);
        }
        layout.advance(line_height);
    }
    layout.advance(0.6);
}

/// A row's complete printed status: `Live`, `Reversed`, or `Reversed —
/// replaced by <id>`. The status column wraps this owned text within its
/// measured width; the row-numbered note remains below the table because it
/// also carries the reversal reason.
fn status_text(liveness: &RegisterPdfLiveness) -> String {
    match liveness {
        RegisterPdfLiveness::Live => "Live".to_string(),
        RegisterPdfLiveness::Reversed {
            replaced_by: Some(replacement_id),
            ..
        } => format!("Reversed — replaced by {replacement_id}"),
        RegisterPdfLiveness::Reversed {
            replaced_by: None, ..
        } => "Reversed".to_string(),
    }
}

/// The row-numbered note(s) [`render_register`] prints beneath the table for
/// one row, if any: a Reversed row's own reason (and, once replaced, the
/// replacement's id in the same note), and — independently — a note for a
/// row that is itself a Replacement, naming what it replaces.
fn row_notes(row_number: usize, row: &RegisterPdfRow) -> Vec<String> {
    let mut notes = Vec::new();
    if let RegisterPdfLiveness::Reversed {
        reason,
        replaced_by,
    } = &row.liveness
    {
        let mut note = format!("{row_number}. Reversed: {reason}");
        if let Some(replacement_id) = replaced_by {
            note.push_str(&format!(" — replaced by {replacement_id}"));
        }
        notes.push(note);
    }
    if let Some(replaces) = &row.replaces {
        notes.push(format!("{row_number}. Replaces {replaces}"));
    }
    notes
}

/// `Total as finalized by this run` labelled exactly as README.md asks —
/// over every row this run produced.
const TOTAL_AS_FINALIZED_LABEL: &str = "Total as finalized by this run";
/// `Total still live from this run` — over the Live rows only.
const TOTAL_STILL_LIVE_LABEL: &str = "Total still live from this run";

/// Renders `input` to PDF bytes: A4 landscape, one row per
/// `FinalizedPayroll` this run produced, both totals, and every reversed
/// row's reason printed beneath the table as a row-numbered note (README.md:
/// "a reversed row's reason printed under the table"). Never fails — see
/// this module's own doc comment on why the Register carries no template
/// version to refuse an unknown one of.
pub fn render_register(input: &RegisterPdfInput) -> Vec<u8> {
    let font = load_font();
    let mut doc = PdfDocument::new("Payroll register");
    let font_id = doc.add_font(&font);
    let mut layout = Layout::new(&font, font_id, A4_LANDSCAPE_MM, REGISTER_MARGIN_MM);

    let margin = layout.margin_mm();
    layout.text_at(margin, TITLE_SIZE, "Payroll register");
    layout.advance(8.0);
    layout.paragraph(
        BODY_SIZE,
        &format!("{} run — {}", input.run_kind_label, input.payroll_run_id),
    );
    layout.paragraph(
        BODY_SIZE,
        &format!(
            "Period: {} to {}",
            format_date(input.period_start),
            format_date(input.period_end)
        ),
    );
    layout.paragraph(
        BODY_SIZE,
        &format!("Pay date: {}", format_date(input.pay_date)),
    );
    layout.advance(4.0);

    let names: Vec<&str> = input
        .rows
        .iter()
        .map(|row| row.full_name.as_str())
        .chain([TOTAL_AS_FINALIZED_LABEL, TOTAL_STILL_LIVE_LABEL])
        .collect();
    let figures: Vec<&RegisterFiguresInput> = input
        .rows
        .iter()
        .map(|row| &row.figures)
        .chain([&input.total_as_finalized, &input.total_still_live])
        .collect();
    let columns = table_columns(&layout, &names, &figures);

    draw_table_header(&mut layout, &columns);

    let mut notes: Vec<String> = Vec::new();
    for (index, row) in input.rows.iter().enumerate() {
        let row_number = index + 1;
        notes.extend(row_notes(row_number, row));
        let status = status_text(&row.liveness);
        draw_table_row(
            &mut layout,
            &columns,
            Some(row_number),
            &row.full_name,
            Some(&row.figures),
            &status,
        );
    }

    draw_table_row(
        &mut layout,
        &columns,
        None,
        TOTAL_AS_FINALIZED_LABEL,
        Some(&input.total_as_finalized),
        "",
    );
    draw_table_row(
        &mut layout,
        &columns,
        None,
        TOTAL_STILL_LIVE_LABEL,
        Some(&input.total_still_live),
        "",
    );

    if !notes.is_empty() {
        layout.advance(4.0);
        layout.paragraph(BODY_SIZE, "Notes");
        for note in &notes {
            layout.paragraph(SMALL_SIZE, note);
        }
    }

    let pages = layout.finish();
    doc.with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

const SUMMARY_AMOUNT_COLUMN_MM: f32 = 30.0;

/// A name-and-amount row that wraps its label rather than letting a long
/// employee name run into the right-aligned figure beside it — the same
/// "measure once, draw once" shape `payslip_render::row` uses for a pay
/// line, narrowed to the one column a Payment Summary row needs.
fn summary_row(layout: &mut Layout, label: &str, amount_cents: i64) {
    let margin = layout.margin_mm();
    let content_right = layout.content_right_mm();
    let amount = format_money(amount_cents);
    let amount_width = SUMMARY_AMOUNT_COLUMN_MM.max(layout.text_width_mm(&amount, BODY_SIZE));
    let label_max_width = (content_right - margin - amount_width - 4.0).max(30.0);
    let label_lines = layout.wrap(label, BODY_SIZE, label_max_width);
    let line_height = line_height_mm(BODY_SIZE);
    layout.ensure_room(label_lines.len() as f32 * line_height + 1.0);
    for (index, line) in label_lines.iter().enumerate() {
        layout.text_at(margin, BODY_SIZE, line);
        if index == 0 {
            layout.text_right_at(content_right, BODY_SIZE, &amount);
        }
        layout.advance(line_height);
    }
    layout.advance(1.0);
}

/// The instruction sentence README.md asks to be printed "near the top, in
/// body size, not small print" — issue #83 acceptance criterion 3.
const NOT_A_BANK_FILE_SENTENCE: &str = "This is an instruction to a person. Producing it does \
     not mean anyone has been paid, and finalizing payroll does not mean a bank transfer \
     happened. This is not a bank file.";
/// Printed beneath every Replacement row's own name/net-pay line (issue #83
/// acceptance criterion 4).
const REPLACEMENT_SENTENCE: &str = "This replaces an earlier payroll for the same period. The \
     original may already have been paid. A person must decide the actual transfer.";
/// The one extra sentence a Correction run's own summary adds.
const CORRECTION_SENTENCE: &str = "Reversing a payroll in Salt does not recover money already \
     paid and does not amend a submitted statutory return.";

/// Renders `input` to PDF bytes: A4 portrait, names and net pay for Live
/// records only, the count excluded as reversed (always printed, even when
/// zero), and every acceptance-criterion sentence README.md's own Payment
/// Summary spec names. Never fails, for the same reason [`render_register`]
/// does not.
pub fn render_payment_summary(input: &PaymentSummaryPdfInput) -> Vec<u8> {
    let font = load_font();
    let mut doc = PdfDocument::new("Payment summary");
    let font_id = doc.add_font(&font);
    let mut layout = Layout::new(&font, font_id, A4_PORTRAIT_MM, 18.0);

    let margin = layout.margin_mm();
    layout.text_at(margin, TITLE_SIZE, "Payment summary");
    layout.advance(9.0);
    layout.paragraph(
        BODY_SIZE,
        &format!(
            "Period: {} to {}",
            format_date(input.period_start),
            format_date(input.period_end)
        ),
    );
    layout.paragraph(
        BODY_SIZE,
        &format!("Pay date: {}", format_date(input.pay_date)),
    );
    layout.advance(3.5);

    layout.paragraph(BODY_SIZE, NOT_A_BANK_FILE_SENTENCE);
    layout.advance(3.0);
    if input.is_correction {
        layout.paragraph(BODY_SIZE, CORRECTION_SENTENCE);
        layout.advance(3.0);
    }

    heading(&mut layout, "Live records");
    for row in &input.rows {
        summary_row(&mut layout, &row.full_name, row.net_pay_cents);
        if let Some(replaces) = &row.replaces {
            layout.paragraph(
                SMALL_SIZE,
                &format!("Replaces finalized payroll {replaces}. {REPLACEMENT_SENTENCE}"),
            );
            layout.advance(1.0);
        }
    }
    layout.advance(1.5);
    total_row(&mut layout, "Total net pay", input.total_net_pay_cents);
    layout.advance(4.0);

    layout.paragraph(
        BODY_SIZE,
        &format!("Excluded as reversed: {}", input.excluded_reversed_count),
    );

    let pages = layout.finish();
    doc.with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_layout::rendered_text;

    fn a_figures(net_pay_cents: i64) -> RegisterFiguresInput {
        RegisterFiguresInput {
            basic_pay_cents: 1_500_000,
            taxable_allowances_cents: 0,
            overtime_cents: 0,
            gross_cents: 1_500_000,
            paye_cents: 120_000,
            employee_social_security_cents: 4_500,
            medical_aid_premium_cents: 0,
            total_deductions_cents: 124_500,
            net_pay_cents,
            employer_social_security_cents: 4_500,
        }
    }

    fn a_register_row(name: &str, net_pay_cents: i64) -> RegisterPdfRow {
        RegisterPdfRow {
            full_name: name.to_string(),
            figures: a_figures(net_pay_cents),
            liveness: RegisterPdfLiveness::Live,
            replaces: None,
        }
    }

    fn minimal_register() -> RegisterPdfInput {
        RegisterPdfInput {
            payroll_run_id: "11111111-1111-1111-1111-111111111111".to_string(),
            run_kind_label: "Ordinary".to_string(),
            period_start: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            pay_date: NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
            rows: vec![
                a_register_row("Ada Lovelace", 1_375_500),
                a_register_row("Bob Marley", 1_375_500),
            ],
            total_as_finalized: a_figures(2_751_000),
            total_still_live: a_figures(2_751_000),
        }
    }

    #[test]
    fn both_total_labels_print_with_their_own_net_pay_figure() {
        let text = rendered_text(&render_register(&minimal_register()));

        assert!(text.contains(TOTAL_AS_FINALIZED_LABEL), "{text}");
        assert!(text.contains(TOTAL_STILL_LIVE_LABEL), "{text}");
        assert_eq!(text.matches("N$ 27,510.00").count(), 2, "{text}");
    }

    #[test]
    fn a_reversed_row_says_reversed_and_prints_its_reason_beneath_the_table() {
        let mut input = minimal_register();
        input.rows[0].liveness = RegisterPdfLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            replaced_by: None,
        };

        let text = rendered_text(&render_register(&input));

        assert!(text.contains("Reversed"), "{text}");
        assert!(text.contains("March salary was wrong"), "{text}");
    }

    #[test]
    fn a_reversed_and_replaced_row_status_names_the_replacement() {
        let liveness = RegisterPdfLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            replaced_by: Some("22222222-2222-2222-2222-222222222222".to_string()),
        };

        assert_eq!(
            status_text(&liveness),
            "Reversed — replaced by 22222222-2222-2222-2222-222222222222"
        );
    }

    #[test]
    fn a_reversed_and_replaced_row_keeps_its_reason_in_the_row_numbered_note() {
        let mut input = minimal_register();
        input.rows[0].liveness = RegisterPdfLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            replaced_by: Some("22222222-2222-2222-2222-222222222222".to_string()),
        };

        let text = rendered_text(&render_register(&input));

        assert!(
            text.contains("replaced by 22222222-2222-2222-2222-222222222222"),
            "{text}"
        );
    }

    #[test]
    fn a_replacement_row_names_what_it_replaces_in_its_row_numbered_note() {
        let mut input = minimal_register();
        input.rows[0].replaces = Some("33333333-3333-3333-3333-333333333333".to_string());

        let text = rendered_text(&render_register(&input));

        assert!(
            text.contains("Replaces 33333333-3333-3333-3333-333333333333"),
            "{text}"
        );
    }

    /// A 50-row Register spans several pages; no two text runs on the same
    /// baseline of the same page may overlap, and nothing may print past
    /// either margin — the same placement-level property
    /// `payslip_render`'s own extreme-payslip test proves for a Payslip.
    #[test]
    fn a_fifty_row_register_paginates_without_overlapping_text() {
        let mut input = minimal_register();
        input.rows = (0..50)
            .map(|index| a_register_row(&format!("Employee Number {index}"), 1_375_500 + index))
            .collect();
        input.rows[0].liveness = RegisterPdfLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            replaced_by: Some("22222222-2222-2222-2222-222222222222".to_string()),
        };

        let bytes = render_register(&input);
        let placements = crate::pdf_layout::text_placements(&bytes);
        assert!(
            placements.iter().any(|p| p.page > 0),
            "the fixture must be long enough to need a second page"
        );

        let font = load_font();
        let mut doc = PdfDocument::new("measure");
        let font_id = doc.add_font(&font);
        let layout = Layout::new(&font, font_id, A4_LANDSCAPE_MM, REGISTER_MARGIN_MM);

        let mut runs = Vec::new();
        for placement in &placements {
            let left = placement.x_mm;
            let right = left + layout.text_width_mm(&placement.text, placement.size_pt);
            assert!(
                left >= REGISTER_MARGIN_MM - 0.01,
                "{placement:?} starts left of the margin"
            );
            assert!(
                right <= layout.content_right_mm() + 0.01,
                "{placement:?} ends at {right}mm, past the right margin"
            );
            assert!(
                placement.y_mm >= REGISTER_MARGIN_MM - 0.01,
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
    }

    fn minimal_summary() -> PaymentSummaryPdfInput {
        PaymentSummaryPdfInput {
            payroll_run_id: "11111111-1111-1111-1111-111111111111".to_string(),
            is_correction: false,
            period_start: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            pay_date: NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
            rows: vec![PaymentSummaryPdfRow {
                full_name: "Ada Lovelace".to_string(),
                net_pay_cents: 1_375_500,
                replaces: None,
            }],
            excluded_reversed_count: 0,
            total_net_pay_cents: 1_375_500,
        }
    }

    #[test]
    fn the_summary_always_states_nobody_has_been_paid_and_the_excluded_count() {
        let text = rendered_text(&render_payment_summary(&minimal_summary()));

        assert!(
            text.contains("does not mean anyone has been paid"),
            "{text}"
        );
        assert!(text.contains("This is not a bank file."), "{text}");
        assert!(text.contains("Excluded as reversed: 0"), "{text}");
    }

    #[test]
    fn a_nonzero_excluded_count_prints_that_number() {
        let mut input = minimal_summary();
        input.excluded_reversed_count = 3;

        let text = rendered_text(&render_payment_summary(&input));

        assert!(text.contains("Excluded as reversed: 3"), "{text}");
    }

    #[test]
    fn the_replacement_note_prints_only_for_a_row_that_replaces_another() {
        let mut input = minimal_summary();
        input.rows.push(PaymentSummaryPdfRow {
            full_name: "Bob Marley".to_string(),
            net_pay_cents: 1_200_000,
            replaces: Some("33333333-3333-3333-3333-333333333333".to_string()),
        });

        let text = rendered_text(&render_payment_summary(&input));

        assert!(
            text.contains("Replaces finalized payroll 33333333-3333-3333-3333-333333333333"),
            "{text}"
        );
        assert_eq!(
            text.matches("original may already have been paid").count(),
            1,
            "{text}"
        );
    }

    #[test]
    fn a_correction_run_adds_its_own_extra_sentence() {
        let mut input = minimal_summary();
        input.is_correction = true;

        let text = rendered_text(&render_payment_summary(&input));

        assert!(
            text.contains("does not recover money already paid"),
            "{text}"
        );
    }

    #[test]
    fn an_ordinary_run_never_prints_the_correction_sentence() {
        let text = rendered_text(&render_payment_summary(&minimal_summary()));

        assert!(
            !text.contains("does not recover money already paid"),
            "{text}"
        );
    }
}
