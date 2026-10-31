//! Which lines of a document differ from git's copy, for the gutter's
//! change markers.

use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp, TextDiff};

use crate::find;

/// Longest a diff may take. Past it the diff is coarser, never wrong.
const DEADLINE: Duration = Duration::from_millis(50);

/// What changed at one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    /// Lines were removed just above this one.
    DeletedAbove,
    /// Lines were removed after this one, the last.
    DeletedBelow,
}

/// The mark of every changed line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineMarks {
    /// Index 0 is line 1.
    lines: Vec<Option<Mark>>,
}

impl LineMarks {
    /// The mark of 1-based `line`.
    pub fn get(&self, line: usize) -> Option<Mark> {
        self.lines.get(line.checked_sub(1)?).copied().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Option::is_none)
    }

    fn set(&mut self, line: usize, mark: Mark) {
        if self.lines.len() < line {
            self.lines.resize(line, None);
        }
        self.lines[line - 1] = Some(mark);
    }
}

/// Marks the lines of `text` that differ from `base`. An empty `base`
/// marks every line added.
pub fn marks(base: &str, text: &str) -> LineMarks {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + DEADLINE)
        .diff_lines(base, text);
    let last_line = find::line_count(text);
    let mut marks = LineMarks::default();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => (new_index..new_index + new_len).for_each(|i| marks.set(i + 1, Mark::Added)),
            DiffOp::Replace {
                new_index, new_len, ..
            } => (new_index..new_index + new_len).for_each(|i| marks.set(i + 1, Mark::Modified)),
            DiffOp::Delete { new_index, .. } => {
                // `new_index` is the 0-based line now following the removed
                // ones; past the end there is none to mark.
                let line = new_index + 1;
                if line <= last_line {
                    // A line already marked keeps its own mark.
                    if marks.get(line).is_none() {
                        marks.set(line, Mark::DeletedAbove);
                    }
                } else if marks.get(last_line).is_none() {
                    marks.set(last_line.max(1), Mark::DeletedBelow);
                }
            }
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(marks: &LineMarks, lines: usize) -> Vec<Option<Mark>> {
        (1..=lines).map(|l| marks.get(l)).collect()
    }

    #[test]
    fn test_unchanged_text_has_no_marks() {
        let m = marks("a\nb\n", "a\nb\n");
        assert!(m.is_empty());
        assert_eq!(m.get(0), None);
    }

    #[test]
    fn test_added_modified_and_deleted_lines() {
        use Mark::*;
        let base = "a\nb\nc\nd\ne\n";
        assert_eq!(list(&marks(base, "a\nNEW\nb\nc\nd\ne\n"), 6), [None, Some(Added), None, None, None, None]);
        assert_eq!(list(&marks(base, "a\nB\nc\nd\ne\n"), 5), [None, Some(Modified), None, None, None]);
        assert_eq!(list(&marks(base, "a\nc\nd\ne\n"), 4), [None, Some(DeletedAbove), None, None]);
        assert_eq!(list(&marks(base, "b\nc\nd\ne\n"), 4), [Some(DeletedAbove), None, None, None]);
    }

    #[test]
    fn test_deleting_the_last_lines() {
        use Mark::*;
        // With the final newline kept, the empty line after it gets the mark.
        assert_eq!(list(&marks("a\nb\nc\n", "a\n"), 2), [None, Some(DeletedAbove)]);
        // Without one, the last line does.
        assert_eq!(list(&marks("a\nb\nc", "a"), 1), [Some(Modified)]);
        // Everything deleted: the one empty line left.
        assert_eq!(marks("a\nb\n", "").get(1), Some(DeletedAbove));
    }

    #[test]
    fn test_empty_base_marks_every_line_added() {
        use Mark::*;
        assert_eq!(list(&marks("", "x\ny\n"), 3), [Some(Added), Some(Added), None]);
    }
}
