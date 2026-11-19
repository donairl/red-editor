//! The agent's file access through the editor: `fs/read_text_file` and
//! `fs/write_text_file`. Pure functions over [`Buffers`], which the app
//! implements for its tabs; they run on the UI thread, never in the reader.
//!
//! Paths must be absolute and, once symlinks are resolved, inside the
//! workspace folder. This bounds only Red's own channel.

use std::path::{Component, Path, PathBuf};

use agent_client_protocol_schema::v1::{
    ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use serde_json::Value;

use super::transport::RpcError;

/// Invalid params (a bad or out-of-workspace path).
const INVALID_PARAMS: i64 = -32602;
/// ACP's "resource not found".
const NOT_FOUND: i64 = -32002;
const INTERNAL: i64 = -32603;
/// Largest file the agent may read from disk.
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

/// The editor's open documents, as far as the agent may see them.
pub trait Buffers {
    /// The text of the tab holding `path` (canonical), unsaved edits included.
    fn text(&self, path: &Path) -> Option<String>;
    /// Puts `text` in the tab holding `path`, opening one when there is
    /// none, and leaves it unsaved. The disk is not touched.
    fn set_text(&mut self, path: &Path, text: &str) -> Result<(), String>;
}

/// A request the agent made that needs the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum FsCall {
    Read(ReadTextFileRequest),
    Write(WriteTextFileRequest),
}

impl FsCall {
    /// `None` for a method that is not an fs method; `Some(Err)` for params
    /// that do not parse.
    pub fn parse(method: &str, params: Value) -> Option<Result<Self, RpcError>> {
        let invalid = |e: serde_json::Error| RpcError { code: INVALID_PARAMS, message: format!("invalid params: {e}") };
        match method {
            "fs/read_text_file" => Some(serde_json::from_value(params).map(Self::Read).map_err(invalid)),
            "fs/write_text_file" => Some(serde_json::from_value(params).map(Self::Write).map_err(invalid)),
            _ => None,
        }
    }
}

/// Answers `call`. A write also returns the path it wrote, for the panel's
/// list of agent edits.
pub fn handle(
    root: &Path,
    buffers: &mut dyn Buffers,
    call: &FsCall,
) -> (Result<Value, RpcError>, Option<PathBuf>) {
    match call {
        FsCall::Read(request) => (read(root, buffers, request), None),
        FsCall::Write(request) => match write(root, buffers, request) {
            Ok(path) => (Ok(to_value(WriteTextFileResponse::new())), Some(path)),
            Err(error) => (Err(error), None),
        },
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("ACP messages serialise")
}

fn error(code: i64, message: impl Into<String>) -> RpcError {
    RpcError { code, message: message.into() }
}

/// The canonical form of `path`, which must be absolute and inside `root`.
/// A file that does not exist yet is allowed when `allow_missing`: its
/// nearest existing ancestor is resolved and the rest appended.
pub fn resolve(root: &Path, path: &Path, allow_missing: bool) -> Result<PathBuf, RpcError> {
    if !path.is_absolute() {
        return Err(error(INVALID_PARAMS, format!("path must be absolute: {}", path.display())));
    }
    let outside = || error(INVALID_PARAMS, format!("path is outside the workspace: {}", path.display()));
    let root = root.canonicalize().map_err(|e| error(INTERNAL, format!("workspace not available: {e}")))?;
    let canonical = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !allow_missing {
                return Err(error(NOT_FOUND, format!("file not found: {}", path.display())));
            }
            // `..` could climb out of a directory that does not exist yet.
            if path.components().any(|c| c == Component::ParentDir) {
                return Err(outside());
            }
            let mut missing = Vec::new();
            let mut ancestor = path;
            let base = loop {
                match ancestor.canonicalize() {
                    Ok(base) => break base,
                    Err(_) => {
                        let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else {
                            return Err(outside());
                        };
                        missing.push(name.to_owned());
                        ancestor = parent;
                    }
                }
            };
            missing.iter().rev().fold(base, |acc, part| acc.join(part))
        }
        Err(e) => return Err(error(INTERNAL, format!("{}: {e}", path.display()))),
    };
    if canonical.starts_with(&root) { Ok(canonical) } else { Err(outside()) }
}

/// Lines `line` (1-based) onward, at most `limit` of them; line endings kept.
pub fn slice_lines(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_owned();
    }
    let skip = line.map_or(0, |l| l.saturating_sub(1) as usize);
    let take = limit.map_or(usize::MAX, |l| l as usize);
    text.split_inclusive('\n').skip(skip).take(take).collect()
}

fn read(root: &Path, buffers: &dyn Buffers, request: &ReadTextFileRequest) -> Result<Value, RpcError> {
    let path = resolve(root, &request.path, false)?;
    let text = match buffers.text(&path) {
        Some(text) => text,
        None => {
            let meta = std::fs::metadata(&path).map_err(|e| error(INTERNAL, format!("{}: {e}", path.display())))?;
            if !meta.is_file() {
                return Err(error(INVALID_PARAMS, format!("not a file: {}", path.display())));
            }
            if meta.len() > MAX_READ_BYTES {
                return Err(error(INTERNAL, format!("file too large: {}", path.display())));
            }
            std::fs::read_to_string(&path).map_err(|e| error(INTERNAL, format!("{}: {e}", path.display())))?
        }
    };
    Ok(to_value(ReadTextFileResponse::new(slice_lines(&text, request.line, request.limit))))
}

fn write(root: &Path, buffers: &mut dyn Buffers, request: &WriteTextFileRequest) -> Result<PathBuf, RpcError> {
    let path = resolve(root, &request.path, true)?;
    if path.is_dir() {
        return Err(error(INVALID_PARAMS, format!("is a directory: {}", path.display())));
    }
    buffers.set_text(&path, &request.content).map_err(|e| error(INTERNAL, e))?;
    Ok(path)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fresh directory under the system temp dir.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("red_acp_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// Open "tabs" in memory.
    #[derive(Default)]
    pub struct MemBuffers {
        pub open: HashMap<PathBuf, String>,
    }

    impl Buffers for MemBuffers {
        fn text(&self, path: &Path) -> Option<String> {
            self.open.get(path).cloned()
        }
        fn set_text(&mut self, path: &Path, text: &str) -> Result<(), String> {
            self.open.insert(path.to_owned(), text.to_owned());
            Ok(())
        }
    }

    fn read_req(path: &Path, line: Option<u32>, limit: Option<u32>) -> FsCall {
        let mut request = ReadTextFileRequest::new("s1", path);
        request.line = line;
        request.limit = limit;
        FsCall::Read(request)
    }

    fn content(result: Result<Value, RpcError>) -> String {
        result.unwrap()["content"].as_str().unwrap().to_owned()
    }

    #[test]
    fn test_slice_lines_by_line_and_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, None, None), text);
        assert_eq!(slice_lines(text, Some(2), None), "b\nc\nd");
        assert_eq!(slice_lines(text, Some(2), Some(2)), "b\nc\n");
        assert_eq!(slice_lines(text, None, Some(1)), "a\n");
        assert_eq!(slice_lines(text, Some(1), Some(0)), "");
        assert_eq!(slice_lines(text, Some(9), None), "");
        assert_eq!(slice_lines(text, Some(0), Some(1)), "a\n", "line 0 is taken as 1");
    }

    #[test]
    fn test_read_from_disk_with_range() {
        let root = temp_dir("read_disk");
        std::fs::write(root.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        let mut buffers = MemBuffers::default();
        let (result, written) = handle(&root, &mut buffers, &read_req(&root.join("f.txt"), Some(2), Some(1)));
        assert_eq!(content(result), "two\n");
        assert_eq!(written, None);
    }

    #[test]
    fn test_read_prefers_the_buffer_over_disk() {
        let root = temp_dir("read_buffer");
        std::fs::write(root.join("f.txt"), "disk\n").unwrap();
        let mut buffers = MemBuffers::default();
        buffers.open.insert(root.join("f.txt"), "unsaved\nedit\n".into());
        let (result, _) = handle(&root, &mut buffers, &read_req(&root.join("f.txt"), Some(2), None));
        assert_eq!(content(result), "edit\n");
    }

    #[test]
    fn test_read_errors() {
        let root = temp_dir("read_errors");
        let other = temp_dir("read_errors_other");
        std::fs::write(other.join("secret.txt"), "x").unwrap();
        std::fs::create_dir(root.join("dir")).unwrap();
        let mut buffers = MemBuffers::default();
        let mut code = |call: FsCall| handle(&root, &mut buffers, &call).0.unwrap_err().code;
        assert_eq!(code(read_req(Path::new("rel.txt"), None, None)), INVALID_PARAMS);
        assert_eq!(code(read_req(&root.join("missing.txt"), None, None)), NOT_FOUND);
        assert_eq!(code(read_req(&other.join("secret.txt"), None, None)), INVALID_PARAMS);
        assert_eq!(code(read_req(&root.join("../secret.txt"), None, None)), NOT_FOUND);
        assert_eq!(
            code(read_req(&root.join("..").join(other.file_name().unwrap()).join("secret.txt"), None, None)),
            INVALID_PARAMS
        );
        assert_eq!(code(read_req(&root.join("dir"), None, None)), INVALID_PARAMS);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_out_of_the_workspace_is_refused() {
        let root = temp_dir("symlink");
        let other = temp_dir("symlink_other");
        std::fs::write(other.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&other, root.join("link")).unwrap();
        let mut buffers = MemBuffers::default();
        let (result, _) = handle(&root, &mut buffers, &read_req(&root.join("link/secret.txt"), None, None));
        assert_eq!(result.unwrap_err().code, INVALID_PARAMS);
        let write = FsCall::Write(WriteTextFileRequest::new("s1", root.join("link/new.txt"), "y"));
        assert_eq!(handle(&root, &mut buffers, &write).0.unwrap_err().code, INVALID_PARAMS);
        assert!(!other.join("new.txt").exists());
    }

    #[test]
    fn test_write_goes_to_the_buffer_not_the_disk() {
        let root = temp_dir("write_buffer");
        std::fs::write(root.join("f.txt"), "old\n").unwrap();
        let mut buffers = MemBuffers::default();
        let call = FsCall::Write(WriteTextFileRequest::new("s1", root.join("f.txt"), "new\n"));
        let (result, written) = handle(&root, &mut buffers, &call);
        assert_eq!(result.unwrap(), serde_json::json!({}));
        assert_eq!(written, Some(root.join("f.txt")));
        assert_eq!(buffers.open[&root.join("f.txt")], "new\n");
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "old\n");
    }

    #[test]
    fn test_write_may_name_a_new_file_but_not_leave_the_workspace() {
        let root = temp_dir("write_new");
        let mut buffers = MemBuffers::default();
        let new = FsCall::Write(WriteTextFileRequest::new("s1", root.join("sub/new.txt"), "hi"));
        let (result, written) = handle(&root, &mut buffers, &new);
        assert!(result.is_ok());
        assert_eq!(written, Some(root.join("sub/new.txt")));
        assert!(!root.join("sub").exists(), "nothing is created on disk");
        for bad in [PathBuf::from("rel.txt"), root.join("../x.txt"), root.join("sub/../../x.txt"), root.clone()] {
            let call = FsCall::Write(WriteTextFileRequest::new("s1", bad.clone(), "x"));
            let code = handle(&root, &mut buffers, &call).0.unwrap_err().code;
            assert_eq!(code, INVALID_PARAMS, "{bad:?}");
        }
        assert_eq!(buffers.open.len(), 1);
    }

    #[test]
    fn test_unparsable_params_are_invalid_params() {
        let error = FsCall::parse("fs/read_text_file", serde_json::json!({"path": 3})).unwrap().unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(FsCall::parse("terminal/create", Value::Null).is_none());
    }
}
