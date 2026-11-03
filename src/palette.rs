//! The Command Palette: a popup that runs any command, or opens any file
//! in the open folder, by typing part of its name.

use std::path::PathBuf;

use crate::app::ShortcutAction;

/// What the palette lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteMode {
    /// Every editor command, run through `apply_shortcut`.
    #[default]
    Commands,
    /// Every file under the open folder.
    Files,
}

#[derive(Debug, Default)]
pub struct Palette {
    pub visible: bool,
    pub mode: PaletteMode,
    pub query: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// Index into the ranked list.
    selected: usize,
}

impl Palette {
    /// Opens in `mode` with an empty query. Opening again in the same mode
    /// closes; in the other mode switches and clears the query.
    pub fn open(&mut self, mode: PaletteMode) {
        if self.visible && self.mode == mode {
            self.visible = false;
            return;
        }
        *self = Self {
            visible: true,
            mode,
            focus: true,
            ..Self::default()
        };
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Moves the selection by `delta` rows within `count` rows, no wrap.
    fn move_selection(&mut self, delta: isize, count: usize) {
        let last = count.saturating_sub(1);
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
    }

    fn clamp_selection(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }
}

/// One row of the list. The app builds these each frame.
#[derive(Clone, Debug)]
pub struct Item<T> {
    pub label: String,
    /// Dimmed text at the right: the shortcut, or the folder of a file.
    pub detail: String,
    /// What the query is scored against: the label for a command, the path
    /// relative to the folder for a file (so `src/app` finds `src/app.rs`).
    pub search: String,
    pub payload: T,
}

/// What the user asked of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Run(ShortcutAction),
    Open(PathBuf),
    Close,
}

/// Indices of the items matching `query`, best first; ties keep their
/// original order.
pub fn rank<T>(query: &str, items: &[Item<T>]) -> Vec<usize> {
    let mut ranked: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score(query, &item.search).map(|s| (s, i)))
        .collect();
    ranked.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// How well `query` matches `text`, higher is better; `None` when it does
/// not match. Case-insensitive subsequence: every char of the query must
/// appear in the text, in order. The first occurrence of each char is
/// taken (greedy), which is good enough for short queries.
///
/// A char matched at the start of a word (the start of the text, or right
/// after `/`, `\`, `_`, `-`, `.` or a space) scores 3; one right after the
/// previous match scores 2; any other 1, minus one per text char skipped
/// since the previous match. Jumping to a word start costs nothing, so
/// `tc` prefers "Toggle Console" to "Watch".
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score: i64 = 0;
    let mut next = 0;
    let mut previous: Option<usize> = None;
    for (at, &c) in text.iter().enumerate() {
        if next == query.len() {
            break;
        }
        if c != query[next] {
            continue;
        }
        let word_start = at == 0 || matches!(text[at - 1], '/' | '\\' | '_' | '-' | '.' | ' ');
        let adjacent = previous.is_some_and(|p| p + 1 == at);
        score += if word_start {
            3
        } else if adjacent {
            2
        } else {
            let skipped = previous.map_or(0, |p| at - p - 1) as i64;
            1 - skipped
        };
        previous = Some(at);
        next += 1;
    }
    (next == query.len()).then(|| score.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_everything_with_zero() {
        assert_eq!(score("", ""), Some(0));
        assert_eq!(score("", "Toggle Console"), Some(0));
    }

    #[test]
    fn test_chars_out_of_order_or_missing_do_not_match() {
        assert_eq!(score("xyz", "Toggle"), None);
        assert_eq!(score("ct", "Toggle Console"), None);
        assert_eq!(score("togglee", "Toggle"), None);
    }

    #[test]
    fn test_word_starts_outrank_mid_word_matches() {
        assert!(score("tc", "Toggle Console") > score("tc", "Watch"));
        assert!(score("app", "src/app.rs") > score("app", "src/wrapper.rs"));
    }

    #[test]
    fn test_adjacent_chars_outrank_spread_ones() {
        assert!(score("app", "app") > score("app", "apxp"));
    }

    #[test]
    fn test_score_ignores_case() {
        assert_eq!(score("TC", "toggle console"), score("tc", "Toggle Console"));
    }

    #[test]
    fn test_score_never_goes_negative() {
        assert_eq!(score("ae", "axxxxxxxxxe"), Some(0));
    }

    fn items(labels: &[&str]) -> Vec<Item<usize>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| Item {
                label: l.to_string(),
                detail: String::new(),
                search: l.to_string(),
                payload: i,
            })
            .collect()
    }

    #[test]
    fn test_rank_keeps_original_order_for_empty_query_and_ties() {
        let items = items(&["b", "a", "c"]);
        assert_eq!(rank("", &items), vec![0, 1, 2]);
    }

    #[test]
    fn test_rank_sorts_by_score_then_original_order() {
        let items = items(&["Watch", "Toggle Console", "Toggle Comment", "Save"]);
        assert_eq!(rank("tc", &items), vec![1, 2, 0]);
    }

    #[test]
    fn test_open_same_mode_twice_toggles_closed() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        assert!(palette.visible && palette.focus);
        palette.open(PaletteMode::Commands);
        assert!(!palette.visible);
    }

    #[test]
    fn test_open_other_mode_switches_and_clears_query() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        palette.query = "tog".into();
        palette.selected = 2;
        palette.open(PaletteMode::Files);
        assert!(palette.visible);
        assert_eq!(palette.mode, PaletteMode::Files);
        assert_eq!(palette.query, "");
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn test_selection_moves_within_bounds() {
        let mut palette = Palette::default();
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        assert_eq!(palette.selected, 2);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        assert_eq!(palette.selected, 0);
        palette.selected = 5;
        palette.clamp_selection(2);
        assert_eq!(palette.selected, 1);
        palette.clamp_selection(0);
        assert_eq!(palette.selected, 0);
    }
}
