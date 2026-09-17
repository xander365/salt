//! Shared low-level PDF drawing primitives (issue #83): the pieces
//! `payslip_render` (issue #82) and `run_outputs_render` (issue #83) both
//! need — font loading, a page-cursor [`Layout`], money/date/instant
//! formatting, a [`heading`]/[`total_row`] pair, and the text-placement
//! parser tests use to read a rendered PDF back out. Kept free of any one
//! document's own layout decisions (a Payslip's pay-line row, a Register's
//! table row): those stay in their own module, built out of what this one
//! exports. [`Layout`] itself carries no fixed page size — a caller picks
//! portrait or landscape (a Payslip is always portrait; the Register prints
//! landscape for its column count) via [`Layout::new`]'s `page_size_mm`.
//!
//! INV-001 bans `f32`/`f64` workspace-wide so money can never silently
//! become inexact — but this module's own money is always `i64` cents,
//! exactly as everywhere else in Salt ([`format_money`] does its rounding
//! and grouping in integer arithmetic). The float type this module *does*
//! use is `printpdf`'s own coordinate system: `Pt`/`Mm` are `f32` newtypes
//! baked into that crate's public API, for page geometry and font metrics
//! that were never money to begin with.
#![allow(clippy::disallowed_types, clippy::float_arithmetic)]

use chrono::{DateTime, NaiveDate, Utc};
use printpdf::{
    Color, FontId, Line, LinePoint, Mm, Op, ParsedFont, PdfFontHandle, PdfPage, Point, Pt, Rgb,
    TextItem,
};
#[cfg(any(test, feature = "test-support"))]
use printpdf::{PdfDocument, PdfParseOptions, PdfWarnMsg};

/// DejaVu Sans, embedded (ADR-0021: "embed the font file in the
/// repository", never a system font). `crates/salt-server/assets/fonts/`
/// carries its own `LICENSE.txt`. One weight only — hierarchy on the page
/// comes from size and spacing, not a second embedded font, so ADR-0021's
/// "every font it needs, kept alive forever" stays a promise about one file.
static DEJAVU_SANS: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

pub(crate) fn load_font() -> ParsedFont {
    let mut warnings = Vec::new();
    ParsedFont::from_bytes(DEJAVU_SANS, 0, &mut warnings)
        .expect("DejaVuSans.ttf is a well-formed embedded font — see assets/fonts/LICENSE.txt")
}

pub(crate) const BODY_SIZE: f32 = 10.0;
pub(crate) const SMALL_SIZE: f32 = 8.5;
pub(crate) const HEADING_SIZE: f32 = 12.0;
pub(crate) const TITLE_SIZE: f32 = 16.0;

/// A4, portrait — every Payslip page.
pub(crate) const A4_PORTRAIT_MM: (f32, f32) = (210.0, 297.0);
/// A4, landscape — the Register's own page (README.md's own "landscape for
/// the register, portrait for the summary").
pub(crate) const A4_LANDSCAPE_MM: (f32, f32) = (297.0, 210.0);

/// The line height a single wrapped line of body-sized text advances the
/// cursor by — the same spacing an address line already used before
/// wrapping existed.
const LINE_HEIGHT_MM: f32 = 4.2;
/// As [`LINE_HEIGHT_MM`], for a wrapped line of small-sized text (a
/// provenance note, a reversal reason continuation, a table cell).
const SMALL_LINE_HEIGHT_MM: f32 = 3.8;
/// The cursor advance a single-line pay-line or table row uses between its
/// own lines — taller than [`SMALL_LINE_HEIGHT_MM`] alone, to keep a dense
/// row of figures legible.
pub(crate) const ROW_LINE_HEIGHT_MM: f32 = 4.6;

/// The cursor advance for one wrapped line of `size_pt` text.
pub(crate) fn line_height_mm(size_pt: f32) -> f32 {
    if size_pt <= SMALL_SIZE {
        SMALL_LINE_HEIGHT_MM
    } else {
        LINE_HEIGHT_MM
    }
}

/// Builds one page's `Op` list at a time, tracking a top-down cursor in mm
/// and closing/reopening the PDF text object around anything that is not
/// text (a rule) — a raw PDF content stream cannot paint a path while a
/// `BT`/`ET` text object is open.
pub(crate) struct Layout<'a> {
    font: &'a ParsedFont,
    font_id: FontId,
    page_width_mm: f32,
    page_height_mm: f32,
    margin_mm: f32,
    content_right_mm: f32,
    bottom_limit_mm: f32,
    pages: Vec<PdfPage>,
    ops: Vec<Op>,
    in_text: bool,
    y_mm: f32,
}

impl<'a> Layout<'a> {
    pub(crate) fn new(
        font: &'a ParsedFont,
        font_id: FontId,
        page_size_mm: (f32, f32),
        margin_mm: f32,
    ) -> Self {
        let (page_width_mm, page_height_mm) = page_size_mm;
        Self {
            font,
            font_id,
            page_width_mm,
            page_height_mm,
            margin_mm,
            content_right_mm: page_width_mm - margin_mm,
            bottom_limit_mm: page_height_mm - margin_mm,
            pages: Vec::new(),
            ops: Vec::new(),
            in_text: false,
            y_mm: margin_mm,
        }
    }

    pub(crate) fn margin_mm(&self) -> f32 {
        self.margin_mm
    }

    pub(crate) fn content_right_mm(&self) -> f32 {
        self.content_right_mm
    }

    /// How much room is left above the bottom margin on the current page —
    /// what a caller with its own row/table logic (`run_outputs_render`)
    /// checks before deciding to start a new page and redraw a table header,
    /// rather than reaching into [`Self::ensure_room`]'s private threshold.
    pub(crate) fn remaining_mm(&self) -> f32 {
        self.bottom_limit_mm - self.y_mm
    }

    fn pdf_y(&self) -> f32 {
        self.page_height_mm - self.y_mm
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
    /// bottom margin.
    pub(crate) fn ensure_room(&mut self, needed_mm: f32) {
        if self.y_mm + needed_mm > self.bottom_limit_mm {
            self.new_page();
        }
    }

    pub(crate) fn new_page(&mut self) {
        self.end_text();
        let ops = std::mem::take(&mut self.ops);
        self.pages.push(PdfPage::new(
            Mm(self.page_width_mm),
            Mm(self.page_height_mm),
            ops,
        ));
        self.y_mm = self.margin_mm;
    }

    pub(crate) fn text_width_mm(&self, text: &str, size_pt: f32) -> f32 {
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
    /// explicitly, since a row often draws several cells at the same `y`.
    ///
    /// Closes and reopens the text object around every single placement,
    /// deliberately: a raw PDF `Td` moves *relative to the current line's*
    /// text matrix, not to the page origin, so leaving one text object open
    /// across several `SetTextCursor`/`ShowText` pairs would make each
    /// absolute `(x_mm, y_mm)` this module computes land at the *sum* of
    /// every position before it. A fresh `BT` resets the text matrix to
    /// identity, which is what makes the very next `Td` land exactly where
    /// this layout says it should.
    pub(crate) fn text_at(&mut self, x_mm: f32, size_pt: f32, text: &str) {
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
    pub(crate) fn text_right_at(&mut self, right_edge_mm: f32, size_pt: f32, text: &str) {
        let width = self.text_width_mm(text, size_pt);
        self.text_at(right_edge_mm - width, size_pt, text);
    }

    /// Greedy word-wrap: every returned line's rendered width is at most
    /// `max_width_mm`. Words break at whitespace; a single word that alone
    /// exceeds the budget is split between characters instead, because text
    /// running off the page loses content, and a broken word does not.
    /// Always returns at least one line (empty for empty input).
    pub(crate) fn wrap(&self, text: &str, size_pt: f32, max_width_mm: f32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if self.text_width_mm(&candidate, size_pt) <= max_width_mm {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if self.text_width_mm(word, size_pt) <= max_width_mm {
                current = word.to_string();
                continue;
            }
            for ch in word.chars() {
                let mut candidate = current.clone();
                candidate.push(ch);
                // A budget narrower than one glyph still makes progress:
                // the glyph goes on a line of its own.
                if current.is_empty() || self.text_width_mm(&candidate, size_pt) <= max_width_mm {
                    current = candidate;
                } else {
                    lines.push(std::mem::replace(&mut current, ch.to_string()));
                }
            }
        }
        if !current.is_empty() || lines.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// Draws `text` word-wrapped to `max_width_mm`, left-aligned at `x_mm`,
    /// one line at a time — each line pays for its own room via
    /// [`Self::ensure_room`] before it is drawn.
    pub(crate) fn text_wrapped(&mut self, x_mm: f32, max_width_mm: f32, size_pt: f32, text: &str) {
        let line_height = line_height_mm(size_pt);
        for line in self.wrap(text, size_pt, max_width_mm) {
            self.ensure_room(line_height);
            self.text_at(x_mm, size_pt, &line);
            self.advance(line_height);
        }
    }

    /// One full-width, wrapped, page-break-aware line of text at the left
    /// margin.
    pub(crate) fn paragraph(&mut self, size_pt: f32, text: &str) {
        let margin = self.margin_mm;
        let right = self.content_right_mm;
        self.text_wrapped(margin, right - margin, size_pt, text);
    }

    pub(crate) fn rule(&mut self) {
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
                        p: Point::new(Mm(self.margin_mm), Mm(self.pdf_y())),
                        bezier: false,
                    },
                    LinePoint {
                        p: Point::new(Mm(self.content_right_mm), Mm(self.pdf_y())),
                        bezier: false,
                    },
                ],
                is_closed: false,
            },
        });
    }

    pub(crate) fn advance(&mut self, mm: f32) {
        self.y_mm += mm;
    }

    /// The in-progress ops of the current page, before [`Self::finish`]
    /// closes it out — needed only by a caller's own placement test that
    /// must inspect a [`Layout`] mid-draw, without ending it.
    #[cfg(test)]
    pub(crate) fn test_only_ops(&self) -> &[Op] {
        &self.ops
    }

    pub(crate) fn finish(mut self) -> Vec<PdfPage> {
        self.end_text();
        if !self.ops.is_empty() || self.pages.is_empty() {
            let ops = std::mem::take(&mut self.ops);
            self.pages.push(PdfPage::new(
                Mm(self.page_width_mm),
                Mm(self.page_height_mm),
                ops,
            ));
        }
        self.pages
    }
}

/// `N$ 12,345.67` — cents formatted with a thousands separator, the
/// convention Namibian currency figures already use elsewhere on a Salt
/// screen. Never a locale-sensitive formatter: the figure a document prints
/// must read the same however this process is configured.
pub(crate) fn format_money(cents: i64) -> String {
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

pub(crate) fn format_date(date: NaiveDate) -> String {
    date.format("%d %b %Y").to_string()
}

/// `12 May 2026, 21:30 UTC` — an instant, with its zone named.
pub(crate) fn format_instant(instant: DateTime<Utc>) -> String {
    instant.format("%d %b %Y, %H:%M UTC").to_string()
}

pub(crate) fn heading(layout: &mut Layout, text: &str) {
    // The heading's own 9mm plus one row beneath it, so a heading is never
    // left alone at the foot of a page with its first line on the next.
    layout.ensure_room(9.0 + ROW_LINE_HEIGHT_MM + 1.5);
    layout.text_at(layout.margin_mm(), HEADING_SIZE, text);
    layout.advance(5.5);
    layout.rule();
    layout.advance(3.5);
}

pub(crate) fn total_row(layout: &mut Layout, label: &str, amount_cents: i64) {
    layout.ensure_room(7.0);
    layout.text_at(layout.margin_mm(), BODY_SIZE, label);
    let right = layout.content_right_mm();
    layout.text_right_at(right, BODY_SIZE, &format_money(amount_cents));
    layout.advance(7.0);
}

/// One placed text run: the page index it was drawn on, its `(x_mm, y_mm)`
/// origin (as [`Layout::text_at`] placed it — bottom-left, PDF coordinates),
/// the font size it was set in, and the text itself.
///
/// This is what lets a test check *placement* — that a wrapped label never
/// overlaps the detail or amount beside it, that no line lands past the
/// page's own margins — which a flat extracted string cannot show, since
/// flattening every `ShowText` into one string cannot tell two overlapping
/// lines from two lines stacked cleanly one above the other.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq)]
pub struct TextPlacement {
    pub page: usize,
    pub x_mm: f32,
    pub y_mm: f32,
    pub size_pt: f32,
    pub text: String,
}

/// Every printed text run of a rendered PDF, in order, read by parsing the
/// PDF back — Deep Instructions' own "verify by extracting the rendered
/// values and comparing them, never by hashing bytes". The one parser every
/// PDF renderer's unit tests and this crate's HTTP integration tests share
/// (issue #82 review). `cfg`-gated like `router.rs`'s own `test-support`
/// feature.
#[cfg(any(test, feature = "test-support"))]
pub fn text_placements(bytes: &[u8]) -> Vec<TextPlacement> {
    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    let doc = PdfDocument::parse(bytes, &PdfParseOptions::default(), &mut warnings)
        .expect("a renderer built on this module must always produce a parseable PDF");
    doc.pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, page)| placements_in_ops(page_index, &page.ops))
        .collect()
}

/// Every printed text run of a rendered PDF, joined into one string with a
/// space between runs — for asserting what a document says, and for
/// comparing two renders' content.
#[cfg(any(test, feature = "test-support"))]
pub fn rendered_text(bytes: &[u8]) -> String {
    text_placements(bytes)
        .into_iter()
        .map(|placement| placement.text)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pairs every `Op::ShowText` with the `Op::SetTextCursor` and `Op::SetFont`
/// before it. Safe to rely on positionally because [`Layout::text_at`]
/// emits exactly that triple for *every single placement* (its own doc
/// comment explains why). Works on `Layout`'s own ops and on ops parsed
/// back out of PDF bytes alike.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn placements_in_ops(page: usize, ops: &[Op]) -> Vec<TextPlacement> {
    let mut placements = Vec::new();
    let mut cursor: Option<(f32, f32)> = None;
    let mut size_pt = 0.0;
    for op in ops {
        match op {
            Op::SetFont { size, .. } => size_pt = size.0,
            Op::SetTextCursor { pos } => cursor = Some((pos.x.0, pos.y.0)),
            Op::ShowText { items } => {
                let mut text = String::new();
                for item in items {
                    match item {
                        TextItem::Text(s) => text.push_str(s),
                        // External fonts round-trip as glyph ids, each
                        // carrying the character it decodes to via the
                        // embedded ToUnicode CMap — this is where that text
                        // actually comes back out.
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
                        page,
                        x_mm: x_pt / 72.0 * 25.4,
                        y_mm: y_pt / 72.0 * 25.4,
                        size_pt,
                        text,
                    });
                }
            }
            _ => {}
        }
    }
    placements
}

/// Asserts that a rendered PDF laid out on `page_size_mm` with
/// `margin_mm` keeps every text run inside the left, right and bottom
/// margins, and that no two runs on the same baseline of the same page
/// overlap — the one placement-level check every renderer's
/// long-content test shares (issue #83 hardening). Returns the placements
/// so a caller can assert more.
#[cfg(test)]
pub(crate) fn assert_text_fits_without_overlap(
    bytes: &[u8],
    page_size_mm: (f32, f32),
    margin_mm: f32,
) -> Vec<TextPlacement> {
    let placements = text_placements(bytes);
    let font = load_font();
    let mut doc = PdfDocument::new("measure");
    let font_id = doc.add_font(&font);
    let layout = Layout::new(&font, font_id, page_size_mm, margin_mm);

    let mut runs = Vec::new();
    for placement in &placements {
        let left = placement.x_mm;
        let right = left + layout.text_width_mm(&placement.text, placement.size_pt);
        assert!(
            left >= margin_mm - 0.01,
            "{placement:?} starts left of the margin"
        );
        assert!(
            right <= layout.content_right_mm() + 0.01,
            "{placement:?} ends at {right}mm, past the right margin"
        );
        assert!(
            placement.y_mm >= margin_mm - 0.01,
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
    placements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> ParsedFont {
        load_font()
    }

    fn a_layout(font: &ParsedFont) -> Layout<'_> {
        let mut doc = PdfDocument::new("test");
        let font_id = doc.add_font(font);
        Layout::new(font, font_id, A4_PORTRAIT_MM, 18.0)
    }

    #[test]
    fn format_money_groups_thousands_and_keeps_two_decimal_places() {
        assert_eq!(format_money(0), "N$ 0.00");
        assert_eq!(format_money(1), "N$ 0.01");
        assert_eq!(format_money(-1), "N$ -0.01");
        assert_eq!(format_money(150_000), "N$ 1,500.00");
        assert_eq!(format_money(123_456_789), "N$ 1,234,567.89");
    }

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
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn wrap_breaks_a_word_that_alone_exceeds_the_budget() {
        let font = dejavu();
        let layout = a_layout(&font);

        let word = "Supercalifragilisticexpialidocious".repeat(4);
        let max_width_mm = 40.0;
        let lines = layout.wrap(&format!("Before {word} after"), BODY_SIZE, max_width_mm);

        assert!(lines.len() > 2, "{lines:?}");
        for line in &lines {
            assert!(
                layout.text_width_mm(line, BODY_SIZE) <= max_width_mm + 0.01,
                "{line:?} is wider than the {max_width_mm}mm budget"
            );
        }
        assert_eq!(lines.concat(), format!("Before{word}after"));
        assert_eq!(lines.first().map(String::as_str), Some("Before"));
    }

    #[test]
    fn wrap_of_empty_text_is_one_empty_line() {
        let font = dejavu();
        let layout = a_layout(&font);

        assert_eq!(layout.wrap("", BODY_SIZE, 50.0), vec![String::new()]);
    }

    #[test]
    fn a_landscape_layout_reports_a_wider_content_right_than_a_portrait_one() {
        let font = dejavu();
        let mut portrait_doc = PdfDocument::new("test");
        let portrait_font_id = portrait_doc.add_font(&font);
        let portrait = Layout::new(&font, portrait_font_id, A4_PORTRAIT_MM, 18.0);

        let mut landscape_doc = PdfDocument::new("test");
        let landscape_font_id = landscape_doc.add_font(&font);
        let landscape = Layout::new(&font, landscape_font_id, A4_LANDSCAPE_MM, 18.0);

        assert!(landscape.content_right_mm() > portrait.content_right_mm());
    }
}
