//! VS Code-style line-number gutter painted beside the editor.
//!
//! The editor reserves room for the gutter through its left margin; this
//! module only measures that room and paints the numbers, a marker on each
//! bookmarked line, git's change markers, and the fold chevrons, into it.

use eframe::egui::{self, Align2, Color32, FontId, Galley, Rect, pos2};

use crate::bookmarks::Bookmarks;
use crate::find;
use crate::line_diff::{LineMarks, Mark};
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
/// The git change bar: in the space between the numbers and the text, this
/// far right of the numbers and this wide, as in VS Code.
const CHANGE_INSET: f32 = 5.0;
const CHANGE_WIDTH: f32 = 3.0;
/// Half the height of the triangle marking removed lines.
const DELETED_SIZE: f32 = 4.0;
/// Half the size of a fold chevron, which sits right of the change bar.
const CHEVRON_SIZE: f32 = 3.0;

/// The editor's folds, as the gutter needs them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Folding<'a> {
    /// The `char` ranges folded away, sorted: their lines get no number.
    pub hidden: &'a [(usize, usize)],
    /// Line ranges `(header, last)` of the foldable blocks, 0-based, in
    /// header order: their headers get a chevron.
    pub foldable: &'a [(usize, usize)],
}

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
    /// Git change markers.
    pub added: Color32,
    pub modified: Color32,
    pub deleted: Color32,
}

impl GutterPalette {
    pub const DARK: GutterPalette = GutterPalette {
        number: Color32::from_rgb(0x6E, 0x76, 0x81),
        number_active: Color32::from_rgb(0xCC, 0xCC, 0xCC),
        bookmark: Color32::from_rgb(0x37, 0x94, 0xFF),
        background: TabPalette::DARK.tab_active,
        added: Color32::from_rgb(0x2E, 0xA0, 0x43),
        modified: Color32::from_rgb(0x00, 0x78, 0xD4),
        deleted: Color32::from_rgb(0xF8, 0x51, 0x49),
    };

    pub const LIGHT: GutterPalette = GutterPalette {
        number: Color32::from_rgb(0x6E, 0x76, 0x81),
        number_active: Color32::from_rgb(0x17, 0x11, 0x84),
        bookmark: Color32::from_rgb(0x00, 0x5F, 0xB8),
        background: TabPalette::LIGHT.tab_active,
        added: Color32::from_rgb(0x2E, 0xA0, 0x43),
        modified: Color32::from_rgb(0x00, 0x5F, 0xB8),
        deleted: Color32::from_rgb(0xF8, 0x51, 0x49),
    };
}

/// Number of lines in `text`. A trailing `\n` opens one more, empty line, as
/// the caret can sit there.
pub fn line_count(text: &str) -> usize {
    find::line_count(text)
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
    change: Option<Mark>,
    /// For the header of a foldable block, whether it is folded.
    fold: Option<bool>,
}

/// The numbers of the lines whose first row overlaps `visible`.
///
/// With word wrap a line spans several rows; only its first row is numbered.
/// Lines folded away get no number.
fn visible_numbers(
    galley: &Galley,
    galley_top: f32,
    visible: egui::Rangef,
    active_line: Option<usize>,
    bookmarks: &Bookmarks,
    changes: &LineMarks,
    folding: &Folding<'_>,
) -> Vec<Number> {
    let mut numbers: Vec<Number> = Vec::new();
    let mut line = 1;
    let mut starts_line = true;
    // The `char` index the row starts at.
    let mut chars = 0;
    let mut hidden = folding.hidden.iter().peekable();
    let mut foldable = folding.foldable.iter().peekable();
    for row in &galley.rows {
        let start = chars;
        chars += row.char_count_including_newline().0;
        let rect = row.rect().translate(egui::vec2(0.0, galley_top));
        if starts_line && rect.top() > visible.max {
            break;
        }
        while hidden.next_if(|&&(_, end)| end < start).is_some() {}
        let is_hidden = hidden.peek().is_some_and(|&&(a, b)| a < start && start <= b);
        if starts_line && !is_hidden && rect.bottom() >= visible.min {
            while foldable.next_if(|&&(header, _)| header + 1 < line).is_some() {}
            let header = foldable.peek().is_some_and(|&&(header, _)| header + 1 == line);
            numbers.push(Number {
                line,
                row: rect.y_range(),
                active: active_line == Some(line),
                bookmarked: bookmarks.contains(line),
                change: changes.get(line),
                fold: header.then_some(false),
            });
        }
        if row.ends_with_newline {
            // A fold hides the text from its header's `\n` on.
            let newline = chars - 1;
            if let Some(number) = numbers.last_mut().filter(|n| n.line == line && n.fold.is_some())
                && folding.hidden.binary_search_by_key(&newline, |&(a, _)| a).is_ok()
            {
                number.fold = Some(true);
            }
            line += 1;
        }
        starts_line = row.ends_with_newline;
    }
    numbers
}

/// What a gutter interaction produced: folded header (0-based) and/or clicked change line (1-based).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GutterOutput {
    pub toggled_fold: Option<usize>,
    pub clicked_change: Option<usize>,
}

/// Paints the gutter over the left `width` points of the editor's viewport.
///
/// The gutter sticks to the left edge of the viewport: when the editor is
/// scrolled sideways the text slides underneath it, as in VS Code.
///
/// A foldable line gets a chevron between its number and the text, as in
/// VS Code: always while folded, only while the pointer is over the gutter
/// otherwise. Returns gutter events (fold toggled, change marker clicked).
pub fn paint(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    font_id: &FontId,
    width: f32,
    bookmarks: &Bookmarks,
    changes: &LineMarks,
    folding: &Folding<'_>,
) -> GutterOutput {
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
    let hovered = ui.rect_contains_pointer(rect);
    let mut gutter_out = GutterOutput::default();
    for number in visible_numbers(
        &output.galley,
        output.galley_pos.y,
        clip.y_range(),
        active_line,
        bookmarks,
        changes,
        folding,
    ) {
        if let Some(folded) = number.fold {
            let cell = Rect::from_x_y_ranges(x + CHANGE_INSET + CHANGE_WIDTH..=rect.right(), number.row);
            let id = output.response.response.id.with(("fold_chevron", number.line));
            let response = ui
                .interact(cell, id, egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                gutter_out.toggled_fold = Some(number.line - 1);
            }
            if folded || hovered {
                let color = if response.hovered() {
                    palette.number_active
                } else {
                    palette.number
                };
                paint_chevron(&painter, cell.center(), folded, color);
            }
        }
        if let Some(change) = number.change {
            let change_x = rect.right() - PAD_RIGHT + CHANGE_INSET;
            let change_cell = Rect::from_x_y_ranges(change_x - 1.0..=change_x + CHANGE_WIDTH + 3.0, number.row);
            let change_id = output.response.response.id.with(("change_marker", number.line));
            let change_resp = ui
                .interact(change_cell, change_id, egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if change_resp.clicked() {
                gutter_out.clicked_change = Some(number.line);
            }
            paint_change(&painter, change_x, number.row, change, &palette);
        }
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
    gutter_out
}

/// A fold chevron centred on `at`: pointing right (`▸`) while folded, down
/// (`▾`) otherwise.
fn paint_chevron(painter: &egui::Painter, at: egui::Pos2, folded: bool, color: Color32) {
    let s = CHEVRON_SIZE;
    let points = if folded {
        vec![pos2(at.x - s * 0.6, at.y - s), pos2(at.x + s * 0.6, at.y), pos2(at.x - s * 0.6, at.y + s)]
    } else {
        vec![pos2(at.x - s, at.y - s * 0.6), pos2(at.x + s, at.y - s * 0.6), pos2(at.x, at.y + s * 0.6)]
    };
    painter.add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
}

/// A git change marker at `x`, beside the line spanning `row`: a bar for an
/// added or modified line, a triangle on its top or bottom edge for removed
/// lines.
fn paint_change(painter: &egui::Painter, x: f32, row: egui::Rangef, change: Mark, palette: &GutterPalette) {
    let bar = |color| {
        let rect = Rect::from_x_y_ranges(x..=x + CHANGE_WIDTH, row);
        painter.rect_filled(rect, 0.0, color);
    };
    let triangle = |y: f32| {
        let points = vec![
            pos2(x, y - DELETED_SIZE),
            pos2(x + DELETED_SIZE, y),
            pos2(x, y + DELETED_SIZE),
        ];
        painter.add(egui::Shape::convex_polygon(points, palette.deleted, egui::Stroke::NONE));
    };
    match change {
        Mark::Added => bar(palette.added),
        Mark::Modified => bar(palette.modified),
        Mark::DeletedAbove => triangle(row.min),
        Mark::DeletedBelow => triangle(row.max),
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
            &LineMarks::default(),
            &Folding::default(),
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
        let numbers = visible_numbers(
            &galley,
            100.0,
            visible,
            None,
            &Bookmarks::default(),
            &LineMarks::default(),
            &Folding::default(),
        );
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
        let numbers = visible_numbers(
            &galley,
            0.0,
            egui::Rangef::new(0.0, 1e6),
            None,
            &bookmarks,
            &LineMarks::default(),
            &Folding::default(),
        );
        let flagged: Vec<bool> = numbers.iter().map(|n| n.bookmarked).collect();
        assert_eq!(flagged, vec![false, true, false]);
    }

    #[test]
    fn test_changed_lines_carry_their_git_mark() {
        let text = "1\n2\n3";
        let galley = with_ui(|ui| layout(ui, text, f32::INFINITY));
        let changes = crate::line_diff::marks("1\n3", text);
        let numbers = visible_numbers(
            &galley,
            0.0,
            egui::Rangef::new(0.0, 1e6),
            None,
            &Bookmarks::default(),
            &changes,
            &Folding::default(),
        );
        let marks: Vec<Option<Mark>> = numbers.iter().map(|n| n.change).collect();
        assert_eq!(marks, vec![None, Some(Mark::Added), None]);
    }

    #[test]
    fn test_folded_lines_get_no_number_and_headers_a_chevron() {
        // Lines 1..=3 ("  b", "    c", "  d") are folded under "a".
        let text = "a\n  b\n    c\n  d\ne\n  f\n  g";
        let galley = with_ui(|ui| layout(ui, text, f32::INFINITY));
        let bookmarks = {
            let mut bookmarks = Bookmarks::default();
            bookmarks.toggle(3, text);
            bookmarks
        };
        let folding = Folding {
            hidden: &[(1, 15)],
            foldable: &[(0, 3), (1, 2), (4, 6)],
        };
        let numbers = visible_numbers(
            &galley,
            0.0,
            egui::Rangef::new(0.0, 1e6),
            None,
            &bookmarks,
            &LineMarks::default(),
            &folding,
        );
        let lines: Vec<usize> = numbers.iter().map(|n| n.line).collect();
        assert_eq!(lines, vec![1, 5, 6, 7]);
        let folds: Vec<Option<bool>> = numbers.iter().map(|n| n.fold).collect();
        assert_eq!(folds, vec![Some(true), Some(false), None, None]);
        assert!(numbers.iter().all(|n| !n.bookmarked), "a hidden bookmark is not drawn");
    }

    #[test]
    fn test_gutter_output_default() {
        let out = GutterOutput::default();
        assert_eq!(out.toggled_fold, None);
        assert_eq!(out.clicked_change, None);
    }
}
