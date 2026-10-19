//! SQL inside string literals: a string whose text reads as an SQL
//! statement is coloured with the SQL grammar, over the string's colour.
//!
//! PHP's and Python's grammars already embed SQL in some strings (scope
//! `source.sql`); those are left alone. Elsewhere the host grammar's scopes
//! tell where string text is, and the SQL grammar re-colours that text,
//! carrying its state from line to line through a multi-line string.

use std::ops::Range;
use std::sync::LazyLock;

use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Style};
use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};

/// A coloured byte range of a line.
pub type StyledRange = (Style, Range<usize>);

/// The string a line ended inside, as the next line picks it up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Carry {
    /// Only white space so far: the next text decides.
    Undecided,
    /// Not SQL, or SQL the host grammar colours itself.
    NotSql,
    /// SQL, with the SQL grammar's state.
    Sql(ParseState, HighlightState),
}

/// What a stretch of a line is, judged by its scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// Outside any string.
    Code,
    /// A string's opening or closing mark (`"`, `r#"`, `` ` ``).
    Delimiter { end: bool },
    /// The text of a string.
    Text,
    /// Code inside a string, such as `${id}` in a template literal.
    Interpolation,
    /// Inside SQL the host grammar already embeds.
    Embedded,
}

/// Scope prefixes [`classify`] looks for, parsed once.
struct Prefixes {
    string: Scope,
    sql: Scope,
    delimiter: [Scope; 2],
    code: [Scope; 6],
}

static PREFIXES: LazyLock<Prefixes> = LazyLock::new(|| {
    let scope = |name| Scope::new(name).expect("valid scope name");
    Prefixes {
        string: scope("string"),
        sql: scope("source.sql"),
        // `storage.type.string` is the `r` of a Rust raw string.
        delimiter: [scope("punctuation.definition.string"), scope("storage.type.string")],
        code: [
            scope("meta.interpolation"),
            scope("meta.template.expression"),
            scope("punctuation.section.interpolation"),
            scope("meta.embedded"),
            scope("variable.other"),
            scope("source"),
        ],
    }
});

fn classify(stack: &ScopeStack) -> Part {
    let p = &*PREFIXES;
    let scopes = stack.as_slice();
    if scopes.iter().any(|&s| p.sql.is_prefix_of(s)) {
        return Part::Embedded;
    }
    let Some(string) = scopes.iter().rposition(|&s| p.string.is_prefix_of(s)) else {
        return Part::Code;
    };
    let inner = &scopes[string + 1..];
    if let Some(&mark) = inner
        .iter()
        .find(|&&s| p.delimiter.iter().any(|d| d.is_prefix_of(s)))
    {
        return Part::Delimiter {
            end: mark.build_string().contains(".end"),
        };
    }
    if inner.iter().any(|&s| p.code.iter().any(|c| c.is_prefix_of(s))) {
        return Part::Interpolation;
    }
    Part::Text
}

/// Statements recognised from their first word. Upper case alone is
/// enough (`SELECT id`); otherwise the text must also read as SQL, so UI
/// text such as "Select a file" or "Update failed" keeps its string colour.
const KEYWORDS: [&str; 13] = [
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "ALTER", "DROP", "REPLACE",
    "MERGE", "TRUNCATE", "GRANT", "EXPLAIN",
];

/// Objects `CREATE`, `ALTER` and `DROP` act on.
const OBJECTS: [&str; 11] = [
    "table", "index", "view", "database", "schema", "unique", "or", "trigger", "function",
    "procedure", "sequence",
];

/// Whether `text`, the start of a string, reads as an SQL statement.
pub fn looks_like_sql(text: &str) -> bool {
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let Some(&first) = words.first() else {
        return false;
    };
    if !text.trim_start().starts_with(first) {
        return false;
    }
    let keyword = first.to_ascii_uppercase();
    if !KEYWORDS.contains(&keyword.as_str()) {
        return false;
    }
    if first == keyword {
        return true;
    }
    let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let next = lower.get(1).map(String::as_str);
    let has = |word: &str| lower[1..].iter().any(|w| w == word);
    match keyword.as_str() {
        "SELECT" => has("from"),
        "INSERT" | "REPLACE" | "MERGE" => next == Some("into"),
        "UPDATE" => has("set"),
        "DELETE" => next == Some("from"),
        "CREATE" | "ALTER" | "DROP" => next.is_some_and(|n| OBJECTS.contains(&n)),
        "TRUNCATE" => next == Some("table"),
        _ => false,
    }
}

/// SQL colours for the string text of one line.
///
/// `line` is the line with its `\n`, `ops` the host grammar's ops for it,
/// and `stack` the host scope stack at the line's start. `carry` is the
/// string the previous line ended inside; it is left as this line ends.
pub fn sql_ranges(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    mut stack: ScopeStack,
    carry: &mut Option<Carry>,
    sql: &SyntaxReference,
    syntax_set: &SyntaxSet,
    highlighter: &Highlighter,
) -> Vec<StyledRange> {
    // The line cut where its scopes change, joining neighbouring text. Text
    // keeps its scopes, for SQL that starts there.
    let mut parts: Vec<(Range<usize>, Part, Option<ScopeStack>)> = Vec::new();
    let mut at = 0;
    let end = (line.len(), ScopeStackOp::Noop);
    for (pos, op) in ops.iter().chain(std::iter::once(&end)) {
        let pos = (*pos).min(line.len());
        if pos > at {
            let part = classify(&stack);
            match parts.last_mut() {
                Some((range, Part::Text, _)) if part == Part::Text => range.end = pos,
                _ => parts.push((at..pos, part, (part == Part::Text).then(|| stack.clone()))),
            }
            at = pos;
        }
        // An op the stack cannot apply only affects colouring here.
        let _ = stack.apply(op);
    }

    let mut out = Vec::new();
    for (range, part, stack) in parts {
        match part {
            Part::Code => *carry = None,
            Part::Delimiter { end: true } => *carry = None,
            Part::Delimiter { end: false } => {
                if carry.is_none() {
                    *carry = Some(Carry::Undecided);
                }
            }
            Part::Embedded => *carry = Some(Carry::NotSql),
            Part::Interpolation => {}
            Part::Text => {
                let text = &line[range.clone()];
                let (mut parse, mut highlight) = match carry.take() {
                    Some(Carry::Sql(parse, highlight)) => (parse, highlight),
                    Some(Carry::NotSql) => {
                        *carry = Some(Carry::NotSql);
                        continue;
                    }
                    None | Some(Carry::Undecided) if text.trim().is_empty() => {
                        *carry = Some(Carry::Undecided);
                        continue;
                    }
                    None | Some(Carry::Undecided) => {
                        if !looks_like_sql(text) {
                            *carry = Some(Carry::NotSql);
                            continue;
                        }
                        // On top of the string's scopes, so SQL names keep
                        // the string colour and only keywords and the like
                        // change.
                        let stack = stack.unwrap_or_default();
                        (ParseState::new(sql), HighlightState::new(highlighter, stack))
                    }
                };
                if let Ok(ops) = parse.parse_line(text, syntax_set) {
                    let mut at = range.start;
                    for (style, piece) in HighlightIterator::new(&mut highlight, &ops, text, highlighter) {
                        out.push((style, at..at + piece.len()));
                        at += piece.len();
                    }
                }
                *carry = Some(Carry::Sql(parse, highlight));
            }
        }
    }
    out
}

/// `base` with the ranges of `patch` laid over it. Both cover one line in
/// order; the result does too.
pub fn overlay(base: Vec<StyledRange>, patch: Vec<StyledRange>) -> Vec<StyledRange> {
    if patch.is_empty() {
        return base;
    }
    let mut out = Vec::with_capacity(base.len() + patch.len());
    for (style, range) in base {
        let mut at = range.start;
        for (_, p) in patch.iter().filter(|(_, p)| p.start < range.end && p.end > range.start) {
            if p.start > at {
                out.push((style, at..p.start));
            }
            at = at.max(p.end);
        }
        if at < range.end {
            out.push((style, at..range.end));
        }
    }
    out.extend(patch);
    out.sort_by_key(|(_, range)| range.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upper_case_keyword_is_enough() {
        assert!(looks_like_sql("SELECT id"));
        assert!(looks_like_sql("  INSERT INTO t"));
        assert!(looks_like_sql("WITH x AS (SELECT 1)"));
    }

    #[test]
    fn test_other_case_needs_a_second_clause() {
        assert!(looks_like_sql("select * from users"));
        assert!(looks_like_sql("insert into t values (1)"));
        assert!(looks_like_sql("update users set name = ?"));
        assert!(looks_like_sql("Delete From t"));
        assert!(looks_like_sql("create table t (id int)"));
        assert!(!looks_like_sql("Select a file"));
        assert!(!looks_like_sql("Update failed"));
        assert!(!looks_like_sql("delete this?"));
        assert!(!looks_like_sql("Drop files here"));
        assert!(!looks_like_sql("selected"), "a keyword must be a whole word");
        assert!(!looks_like_sql("-- SELECT"), "the keyword must open the text");
        assert!(!looks_like_sql(""));
    }

    fn style(n: u8) -> Style {
        let mut style = Style::default();
        style.foreground.r = n;
        style
    }

    #[test]
    fn test_overlay_cuts_base_ranges_around_the_patch() {
        let base = vec![(style(1), 0..4), (style(2), 4..10)];
        let patch = vec![(style(9), 2..6)];
        let ranges: Vec<(u8, Range<usize>)> = overlay(base, patch)
            .into_iter()
            .map(|(s, r)| (s.foreground.r, r))
            .collect();
        assert_eq!(ranges, vec![(1, 0..2), (9, 2..6), (2, 6..10)]);
    }
}
