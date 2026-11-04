//! Bracket matching: the bracket touching the caret and its partner, and
//! where Go to Bracket puts the caret.
//!
//! Pure functions over `char` indices, which is how egui's `TextEdit` counts
//! its cursor. A plain character scan: brackets inside strings or comments
//! count like any other.

/// Most chars scanned in each direction, so a huge file stays fast.
pub const SCAN_LIMIT: usize = 100_000;

/// The bracket touching `caret` and its partner, as `(char index of the
/// bracket at the caret, char index of its partner)`. The char just before
/// the caret is tried first, then the char just after it. Pairs are
/// `()` `[]` `{}`; nesting is counted for the same kind only. `None` when no
/// bracket touches the caret or no partner is found within `SCAN_LIMIT`.
pub fn matching_pair(text: &str, caret: usize) -> Option<(usize, usize)> {
    // One walk from the start finds the bytes of the two chars at the caret.
    let mut chars = text.char_indices().skip(caret.saturating_sub(1));
    let before = if caret > 0 { chars.next() } else { None };
    let after = chars.next();
    let candidates = [before.map(|c| (caret - 1, c)), after.map(|c| (caret, c))];
    candidates
        .into_iter()
        .flatten()
        .find_map(|(at, (byte, ch))| partner_of(text, at, byte, ch).map(|p| (at, p)))
}

/// The char index of the partner of `ch`, which sits at char index `at` and
/// byte `byte` of `text`. `None` when `ch` is not a bracket or has no
/// partner within `SCAN_LIMIT`.
fn partner_of(text: &str, at: usize, byte: usize, ch: char) -> Option<usize> {
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        '}' => ('{', '}', false),
        _ => return None,
    };
    // Going forward from an opener, a closer ends a level; going back from a
    // closer, an opener does.
    let (deeper, shallower) = if forward { (open, close) } else { (close, open) };
    let step = |iter: &mut dyn Iterator<Item = char>| {
        let mut depth = 0usize;
        iter.take(SCAN_LIMIT).position(|c| {
            if c == deeper {
                depth += 1;
            } else if c == shallower {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        })
    };
    if forward {
        step(&mut text[byte + ch.len_utf8()..].chars()).map(|n| at + 1 + n)
    } else {
        step(&mut text[..byte].chars().rev()).map(|n| at - 1 - n)
    }
}

/// Where Go to Bracket puts the caret: next to the partner, on its outer
/// side — before an opener, after a closer — so pressing again returns.
pub fn jump_target(text: &str, caret: usize) -> Option<usize> {
    let (at, partner) = matching_pair(text, caret)?;
    // A partner before the bracket is its opener; after it, its closer.
    Some(if partner < at { partner } else { partner + 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bracket_before_the_caret_is_preferred() {
        // Caret between ")" and "(": the ")" before it wins.
        let text = "(a)(b)";
        assert_eq!(matching_pair(text, 3), Some((2, 0)));
        // Only the char after the caret is a bracket.
        assert_eq!(matching_pair(text, 0), Some((0, 2)));
        assert_eq!(matching_pair(text, 6), Some((5, 3)));
    }

    #[test]
    fn test_nesting_of_the_same_kind_is_counted() {
        let text = "((a)(b))";
        assert_eq!(matching_pair(text, 0), Some((0, 7)));
        assert_eq!(matching_pair(text, 8), Some((7, 0)));
        assert_eq!(matching_pair(text, 4), Some((3, 1)));
    }

    #[test]
    fn test_other_kinds_do_not_count() {
        // The "]" and "{" don't close or open a "(".
        let text = "(a] { b)";
        assert_eq!(matching_pair(text, 0), Some((0, 7)));
        assert_eq!(matching_pair(text, 8), Some((7, 0)));
        assert_eq!(matching_pair("{[}]", 0), Some((0, 2)));
    }

    #[test]
    fn test_unmatched_bracket_gives_none() {
        assert_eq!(matching_pair("(a", 0), None);
        assert_eq!(matching_pair("a)", 2), None);
        assert_eq!(matching_pair("((a)", 0), None);
        assert_eq!(jump_target("(a", 0), None);
    }

    #[test]
    fn test_caret_not_touching_a_bracket_gives_none() {
        let text = "(abc)";
        assert_eq!(matching_pair(text, 2), None);
        assert_eq!(matching_pair("", 0), None);
        assert_eq!(matching_pair("abc", 3), None);
        assert_eq!(jump_target(text, 2), None);
    }

    #[test]
    fn test_caret_past_the_end_gives_none() {
        assert_eq!(matching_pair("()", 5), None);
    }

    #[test]
    fn test_unmatched_bracket_before_falls_back_to_the_one_after() {
        // As in VS Code: ")" before the caret has no partner, "(" after does.
        assert_eq!(matching_pair(")(a)", 1), Some((1, 3)));
    }

    #[test]
    fn test_indices_count_chars_not_bytes() {
        let text = "é😀 (日本) ü";
        assert_eq!(matching_pair(text, 3), Some((3, 6)));
        assert_eq!(matching_pair(text, 7), Some((6, 3)));
        assert_eq!(jump_target(text, 3), Some(7));
        assert_eq!(jump_target(text, 7), Some(3));
    }

    #[test]
    fn test_scan_stops_at_the_limit() {
        let far = format!("({})", "a".repeat(SCAN_LIMIT));
        assert_eq!(matching_pair(&far, 0), None);
        let near = format!("({})", "a".repeat(SCAN_LIMIT - 1));
        assert_eq!(matching_pair(&near, 0), Some((0, SCAN_LIMIT)));
    }

    #[test]
    fn test_jump_target_lands_outside_the_partner_and_returns() {
        let text = "f(a[1]) + x";
        // Before "(": lands after ")".
        assert_eq!(jump_target(text, 1), Some(7));
        // After ")": lands before "(".
        assert_eq!(jump_target(text, 7), Some(1));
        // Just inside "(": also lands after ")".
        assert_eq!(jump_target(text, 2), Some(7));
        // Pressing twice comes back.
        let once = jump_target(text, 3).unwrap();
        assert_eq!(once, 6);
        assert_eq!(jump_target(text, once), Some(3));
    }
}
