//! Code folding by indentation.
//!
//! A folded block's lines stay in the text; the editor only lays them out
//! at (near) zero height, so every `char` index keeps working unchanged.
//! Lines are 0-based here.

use std::collections::BTreeSet;

/// Columns a tab advances to the next multiple of, as VS Code does.
const TAB_SIZE: usize = 4;

/// The indent of `line` in columns, or `None` when it is blank.
fn indent(line: &str) -> Option<usize> {
    let mut columns = 0;
    for ch in line.chars() {
        match ch {
            ' ' => columns += 1,
            '\t' => columns = columns - columns % TAB_SIZE + TAB_SIZE,
            '\r' => {}
            _ => return Some(columns),
        }
    }
    None
}

/// Line ranges `(header, last)` of every foldable block, in header order.
///
/// A line is foldable when the next non-blank line is indented deeper. Its
/// block runs to the last following line indented deeper than the header;
/// blank lines inside the block belong to it, trailing blank lines do not.
pub fn foldable(text: &str) -> Vec<(usize, usize)> {
    let mut blocks = Vec::new();
    // Headers whose block is still open, outermost first, with their indent.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_non_blank = 0;
    for (number, line) in text.split('\n').enumerate() {
        let Some(columns) = indent(line) else {
            continue;
        };
        while let Some(&(header, header_columns)) = open.last() {
            if header_columns < columns {
                break;
            }
            open.pop();
            if last_non_blank > header {
                blocks.push((header, last_non_blank));
            }
        }
        open.push((number, columns));
        last_non_blank = number;
    }
    for (header, _) in open {
        if last_non_blank > header {
            blocks.push((header, last_non_blank));
        }
    }
    blocks.sort_unstable();
    blocks
}

/// The headers of the blocks in `blocks` (as [`foldable`] returns them)
/// that contain `line`, its header included, innermost first.
pub fn enclosing(blocks: &[(usize, usize)], line: usize) -> Vec<usize> {
    blocks
        .iter()
        .rev()
        .filter(|&&(header, last)| header <= line && line <= last)
        .map(|&(header, _)| header)
        .collect()
}

/// The folds of one document: header lines that are folded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    headers: BTreeSet<usize>,
}

impl Folds {
    /// Folds `header` when it is unfolded, unfolds it otherwise.
    pub fn toggle(&mut self, header: usize) {
        if !self.headers.remove(&header) {
            self.headers.insert(header);
        }
    }

    pub fn fold(&mut self, header: usize) {
        self.headers.insert(header);
    }

    pub fn unfold(&mut self, header: usize) {
        self.headers.remove(&header);
    }

    pub fn clear(&mut self) {
        self.headers.clear();
    }

    #[must_use]
    pub fn is_folded(&self, header: usize) -> bool {
        self.headers.contains(&header)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    /// Keeps the headers on their lines after `old` became `new`. A fold
    /// whose hidden text the edit touched is unfolded first; headers that
    /// are no longer foldable in `new` are dropped.
    pub fn follow_edit(&mut self, old: &str, new: &str) {
        if self.headers.is_empty() || old == new {
            return;
        }
        let (start, old_end) = changed_span(old, new);
        let old_blocks = foldable(old);
        let starts = line_starts(old);
        self.headers.retain(|&header| {
            let Some(&(_, last)) = old_blocks.iter().find(|&&(h, _)| h == header) else {
                return false;
            };
            let (hidden_start, hidden_end) = block_range(old, &starts, header, last);
            // Typing at the end of the header line keeps the fold; any edit
            // reaching past it into the hidden lines (or joining the line
            // below onto them) does not.
            !(start <= hidden_end && old_end > hidden_start)
        });
        // `bookmarks::follow_edit` counts lines from 1.
        let lines = self.headers.iter().map(|&header| header + 1).collect();
        let new_blocks = foldable(new);
        self.headers = crate::bookmarks::follow_edit(&lines, old, new)
            .into_iter()
            .map(|line| line - 1)
            .filter(|header| new_blocks.iter().any(|&(h, _)| h == *header))
            .collect();
    }

    /// Unfolds every fold whose hidden lines contain `line`.
    pub fn reveal_line(&mut self, text: &str, line: usize) {
        if self.headers.is_empty() {
            return;
        }
        for (header, last) in foldable(text) {
            if header < line && line <= last {
                self.headers.remove(&header);
            }
        }
    }
}

/// The `char` index where each line of `text` starts, plus one past the
/// end of the text.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    let mut count = 0;
    for ch in text.chars() {
        count += 1;
        if ch == '\n' {
            starts.push(count);
        }
    }
    starts.push(count + 1);
    starts
}

/// The hidden `char` range of the block `(header, last)`: from the `\n`
/// ending the header line up to (not including) the `\n` ending line
/// `last`, or the end of the text when `last` is the final line.
fn block_range(text: &str, starts: &[usize], header: usize, last: usize) -> (usize, usize) {
    let end = starts.get(last + 1).map_or(text.chars().count(), |&next| next - 1);
    (starts[header + 1] - 1, end)
}

/// The edit that turned `old` into `new`, as the `char` span of `old`
/// between their longest common prefix and suffix.
fn changed_span(old: &str, new: &str) -> (usize, usize) {
    let prefix = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let (old_len, new_len) = (old.chars().count(), new.chars().count());
    let room = old_len.min(new_len) - prefix;
    let suffix = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, old_len - suffix)
}

/// The hidden `char` ranges for `folds` in `text`, sorted, merged: for a
/// folded block `(header, last)`, from the `\n` ending the header line up
/// to (not including) the `\n` ending line `last`, so the header row is
/// followed directly by the next visible line.
pub fn hidden_ranges(text: &str, folds: &Folds) -> Vec<(usize, usize)> {
    hidden_ranges_of(text, &foldable(text), folds)
}

/// [`hidden_ranges`] with the text's [`foldable`] blocks already at hand.
pub fn hidden_ranges_of(
    text: &str,
    blocks: &[(usize, usize)],
    folds: &Folds,
) -> Vec<(usize, usize)> {
    if folds.is_empty() {
        return Vec::new();
    }
    let starts = line_starts(text);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &(header, last) in blocks {
        if !folds.is_folded(header) {
            continue;
        }
        let (start, end) = block_range(text, &starts, header, last);
        match ranges.last_mut() {
            Some(previous) if start <= previous.1 => previous.1 = previous.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folded(headers: &[usize]) -> Folds {
        let mut folds = Folds::default();
        for &header in headers {
            folds.fold(header);
        }
        folds
    }

    #[test]
    fn test_foldable_finds_nested_blocks() {
        let text = "fn a() {\n    if x {\n        y\n    }\n}\n";
        assert_eq!(foldable(text), vec![(0, 3), (1, 2)]);
    }

    #[test]
    fn test_foldable_keeps_inner_blank_lines_and_drops_trailing_ones() {
        let text = "def f():\n    a\n\n    b\n\n\nnext\n";
        assert_eq!(foldable(text), vec![(0, 3)]);
    }

    #[test]
    fn test_foldable_counts_tabs_as_four_columns() {
        // The tab line is deeper than the 2-space header, the 4-space line
        // is as deep as the tab and so not inside its block.
        let text = "  a\n\tb\n    c\n";
        assert_eq!(foldable(text), vec![(0, 2)]);
    }

    #[test]
    fn test_foldable_without_indent_finds_nothing() {
        assert!(foldable("a\nb\n\nc").is_empty());
        assert!(foldable("").is_empty());
    }

    #[test]
    fn test_enclosing_lists_innermost_first() {
        let blocks = foldable("a\n  b\n    c\n  d\n");
        assert_eq!(enclosing(&blocks, 2), vec![1, 0]);
        assert_eq!(enclosing(&blocks, 1), vec![1, 0]);
        assert_eq!(enclosing(&blocks, 3), vec![0]);
        assert!(enclosing(&blocks, 4).is_empty());
    }

    #[test]
    fn test_hidden_ranges_of_one_fold() {
        // "a {" is chars 0..3, its `\n` at 3; the `\n` ending "  x" at 7.
        let text = "a {\n  x\n}\n";
        assert_eq!(hidden_ranges(text, &folded(&[0])), vec![(3, 7)]);
        assert!(hidden_ranges(text, &Folds::default()).is_empty());
    }

    #[test]
    fn test_hidden_ranges_of_nested_folds_merge() {
        let text = "a\n  b\n    c\n  d\ne";
        assert_eq!(hidden_ranges(text, &folded(&[0, 1])), vec![(1, 15)]);
        assert_eq!(hidden_ranges(text, &folded(&[1])), vec![(5, 11)]);
    }

    #[test]
    fn test_hidden_ranges_of_a_fold_at_the_end_without_newline() {
        let text = "é\n  x\n  y";
        assert_eq!(hidden_ranges(text, &folded(&[0])), vec![(1, 9)]);
    }

    #[test]
    fn test_hidden_ranges_skip_headers_that_are_not_foldable() {
        assert!(hidden_ranges("a\nb\n", &folded(&[0])).is_empty());
    }

    #[test]
    fn test_follow_edit_moves_the_header_with_a_line_inserted_above() {
        let old = "x\na\n  b\n";
        let new = "x\nnew\na\n  b\n";
        let mut folds = folded(&[1]);
        folds.follow_edit(old, new);
        assert_eq!(folds, folded(&[2]));
    }

    #[test]
    fn test_follow_edit_drops_a_header_made_unfoldable() {
        let mut folds = folded(&[0]);
        // Typing on the header line keeps the fold...
        folds.follow_edit("a\n  b\n", "ab\n  b\n");
        assert_eq!(folds, folded(&[0]));
        // ...but indenting the header as deep as its block drops it.
        folds.follow_edit("ab\n  b\n", "  ab\n  b\n");
        assert!(folds.is_empty());
    }

    #[test]
    fn test_follow_edit_keeps_the_fold_when_typing_at_the_end_of_the_header() {
        let mut folds = folded(&[0]);
        folds.follow_edit("a {\n  x\n}\n", "a { 1\n  x\n}\n");
        assert_eq!(folds, folded(&[0]));
    }

    #[test]
    fn test_follow_edit_unfolds_when_the_edit_touches_hidden_lines() {
        // Deleting the header's `\n` joins it with a hidden line.
        let mut folds = folded(&[0]);
        folds.follow_edit("a {\n  x\n  y\n}\n", "a {  x\n  y\n}\n");
        assert!(folds.is_empty());
        // An edit inside the hidden lines.
        let mut folds = folded(&[0]);
        folds.follow_edit("a {\n  x\n  y\n}\n", "a {\n  xz\n  y\n}\n");
        assert!(folds.is_empty());
        // Joining the next line onto the last hidden one.
        let mut folds = folded(&[0]);
        folds.follow_edit("a {\n  x\n  y\n}\n", "a {\n  x\n  y}\n");
        assert!(folds.is_empty());
        // Backspace at the end of the header line keeps it.
        let mut folds = folded(&[0]);
        folds.follow_edit("a {\n  x\n}\n", "a \n  x\n}\n");
        assert_eq!(folds, folded(&[0]));
    }

    #[test]
    fn test_reveal_line_unfolds_the_folds_hiding_it() {
        let text = "a\n  b\n    c\n  d\ne\n  f\n";
        let mut folds = folded(&[0, 1, 4]);
        folds.reveal_line(text, 2);
        assert_eq!(folds, folded(&[4]));
        // A header line itself is not hidden by its own fold.
        folds.reveal_line(text, 4);
        assert_eq!(folds, folded(&[4]));
    }

    #[test]
    fn test_toggle_fold_unfold_and_clear() {
        let mut folds = Folds::default();
        folds.toggle(3);
        assert!(folds.is_folded(3));
        folds.toggle(3);
        assert!(!folds.is_folded(3));
        folds.fold(1);
        folds.fold(2);
        folds.unfold(1);
        assert_eq!(folds, folded(&[2]));
        folds.clear();
        assert!(folds.is_empty());
    }
}
