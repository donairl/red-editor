//! Re-indents a document: JSON, HTML and CSS in-process, other languages through
//! the formatter installed on the machine (`rustfmt`, `gofmt`, ...).

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::highlighter::Language;

/// How long an external formatter may run before it is given up on.
const EXTERNAL_TIMEOUT: Duration = Duration::from_secs(10);

const INDENT: &str = "  ";

/// Elements whose content is whitespace-sensitive or not markup.
const RAW_ELEMENTS: [&str; 4] = ["script", "style", "pre", "textarea"];

const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Whether [`prettify`] knows how to format `language`.
pub fn supports(language: Language) -> bool {
    matches!(language, Language::Json | Language::Html | Language::Css) || !formatters(language).is_empty()
}

/// External formatters for `language`, best first. Each reads the document
/// on stdin and writes the result to stdout.
fn formatters(language: Language) -> &'static [&'static [&'static str]] {
    match language {
        Language::Rust => &[&["rustfmt", "--edition", "2024"]],
        Language::Go => &[&["gofmt"]],
        Language::C | Language::Cpp => &[&["clang-format"]],
        Language::Python => &[&["ruff", "format", "-"], &["black", "-q", "-"]],
        Language::JavaScript => &[&["prettier", "--stdin-filepath", "file.js"]],
        Language::TypeScript => &[&["prettier", "--stdin-filepath", "file.ts"]],
        _ => &[],
    }
}

/// The re-indented `text`, or why it could not be formatted.
pub fn prettify(language: Language, text: &str) -> Result<String, String> {
    match language {
        Language::Json => json(text),
        Language::Html => Ok(html(text)),
        Language::Css => css(text),
        _ => external(language, text),
    }
}

/// Runs the first installed formatter for `language` over `text`.
fn external(language: Language, text: &str) -> Result<String, String> {
    let candidates = formatters(language);
    if candidates.is_empty() {
        return Err("unsupported language".to_owned());
    }
    for argv in candidates {
        match run_formatter(argv, text) {
            Err(RunError::NotInstalled) => continue,
            Err(RunError::Failed(message)) => return Err(message),
            Ok(out) => return Ok(out),
        }
    }
    let names: Vec<&str> = candidates.iter().map(|argv| argv[0]).collect();
    Err(format!("{} not installed", names.join(" / ")))
}

enum RunError {
    NotInstalled,
    Failed(String),
}

/// Keeps a console formatter from opening a console window of its own: on
/// Windows the editor runs without a console for it to share.
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

fn run_formatter(argv: &[&str], text: &str) -> Result<String, RunError> {
    let mut command = Command::new(argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotInstalled,
            _ => RunError::Failed(format!("{}: {e}", argv[0])),
        })?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input = text.to_owned();
    // Feed stdin from a thread so a formatter that writes before it has read
    // everything cannot deadlock on full pipes.
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let (tx, rx) = mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(EXTERNAL_TIMEOUT) {
        Ok(result) => result.map_err(|e| RunError::Failed(format!("{}: {e}", argv[0])))?,
        Err(_) => {
            let _ = Command::new("kill").arg(pid.to_string()).status();
            return Err(RunError::Failed(format!("{} timed out", argv[0])));
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed");
        return Err(RunError::Failed(format!("{}: {first}", argv[0])));
    }
    String::from_utf8(output.stdout).map_err(|e| RunError::Failed(format!("{}: {e}", argv[0])))
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

/// Re-indents CSS: one declaration per line, one selector of a list per line,
/// two spaces per nesting level. Comments and strings stay verbatim; other
/// whitespace collapses. Unbalanced braces, strings or comments are errors.
fn css(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut segment = String::new();
    let mut parens = 0usize;
    let mut chars = text.chars().peekable();
    let emit = |out: &mut String, depth: usize, s: &str| {
        for _ in 0..depth {
            out.push_str(INDENT);
        }
        out.push_str(s);
        out.push('\n');
    };
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                segment.push(c);
                loop {
                    let s = chars.next().ok_or("unterminated string")?;
                    segment.push(s);
                    if s == '\\' {
                        segment.extend(chars.next());
                    } else if s == c {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                let mut comment = String::from("/*");
                chars.next();
                let mut closed = false;
                while let Some(s) = chars.next() {
                    comment.push(s);
                    if s == '*' && chars.next_if_eq(&'/').is_some() {
                        comment.push('/');
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err("unterminated comment".to_owned());
                }
                if segment.trim().is_empty() {
                    segment.clear();
                    emit(&mut out, depth, &comment);
                } else {
                    segment.push_str(&comment);
                }
            }
            '(' => {
                parens += 1;
                segment.push(c);
            }
            ')' => {
                parens = parens.saturating_sub(1);
                segment.push(c);
            }
            '{' if parens == 0 => {
                let selector = css_selector(segment.trim(), depth);
                out.push_str(&selector);
                out.push_str(" {\n");
                segment.clear();
                depth += 1;
            }
            ';' if parens == 0 => {
                if !segment.trim().is_empty() {
                    emit(&mut out, depth, &format!("{};", css_declaration(segment.trim())));
                }
                segment.clear();
            }
            '}' if parens == 0 => {
                if !segment.trim().is_empty() {
                    emit(&mut out, depth, &format!("{};", css_declaration(segment.trim())));
                    segment.clear();
                }
                depth = depth.checked_sub(1).ok_or("unmatched }")?;
                emit(&mut out, depth, "}");
            }
            c if c.is_whitespace() => {
                if !segment.is_empty() && !segment.ends_with(' ') {
                    segment.push(' ');
                }
            }
            c => segment.push(c),
        }
    }
    if depth != 0 {
        return Err("unclosed {".to_owned());
    }
    if !segment.trim().is_empty() {
        emit(&mut out, 0, segment.trim());
    }
    Ok(out)
}

/// `prop:value` as `prop: value`; at-rules such as `@import` are untouched.
fn css_declaration(segment: &str) -> String {
    if segment.starts_with('@') {
        return segment.to_owned();
    }
    match segment.split_once(':') {
        Some((property, value)) => format!("{}: {}", property.trim_end(), value.trim_start()),
        None => segment.to_owned(),
    }
}

/// A selector list with one selector per line; at-rule preludes stay on one line.
fn css_selector(segment: &str, depth: usize) -> String {
    let indent = INDENT.repeat(depth);
    if segment.starts_with('@') {
        return format!("{indent}{segment}");
    }
    let mut parts = Vec::new();
    let (mut start, mut nesting, mut quote) = (0, 0usize, None);
    for (i, c) in segment.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[') => nesting += 1,
            (None, ')' | ']') => nesting = nesting.saturating_sub(1),
            (None, ',') if nesting == 0 => {
                parts.push(segment[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(segment[start..].trim());
    parts
        .iter()
        .map(|part| format!("{indent}{part}"))
        .collect::<Vec<_>>()
        .join(",\n")
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
    fn test_css_formats_rules() {
        let out = prettify(
            Language::Css,
            "@import url(\"a.css\");a,b>c:hover{color:red;margin:0 auto}@media (min-width:600px){.x{background:url(data:image/png;base64,AA==)}}",
        )
        .unwrap();
        assert_eq!(
            out,
            "@import url(\"a.css\");\na,\nb>c:hover {\n  color: red;\n  margin: 0 auto;\n}\n@media (min-width:600px) {\n  .x {\n    background: url(data:image/png;base64,AA==);\n  }\n}\n"
        );
    }

    #[test]
    fn test_css_keeps_comments_and_strings() {
        let src = "/* a\n  b */\na::after{content:\"x;{}\" /* in */;}";
        assert_eq!(
            prettify(Language::Css, src).unwrap(),
            "/* a\n  b */\na::after {\n  content: \"x;{}\" /* in */;\n}\n"
        );
    }

    #[test]
    fn test_css_is_idempotent() {
        let once = prettify(Language::Css, "a,b{color:red}@media x{c{d:e}}").unwrap();
        assert_eq!(prettify(Language::Css, &once).unwrap(), once);
    }

    #[test]
    fn test_css_unbalanced_is_error() {
        for bad in ["a{", "a}", "a{b:\"x}", "a{/* x}"] {
            assert!(prettify(Language::Css, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_unsupported_language() {
        assert!(!supports(Language::PlainText));
        assert!(prettify(Language::PlainText, "x").is_err());
    }

    fn installed(tool: &str) -> bool {
        Command::new(tool).arg("--version").output().is_ok()
    }

    #[test]
    fn test_rust_uses_rustfmt() {
        if !installed("rustfmt") {
            return;
        }
        assert_eq!(prettify(Language::Rust, "fn main(){let x=1;}").unwrap(), "fn main() {\n    let x = 1;\n}\n");
    }

    #[test]
    fn test_external_syntax_error_reports_tool() {
        if !installed("rustfmt") {
            return;
        }
        let err = prettify(Language::Rust, "fn main( {").unwrap_err();
        assert!(err.starts_with("rustfmt:"), "{err}");
    }
}
