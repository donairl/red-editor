//! Context the user attaches to a prompt (stage 5): files and selections
//! shown as chips, and the content blocks they become on send.

use std::path::{Path, PathBuf};

use agent_client_protocol_schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextContent,
    TextResourceContents,
};

/// A selection larger than this is refused rather than cut short.
pub const MAX_SELECTION_BYTES: usize = 256 * 1024;

/// Lines (1-based, inclusive) and text of a selection, as it was when the
/// chip was made.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

/// One attachment: a whole file, or a selection in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub path: PathBuf,
    pub selection: Option<Selection>,
}

impl Chip {
    pub fn file(path: PathBuf) -> Self {
        Self { path, selection: None }
    }

    pub fn selection(path: PathBuf, start_line: u32, end_line: u32, text: String) -> Self {
        Self { path, selection: Some(Selection { start_line, end_line, text }) }
    }

    /// `app.rs` or `app.rs:12-30`.
    pub fn label(&self) -> String {
        let name = self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        match &self.selection {
            Some(s) if s.start_line == s.end_line => format!("{name}:{}", s.start_line),
            Some(s) => format!("{name}:{}-{}", s.start_line, s.end_line),
            None => name,
        }
    }

    /// The line a click on the chip goes to.
    pub fn line(&self) -> Option<u32> {
        self.selection.as_ref().map(|s| s.start_line)
    }

    fn uri(&self) -> String {
        let uri = file_uri(&self.path);
        match &self.selection {
            Some(s) => format!("{uri}#L{}-{}", s.start_line, s.end_line),
            None => uri,
        }
    }
}

/// A `file://` URI; bytes outside the unreserved set (and `/`) are
/// percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The 1-based lines a selection of `char` indices `a..b` covers. A
/// selection that ends right after a newline does not take the next line.
pub fn selection_lines(text: &str, a: usize, b: usize) -> (u32, u32) {
    let start = crate::find::line_col(text, a).0;
    let ends_on_newline = b > a && text.chars().nth(b - 1) == Some('\n');
    let end = crate::find::line_col(text, if ends_on_newline { b - 1 } else { b }).0;
    (start as u32, end as u32)
}

/// The prompt's content blocks: file chips as links, selections embedded
/// (or inlined as text when the agent takes no embedded context), then
/// the typed text last.
pub fn build_blocks(chips: &[Chip], text: &str, embedded: bool) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    for chip in chips {
        let name = chip.label();
        match &chip.selection {
            None => blocks.push(ContentBlock::ResourceLink(ResourceLink::new(name, chip.uri()))),
            Some(s) if embedded => {
                blocks.push(ContentBlock::Resource(EmbeddedResource::new(
                    EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                        s.text.clone(),
                        chip.uri(),
                    )),
                )));
            }
            Some(s) => {
                let fence = if s.text.contains("```") { "~~~~" } else { "```" };
                let body = format!("{}:{}-{}\n{fence}\n{}\n{fence}", chip.path.display(), s.start_line, s.end_line, s.text.trim_end_matches('\n'));
                blocks.push(ContentBlock::Text(TextContent::new(body)));
            }
        }
    }
    if !text.is_empty() || blocks.is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(text)));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sel() -> Chip {
        Chip::selection(PathBuf::from("/w/src/app.rs"), 12, 14, "a\nb\nc".into())
    }

    fn as_json(blocks: &[ContentBlock]) -> serde_json::Value {
        serde_json::to_value(blocks).unwrap()
    }

    #[test]
    fn test_labels() {
        assert_eq!(Chip::file("/w/a b.rs".into()).label(), "a b.rs");
        assert_eq!(sel().label(), "app.rs:12-14");
        assert_eq!(Chip::selection("/w/x".into(), 3, 3, String::new()).label(), "x:3");
    }

    #[test]
    fn test_file_uri_encodes() {
        assert_eq!(file_uri(Path::new("/w/a b#.rs")), "file:///w/a%20b%23.rs");
        assert_eq!(file_uri(Path::new("C:\\w\\a.rs")), "file:///C:/w/a.rs");
    }

    #[test]
    fn test_selection_lines() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(selection_lines(text, 0, 3), (1, 1));
        assert_eq!(selection_lines(text, 4, 13), (2, 3));
        // "one\n" ends on a newline: just line 1.
        assert_eq!(selection_lines(text, 0, 4), (1, 1));
        assert_eq!(selection_lines(text, 5, 5), (2, 2));
    }

    #[test]
    fn test_embedded_blocks() {
        let blocks = build_blocks(&[Chip::file("/w/a.rs".into()), sel()], "why?", true);
        let v = as_json(&blocks);
        assert_eq!(v[0]["type"], "resource_link");
        assert_eq!(v[0]["uri"], "file:///w/a.rs");
        assert_eq!(v[1]["type"], "resource");
        assert_eq!(v[1]["resource"]["uri"], "file:///w/src/app.rs#L12-14");
        assert_eq!(v[1]["resource"]["text"], "a\nb\nc");
        assert_eq!(v[2], json!({"type": "text", "text": "why?"}));
        assert_eq!(v.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_selection_falls_back_to_text_without_embedded_context() {
        let blocks = build_blocks(&[Chip::file("/w/a.rs".into()), sel()], "why?", false);
        let v = as_json(&blocks);
        assert_eq!(v[0]["type"], "resource_link", "files stay links");
        assert_eq!(v[1], json!({"type": "text", "text": "/w/src/app.rs:12-14\n```\na\nb\nc\n```"}));
        assert_eq!(v[2]["text"], "why?");
    }

    #[test]
    fn test_no_chips_is_one_text_block() {
        assert_eq!(as_json(&build_blocks(&[], "hi", true)), json!([{"type": "text", "text": "hi"}]));
    }

    #[test]
    fn test_chips_alone_send_no_empty_text() {
        assert_eq!(build_blocks(&[sel()], "", true).len(), 1);
    }
}
