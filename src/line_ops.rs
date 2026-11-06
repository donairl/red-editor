//! Line-wise editing commands: move, copy and delete the lines under the
//! selection, toggle their comment markers, and extend a selection to the
//! next occurrence of its text.
//!
//! Pure functions over `char` indices, which is how egui's `TextEdit` counts
//! its cursor. Each returns the new text with the selection to put back.

use crate::highlighter::Language;

/// A sorted `char` range; equal ends are a caret.
pub type Range = (usize, usize);

/// A changed document and the selection the editor should show in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub selection: Range,
}

/// How a language comments out one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentStyle {
    /// A marker that runs to the end of the line, such as `//`.
    Line(&'static str),
    /// Markers that open and close, such as `<!--` and `-->`.
    Block(&'static str, &'static str),
}

/// The comment markers of `language`; `None` when it has no comments.
pub fn comment_style(language: Language) -> Option<CommentStyle> {
    use CommentStyle::{Block, Line};
    Some(match language {
        Language::PlainText => return None,
        Language::Rust
        | Language::C
        | Language::Cpp
        | Language::Php
        | Language::JavaScript
        | Language::TypeScript
        | Language::Go
        | Language::Json => Line("//"),
        Language::Python | Language::Bash => Line("#"),
        Language::Sql => Line("--"),
        Language::Css => Block("/*", "*/"),
        Language::Html | Language::Markdown => Block("<!--", "-->"),
        Language::Blade => Block("{{--", "--}}"),
        Language::Twig => Block("{#", "#}"),
        Language::Smarty => Block("{*", "*}"),
    })
}

/// The document as rows without their `\n`, with each row's starting `char`
/// index. `"a\n"` has two rows, the second empty.
struct Rows<'a> {
    rows: Vec<&'a str>,
    starts: Vec<usize>,
}

impl<'a> Rows<'a> {
    fn new(text: &'a str) -> Self {
        let rows: Vec<&str> = text.split('\n').collect();
        let mut starts = Vec::with_capacity(rows.len());
        let mut at = 0;
        for row in &rows {
            starts.push(at);
            at += row.chars().count() + 1;
        }
        Self { rows, starts }
    }

    /// Index of the row holding the `char` index `at`.
    fn row_of(&self, at: usize) -> usize {
        self.starts.partition_point(|start| *start <= at).saturating_sub(1)
    }

    /// First and last (inclusive) rows the selection touches. A selection
    /// that ends at the start of a row leaves that row out, as editors do.
    fn block(&self, (a, b): Range) -> (usize, usize) {
        let first = self.row_of(a);
        let mut last = self.row_of(b);
        if b > a && last > first && self.starts[last] == b {
            last -= 1;
        }
        (first, last)
    }

    fn len(&self, row: usize) -> usize {
        self.rows[row].chars().count()
    }
}

fn join(rows: &[&str]) -> String {
    rows.join("\n")
}

/// Moves the selected lines one line up or down, selection included. `None`
/// at the edge of the document.
pub fn move_lines(text: &str, selection: Range, up: bool) -> Option<Edit> {
    let doc = Rows::new(text);
    let (first, last) = doc.block(selection);
    let mut rows = doc.rows.clone();
    let (a, b) = selection;
    let selection = if up {
        if first == 0 {
            return None;
        }
        let shift = doc.len(first - 1) + 1;
        let above = rows.remove(first - 1);
        rows.insert(last, above);
        (a - shift, b - shift)
    } else {
        if last + 1 >= rows.len() {
            return None;
        }
        let shift = doc.len(last + 1) + 1;
        let below = rows.remove(last + 1);
        rows.insert(first, below);
        (a + shift, b + shift)
    };
    Some(Edit { text: join(&rows), selection })
}

/// Duplicates the selected lines. Copying down puts the selection on the
/// new copy below; copying up leaves it on the upper copy, where it was.
pub fn copy_lines(text: &str, selection: Range, up: bool) -> Edit {
    let doc = Rows::new(text);
    let (first, last) = doc.block(selection);
    let mut rows = doc.rows.clone();
    let copy: Vec<&str> = rows[first..=last].to_vec();
    let shift: usize = copy.iter().map(|row| row.chars().count() + 1).sum();
    for (offset, row) in copy.into_iter().enumerate() {
        rows.insert(last + 1 + offset, row);
    }
    let (a, b) = selection;
    let selection = if up { (a, b) } else { (a + shift, b + shift) };
    Edit { text: join(&rows), selection }
}

/// Deletes the selected lines. The caret lands on the line that takes their
/// place, keeping its column where the line is long enough.
pub fn delete_lines(text: &str, selection: Range) -> Edit {
    let doc = Rows::new(text);
    let (first, last) = doc.block(selection);
    let column = selection.0 - doc.starts[first];
    let mut rows = doc.rows.clone();
    rows.drain(first..=last);
    if rows.is_empty() {
        return Edit { text: String::new(), selection: (0, 0) };
    }
    let text = join(&rows);
    let doc = Rows::new(&text);
    let row = first.min(doc.rows.len() - 1);
    let caret = doc.starts[row] + column.min(doc.len(row));
    Edit { text, selection: (caret, caret) }
}

/// Comments the selected lines out, or back in when every non-blank one is
/// already commented. Markers go at the block's shallowest indentation.
/// `None` when the selection holds only blank lines.
pub fn toggle_comment(text: &str, selection: Range, style: CommentStyle) -> Option<Edit> {
    let doc = Rows::new(text);
    let (first, last) = doc.block(selection);
    let (open, close) = match style {
        CommentStyle::Line(marker) => (marker, None),
        CommentStyle::Block(open, close) => (open, Some(close)),
    };
    let is_blank = |row: &str| row.trim().is_empty();
    let is_commented = |row: &str| {
        let body = row.trim_start();
        body.starts_with(open) && close.is_none_or(|close| body.trim_end().ends_with(close))
    };
    let targets: Vec<usize> = (first..=last).filter(|&row| !is_blank(doc.rows[row])).collect();
    if targets.is_empty() {
        return None;
    }
    let uncomment = targets.iter().all(|&row| is_commented(doc.rows[row]));
    let indent = |row: &str| row.chars().take_while(|c| c.is_whitespace()).count();
    let column = targets.iter().map(|&row| indent(doc.rows[row])).min().unwrap_or(0);

    let mut rows: Vec<String> = doc.rows.iter().map(|row| (*row).to_owned()).collect();
    // How the column of a caret on each row changes: (at, delta).
    let mut shifts: Vec<Option<(usize, isize)>> = vec![None; rows.len()];
    for &index in &targets {
        let row = &rows[index];
        let (new_row, at, delta) = if uncomment {
            let at = indent(row);
            let body = row.trim_start();
            let mut rest = &body[open.len()..];
            let removed_space = rest.starts_with(' ');
            if removed_space {
                rest = &rest[1..];
            }
            let mut rest = rest.to_owned();
            if let Some(close) = close {
                let trimmed = rest.trim_end();
                let trailing = &rest[trimmed.len()..];
                let mut body = trimmed[..trimmed.len() - close.len()].to_owned();
                if body.ends_with(' ') {
                    body.pop();
                }
                rest = body + trailing;
            }
            let removed = open.chars().count() + usize::from(removed_space);
            (
                format!("{}{}", &row[..row.len() - body.len()], rest),
                at,
                -(removed as isize),
            )
        } else {
            let byte_at = row
                .char_indices()
                .nth(column)
                .map_or(row.len(), |(byte, _)| byte);
            let mut new_row = format!("{}{} {}", &row[..byte_at], open, &row[byte_at..]);
            if let Some(close) = close {
                let trimmed_len = new_row.trim_end().len();
                new_row.insert_str(trimmed_len, &format!(" {close}"));
            }
            (new_row, column, (open.chars().count() + 1) as isize)
        };
        rows[index] = new_row;
        shifts[index] = Some((at, delta));
    }

    let new_rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let new_text = join(&new_rows);
    let new_doc = Rows::new(&new_text);
    let map = |at: usize| {
        let row = doc.row_of(at);
        let column = at - doc.starts[row];
        let column = match shifts[row] {
            Some((from, delta)) if column >= from => {
                (column as isize + delta).max(from as isize) as usize
            }
            _ => column,
        };
        new_doc.starts[row] + column.min(new_doc.len(row))
    };
    let selection = (map(selection.0), map(selection.1));
    Some(Edit { text: new_text, selection })
}

/// With a caret, the word around it; with a selection, its next occurrence
/// after it, wrapping to the start of the document. `None` when there is no
/// word at the caret.
pub fn select_next_match(text: &str, (a, b): Range) -> Option<Range> {
    let byte_at = |at: usize| text.char_indices().nth(at).map_or(text.len(), |(byte, _)| byte);
    if a == b {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let caret = byte_at(a);
        let start = text[..caret].trim_end_matches(is_word).len();
        let end = caret + text[caret..].len() - text[caret..].trim_start_matches(is_word).len();
        if start == end {
            return None;
        }
        let start_char = text[..start].chars().count();
        return Some((start_char, start_char + text[start..end].chars().count()));
    }
    let (start, end) = (byte_at(a), byte_at(b));
    let needle = &text[start..end];
    let found = text[end..]
        .find(needle)
        .map(|at| end + at)
        .or_else(|| text.find(needle))?;
    let start_char = text[..found].chars().count();
    Some((start_char, start_char + (b - a)))
}

/// The text with spaces and tabs removed from the end of every line, `\r\n`
/// endings kept; `None` when there is nothing to remove.
pub fn trim_trailing_whitespace(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let (body, cr) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r"),
            None => (line, ""),
        };
        out.push_str(body.trim_end_matches([' ', '\t']));
        out.push_str(cr);
        if lines.peek().is_some() {
            out.push('\n');
        }
    }
    (out != text).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "one\ntwo\nthree\nfour";

    #[test]
    fn test_block_leaves_out_a_row_the_selection_only_touches() {
        let doc = Rows::new(DOC);
        assert_eq!(doc.block((0, 0)), (0, 0));
        assert_eq!(doc.block((5, 5)), (1, 1));
        // "two\n" selected: ends at the start of "three", which stays out.
        assert_eq!(doc.block((4, 8)), (1, 1));
        assert_eq!(doc.block((4, 9)), (1, 2));
        assert_eq!(doc.block((17, 17)), (3, 3));
    }

    #[test]
    fn test_move_line_down_and_up_carries_the_caret() {
        let down = move_lines(DOC, (1, 1), false).unwrap();
        assert_eq!(down.text, "two\none\nthree\nfour");
        assert_eq!(down.selection, (5, 5));
        let up = move_lines(&down.text, down.selection, true).unwrap();
        assert_eq!(up.text, DOC);
        assert_eq!(up.selection, (1, 1));
    }

    #[test]
    fn test_move_lines_moves_the_whole_selection() {
        let edit = move_lines(DOC, (1, 9), false).unwrap();
        assert_eq!(edit.text, "four\none\ntwo\nthree");
        assert_eq!(edit.selection, (6, 14));
    }

    #[test]
    fn test_move_lines_stops_at_the_edges() {
        assert_eq!(move_lines(DOC, (0, 0), true), None);
        assert_eq!(move_lines(DOC, (15, 15), false), None);
        assert_eq!(move_lines("", (0, 0), false), None);
    }

    #[test]
    fn test_move_lines_handles_a_trailing_newline() {
        let edit = move_lines("a\nb\n", (2, 2), false).unwrap();
        assert_eq!(edit.text, "a\n\nb");
        assert_eq!(edit.selection, (3, 3));
    }

    #[test]
    fn test_move_lines_counts_chars_not_bytes() {
        let edit = move_lines("héllo\nwörld", (0, 0), false).unwrap();
        assert_eq!(edit.text, "wörld\nhéllo");
        assert_eq!(edit.selection, (6, 6));
    }

    #[test]
    fn test_copy_lines_down_selects_the_copy_and_up_keeps_the_original() {
        let down = copy_lines(DOC, (5, 6), false);
        assert_eq!(down.text, "one\ntwo\ntwo\nthree\nfour");
        assert_eq!(down.selection, (9, 10));
        let up = copy_lines(DOC, (5, 6), true);
        assert_eq!(up.text, down.text);
        assert_eq!(up.selection, (5, 6));
    }

    #[test]
    fn test_copy_lines_duplicates_a_multi_line_selection() {
        let edit = copy_lines(DOC, (1, 9), false);
        assert_eq!(edit.text, "one\ntwo\nthree\none\ntwo\nthree\nfour");
        assert_eq!(edit.selection, (15, 23));
    }

    #[test]
    fn test_delete_lines_keeps_the_column_when_it_can() {
        let edit = delete_lines(DOC, (6, 6));
        assert_eq!(edit.text, "one\nthree\nfour");
        assert_eq!(edit.selection, (6, 6));
        let edit = delete_lines(DOC, (10, 10));
        assert_eq!(edit.text, "one\ntwo\nfour");
        assert_eq!(edit.selection, (8 + 2, 8 + 2));
        let last = delete_lines(DOC, (16, 16));
        assert_eq!(last.text, "one\ntwo\nthree");
        assert_eq!(last.selection, (8 + 2, 8 + 2));
    }

    #[test]
    fn test_delete_lines_removes_the_selected_block() {
        let edit = delete_lines(DOC, (4, 9));
        assert_eq!(edit.text, "one\nfour");
        assert_eq!(edit.selection, (4, 4));
        assert_eq!(
            delete_lines("only", (2, 2)),
            Edit { text: String::new(), selection: (0, 0) }
        );
    }

    #[test]
    fn test_toggle_comment_adds_and_removes_a_line_marker() {
        let style = CommentStyle::Line("//");
        let on = toggle_comment("    let x = 1;", (8, 8), style).unwrap();
        assert_eq!(on.text, "    // let x = 1;");
        assert_eq!(on.selection, (11, 11));
        let off = toggle_comment(&on.text, on.selection, style).unwrap();
        assert_eq!(off.text, "    let x = 1;");
        assert_eq!(off.selection, (8, 8));
    }

    #[test]
    fn test_toggle_comment_uses_the_shallowest_indent_and_skips_blank_lines() {
        let style = CommentStyle::Line("#");
        let text = "def f():\n    a\n\n        b\n";
        let on = toggle_comment(text, (0, text.len()), style).unwrap();
        assert_eq!(on.text, "# def f():\n#     a\n\n#         b\n");
        let off = toggle_comment(&on.text, on.selection, style).unwrap();
        assert_eq!(off.text, text);
        assert_eq!(off.selection, (0, text.len()));
    }

    #[test]
    fn test_toggle_comment_comments_when_some_lines_are_not() {
        let style = CommentStyle::Line("//");
        let edit = toggle_comment("// a\nb", (0, 6), style).unwrap();
        assert_eq!(edit.text, "// // a\n// b");
    }

    #[test]
    fn test_toggle_comment_wraps_with_block_markers() {
        let style = CommentStyle::Block("<!--", "-->");
        let on = toggle_comment("  <p>hi</p>  ", (4, 4), style).unwrap();
        assert_eq!(on.text, "  <!-- <p>hi</p> -->  ");
        assert_eq!(on.selection, (9, 9));
        let off = toggle_comment(&on.text, on.selection, style).unwrap();
        assert_eq!(off.text, "  <p>hi</p>  ");
        assert_eq!(off.selection, (4, 4));
    }

    #[test]
    fn test_toggle_comment_keeps_a_caret_before_the_marker_in_place() {
        let style = CommentStyle::Line("//");
        let on = toggle_comment("    x", (2, 2), style).unwrap();
        assert_eq!(on.text, "    // x");
        assert_eq!(on.selection, (2, 2));
        let off = toggle_comment("    //x", (6, 6), style).unwrap();
        assert_eq!(off.text, "    x");
        assert_eq!(off.selection, (4, 4));
    }

    #[test]
    fn test_toggle_comment_ignores_blank_selections() {
        assert_eq!(toggle_comment("\n\n", (0, 2), CommentStyle::Line("//")), None);
    }

    #[test]
    fn test_comment_style_covers_every_language_but_plain_text() {
        for language in Language::all() {
            assert_eq!(
                comment_style(*language).is_none(),
                *language == Language::PlainText,
                "{language:?}"
            );
        }
    }

    #[test]
    fn test_select_next_match_picks_the_word_at_the_caret() {
        assert_eq!(select_next_match("foo bar_1 baz", (6, 6)), Some((4, 9)));
        assert_eq!(select_next_match("foo bar", (3, 3)), Some((0, 3)));
        assert_eq!(select_next_match("foo  bar", (4, 4)), None);
        assert_eq!(select_next_match("héllo wörld", (8, 8)), Some((6, 11)));
    }

    #[test]
    fn test_select_next_match_steps_to_the_next_occurrence_and_wraps() {
        let text = "ab cd ab ef ab";
        assert_eq!(select_next_match(text, (0, 2)), Some((6, 8)));
        assert_eq!(select_next_match(text, (6, 8)), Some((12, 14)));
        assert_eq!(select_next_match(text, (12, 14)), Some((0, 2)));
        assert_eq!(select_next_match(text, (3, 5)), Some((3, 5)));
    }

    #[test]
    fn test_trim_trailing_whitespace() {
        assert_eq!(trim_trailing_whitespace("a  \nb\t\n c \t"), Some("a\nb\n c".to_owned()));
        assert_eq!(trim_trailing_whitespace("a \r\nb\r\n"), Some("a\r\nb\r\n".to_owned()));
        assert_eq!(trim_trailing_whitespace("a\n  \n"), Some("a\n\n".to_owned()));
        assert_eq!(trim_trailing_whitespace("a\nb\n"), None);
        assert_eq!(trim_trailing_whitespace(""), None);
    }
}
