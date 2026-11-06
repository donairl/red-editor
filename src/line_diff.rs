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

/// A contiguous changed hunk between `base` and `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based line range in the current `text` [start, end].
    /// For pure deletion above line L, this is L..=L.
    pub new_line_range: std::ops::RangeInclusive<usize>,
    /// The lines from `base` that were removed/replaced (without trailing newline).
    pub old_lines: Vec<String>,
    /// The lines from `text` that were added/modified (without trailing newline).
    pub new_lines: Vec<String>,
    /// Char range in `text` that this hunk replaces.
    pub char_range: (usize, usize),
    /// The original text in `base` that should replace `char_range` to revert this hunk.
    pub revert_replacement: String,
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

    #[cfg_attr(not(test), allow(dead_code))]
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

/// The changed block of `ops` the gutter shows at 1-based `line`: its line
/// ranges in the old and the new text (0-based, end exclusive). A removal
/// is shown on the line now in its place, or on the last line past the end.
fn op_at(ops: &[DiffOp], last_line: usize, line: usize) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    ops.iter().find_map(|op| {
        let hit = match *op {
            DiffOp::Equal { .. } => false,
            DiffOp::Insert { .. } | DiffOp::Replace { .. } => op.new_range().contains(&(line - 1)),
            DiffOp::Delete { new_index, .. } => {
                let marker = if new_index >= last_line { last_line.max(1) } else { new_index + 1 };
                marker == line
            }
        };
        hit.then(|| (op.old_range(), op.new_range()))
    })
}

fn trim_newline(s: &str) -> String {
    s.strip_suffix('\n').map(|s| s.strip_suffix('\r').unwrap_or(s)).unwrap_or(s).to_owned()
}

/// Finds the changed hunk the gutter marks at 1-based `target_line`.
pub fn hunk_at(base: &str, text: &str, target_line: usize) -> Option<Hunk> {
    if target_line == 0 {
        return None;
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + DEADLINE)
        .diff_lines(base, text);
    let (base_lines, text_lines) = (diff.old_slices(), diff.new_slices());
    let (old, new) = op_at(diff.ops(), find::line_count(text), target_line)?;
    let chars = |lines: &[&str]| lines.iter().map(|s| s.chars().count()).sum::<usize>();
    let char_start = chars(&text_lines[..new.start]);
    let char_end = char_start + chars(&text_lines[new.clone()]);
    let shown = if new.is_empty() { target_line..=target_line } else { new.start + 1..=new.end };
    Some(Hunk {
        new_line_range: shown,
        old_lines: base_lines[old.clone()].iter().map(|s| trim_newline(s)).collect(),
        new_lines: text_lines[new.clone()].iter().map(|s| trim_newline(s)).collect(),
        char_range: (char_start, char_end),
        revert_replacement: base_lines[old].concat(),
    })
}

/// `base` with only the hunk shown at 1-based `line` taken from `text`:
/// what the index holds after staging that one hunk.
pub fn apply_hunk(base: &str, text: &str, line: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + DEADLINE)
        .diff_lines(base, text);
    let (base_lines, text_lines) = (diff.old_slices(), diff.new_slices());
    let (old, new) = op_at(diff.ops(), find::line_count(text), line)?;
    let mut out = base_lines[..old.start].concat();
    let added = text_lines[new].concat();
    // A base without a final newline followed by inserted lines.
    if !out.is_empty() && !out.ends_with('\n') && !added.is_empty() {
        out.push('\n');
    }
    out.push_str(&added);
    out.push_str(&base_lines[old.end..].concat());
    Some(out)
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

    #[test]
    fn test_hunk_at_modified_added_and_deleted() {
        let base = "line1\nline2\nline3\nline4\n";
        let text = "line1\nMODIFIED\nline3\nline4\n";
        let hunk = hunk_at(base, text, 2).expect("should find hunk at line 2");
        assert_eq!(hunk.new_line_range, 2..=2);
        assert_eq!(hunk.old_lines, vec!["line2"]);
        assert_eq!(hunk.new_lines, vec!["MODIFIED"]);
        assert_eq!(hunk.revert_replacement, "line2\n");

        // Test revert application
        let mut reverted = text.to_string();
        reverted.replace_range(hunk.char_range.0..hunk.char_range.1, &hunk.revert_replacement);
        assert_eq!(reverted, base);

        // Test insertion
        let text_ins = "line1\nline2\nINSERTED1\nINSERTED2\nline3\nline4\n";
        let hunk_ins = hunk_at(base, text_ins, 3).expect("hunk at line 3");
        assert_eq!(hunk_ins.new_line_range, 3..=4);
        assert!(hunk_ins.old_lines.is_empty());
        assert_eq!(hunk_ins.new_lines, vec!["INSERTED1", "INSERTED2"]);
        assert_eq!(hunk_ins.revert_replacement, "");
        let mut reverted_ins = text_ins.to_string();
        reverted_ins.replace_range(hunk_ins.char_range.0..hunk_ins.char_range.1, &hunk_ins.revert_replacement);
        assert_eq!(reverted_ins, base);

        // Test deletion
        let text_del = "line1\nline3\nline4\n";
        // line2 was deleted; in text_del, line2 is now line3 (DeletedAbove)
        let hunk_del = hunk_at(base, text_del, 2).expect("hunk at line 2");
        assert_eq!(hunk_del.old_lines, vec!["line2"]);
        assert!(hunk_del.new_lines.is_empty());
        assert_eq!(hunk_del.revert_replacement, "line2\n");
        let mut reverted_del = text_del.to_string();
        reverted_del.replace_range(hunk_del.char_range.0..hunk_del.char_range.1, &hunk_del.revert_replacement);
        assert_eq!(reverted_del, base);
    }

    #[test]
    fn test_apply_hunk_takes_only_that_hunk() {
        let base = "a\nb\nc\nd\ne\n";
        let text = "a\nB\nc\nd\nE\nf\n";
        assert_eq!(apply_hunk(base, text, 2).unwrap(), "a\nB\nc\nd\ne\n");
        assert_eq!(apply_hunk(base, text, 5).unwrap(), "a\nb\nc\nd\nE\nf\n");
        assert_eq!(apply_hunk(base, text, 3), None, "unchanged line");
        // A removal, marked on the line now in its place.
        assert_eq!(apply_hunk(base, "a\nc\nd\ne\n", 2).unwrap(), "a\nc\nd\ne\n");
        assert_eq!(hunk_at(base, text, 0), None);
    }
}
