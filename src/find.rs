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

/// How many of the whitespace-separated words of `query` occur in `text`.
pub fn words_found(text: &str, query: &str, case_sensitive: bool) -> usize {
    query
        .split_whitespace()
        .filter(|word| !find_all(text, word, case_sensitive).is_empty())
        .count()
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

/// Bytes counted at a time: a chunk's count fits a `u8`, which lets the
/// compiler compare many bytes per instruction. Counting this way is over
/// ten times faster than byte by byte, and the editor counts lines and the
/// caret's place in the whole document every frame.
const CHUNK: usize = 255;

/// How many bytes of a chunk of at most [`CHUNK`] bytes pass `test`.
fn count_in_chunk(chunk: &[u8], test: impl Fn(u8) -> bool) -> usize {
    chunk.iter().map(|&b| test(b) as u8).sum::<u8>() as usize
}

/// Whether `byte` starts a `char` rather than continuing one.
fn starts_char(byte: u8) -> bool {
    (byte as i8) >= -0x40
}

/// Byte offset of the `char` index `char_index`, or the end of `text` past it.
fn byte_offset(text: &str, char_index: usize) -> usize {
    let mut chars = 0;
    for (n, chunk) in text.as_bytes().chunks(CHUNK).enumerate() {
        let in_chunk = count_in_chunk(chunk, starts_char);
        if chars + in_chunk <= char_index {
            chars += in_chunk;
            continue;
        }
        for (i, &byte) in chunk.iter().enumerate() {
            if starts_char(byte) {
                if chars == char_index {
                    return n * CHUNK + i;
                }
                chars += 1;
            }
        }
    }
    text.len()
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

/// 1-based line and column of the `char` index `at`, clamped to the end of
/// `text`. Columns count `char`s.
pub fn line_col(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..byte_offset(text, at)];
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    (line_count(before), before[line_start..].chars().count() + 1)
}

/// Number of lines in `text`; an empty text has one.
pub fn line_count(text: &str) -> usize {
    let newlines: usize = text
        .as_bytes()
        .chunks(CHUNK)
        .map(|chunk| count_in_chunk(chunk, |b| b == b'\n'))
        .sum();
    newlines + 1
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
    fn test_words_found_counts_query_words_in_the_text() {
        assert_eq!(words_found("SMSSender.php", "SMS keluar", false), 1);
        assert_eq!(words_found("SMSSender.php", "sms SENDER", false), 2);
        assert_eq!(words_found("SMSSender.php", "sms sender", true), 0);
        assert_eq!(words_found("Kirim.php", "SMS keluar", false), 0);
        assert_eq!(words_found("SMSSender.php", "  \t ", false), 0, "no words");
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
    fn test_line_col_is_one_based_in_chars() {
        let text = "ab\ncde\n\nf";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 2), (1, 3), "before the newline");
        assert_eq!(line_col(text, 3), (2, 1));
        assert_eq!(line_col(text, 7), (3, 1), "empty line");
        assert_eq!(line_col(text, 9), (4, 2), "end of the text");
        assert_eq!(line_col(text, 99), (4, 2), "clamps to the end");
        assert_eq!(line_col("héé\nxé", 6), (2, 3), "counts chars, not bytes");
        assert_eq!(line_col("", 0), (1, 1));
    }

    /// Long texts are counted in chunks: positions and lines stay right
    /// across the chunk edges, multibyte characters included.
    #[test]
    fn test_counts_hold_across_long_texts() {
        let text: String = (0..2000)
            .map(|i| match i % 7 {
                0 => '\n',
                1 => 'é',
                2 => '漢',
                3 => '🦀',
                _ => 'a',
            })
            .collect();
        let chars: Vec<usize> = text.char_indices().map(|(byte, _)| byte).collect();
        for at in 0..chars.len() + 3 {
            let byte = chars.get(at).copied().unwrap_or(text.len());
            assert_eq!(byte_offset(&text, at), byte, "char {at}");
            let before = &text[..byte];
            let line = before.matches('\n').count() + 1;
            let line_start = before.rfind('\n').map_or(0, |n| n + 1);
            let col = before[line_start..].chars().count() + 1;
            assert_eq!(line_col(&text, at), (line, col), "char {at}");
        }
        assert_eq!(line_count(&text), text.matches('\n').count() + 1);
    }

    #[test]
    fn test_replace_all() {
        assert_eq!(replace_all("a-b-a", "a", "xy", true), ("xy-b-xy".to_owned(), 2));
        assert_eq!(replace_all("Ünï Ünï", "ünï", "x", false), ("x x".to_owned(), 2));
        assert_eq!(replace_all("abc", "z", "q", true), ("abc".to_owned(), 0));
        assert_eq!(replace_all("aaa", "a", "", true), (String::new(), 3));
    }
}
