//! Find and replace: the matching logic, its state, and the bar drawn above
//! the editor.
//!
//! Positions are `char` indices, the unit egui's text cursor uses.

use eframe::egui::{self, Key};

use crate::i18n::I18n;

/// Half-open range of `char` indices.
pub type Range = (usize, usize);

/// Lowercase one `char` to one `char`, so indices stay aligned with the text.
fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        return c;
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(only), None) => only,
        _ => c,
    }
}

/// Every non-overlapping match of `query` in `text`, in order.
pub fn find_all(text: &str, query: &str, case_sensitive: bool) -> Vec<Range> {
    let needle: Vec<char> = query.chars().map(|c| fold(c, case_sensitive)).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<char> = text.chars().map(|c| fold(c, case_sensitive)).collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            found.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

/// First match starting at or after `from`, wrapping to the first one.
pub fn next_match(matches: &[Range], from: usize) -> Option<Range> {
    matches
        .iter()
        .find(|m| m.0 >= from)
        .or_else(|| matches.first())
        .copied()
}

/// Last match ending at or before `before`, wrapping to the last one.
pub fn prev_match(matches: &[Range], before: usize) -> Option<Range> {
    matches
        .iter()
        .rev()
        .find(|m| m.1 <= before)
        .or_else(|| matches.last())
        .copied()
}

fn byte_offset(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Replaces the `char` range `range` of `text` with `with`.
pub fn replace_range(text: &mut String, range: Range, with: &str) {
    let (start, end) = (byte_offset(text, range.0), byte_offset(text, range.1));
    text.replace_range(start..end, with);
}

/// Replaces every match; returns the new text and how many were replaced.
pub fn replace_all(text: &str, query: &str, with: &str, case_sensitive: bool) -> (String, usize) {
    let matches = find_all(text, query, case_sensitive);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().enumerate().peekable();
    for &(start, end) in &matches {
        while let Some((_, c)) = chars.next_if(|(i, _)| *i < start) {
            out.push(c);
        }
        out.push_str(with);
        while chars.next_if(|(i, _)| *i < end).is_some() {}
    }
    out.extend(chars.map(|(_, c)| c));
    (out, matches.len())
}

/// `char` index where 1-based `line` starts; lines past the end clamp to the
/// last one.
pub fn line_start(text: &str, line: usize) -> usize {
    let line = line.clamp(1, line_count(text));
    text.split('\n')
        .take(line - 1)
        .map(|row| row.chars().count() + 1)
        .sum()
}

/// Number of lines in `text`; an empty text has one.
pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

/// State of the go-to-line bar.
#[derive(Default)]
pub struct GotoState {
    pub visible: bool,
    pub input: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
}

/// What the user asked of the go-to-line bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GotoAction {
    /// Jump to this 1-based line.
    Go(usize),
    Close,
}

/// Draws the go-to-line bar; `lines` is the document's line count.
pub fn show_goto_bar(
    ui: &mut egui::Ui,
    state: &mut GotoState,
    lines: usize,
    i18n: &I18n,
) -> Option<GotoAction> {
    let mut action = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut state.input)
                        .id(egui::Id::unique("goto_line"))
                        .hint_text(i18n.tf("goto.hint", &[("max", &lines.to_string())]))
                        .desired_width(160.0),
                );
                if std::mem::take(&mut state.focus) {
                    field.request_focus();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    action = match state.input.trim().parse::<usize>() {
                        Ok(line) => Some(GotoAction::Go(line.clamp(1, lines))),
                        Err(_) => {
                            field.request_focus();
                            None
                        }
                    };
                }
                if ui.button("\u{d7}").clicked() {
                    action = Some(GotoAction::Close);
                }
            });
        });
    action
}

/// State of the find/replace bar.
#[derive(Default)]
pub struct FindState {
    pub visible: bool,
    /// Also show the replace row.
    pub replace_mode: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    /// Put the caret in the query field on the next frame.
    pub focus_query: bool,
    /// All matches in the active document, refreshed every frame the bar
    /// is open.
    pub matches: Vec<Range>,
    /// The match last jumped to; painted highlighted.
    pub current: Option<Range>,
    /// Where to search from when there is no current match.
    pub anchor: Option<usize>,
}

/// What the user asked of the bar this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindAction {
    /// The query or its options changed: jump to the first match from here.
    Refresh,
    Next,
    Prev,
    Replace,
    ReplaceAll,
    Close,
}

pub const QUERY_ID: &str = "find_query";

/// Draws the bar and reports what was asked.
pub fn show_bar(ui: &mut egui::Ui, state: &mut FindState, i18n: &I18n) -> Option<FindAction> {
    let mut action = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let query = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .id(egui::Id::unique(QUERY_ID))
                        .hint_text(i18n.t("find.query"))
                        .desired_width(220.0),
                );
                if std::mem::take(&mut state.focus_query) {
                    query.request_focus();
                }
                if query.changed() {
                    action = Some(FindAction::Refresh);
                }
                if query.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let backwards = ui.input(|i| i.modifiers.shift);
                    action = Some(if backwards { FindAction::Prev } else { FindAction::Next });
                    query.request_focus();
                }
                if ui
                    .toggle_value(&mut state.case_sensitive, "Aa")
                    .on_hover_text(i18n.t("find.case"))
                    .changed()
                {
                    action = Some(FindAction::Refresh);
                }
                if ui.button(i18n.t("find.prev")).clicked() {
                    action = Some(FindAction::Prev);
                }
                if ui.button(i18n.t("find.next")).clicked() {
                    action = Some(FindAction::Next);
                }
                if !state.query.is_empty() {
                    let label = if state.matches.is_empty() {
                        i18n.t("find.none").to_owned()
                    } else {
                        i18n.tf("find.count", &[("count", &state.matches.len().to_string())])
                    };
                    ui.label(label);
                }
                if ui.button("\u{d7}").clicked() {
                    action = Some(FindAction::Close);
                }
            });
            if state.replace_mode {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.replacement)
                            .hint_text(i18n.t("find.replacement"))
                            .desired_width(220.0),
                    );
                    if ui.button(i18n.t("find.replace_one")).clicked() {
                        action = Some(FindAction::Replace);
                    }
                    if ui.button(i18n.t("find.replace_all")).clicked() {
                        action = Some(FindAction::ReplaceAll);
                    }
                });
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_all_is_case_insensitive_by_default() {
        assert_eq!(find_all("Foo foo FOO", "foo", false), [(0, 3), (4, 7), (8, 11)]);
        assert_eq!(find_all("Foo foo FOO", "foo", true), [(4, 7)]);
    }

    #[test]
    fn test_find_all_edge_cases() {
        assert!(find_all("abc", "", false).is_empty());
        assert!(find_all("ab", "abc", false).is_empty());
        assert_eq!(find_all("aaaa", "aa", true), [(0, 2), (2, 4)], "no overlap");
    }

    #[test]
    fn test_positions_are_chars_not_bytes() {
        assert_eq!(find_all("héllo wörld", "wör", true), [(6, 9)]);
    }

    #[test]
    fn test_next_and_prev_wrap_around() {
        let m = [(0, 1), (4, 5), (9, 10)];
        assert_eq!(next_match(&m, 0), Some((0, 1)));
        assert_eq!(next_match(&m, 1), Some((4, 5)));
        assert_eq!(next_match(&m, 10), Some((0, 1)), "wraps to first");
        assert_eq!(prev_match(&m, 9), Some((4, 5)));
        assert_eq!(prev_match(&m, 1), Some((0, 1)));
        assert_eq!(prev_match(&m, 0), Some((9, 10)), "wraps to last");
        assert_eq!(next_match(&[], 0), None);
        assert_eq!(prev_match(&[], 0), None);
    }

    #[test]
    fn test_replace_range_uses_char_indices() {
        let mut text = "héllo wörld".to_owned();
        replace_range(&mut text, (6, 9), "WOR");
        assert_eq!(text, "héllo WORld");
    }

    #[test]
    fn test_line_start_and_count() {
        let text = "ab\ncde\n\nf";
        assert_eq!(line_count(text), 4);
        assert_eq!(line_count(""), 1);
        assert_eq!(line_start(text, 1), 0);
        assert_eq!(line_start(text, 2), 3);
        assert_eq!(line_start(text, 3), 7);
        assert_eq!(line_start(text, 4), 8);
        assert_eq!(line_start(text, 99), 8, "clamps to the last line");
        assert_eq!(line_start("héé\nx", 2), 4, "counts chars, not bytes");
    }

    #[test]
    fn test_replace_all() {
        assert_eq!(replace_all("a-b-a", "a", "xy", true), ("xy-b-xy".to_owned(), 2));
        assert_eq!(replace_all("Ünï Ünï", "ünï", "x", false), ("x x".to_owned(), 2));
        assert_eq!(replace_all("abc", "z", "q", true), ("abc".to_owned(), 0));
        assert_eq!(replace_all("aaa", "a", "", true), (String::new(), 3));
    }
}
