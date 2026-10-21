//! VS Code-style line-number gutter painted beside the editor.
//!
//! The editor reserves room for the gutter through its left margin; this
//! module only measures that room and paints the numbers, and a marker on
//! each bookmarked line, into it.

use eframe::egui::{self, Align2, Color32, FontId, Galley, Rect, pos2};

use crate::bookmarks::Bookmarks;
use crate::tab_bar::TabPalette;
use crate::theme;

/// Space between the left edge and the widest number.
const PAD_LEFT: f32 = 8.0;
/// Space between the numbers and the text.
const PAD_RIGHT: f32 = 16.0;
/// Room is kept for at least this many digits, so the text does not shift
/// sideways while a short document grows past 9 or 99 lines.
const MIN_DIGITS: usize = 3;
/// The bookmark marker: a bar in the left padding, this far from the edge
/// and this wide.
const MARK_INSET: f32 = 2.0;
const MARK_WIDTH: f32 = 4.0;

/// Number colours. `DARK` and `LIGHT` are VS Code's "Dark Modern" and
/// "Light Modern"; the other themes derive theirs in `theme.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GutterPalette {
    pub number: Color32,
    /// The number of the line holding the caret.
    pub number_active: Color32,
    /// The marker beside a bookmarked line.
    pub bookmark: Color32,
    /// Same as the editor, so the gutter reads as part of it.
    pub background: Color32,
}

impl GutterPalette {
    pub const DARK: GutterPalette = GutterPalette {
        number: Color32::from_rgb(0x6E, 0x76, 0x81),
        number_active: Color32::from_rgb(0xCC, 0xCC, 0xCC),
        bookmark: Color32::from_rgb(0x37, 0x94, 0xFF),
        background: TabPalette::DARK.tab_active,
    };

    pub const LIGHT: GutterPalette = GutterPalette {
        number: Color32::from_rgb(0x6E, 0x76, 0x81),
        number_active: Color32::from_rgb(0x17, 0x11, 0x84),
        bookmark: Color32::from_rgb(0x00, 0x5F, 0xB8),
        background: TabPalette::LIGHT.tab_active,
    };
}

/// Number of lines in `text`. A trailing `\n` opens one more, empty line, as
/// the caret can sit there.
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Digits the gutter makes room for when the last line is `lines`.
fn digits(lines: usize) -> usize {
    (lines.max(1).ilog10() as usize + 1).max(MIN_DIGITS)
}

/// Width of the gutter for a document of `lines` lines, in whole points so
/// it can serve as the editor's left margin (an `i8`).
pub fn width(ui: &egui::Ui, font_id: &FontId, lines: usize) -> f32 {
    let digit = ui.fonts_mut(|f| f.glyph_width(font_id, '0'));
    (PAD_LEFT + digits(lines) as f32 * digit + PAD_RIGHT)
        .ceil()
        .min(i8::MAX as f32)
}

/// 1-based line holding the character at `char_index`.
fn line_of_char(galley: &Galley, char_index: usize) -> usize {
    let mut line = 1;
    let mut chars = 0;
    for row in &galley.rows {
        chars += row.char_count_including_newline().0;
        if chars > char_index {
            break;
        }
        if row.ends_with_newline {
            line += 1;
        }
    }
    line
}

/// One number to paint.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Number {
    line: usize,
    /// Vertical extent of the line's first row, in screen coordinates.
    row: egui::Rangef,
    active: bool,
    bookmarked: bool,
}

/// The numbers of the lines whose first row overlaps `visible`.
///
/// With word wrap a line spans several rows; only its first row is numbered.
fn visible_numbers(
    galley: &Galley,
    galley_top: f32,
    visible: egui::Rangef,
    active_line: Option<usize>,
    bookmarks: &Bookmarks,
) -> Vec<Number> {
    let mut numbers = Vec::new();
    let mut line = 1;
    let mut starts_line = true;
    for row in &galley.rows {
        let rect = row.rect().translate(egui::vec2(0.0, galley_top));
        if rect.top() > visible.max {
            break;
        }
        if starts_line && rect.bottom() >= visible.min {
            numbers.push(Number {
                line,
                row: rect.y_range(),
                active: active_line == Some(line),
                bookmarked: bookmarks.contains(line),
            });
        }
        starts_line = row.ends_with_newline;
        if row.ends_with_newline {
            line += 1;
        }
    }
    numbers
}

/// Paints the gutter over the left `width` points of the editor's viewport.
///
/// The gutter sticks to the left edge of the viewport: when the editor is
/// scrolled sideways the text slides underneath it, as in VS Code.
pub fn paint(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    font_id: &FontId,
    width: f32,
    bookmarks: &Bookmarks,
) {
    let palette = theme::active(ui.ctx()).gutter;
    let clip = ui.clip_rect();
    let left = output.response.response.rect.left().max(clip.left());
    let rect = Rect::from_x_y_ranges(left..=left + width, clip.y_range());
    let painter = ui.painter().with_clip_rect(rect.intersect(clip));
    painter.rect_filled(rect, 0.0, palette.background);

    // `cursor_range` is only set while the editor has focus; the stored
    // cursor keeps the caret's line lit when focus moves elsewhere.
    let active_line = output
        .cursor_range
        .or_else(|| output.state.cursor.range(&output.galley))
        .map(|range| line_of_char(&output.galley, range.primary.index.0));

    let x = rect.right() - PAD_RIGHT;
    for number in visible_numbers(
        &output.galley,
        output.galley_pos.y,
        clip.y_range(),
        active_line,
        bookmarks,
    ) {
        if number.bookmarked {
            let mark_left = rect.left() + MARK_INSET;
            let mark = Rect::from_x_y_ranges(mark_left..=mark_left + MARK_WIDTH, number.row);
            painter.rect_filled(mark, 1.0, palette.bookmark);
        }
        let color = if number.active {
            palette.number_active
        } else {
            palette.number
        };
        painter.text(
            pos2(x, number.row.center()),
            Align2::RIGHT_CENTER,
            number.line.to_string(),
            font_id.clone(),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_palettes_match_vscode_modern_themes() {
        assert_eq!(
            GutterPalette::DARK.number,
            Color32::from_rgb(0x6E, 0x76, 0x81)
        );
        assert_eq!(
            GutterPalette::DARK.number_active,
            Color32::from_rgb(0xCC, 0xCC, 0xCC)
        );
        assert_eq!(GutterPalette::DARK.background, TabPalette::DARK.tab_active);
        assert_ne!(GutterPalette::DARK.bookmark, GutterPalette::DARK.number);
        assert_ne!(GutterPalette::LIGHT.bookmark, GutterPalette::LIGHT.number);
        assert_eq!(
            GutterPalette::LIGHT.number_active,
            Color32::from_rgb(0x17, 0x11, 0x84)
        );
        assert_eq!(
            GutterPalette::LIGHT.background,
            TabPalette::LIGHT.tab_active
        );
    }

    #[test]
    fn test_line_count() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("abc"), 1);
        assert_eq!(line_count("a\nb"), 2);
        assert_eq!(line_count("a\nb\n"), 3, "trailing newline opens a line");
    }

    #[test]
    fn test_digits_keep_a_minimum() {
        assert_eq!(digits(1), MIN_DIGITS);
        assert_eq!(digits(999), 3);
        assert_eq!(digits(1000), 4);
        assert_eq!(digits(123_456), 6);
    }

    /// Runs `f` inside one headless frame.
    fn with_ui<R>(f: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let ctx = egui::Context::default();
        // `run_ui` takes an `FnMut`; it runs the closure once here.
        let mut f = Some(f);
        let mut out = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                out = Some(f(ui));
            }
        });
        output.textures_delta.clear();
        out.expect("frame ran")
    }

    /// Lays `text` out in monospace, wrapping at `wrap_width`.
    fn layout(ui: &egui::Ui, text: &str, wrap_width: f32) -> Arc<Galley> {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        ui.fonts_mut(|f| f.layout(text.to_owned(), font_id, Color32::WHITE, wrap_width))
    }

    #[test]
    fn test_width_grows_with_digits() {
        let (narrow, wide, digit) = with_ui(|ui| {
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let digit = ui.fonts_mut(|f| f.glyph_width(&font_id, '0'));
            (width(ui, &font_id, 9), width(ui, &font_id, 10_000), digit)
        });
        assert_eq!(narrow, narrow.ceil(), "width is whole points");
        assert!(narrow >= PAD_LEFT + 3.0 * digit + PAD_RIGHT);
        assert!(wide >= PAD_LEFT + 5.0 * digit + PAD_RIGHT);
        assert!(wide > narrow);
    }

    #[test]
    fn test_line_of_char() {
        let galley = with_ui(|ui| layout(ui, "ab\ncd\n", f32::INFINITY));
        assert_eq!(line_of_char(&galley, 0), 1);
        assert_eq!(line_of_char(&galley, 2), 1, "before the newline");
        assert_eq!(line_of_char(&galley, 3), 2);
        assert_eq!(line_of_char(&galley, 5), 2);
        assert_eq!(line_of_char(&galley, 6), 3, "after the trailing newline");
    }

    #[test]
    fn test_line_of_char_ignores_wrapped_rows() {
        let galley = with_ui(|ui| layout(ui, "aaaa aaaa aaaa\nb", 40.0));
        assert!(galley.rows.len() > 2, "first line must wrap");
        assert_eq!(line_of_char(&galley, 12), 1);
        assert_eq!(line_of_char(&galley, 15), 2);
    }

    #[test]
    fn test_only_first_row_of_a_wrapped_line_is_numbered() {
        let galley = with_ui(|ui| layout(ui, "aaaa aaaa aaaa\nb", 40.0));
        let numbers = visible_numbers(
            &galley,
            0.0,
            egui::Rangef::new(0.0, 1e6),
            Some(2),
            &Bookmarks::default(),
        );
        let lines: Vec<usize> = numbers.iter().map(|n| n.line).collect();
        assert_eq!(lines, vec![1, 2]);
        let last = galley.rows.last().unwrap().rect();
        assert_eq!(numbers[1].row, last.y_range());
        assert!(!numbers[0].active);
        assert!(numbers[1].active);
    }

    #[test]
    fn test_rows_outside_the_viewport_are_skipped() {
        let galley = with_ui(|ui| layout(ui, "1\n2\n3\n4\n5", f32::INFINITY));
        let row_h = galley.rows[0].rect().height();
        // Galley placed at y = 100; show only rows 2 and 3 (lines 2..=3).
        let visible = egui::Rangef::new(100.0 + row_h * 1.5, 100.0 + row_h * 2.5);
        let numbers = visible_numbers(&galley, 100.0, visible, None, &Bookmarks::default());
        let lines: Vec<usize> = numbers.iter().map(|n| n.line).collect();
        assert_eq!(lines, vec![2, 3]);
        assert_eq!(
            numbers[0].row.center(),
            100.0 + galley.rows[1].rect().center().y
        );
    }

    #[test]
    fn test_bookmarked_lines_are_flagged() {
        let text = "1\n2\n3";
        let galley = with_ui(|ui| layout(ui, text, f32::INFINITY));
        let mut bookmarks = Bookmarks::default();
        bookmarks.toggle(2, text);
        let numbers = visible_numbers(&galley, 0.0, egui::Rangef::new(0.0, 1e6), None, &bookmarks);
        let flagged: Vec<bool> = numbers.iter().map(|n| n.bookmarked).collect();
        assert_eq!(flagged, vec![false, true, false]);
    }
}
