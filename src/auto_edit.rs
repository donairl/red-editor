//! Auto-indent and auto-close: Enter keeps the line's indentation (one level
//! deeper after an opening bracket) and an opening bracket or quote brings
//! its partner along.
//!
//! Pure functions over `char` indices, which is how egui's `TextEdit` counts
//! its cursor. Each returns the new text with the selection to put back.

use eframe::egui;

use crate::highlighter::Language;
use crate::line_ops::{Edit, Range};

/// One level of indentation in `text`: the leading whitespace of the first
/// indented line decides — a tab gives "\t", spaces give that many spaces
/// clamped to 2..=8. A document with no indented line gives 4 spaces.
pub fn indent_unit(text: &str) -> String {
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
    "    ".to_owned()
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
) -> Option<(Edit, Vec<usize>)> {
    if events.iter().any(|e| matches!(e, egui::Event::Ime(_))) {
        return None;
    }
    let unit = indent_unit(text);
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
            // `TextEdit` leaves newlines in `Text` to its Enter handling.
            egui::Event::Text(typed) if !typed.is_empty() && typed != "\n" && typed != "\r" => {
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
        assert_eq!(indent_unit("fn f() {\n\tx;\n}"), "\t");
        assert_eq!(indent_unit("a:\n  b:\n    c"), "  ");
        assert_eq!(indent_unit("a\nb\n"), "    ");
        assert_eq!(indent_unit(""), "    ");
        // Clamped to 2..=8.
        assert_eq!(indent_unit("a\n b"), "  ");
        assert_eq!(indent_unit("a\n            b"), "        ");
        // A line of nothing but whitespace is not indentation.
        assert_eq!(indent_unit("a\n \nb\n\tc"), "\t");
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
        assert_eq!(apply_events("x", caret(1), &events, Language::Rust), None);
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
        let (got, handled) = apply_events("", caret(0), &events, Language::Rust).unwrap();
        assert_eq!(got, edit("f(x)", caret(4)));
        assert_eq!(handled, vec![0, 1, 3, 4]);
    }

    #[test]
    fn test_apply_events_enter_and_backspace() {
        let enter = key(egui::Key::Enter, egui::Modifiers::NONE);
        let (got, handled) = apply_events("  {}", caret(3), &[enter], Language::Rust).unwrap();
        // The document's own two-space indent is the unit.
        assert_eq!(got, edit("  {\n    \n  }", caret(8)));
        assert_eq!(handled, vec![0]);

        // Shift+Enter is an Enter too; Ctrl+Enter is not ours.
        let shift_enter = key(egui::Key::Enter, egui::Modifiers::SHIFT);
        assert!(apply_events("x", caret(1), &[shift_enter], Language::Rust).is_some());
        let ctrl_enter = key(egui::Key::Enter, egui::Modifiers::COMMAND);
        assert_eq!(apply_events("x", caret(1), &[ctrl_enter], Language::Rust), None);

        // A plain backspace in a handled frame deletes as egui would.
        let backspace = key(egui::Key::Backspace, egui::Modifiers::NONE);
        let events = [typed("("), backspace.clone(), backspace];
        let (got, _) = apply_events("ab", caret(2), &events, Language::Rust).unwrap();
        assert_eq!(got, edit("a", caret(1)));
    }

    #[test]
    fn test_apply_events_skips_a_frame_with_ime() {
        let events = [egui::Event::Ime(egui::ImeEvent::Commit(String::from("x"))), typed("(")];
        assert_eq!(apply_events("", caret(0), &events, Language::Rust), None);
    }
}
