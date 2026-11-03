//! The Command Palette: a popup that runs any command, or opens any file
//! in the open folder, by typing part of its name.

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
}
