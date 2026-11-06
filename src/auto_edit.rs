//! Auto-indent and auto-close: Enter keeps the line's indentation (one level
//! deeper after an opening bracket) and an opening bracket or quote brings
//! its partner along.
//!
//! Pure functions over `char` indices, which is how egui's `TextEdit` counts
//! its cursor. Each returns the new text with the selection to put back.

use eframe::egui;

use crate::highlighter::Language;
use crate::line_ops::{Edit, Range};
use crate::settings::EditorSettings;

/// One level of indentation in `text`: the leading whitespace of the first
/// indented line decides — a tab gives "\t", spaces give that many spaces
/// clamped to 2..=8. A document with no indented line gives `fallback`
/// (the user's setting).
pub fn indent_unit(text: &str, fallback: &str) -> String {
    for line in text.split('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        // A line of nothing but whitespace says nothing about indentation.
        if body.len() == line.len() || body.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            return "\t".to_owned();
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        return " ".repeat(spaces.clamp(2, 8));
    }
    fallback.to_owned()
}

/// The partner of an opening bracket.
fn closer_of(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn is_closer(ch: char) -> bool {
    matches!(ch, ')' | ']' | '}')
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Whether typing `quote` brings its partner in `language`.
fn quote_pairs(quote: char, language: Language) -> bool {
    use Language::{Go, JavaScript, Markdown, PlainText, Rust, TypeScript};
    match quote {
        '"' => language != PlainText,
        '\'' => !matches!(language, PlainText | Rust | Markdown),
        '`' => matches!(language, JavaScript | TypeScript | Go | Markdown),
        _ => false,
    }
}

/// `chars` with `a..b` replaced by `insert`.
fn splice(chars: &[char], (a, b): Range, insert: &str) -> String {
    let mut out: String = chars[..a].iter().collect();
    out.push_str(insert);
    out.extend(&chars[b..]);
    out
}

/// A sorted, clamped copy of `sel`.
fn sorted(chars: &[char], (a, b): Range) -> Range {
    let n = chars.len();
    (a.min(b).min(n), a.max(b).min(n))
}

/// The start of the line holding the `char` index `at`.
fn line_start(chars: &[char], at: usize) -> usize {
    chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
}

/// Enter: replaces the selection with "\n" + the current line's leading
/// whitespace (only the part before the caret). After `{` `(` `[` (the last
/// non-space char before the caret), one more `unit`. When the caret also
/// sits right before the matching closer (`{|}`), the closer goes on its own
/// line at the original indentation and the caret stays on the middle line.
pub fn on_enter(text: &str, sel: Range, unit: &str) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, sel);
    let before = &chars[line_start(&chars, a)..a];
    let indent: String = before.iter().take_while(|c| matches!(c, ' ' | '\t')).collect();
    let opener = before.iter().rev().find(|c| !c.is_whitespace()).copied();
    let close = opener.and_then(closer_of);
    let mut insert = format!("\n{indent}");
    if close.is_some() {
        insert.push_str(unit);
    }
    let caret = a + insert.chars().count();
    if close.is_some() && chars.get(b).copied() == close {
        insert.push('\n');
        insert.push_str(&indent);
    }
    Edit { text: splice(&chars, (a, b), &insert), selection: (caret, caret) }
}

/// A typed `ch`. `None` = not special, insert it normally.
/// - Opener `(` `[` `{`: with a selection, wraps it (selection kept, inside
///   the pair). With a caret, inserts the pair and puts the caret between —
///   only when the next char is whitespace, end of text, or a closer
///   `)` `]` `}` (else `None`).
/// - Quote `"`, `'`, `` ` ``: if the next char is the same quote, steps over
///   it. Else as an opener, but also never right after a letter, digit or
///   `_` (`don't`). Which quotes pair depends on `language`.
/// - Closer `)` `]` `}`: if the next char is the same, steps over it. Typed
///   where everything before the caret on the line is whitespace, it first
///   removes one `unit` from that whitespace (if present).
///
/// `Language::PlainText` never auto-closes; stepping over and dedent still
/// apply.
pub fn on_char(text: &str, sel: Range, ch: char, language: Language, unit: &str) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, sel);
    let caret = a == b;
    let next = chars.get(b).copied();
    let prev = a.checked_sub(1).map(|i| chars[i]);
    let step_over = || Edit { text: text.to_owned(), selection: (a + 1, a + 1) };

    let is_quote = matches!(ch, '"' | '\'' | '`');
    if caret && next == Some(ch) && (is_quote || is_closer(ch)) {
        return Some(step_over());
    }
    if is_closer(ch) {
        if !caret {
            return None;
        }
        let start = line_start(&chars, a);
        let before: String = chars[start..a].iter().collect();
        if before.is_empty() || !before.chars().all(|c| c == ' ' || c == '\t') {
            return None;
        }
        let kept = before.strip_suffix(unit)?.chars().count();
        let text = splice(&chars, (start + kept, a), &ch.to_string());
        let at = start + kept + 1;
        return Some(Edit { text, selection: (at, at) });
    }

    let close = match (closer_of(ch), is_quote) {
        (Some(close), _) if language != Language::PlainText => close,
        (None, true) if quote_pairs(ch, language) => ch,
        _ => return None,
    };
    if !caret {
        let inner: String = chars[a..b].iter().collect();
        let text = splice(&chars, (a, b), &format!("{ch}{inner}{close}"));
        return Some(Edit { text, selection: (a + 1, b + 1) });
    }
    let next_ok = next.is_none_or(|c| c.is_whitespace() || is_closer(c));
    if !next_ok || (is_quote && prev.is_some_and(is_word)) {
        return None;
    }
    let text = splice(&chars, (a, a), &format!("{ch}{close}"));
    Some(Edit { text, selection: (a + 1, a + 1) })
}

/// Backspace with a caret between an empty pair — `()` `[]` `{}` `""` `''`
/// `` `` `` — removes both. `None` otherwise (normal backspace).
pub fn on_backspace(text: &str, sel: Range) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, sel);
    if a != b || a == 0 {
        return None;
    }
    let pair = (chars[a - 1], *chars.get(a)?);
    let empty_pair = matches!(
        pair,
        ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')
    );
    empty_pair.then(|| Edit { text: splice(&chars, (a - 1, a + 1), ""), selection: (a - 1, a - 1) })
}

/// Backspace as `TextEdit` does it: the selection, else the char before
/// the caret.
fn plain_backspace(text: &str, sel: Range) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, sel);
    let a = if a == b { a.saturating_sub(1) } else { a };
    Edit { text: splice(&chars, (a, b), ""), selection: (a, a) }
}

/// Typed text as `TextEdit` inserts it: in place of the selection.
fn plain_insert(text: &str, sel: Range, insert: &str) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, sel);
    let at = a + insert.chars().count();
    Edit { text: splice(&chars, (a, b), insert), selection: (at, at) }
}

/// The `char` index where each line of `chars` starts.
fn line_starts(chars: &[char]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
    starts
}

/// The line `pos` is on.
fn line_of(starts: &[usize], pos: usize) -> usize {
    starts.partition_point(|&s| s <= pos) - 1
}

/// The first and last line a block indent or outdent acts on. A last line
/// the selection reaches only at its column 0 is left out, as in VS Code:
/// selecting whole lines ends there.
fn touched_lines(starts: &[usize], (a, b): Range) -> (usize, usize) {
    let first = line_of(starts, a);
    let mut last = line_of(starts, b);
    if last > first && starts[last] == b {
        last -= 1;
    }
    (first, last)
}

/// Tab: with a caret or a selection inside one line, `unit` replaces the
/// selection; a selection across lines gets `unit` in front of every line it
/// touches and grows to keep covering the same text.
pub fn indent(text: &str, selection: Range, unit: &str) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, selection);
    let starts = line_starts(&chars);
    if line_of(&starts, a) == line_of(&starts, b) {
        return Some(plain_insert(text, (a, b), unit));
    }
    let (first, last) = touched_lines(&starts, (a, b));
    let mut out = String::with_capacity(text.len() + unit.len() * (last - first + 1));
    let mut next = first;
    for (i, c) in chars.iter().enumerate() {
        if next <= last && i == starts[next] {
            out.push_str(unit);
            next += 1;
        }
        out.push(*c);
    }
    let n = unit.chars().count();
    let shift = |p: usize| p + n * (first..=last).filter(|&l| starts[l] <= p).count();
    Some(Edit { text: out, selection: (shift(a), shift(b)) })
}

/// Shift+Tab: every line the caret or selection touches loses one level from
/// its front — one `\t`, or up to `width` spaces. `None` when no line has
/// any indentation to lose.
pub fn outdent(text: &str, selection: Range, width: usize) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = sorted(&chars, selection);
    let starts = line_starts(&chars);
    let (first, last) = touched_lines(&starts, (a, b));
    // (line start, chars removed) for each line that loses indentation.
    let cuts: Vec<(usize, usize)> = (first..=last)
        .filter_map(|line| {
            let rest = &chars[starts[line]..];
            let cut = if rest.first() == Some(&'\t') {
                1
            } else {
                rest.iter().take_while(|&&c| c == ' ').take(width.max(1)).count()
            };
            (cut > 0).then_some((starts[line], cut))
        })
        .collect();
    if cuts.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let (mut i, mut k) = (0, 0);
    while i < chars.len() {
        if k < cuts.len() && i == cuts[k].0 {
            i += cuts[k].1;
            k += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    let shift = |p: usize| {
        p - cuts.iter().map(|&(s, cut)| if s <= p { cut.min(p - s) } else { 0 }).sum::<usize>()
    };
    Some(Edit { text: out, selection: (shift(a), shift(b)) })
}

/// Runs one frame's typing through the rules above. When any of `events`
/// is special — Enter, or a one-char `Text` or a Backspace that a rule
/// changes — every `Text`, Enter and Backspace event is applied in order
/// and the result comes back with the indices of the events handled, which
/// the editor must then drop. `None` leaves the whole frame to `TextEdit`,
/// as does any frame holding an IME event.
pub fn apply_events(
    text: &str,
    sel: Range,
    events: &[egui::Event],
    language: Language,
    settings: &EditorSettings,
) -> Option<(Edit, Vec<usize>)> {
    // While an input method composes, its preedit text is not in the
    // document yet: leave the frame to `TextEdit`. An empty preedit only
    // marks the end of a composition; fcitx and ibus wrap plain typing in it.
    let composing = events.iter().any(|e| {
        matches!(e, egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !text.is_empty())
    });
    if composing {
        return None;
    }
    let unit = indent_unit(text, &settings.indent_unit());
    let mut state = Edit { text: text.to_owned(), selection: sel };
    let mut handled = Vec::new();
    let mut special = false;
    for (index, event) in events.iter().enumerate() {
        let next = match event {
            egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                if !(modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd) =>
            {
                special = true;
                on_enter(&state.text, state.selection, &unit)
            }
            egui::Event::Key { key: egui::Key::Backspace, pressed: true, modifiers, .. }
                if modifiers.is_none() =>
            {
                match on_backspace(&state.text, state.selection) {
                    Some(edit) => {
                        special = true;
                        edit
                    }
                    None => plain_backspace(&state.text, state.selection),
                }
            }
            // Tab and Shift+Tab indent and outdent. Always handled, so egui
            // neither types a raw `\t` nor moves the focus. Ctrl+Tab is
            // Next Tab and is left alone.
            egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. }
                if modifiers.is_none() || modifiers.shift_only() =>
            {
                special = true;
                let edit = if modifiers.shift {
                    outdent(&state.text, state.selection, usize::from(settings.tab_size))
                } else {
                    indent(&state.text, state.selection, &unit)
                };
                edit.unwrap_or_else(|| state.clone())
            }
            // `TextEdit` leaves newlines in `Text` to its Enter handling. An
            // input method delivers typed text as a commit instead.
            egui::Event::Text(typed) | egui::Event::Ime(egui::ImeEvent::Commit(typed))
                if !typed.is_empty() && typed != "\n" && typed != "\r" =>
            {
                let mut one = typed.chars();
                let rule = match (one.next(), one.next()) {
                    (Some(ch), None) => on_char(&state.text, state.selection, ch, language, &unit),
                    _ => None,
                };
                match rule {
                    Some(edit) => {
                        special = true;
                        edit
                    }
                    None => plain_insert(&state.text, state.selection, typed),
                }
            }
            _ => continue,
        };
        state = next;
        handled.push(index);
    }
    special.then_some((state, handled))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = "    ";

    fn caret(at: usize) -> Range {
        (at, at)
    }

    fn edit(text: &str, selection: Range) -> Edit {
        Edit { text: text.to_owned(), selection }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    fn typed(s: &str) -> egui::Event {
        egui::Event::Text(String::from(s))
    }

    #[test]
    fn test_indent_unit_follows_the_first_indented_line() {
        assert_eq!(indent_unit("fn f() {\n\tx;\n}", "  "), "\t");
        assert_eq!(indent_unit("a:\n  b:\n    c", "\t"), "  ");
        assert_eq!(indent_unit("a\n b", "    "), "  ");
        assert_eq!(indent_unit("a\n            b", "    "), "        ");
        assert_eq!(indent_unit("a\n \nb\n\tc", "    "), "\t");
    }

    #[test]
    fn test_indent_unit_falls_back_only_without_an_indented_line() {
        assert_eq!(indent_unit("a\nb\n", "\t"), "\t");
        assert_eq!(indent_unit("", "  "), "  ");
        assert_eq!(indent_unit("a\n  b", "\t"), "  ", "the document wins");
    }

    #[test]
    fn test_enter_keeps_the_indent() {
        assert_eq!(on_enter("    let x;", caret(10), UNIT), edit("    let x;\n    ", caret(15)));
        assert_eq!(on_enter("plain", caret(5), UNIT), edit("plain\n", caret(6)));
    }

    #[test]
    fn test_enter_mid_line_keeps_only_the_whitespace_before_the_caret() {
        // Caret inside the indentation: two of the four spaces come along.
        assert_eq!(on_enter("    x", caret(2), UNIT), edit("  \n    x", caret(5)));
        assert_eq!(on_enter("  ab", caret(3), UNIT), edit("  a\n  b", caret(6)));
    }

    #[test]
    fn test_enter_after_an_opener_adds_a_unit() {
        assert_eq!(on_enter("  if x {", caret(8), UNIT), edit("  if x {\n      ", caret(15)));
        // Trailing spaces after the opener still count.
        assert_eq!(on_enter("f( ", caret(3), "\t"), edit("f( \n\t", caret(5)));
    }

    #[test]
    fn test_enter_between_a_pair_splits_it_into_three_lines() {
        let got = on_enter("  x {}", caret(5), UNIT);
        assert_eq!(got, edit("  x {\n      \n  }", caret(12)));
        let got = on_enter("[]", caret(1), "\t");
        assert_eq!(got, edit("[\n\t\n]", caret(3)));
    }

    #[test]
    fn test_enter_replaces_the_selection() {
        assert_eq!(on_enter("  abcd", (3, 5), UNIT), edit("  a\n  d", caret(6)));
        // The pair is judged around the selection: `{` before, `}` after.
        assert_eq!(on_enter("{xx}", (1, 3), UNIT), edit("{\n    \n}", caret(6)));
    }

    #[test]
    fn test_opener_inserts_the_pair() {
        let rust = Language::Rust;
        assert_eq!(on_char("", caret(0), '(', rust, UNIT), Some(edit("()", caret(1))));
        assert_eq!(on_char("f x", caret(1), '[', rust, UNIT), Some(edit("f[] x", caret(2))));
        assert_eq!(on_char("f)", caret(1), '{', rust, UNIT), Some(edit("f{})", caret(2))));
    }

    #[test]
    fn test_opener_does_not_pair_before_a_word() {
        assert_eq!(on_char("foo", caret(0), '(', Language::Rust, UNIT), None);
        assert_eq!(on_char("x.", caret(1), '[', Language::Rust, UNIT), None);
    }

    #[test]
    fn test_opener_and_quote_wrap_the_selection() {
        let got = on_char("a bc d", (2, 4), '(', Language::Rust, UNIT);
        assert_eq!(got, Some(edit("a (bc) d", (3, 5))));
        let got = on_char("bc", (0, 2), '"', Language::Python, UNIT);
        assert_eq!(got, Some(edit("\"bc\"", (1, 3))));
    }

    #[test]
    fn test_quote_pairs_but_not_after_a_word_char() {
        let py = Language::Python;
        assert_eq!(on_char("x = ", caret(4), '\'', py, UNIT), Some(edit("x = ''", caret(5))));
        assert_eq!(on_char("don", caret(3), '\'', py, UNIT), None);
        assert_eq!(on_char("a_", caret(2), '"', py, UNIT), None);
        assert_eq!(on_char("v1", caret(2), '"', py, UNIT), None);
        // Not before a word either.
        assert_eq!(on_char("x", caret(0), '"', py, UNIT), None);
    }

    #[test]
    fn test_closer_and_quote_step_over_their_twin() {
        let rust = Language::Rust;
        assert_eq!(on_char("f()", caret(2), ')', rust, UNIT), Some(edit("f()", caret(3))));
        assert_eq!(on_char("[]", caret(1), ']', rust, UNIT), Some(edit("[]", caret(2))));
        assert_eq!(on_char("\"\"", caret(1), '"', rust, UNIT), Some(edit("\"\"", caret(2))));
        // A different closer is typed as usual.
        assert_eq!(on_char("(]", caret(1), ')', rust, UNIT), None);
    }

    #[test]
    fn test_quotes_pair_by_language() {
        let pairs = |language, quote| on_char("", caret(0), quote, language, UNIT).is_some();
        assert!(!pairs(Language::Rust, '\''), "lifetimes");
        assert!(!pairs(Language::Markdown, '\''));
        assert!(pairs(Language::Rust, '"'));
        assert!(pairs(Language::C, '\''));
        assert!(pairs(Language::JavaScript, '`'));
        assert!(pairs(Language::TypeScript, '`'));
        assert!(pairs(Language::Go, '`'));
        assert!(pairs(Language::Markdown, '`'));
        assert!(!pairs(Language::Rust, '`'));
        assert!(!pairs(Language::Python, '`'));
    }

    #[test]
    fn test_nothing_pairs_in_plain_text() {
        let plain = Language::PlainText;
        for ch in ['(', '[', '{', '"', '\'', '`'] {
            assert_eq!(on_char("", caret(0), ch, plain, UNIT), None, "{ch}");
            assert_eq!(on_char("ab", (0, 2), ch, plain, UNIT), None, "{ch} around a selection");
        }
        // Stepping over and dedent still apply.
        assert_eq!(on_char("()", caret(1), ')', plain, UNIT), Some(edit("()", caret(2))));
        assert_eq!(on_char("    ", caret(4), '}', plain, UNIT), Some(edit("}", caret(1))));
    }

    #[test]
    fn test_closer_on_a_blank_indented_line_dedents_one_unit() {
        let rust = Language::Rust;
        let got = on_char("{\n        ", caret(10), '}', rust, UNIT);
        assert_eq!(got, Some(edit("{\n    }", caret(7))));
        assert_eq!(on_char("x\n\t", caret(3), ')', rust, "\t"), Some(edit("x\n)", caret(3))));
        // Less than a unit of whitespace, or text before the caret: as usual.
        assert_eq!(on_char("  ", caret(2), '}', rust, UNIT), None);
        assert_eq!(on_char("    x", caret(5), '}', rust, UNIT), None);
    }

    #[test]
    fn test_backspace_removes_an_empty_pair() {
        for pair in ["()", "[]", "{}", "\"\"", "''", "``"] {
            let text = format!("a{pair}b");
            assert_eq!(on_backspace(&text, caret(2)), Some(edit("ab", caret(1))), "{pair}");
        }
    }

    #[test]
    fn test_backspace_leaves_anything_else_alone() {
        assert_eq!(on_backspace("(x)", caret(2)), None);
        assert_eq!(on_backspace("(x)", caret(1)), None);
        assert_eq!(on_backspace("(]", caret(1)), None);
        assert_eq!(on_backspace("()", (0, 1)), None);
        assert_eq!(on_backspace("()", caret(0)), None);
        assert_eq!(on_backspace("", caret(0)), None);
    }

    #[test]
    fn test_apply_events_leaves_plain_typing_to_the_editor() {
        let events = [typed("a"), key(egui::Key::Backspace, egui::Modifiers::NONE)];
        assert_eq!(apply_events("x", caret(1), &events, Language::Rust, &EditorSettings::default()), None);
    }

    #[test]
    fn test_apply_events_handles_the_whole_frame_in_order() {
        let events = [
            typed("f"),
            typed("("),
            egui::Event::PointerMoved(egui::pos2(1.0, 1.0)),
            typed("x"),
            typed(")"),
        ];
        let (got, handled) = apply_events("", caret(0), &events, Language::Rust, &EditorSettings::default()).unwrap();
        assert_eq!(got, edit("f(x)", caret(4)));
        assert_eq!(handled, vec![0, 1, 3, 4]);
    }

    #[test]
    fn test_apply_events_enter_and_backspace() {
        let enter = key(egui::Key::Enter, egui::Modifiers::NONE);
        let (got, handled) = apply_events("  {}", caret(3), &[enter], Language::Rust, &EditorSettings::default()).unwrap();
        // The document's own two-space indent is the unit.
        assert_eq!(got, edit("  {\n    \n  }", caret(8)));
        assert_eq!(handled, vec![0]);

        // Shift+Enter is an Enter too; Ctrl+Enter is not ours.
        let shift_enter = key(egui::Key::Enter, egui::Modifiers::SHIFT);
        assert!(apply_events("x", caret(1), &[shift_enter], Language::Rust, &EditorSettings::default()).is_some());
        let ctrl_enter = key(egui::Key::Enter, egui::Modifiers::COMMAND);
        assert_eq!(apply_events("x", caret(1), &[ctrl_enter], Language::Rust, &EditorSettings::default()), None);

        // A plain backspace in a handled frame deletes as egui would.
        let backspace = key(egui::Key::Backspace, egui::Modifiers::NONE);
        let events = [typed("("), backspace.clone(), backspace];
        let (got, _) = apply_events("ab", caret(2), &events, Language::Rust, &EditorSettings::default()).unwrap();
        assert_eq!(got, edit("a", caret(1)));
    }

    fn preedit(s: &str) -> egui::Event {
        egui::Event::Ime(egui::ImeEvent::Preedit { text: String::from(s), active_range_chars: None })
    }

    fn commit(s: &str) -> egui::Event {
        egui::Event::Ime(egui::ImeEvent::Commit(String::from(s)))
    }

    #[test]
    fn test_apply_events_skips_a_frame_while_composing() {
        let events = [preedit("ni"), typed("(")];
        assert_eq!(apply_events("", caret(0), &events, Language::Rust, &EditorSettings::default()), None);
    }

    /// With an input method running (fcitx, ibus), egui-winit delivers plain
    /// typing as IME commits wrapped in empty preedits.
    #[test]
    fn test_apply_events_treats_an_ime_commit_as_typing() {
        let events = [preedit(""), commit("("), preedit("")];
        let (got, handled) = apply_events("", caret(0), &events, Language::Php, &EditorSettings::default()).unwrap();
        assert_eq!(got, edit("()", caret(1)));
        assert_eq!(handled, vec![1]);
    }

    #[test]
    fn test_apply_events_handles_enter_next_to_an_empty_preedit() {
        let enter = key(egui::Key::Enter, egui::Modifiers::NONE);
        let events = [preedit(""), enter];
        let (got, _) = apply_events("{}", caret(1), &events, Language::Php, &EditorSettings::default()).unwrap();
        assert_eq!(got, edit("{\n    \n}", caret(6)));
    }

    fn ed(text: &str, a: usize, b: usize) -> Edit {
        Edit { text: text.to_owned(), selection: (a, b) }
    }

    #[test]
    fn test_indent_caret_inserts_the_unit() {
        assert_eq!(indent("ab", (1, 1), "    "), Some(ed("a    b", 5, 5)));
        assert_eq!(indent("ab", (1, 1), "\t"), Some(ed("a\tb", 2, 2)));
    }

    #[test]
    fn test_indent_selection_in_one_line_is_replaced() {
        assert_eq!(indent("abcd", (1, 3), "  "), Some(ed("a  d", 3, 3)));
    }

    #[test]
    fn test_indent_multi_line_selection_indents_every_line() {
        // "a\nb\nc", selection from inside "a" to inside "c"
        assert_eq!(indent("a\nb\nc", (0, 5), "  "), Some(ed("  a\n  b\n  c", 2, 11)));
    }

    #[test]
    fn test_indent_leaves_out_a_last_line_reached_at_column_0() {
        // selects "a\nb\n": the caret sits at the start of "c"
        assert_eq!(indent("a\nb\nc", (0, 4), "\t"), Some(ed("\ta\n\tb\nc", 1, 6)));
    }

    #[test]
    fn test_outdent_removes_one_level() {
        assert_eq!(outdent("\tab", (2, 2), 4), Some(ed("ab", 1, 1)));
        assert_eq!(outdent("      ab", (7, 7), 4), Some(ed("  ab", 3, 3)));
        assert_eq!(outdent("  ab", (3, 3), 4), Some(ed("ab", 1, 1)), "fewer spaces than a level");
    }

    #[test]
    fn test_outdent_multi_line_and_lines_without_indent() {
        assert_eq!(outdent("  a\nb\n  c", (0, 9), 2), Some(ed("a\nb\nc", 0, 5)));
    }

    #[test]
    fn test_outdent_caret_inside_the_indent_moves_to_the_line_start() {
        assert_eq!(outdent("    ab", (2, 2), 4), Some(ed("ab", 0, 0)));
    }

    #[test]
    fn test_outdent_without_indentation_does_nothing() {
        assert_eq!(outdent("ab\ncd", (0, 4), 4), None);
    }

    fn tab(shift: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
        }
    }

    #[test]
    fn test_apply_events_handles_tab_and_shift_tab() {
        let s = EditorSettings::default();
        let (edit, handled) =
            apply_events("ab", (0, 0), &[tab(false)], Language::PlainText, &s).unwrap();
        assert_eq!(edit, ed("    ab", 4, 4));
        assert_eq!(handled, vec![0]);
        let (edit, _) =
            apply_events("    ab", (4, 4), &[tab(true)], Language::PlainText, &s).unwrap();
        assert_eq!(edit, ed("ab", 0, 0));
    }

    #[test]
    fn test_apply_events_shift_tab_with_nothing_to_outdent_is_still_handled() {
        // handled, so egui does not type a tab or move focus
        let s = EditorSettings::default();
        let (edit, handled) =
            apply_events("ab", (1, 1), &[tab(true)], Language::PlainText, &s).unwrap();
        assert_eq!(edit, ed("ab", 1, 1));
        assert_eq!(handled, vec![0]);
    }

    #[test]
    fn test_apply_events_ignores_ctrl_tab() {
        let s = EditorSettings::default();
        let ctrl_tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        assert!(apply_events("ab", (0, 0), &[ctrl_tab], Language::PlainText, &s).is_none());
    }
}
