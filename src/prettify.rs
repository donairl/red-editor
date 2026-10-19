//! Re-indents a document: JSON and HTML.

use crate::highlighter::Language;

const INDENT: &str = "  ";

/// Elements whose content is whitespace-sensitive or not markup.
const RAW_ELEMENTS: [&str; 4] = ["script", "style", "pre", "textarea"];

const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Whether [`prettify`] knows how to format `language`.
pub fn supports(language: Language) -> bool {
    matches!(language, Language::Json | Language::Html)
}

/// The re-indented `text`, or why it could not be formatted.
pub fn prettify(language: Language, text: &str) -> Result<String, String> {
    match language {
        Language::Json => json(text),
        Language::Html => Ok(html(text)),
        _ => Err("unsupported language".to_owned()),
    }
}

/// Pretty-prints JSON without reordering keys or touching number and string
/// spelling. Invalid JSON is an error.
fn json(text: &str) -> Result<String, String> {
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| e.to_string())?;

    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(INDENT);
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                while let Some(s) = chars.next() {
                    out.push(s);
                    if s == '\\' {
                        out.extend(chars.next());
                    } else if s == '"' {
                        break;
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                while chars.next_if(|n| n.is_whitespace()).is_some() {}
                if chars.next_if(|&n| n == if c == '{' { '}' } else { ']' }).is_some() {
                    out.push(if c == '{' { '}' } else { ']' });
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out.push('\n');
    Ok(out)
}

/// A piece of an HTML document.
enum Token<'a> {
    Open(&'a str, String),
    Close(String),
    /// Self-contained: void element, comment, doctype, `<?...?>` or a raw element.
    Standalone(&'a str),
    Text(&'a str),
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches(['<', '/'])
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == ':')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Index just past the `>` that ends the tag at the start of `s`, skipping
/// quoted attribute values.
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return i + 1,
            _ => {}
        }
    }
    s.len()
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            tokens.push(Token::Text(&rest[..end]));
            rest = &rest[end..];
            continue;
        }
        let end = if rest.starts_with("<!--") {
            rest.find("-->").map_or(rest.len(), |i| i + 3)
        } else if rest.starts_with("<?") {
            rest.find("?>").map_or(rest.len(), |i| i + 2)
        } else {
            tag_end(rest)
        };
        let tag = &rest[..end];
        let name = tag_name(tag);
        let closing = tag.starts_with("</");
        let standalone = tag.starts_with("<!") || tag.starts_with("<?") || tag.ends_with("/>");
        if standalone || (!closing && VOID_ELEMENTS.contains(&name.as_str())) {
            tokens.push(Token::Standalone(tag));
            rest = &rest[end..];
        } else if closing {
            tokens.push(Token::Close(name));
            rest = &rest[end..];
        } else if RAW_ELEMENTS.contains(&name.as_str()) {
            let lower = rest.to_ascii_lowercase();
            let close = lower[end..]
                .find(&format!("</{name}"))
                .map(|i| end + i)
                .map(|i| i + tag_end(&rest[i..]))
                .unwrap_or(rest.len());
            tokens.push(Token::Standalone(&rest[..close]));
            rest = &rest[close..];
        } else {
            tokens.push(Token::Open(tag, name));
            rest = &rest[end..];
        }
    }
    tokens
}

/// One tag or text run per line, indented by nesting depth. Whitespace runs
/// in text collapse to one space; raw elements and comments stay verbatim.
fn html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut stack: Vec<String> = Vec::new();
    let line = |out: &mut String, depth: usize, s: &str| {
        for _ in 0..depth {
            out.push_str(INDENT);
        }
        out.push_str(s);
        out.push('\n');
    };
    for token in tokenize(text) {
        match token {
            Token::Open(tag, name) => {
                line(&mut out, stack.len(), tag);
                stack.push(name);
            }
            Token::Close(name) => {
                // Tolerate stray close tags: only pop when it matches an open one.
                if let Some(pos) = stack.iter().rposition(|n| *n == name) {
                    stack.truncate(pos);
                }
                let tag = format!("</{name}>");
                line(&mut out, stack.len(), &tag);
            }
            Token::Standalone(s) => line(&mut out, stack.len(), s),
            Token::Text(t) => {
                let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !collapsed.is_empty() {
                    line(&mut out, stack.len(), &collapsed);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_indents_and_keeps_key_order() {
        let out = prettify(Language::Json, r#"{"b":1,"a":[1,2,{"x":null}],"e":{},"s":"a, {b}\"c"}"#).unwrap();
        assert_eq!(
            out,
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2,\n    {\n      \"x\": null\n    }\n  ],\n  \"e\": {},\n  \"s\": \"a, {b}\\\"c\"\n}\n"
        );
    }

    #[test]
    fn test_json_is_idempotent() {
        let once = prettify(Language::Json, r#"[1,{"a":[]}]"#).unwrap();
        assert_eq!(prettify(Language::Json, &once).unwrap(), once);
    }

    #[test]
    fn test_json_invalid_is_error() {
        assert!(prettify(Language::Json, "{\"a\":").is_err());
    }

    #[test]
    fn test_html_indents_nesting() {
        let out = prettify(Language::Html, "<div><p>Hi   <b>there</b></p><br><img src=\"a>b\"></div>").unwrap();
        assert_eq!(
            out,
            "<div>\n  <p>\n    Hi\n    <b>\n      there\n    </b>\n  </p>\n  <br>\n  <img src=\"a>b\">\n</div>\n"
        );
    }

    #[test]
    fn test_html_keeps_raw_elements_verbatim() {
        let src = "<body><pre>  a\n   b </pre><script>if (a < b) {}</script></body>";
        let out = prettify(Language::Html, src).unwrap();
        assert_eq!(out, "<body>\n  <pre>  a\n   b </pre>\n  <script>if (a < b) {}</script>\n</body>\n");
    }

    #[test]
    fn test_html_stray_close_tag_does_not_underflow() {
        assert_eq!(prettify(Language::Html, "</div><p>x</p>").unwrap(), "</div>\n<p>\n  x\n</p>\n");
    }

    #[test]
    fn test_unsupported_language() {
        assert!(!supports(Language::Rust));
        assert!(prettify(Language::Rust, "fn main(){}").is_err());
    }
}
