//! A side-by-side diff in place of the editor: the active document against
//! HEAD's copy of it, or against another file. The right side is the
//! document as it is in the editor, unsaved edits included.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, pos2, vec2};
use similar::{Algorithm, DiffOp, TextDiff};

use crate::i18n::I18n;
use crate::theme;

/// Longest a diff may take. Past it the diff is coarser, never wrong.
const DEADLINE: Duration = Duration::from_millis(200);

/// What the right side, the active document, is compared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Against {
    /// HEAD's copy of the document's file.
    Head,
    /// Another file, read when the view opened.
    File { path: PathBuf, text: String },
}

/// How a row of the side-by-side view differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    /// Only on the left: the line was removed.
    Removed,
    /// Only on the right: the line was added.
    Added,
    /// On both sides, and different.
    Changed,
}

/// One row: the left line and the right line, 1-based number and text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
}

fn trim_newline(s: &str) -> String {
    s.strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .unwrap_or(s)
        .to_owned()
}

/// The rows of `old` beside `new`. Replaced lines pair up row by row; the
/// longer side's extra lines get an empty partner.
pub fn rows(old: &str, new: &str) -> Vec<Row> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + DEADLINE)
        .diff_lines(old, new);
    let (olds, news) = (diff.old_slices(), diff.new_slices());
    let left = |i: usize| (i + 1, trim_newline(olds[i]));
    let right = |i: usize| (i + 1, trim_newline(news[i]));
    let mut rows = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => rows.extend((0..len).map(|k| Row {
                kind: RowKind::Equal,
                left: Some(left(old_index + k)),
                right: Some(right(new_index + k)),
            })),
            DiffOp::Delete { old_index, old_len, .. } => rows.extend((0..old_len).map(|k| Row {
                kind: RowKind::Removed,
                left: Some(left(old_index + k)),
                right: None,
            })),
            DiffOp::Insert { new_index, new_len, .. } => rows.extend((0..new_len).map(|k| Row {
                kind: RowKind::Added,
                left: None,
                right: Some(right(new_index + k)),
            })),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                rows.extend((0..old_len.max(new_len)).map(|k| {
                    let (l, r) = ((k < old_len).then(|| left(old_index + k)), (k < new_len).then(|| right(new_index + k)));
                    let kind = match (&l, &r) {
                        (Some(_), Some(_)) => RowKind::Changed,
                        (Some(_), None) => RowKind::Removed,
                        _ => RowKind::Added,
                    };
                    Row { kind, left: l, right: r }
                }))
            }
        }
    }
    rows
}

/// Indices of the rows that start a block of changes.
pub fn change_starts(rows: &[Row]) -> Vec<usize> {
    (0..rows.len())
        .filter(|&i| rows[i].kind != RowKind::Equal && (i == 0 || rows[i - 1].kind == RowKind::Equal))
        .collect()
}

/// The open diff view.
#[derive(Clone, Debug)]
pub struct DiffViewUi {
    /// The tab whose document is on the right; the view closes when
    /// another tab is shown.
    pub tab: usize,
    pub against: Against,
    /// Hash of the two texts `rows` were built from.
    key: Option<u64>,
    rows: Vec<Row>,
    starts: Vec<usize>,
    /// The change block last stepped to, an index into `starts`.
    current: Option<usize>,
    /// Row to scroll to on the next frame.
    scroll_to: Option<usize>,
}

impl DiffViewUi {
    pub fn new(tab: usize, against: Against) -> Self {
        DiffViewUi { tab, against, key: None, rows: Vec::new(), starts: Vec::new(), current: None, scroll_to: None }
    }

    /// Diffs the texts again if either changed since the last frame.
    fn update(&mut self, old: &str, new: &str) {
        let mut hasher = DefaultHasher::new();
        (old, new).hash(&mut hasher);
        let key = hasher.finish();
        if self.key == Some(key) {
            return;
        }
        let first = self.key.is_none();
        self.key = Some(key);
        self.rows = rows(old, new);
        self.starts = change_starts(&self.rows);
        self.current = self.current.filter(|&c| c < self.starts.len());
        if first && !self.starts.is_empty() {
            // Open on the first change, as the gutter's diff does.
            self.current = Some(0);
            self.scroll_to = Some(self.starts[0]);
        }
    }

    /// Steps to the next or previous block of changes, wrapping around.
    pub fn step(&mut self, forward: bool) {
        let n = self.starts.len();
        if n == 0 {
            return;
        }
        let next = match (self.current, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(c), true) => (c + 1) % n,
            (Some(c), false) => (c + n - 1) % n,
        };
        self.current = Some(next);
        self.scroll_to = Some(self.starts[next]);
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// What the user asked of the diff view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffViewAction {
    Close,
    /// Go to this 1-based line of the document, in the editor.
    Goto(usize),
}

/// What the left side holds, as the app knows it this frame.
pub enum Left<'a> {
    Text(&'a str),
    /// HEAD's copy is still being read.
    Loading,
}

/// Labels the header shows.
pub struct Titles<'a> {
    pub left: &'a str,
    pub right: &'a str,
}

const PAD: f32 = 6.0;

/// Draws the view over the whole of `ui` and returns what the user asked.
pub fn show(
    ui: &mut egui::Ui,
    view: &mut DiffViewUi,
    left: Left<'_>,
    right: &str,
    titles: &Titles<'_>,
    i18n: &I18n,
) -> Option<DiffViewAction> {
    let theme = theme::active(ui.ctx());
    let palette = theme.sidebar;
    let mut action = None;

    let old = match left {
        Left::Text(text) => Some(text),
        Left::Loading => None,
    };
    if let Some(old) = old {
        view.update(old, right);
    }

    // Header: the two titles, the change count, previous / next, close.
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(titles.left).strong());
            ui.label(egui::RichText::new("↔").weak());
            ui.label(egui::RichText::new(titles.right).strong());
            if old.is_some() {
                let count = view.starts.len();
                let text = if count == 0 {
                    i18n.t("scm.diff_empty").to_owned()
                } else {
                    i18n.tf("diff.changes", &[("count", &count.to_string())])
                };
                ui.label(egui::RichText::new(text).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("×").frame(false))
                    .on_hover_text(i18n.t("scm.diff_close"))
                    .clicked()
                {
                    action = Some(DiffViewAction::Close);
                }
                let any = !view.starts.is_empty();
                if ui.add_enabled(any, egui::Button::new("↓").frame(false)).on_hover_text(i18n.t("diff.next")).clicked() {
                    view.step(true);
                }
                if ui.add_enabled(any, egui::Button::new("↑").frame(false)).on_hover_text(i18n.t("diff.prev")).clicked() {
                    view.step(false);
                }
            });
        });
    });
    ui.separator();

    match left {
        Left::Loading => {
            ui.spinner();
            return action;
        }
        Left::Text(_) => {}
    }

    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let (row_height, digit) = ui.fonts_mut(|f| (f.row_height(&font) + 2.0, f.glyph_width(&font, '0')));
    let widest_number = view
        .rows
        .iter()
        .filter_map(|r| r.left.as_ref().map(|l| l.0).max(r.right.as_ref().map(|r| r.0)))
        .max()
        .unwrap_or(1);
    let number_width = (widest_number.ilog10() as f32 + 1.0) * digit + 2.0 * PAD;
    let widest_text = view
        .rows
        .iter()
        .flat_map(|r| [&r.left, &r.right])
        .filter_map(|s| s.as_ref().map(|s| s.1.chars().count()))
        .max()
        .unwrap_or(0);
    let side_width = (number_width + widest_text as f32 * digit + 2.0 * PAD).max(ui.available_width() / 2.0);
    let colors = Colors {
        added: theme.gutter.added.gamma_multiply(0.2),
        removed: theme.gutter.deleted.gamma_multiply(0.2),
        empty: palette.row_hover,
        number: theme.gutter.number,
        text: palette.foreground,
        divider: palette.border,
        current: palette.accent,
    };
    let current_row = view.current.map(|c| view.starts[c]);

    let mut scroll = egui::ScrollArea::both().id_salt("diff_view_rows").auto_shrink([false, false]);
    if let Some(row) = view.scroll_to.take() {
        // A few rows of context above the change.
        scroll = scroll.vertical_scroll_offset((row.saturating_sub(3)) as f32 * row_height);
    }
    scroll.show_rows(ui, row_height, view.rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        for index in range {
            let row = &view.rows[index];
            let (rect, response) = ui.allocate_exact_size(vec2(2.0 * side_width, row_height), Sense::click());
            let painter = ui.painter_at(rect);
            let left_rect = Rect::from_min_size(rect.min, vec2(side_width, row_height));
            let right_rect = Rect::from_min_size(pos2(rect.min.x + side_width, rect.min.y), vec2(side_width, row_height));
            paint_side(&painter, left_rect, row.left.as_ref(), row.kind, true, number_width, &font, &colors);
            paint_side(&painter, right_rect, row.right.as_ref(), row.kind, false, number_width, &font, &colors);
            painter.vline(right_rect.left(), rect.y_range(), egui::Stroke::new(1.0, colors.divider));
            if current_row == Some(index) {
                painter.vline(rect.left() + 1.0, rect.y_range(), egui::Stroke::new(2.0, colors.current));
            }
            let target = row.right.as_ref().map(|r| r.0).or_else(|| {
                // A removed line: the line now in its place.
                view.rows[index..].iter().find_map(|r| r.right.as_ref().map(|r| r.0))
            });
            if let Some(line) = target {
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.double_clicked() {
                    action = Some(DiffViewAction::Goto(line));
                }
            }
        }
    });
    action
}

struct Colors {
    added: Color32,
    removed: Color32,
    empty: Color32,
    number: Color32,
    text: Color32,
    divider: Color32,
    current: Color32,
}

#[allow(clippy::too_many_arguments)]
fn paint_side(
    painter: &egui::Painter,
    rect: Rect,
    line: Option<&(usize, String)>,
    kind: RowKind,
    left: bool,
    number_width: f32,
    font: &FontId,
    colors: &Colors,
) {
    let fill = match (line, kind) {
        (None, _) => Some(colors.empty),
        (Some(_), RowKind::Equal) => None,
        (Some(_), RowKind::Removed) => Some(colors.removed),
        (Some(_), RowKind::Added) => Some(colors.added),
        (Some(_), RowKind::Changed) => Some(if left { colors.removed } else { colors.added }),
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 0.0, fill);
    }
    let Some((number, text)) = line else { return };
    let y = rect.center().y;
    painter.text(pos2(rect.left() + number_width - PAD, y), Align2::RIGHT_CENTER, number.to_string(), font.clone(), colors.number);
    let sign = match kind {
        RowKind::Equal => " ",
        _ if left => "-",
        _ => "+",
    };
    painter.text(pos2(rect.left() + number_width, y), Align2::LEFT_CENTER, format!("{sign} {text}"), font.clone(), colors.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(rows: &[Row]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn test_rows_pair_lines_side_by_side() {
        use RowKind::*;
        let rows = rows("a\nb\nc\nd\n", "a\nB\nc\nx\ny\n");
        assert_eq!(kinds(&rows), [Equal, Changed, Equal, Changed, Added]);
        assert_eq!(rows[1].left, Some((2, "b".into())));
        assert_eq!(rows[1].right, Some((2, "B".into())));
        assert_eq!(rows[4].left, None);
        assert_eq!(rows[4].right, Some((5, "y".into())));
    }

    #[test]
    fn test_rows_of_removed_lines_have_no_right_side() {
        use RowKind::*;
        let rows = rows("a\nb\nc\n", "a\nc\n");
        assert_eq!(kinds(&rows), [Equal, Removed, Equal]);
        assert_eq!(rows[1].left, Some((2, "b".into())));
        assert_eq!(rows[2].right, Some((2, "c".into())));
    }

    #[test]
    fn test_rows_handle_crlf_and_a_missing_final_newline() {
        let rows = rows("a\r\nb", "a\r\nb");
        assert!(rows.iter().all(|r| r.kind == RowKind::Equal));
        assert_eq!(rows[0].left, Some((1, "a".into())));
        assert_eq!(rows[1].right, Some((2, "b".into())));
    }

    #[test]
    fn test_change_starts_and_stepping_wrap_around() {
        let mut view = DiffViewUi::new(0, Against::Head);
        view.update("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n");
        assert_eq!(view.starts, [1, 4]);
        assert_eq!(view.current, Some(0), "opens on the first change");
        view.step(true);
        assert_eq!(view.current, Some(1));
        view.step(true);
        assert_eq!(view.current, Some(0), "wraps");
        view.step(false);
        assert_eq!(view.current, Some(1));
        assert_eq!(view.scroll_to, Some(4));
    }

    #[test]
    fn test_update_rebuilds_only_when_a_text_changes() {
        let mut view = DiffViewUi::new(0, Against::Head);
        view.update("a\n", "a\n");
        assert!(view.starts.is_empty());
        view.update("a\n", "b\n");
        assert_eq!(view.rows().len(), 1);
        assert_eq!(view.rows()[0].kind, RowKind::Changed);
    }

    #[test]
    fn test_show_lays_out_the_rows() {
        let ctx = egui::Context::default();
        let mut view = DiffViewUi::new(0, Against::Head);
        let i18n = I18n::builtin();
        let titles = Titles { left: "HEAD", right: "a.txt" };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                assert_eq!(show(ui, &mut view, Left::Text("a\n"), "b\n", &titles, &i18n), None);
            });
        });
        output.textures_delta.clear();
        assert_eq!(view.rows().len(), 1);
    }
}
