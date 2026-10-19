//! Bookmarked lines of a document.
//!
//! Lines are 1-based. Edits move the lines below them up or down, and a
//! bookmark follows its line, as in VS Code.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmarks {
    lines: BTreeSet<usize>,
    /// The text `lines` were last checked against; empty while there are
    /// none, so an unbookmarked document keeps no second copy.
    text: String,
}

impl Bookmarks {
    /// Adds or removes the bookmark on `line` of `text`; true when the line
    /// is now bookmarked.
    pub fn toggle(&mut self, line: usize, text: &str) -> bool {
        self.sync(text);
        if self.lines.remove(&line) {
            if self.lines.is_empty() {
                self.text = String::new();
            }
            return false;
        }
        if self.lines.is_empty() {
            self.text = text.to_owned();
        }
        self.lines.insert(line)
    }

    pub fn contains(&self, line: usize) -> bool {
        self.lines.contains(&line)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// The bookmarked lines, top to bottom.
    pub fn lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.lines.iter().copied()
    }

    /// Moves the bookmarks along with the edits that turned the text they
    /// were last checked against into `text`.
    pub fn sync(&mut self, text: &str) {
        if self.lines.is_empty() || self.text == text {
            return;
        }
        self.lines = follow_edit(&self.lines, &self.text, text);
        if self.lines.is_empty() {
            self.text = String::new();
        } else {
            self.text.clear();
            self.text.push_str(text);
        }
    }
}

fn newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

/// Where the bookmarks on `lines` of `old` land in `new`.
///
/// The edit is taken to be the single span between the longest common
/// prefix and suffix. A line starting before the span keeps its number, one
/// starting after it moves by the lines the span gained or lost, and one
/// starting inside it (it was edited over) keeps its number, held within the
/// lines that replaced the span. Bookmarks that meet are merged.
fn follow_edit(lines: &BTreeSet<usize>, old: &str, new: &str) -> BTreeSet<usize> {
    // Bytes, not `str`: the span may split a character, and only `\n`
    // matters here.
    let (old, new) = (old.as_bytes(), new.as_bytes());
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let old_end = old.len() - suffix;
    let old_last = 1 + newlines(&old[..old_end]);
    let new_last = 1 + newlines(&new[..new.len() - suffix]);

    let mut starts = std::iter::once(0).chain(
        old.iter()
            .enumerate()
            .filter(|&(_, &b)| b == b'\n')
            .map(|(i, _)| i + 1),
    );
    let mut line = 0;
    let mut start = 0;
    lines
        .iter()
        .filter_map(|&bookmark| {
            while line < bookmark {
                start = starts.next()?;
                line += 1;
            }
            Some(if start < prefix {
                bookmark
            } else if start >= old_end {
                bookmark + new_last - old_last
            } else {
                bookmark.min(new_last)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmarked(text: &str, lines: &[usize]) -> Bookmarks {
        let mut bookmarks = Bookmarks::default();
        for &line in lines {
            bookmarks.toggle(line, text);
        }
        bookmarks
    }

    /// The lines bookmarked on `lines` of `old` after it is edited into `new`.
    fn after_edit(old: &str, lines: &[usize], new: &str) -> Vec<usize> {
        let mut bookmarks = bookmarked(old, lines);
        bookmarks.sync(new);
        bookmarks.lines().collect()
    }

    #[test]
    fn test_toggle_adds_then_removes() {
        let mut bookmarks = Bookmarks::default();
        assert!(bookmarks.toggle(2, "a\nb\nc"));
        assert!(bookmarks.toggle(1, "a\nb\nc"));
        assert!(bookmarks.contains(2));
        assert_eq!(bookmarks.len(), 2);
        assert_eq!(bookmarks.lines().collect::<Vec<_>>(), vec![1, 2], "top to bottom");

        assert!(!bookmarks.toggle(2, "a\nb\nc"));
        assert!(!bookmarks.contains(2));
        assert!(!bookmarks.toggle(1, "a\nb\nc"));
        assert_eq!(bookmarks, Bookmarks::default(), "no copy of the text kept");
    }

    #[test]
    fn test_typing_within_lines_moves_nothing() {
        assert_eq!(after_edit("a\nb\nc", &[1, 3], "ax\nbyy\nc"), vec![1, 3]);
    }

    #[test]
    fn test_lines_added_above_push_bookmarks_down() {
        assert_eq!(after_edit("a\nb\nc", &[2, 3], "new\na\nb\nc"), vec![3, 4]);
        // Enter at the start of a bookmarked line takes the bookmark along.
        assert_eq!(after_edit("a\nb\nc", &[2], "a\n\nb\nc"), vec![3]);
    }

    #[test]
    fn test_lines_added_below_leave_bookmarks_in_place() {
        assert_eq!(after_edit("a\nb\nc", &[1, 2], "a\nb\nc\nd\ne"), vec![1, 2]);
        // Enter inside a bookmarked line leaves the bookmark on its start.
        assert_eq!(after_edit("a\nbc\nd", &[2], "a\nb\nc\nd"), vec![2]);
    }

    #[test]
    fn test_lines_removed_above_pull_bookmarks_up() {
        assert_eq!(after_edit("a\nb\nc\nd", &[3, 4], "c\nd"), vec![1, 2]);
        // Backspace at the start of a line joins it to the one above.
        assert_eq!(after_edit("a\nb\nc", &[3], "a\nbc"), vec![2]);
    }

    #[test]
    fn test_bookmarks_on_deleted_lines_merge_where_the_lines_were() {
        assert_eq!(after_edit("a\nb\nc\nd\ne", &[2, 3, 5], "a\ne"), vec![2]);
    }

    #[test]
    fn test_rewriting_a_block_in_place_keeps_its_bookmarks() {
        // Prettify re-indents every line but keeps the line count.
        let old = "fn f() {\nlet x = 1;\nx\n}";
        let new = "fn f() {\n    let x = 1;\n    x\n}";
        assert_eq!(after_edit(old, &[2, 3], new), vec![2, 3]);
    }

    #[test]
    fn test_split_characters_are_handled() {
        // "é" and "è" share their first byte.
        assert_eq!(after_edit("é\nb", &[2], "è\n\nb"), vec![3]);
    }

    #[test]
    fn test_bookmarks_past_the_end_are_dropped() {
        // A wholesale replace keeps a bookmark within the new text...
        assert_eq!(after_edit("a\nb\nc", &[3], "x"), vec![1]);
        // ...but one past the end of the old text has no line to follow.
        let mut bookmarks = bookmarked("a", &[5]);
        bookmarks.sync("b");
        assert_eq!(bookmarks, Bookmarks::default(), "no copy of the text kept");
    }

    #[test]
    fn test_toggle_applies_pending_edits_first() {
        let mut bookmarks = bookmarked("a\nb", &[2]);
        // Line 2 moved to line 3; toggling line 3 removes its bookmark.
        assert!(!bookmarks.toggle(3, "x\na\nb"));
        assert_eq!(bookmarks.len(), 0);
    }
}
