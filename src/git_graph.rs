//! The Git Graph: every branch and tag as a commit list with coloured
//! lanes, a detail pane for the selected commit, and branch actions.
//!
//! The lane layout is pure and incremental; the view returns what the
//! user asked for as a [`GraphAction`] and `app.rs` runs it.

use crate::git::Commit;

/// How many colours lanes cycle through.
pub const PALETTE_LEN: usize = 8;

/// A line from this row's lane `from` to the next row's lane `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// Where one commit sits and the lines leaving it downward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub edges: Vec<Edge>,
}

/// A lane waiting for a commit to appear further down.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    hash: String,
    color: usize,
    /// Lanes of the row above that feed this slot: itself when it merely
    /// continues, the committing lane when it was just opened or joined.
    sources: Vec<usize>,
}

/// Lanes for commits in topological order (children before parents).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<GraphRow>,
    active: Vec<Option<Slot>>,
    next_color: usize,
    /// The widest the graph has been.
    pub lanes: usize,
}

impl Layout {
    pub fn new(commits: &[Commit]) -> Layout {
        let mut layout = Layout::default();
        layout.extend(commits);
        layout
    }

    /// Lays out further commits; earlier rows keep their places.
    pub fn extend(&mut self, commits: &[Commit]) {
        for commit in commits {
            self.push(commit);
        }
    }

    fn push(&mut self, commit: &Commit) {
        // 1. The commit's lane: the first slot waiting for it, else a free one.
        let waiting = self.active.iter().position(|s| s.as_ref().is_some_and(|s| s.hash == commit.hash));
        let lane = match waiting {
            Some(lane) => lane,
            None => self.free_slot(),
        };
        let color = match &self.active[lane] {
            Some(slot) => slot.color,
            None => self.new_color(),
        };

        // 2. Lines from the row above into this row.
        if let Some(prev) = self.rows.last_mut() {
            for (i, slot) in self.active.iter_mut().enumerate() {
                let Some(slot) = slot else { continue };
                let to = if slot.hash == commit.hash { lane } else { i };
                for &from in &slot.sources {
                    prev.edges.push(Edge { from, to, color: slot.color });
                }
                slot.sources = vec![i];
            }
        }

        // 3. Every slot waiting for this commit ends here.
        for slot in &mut self.active {
            if slot.as_ref().is_some_and(|s| s.hash == commit.hash) {
                *slot = None;
            }
        }

        // 4. Parents: the first keeps the lane, others open or join lanes.
        for (k, parent) in commit.parents.iter().enumerate() {
            let joined = self.active.iter().position(|s| s.as_ref().is_some_and(|s| s.hash == *parent));
            match joined {
                Some(j) => self.active[j].as_mut().unwrap().sources.push(lane),
                None if k == 0 => {
                    self.active[lane] = Some(Slot { hash: parent.clone(), color, sources: vec![lane] });
                }
                None => {
                    let j = self.free_slot();
                    let color = self.new_color();
                    self.active[j] = Some(Slot { hash: parent.clone(), color, sources: vec![lane] });
                }
            }
        }

        while self.active.last().is_some_and(Option::is_none) {
            self.active.pop();
        }
        self.lanes = self.lanes.max(self.active.len()).max(lane + 1);
        self.rows.push(GraphRow { lane, color, edges: Vec::new() });
    }

    /// The first empty slot, opening one at the end when none is.
    fn free_slot(&mut self) -> usize {
        match self.active.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.active.push(None);
                self.active.len() - 1
            }
        }
    }

    fn new_color(&mut self) -> usize {
        let color = self.next_color % PALETTE_LEN;
        self.next_color += 1;
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.to_owned(),
            short: hash.chars().take(7).collect(),
            author: "Test".into(),
            time: 0,
            subject: hash.to_owned(),
            refs: Vec::new(),
            head: false,
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn lanes(layout: &Layout) -> Vec<usize> {
        layout.rows.iter().map(|r| r.lane).collect()
    }

    #[test]
    fn test_linear_history_is_one_lane() {
        let layout = Layout::new(&[commit("a", &["b"]), commit("b", &["c"]), commit("c", &[])]);
        assert_eq!(lanes(&layout), [0, 0, 0]);
        assert_eq!(layout.lanes, 1);
        assert_eq!(layout.rows[0].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert_eq!(layout.rows[1].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert!(layout.rows[2].edges.is_empty());
        assert!(layout.active.is_empty(), "nothing waits after the root");
    }

    #[test]
    fn test_branch_and_merge_use_two_lanes() {
        let commits = [
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 0, 1, 0]);
        assert_eq!(layout.lanes, 2);
        // The merge forks into lane 1 for its second parent.
        assert!(layout.rows[0].edges.contains(&Edge { from: 0, to: 1, color: 1 }));
        assert!(layout.rows[0].edges.contains(&Edge { from: 0, to: 0, color: 0 }));
        // `b` sits in lane 1 and its line joins `r` in lane 0.
        assert!(layout.rows[2].edges.contains(&Edge { from: 1, to: 0, color: 0 }));
        assert!(layout.rows[2].edges.contains(&Edge { from: 0, to: 0, color: 0 }));
        assert_eq!(layout.rows[2].color, 1);
        assert!(layout.active.is_empty());
    }

    #[test]
    fn test_octopus_merge_opens_a_lane_per_extra_parent() {
        let commits = [
            commit("m", &["a", "b", "c"]),
            commit("a", &[]),
            commit("b", &[]),
            commit("c", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 0, 1, 2]);
        assert_eq!(layout.lanes, 3);
        let colors: Vec<usize> = layout.rows.iter().map(|r| r.color).collect();
        assert_eq!(colors, [0, 0, 1, 2]);
    }

    #[test]
    fn test_second_root_takes_a_free_lane() {
        let layout = Layout::new(&[commit("a", &["b"]), commit("x", &[]), commit("b", &[])]);
        assert_eq!(lanes(&layout), [0, 1, 0]);
        assert_eq!(layout.lanes, 2);
        assert_eq!(layout.rows[0].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert_eq!(layout.rows[1].edges, [Edge { from: 0, to: 0, color: 0 }], "the line passes `x`");
    }

    #[test]
    fn test_extending_in_two_halves_matches_one_pass() {
        let commits = [
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let whole = Layout::new(&commits);
        let mut halves = Layout::new(&commits[..2]);
        halves.extend(&commits[2..]);
        assert_eq!(halves, whole);
    }

    #[test]
    fn test_colors_cycle_through_the_palette() {
        let mut commits = vec![commit("m", &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8"])];
        for i in 0..9 {
            commits.push(commit(&format!("p{i}"), &[]));
        }
        let layout = Layout::new(&commits);
        assert_eq!(layout.rows[9].color, 0, "the ninth lane reuses the first colour");
        assert_eq!(layout.lanes, 9);
    }
}
