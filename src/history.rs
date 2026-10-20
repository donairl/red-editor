//! Recently opened files and folders, newest first.
//!
//! File > Open Recent lists them so one click reopens any of them. The list
//! is kept across runs in eframe's storage, as a JSON array.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Most entries kept; opening one more drops the oldest.
pub const MAX_ENTRIES: usize = 20;

/// What an entry reopens as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

/// One opened file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

impl Entry {
    /// Whether the path is still there as the kind it was opened as.
    #[must_use]
    pub fn exists(&self) -> bool {
        match self.kind {
            EntryKind::File => self.path.is_file(),
            EntryKind::Folder => self.path.is_dir(),
        }
    }

    /// The name shown in File > Open Recent, a folder's ending in the path
    /// separator, and the folder it is in.
    #[must_use]
    pub fn label(&self) -> (String, String) {
        let mut name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        if self.kind == EntryKind::Folder && !name.ends_with(std::path::MAIN_SEPARATOR) {
            name.push(std::path::MAIN_SEPARATOR);
        }
        let folder = self
            .path
            .parent()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        (name, folder)
    }
}

/// The recently opened files and folders, newest first, each path once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    /// Puts `path` first, dropping its older entry and, past
    /// [`MAX_ENTRIES`], the oldest one.
    pub fn record(&mut self, path: PathBuf, kind: EntryKind) {
        self.remove(&path);
        self.entries.insert(0, Entry { path, kind });
        self.entries.truncate(MAX_ENTRIES);
    }

    /// Forgets `path`; true when it was listed.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.path != path);
        self.entries.len() < before
    }

    /// Forgets every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, newest first.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entries as a JSON array of `{"path", "folder"}` objects, for
    /// eframe's storage. Paths that are not valid UTF-8 are left out.
    #[must_use]
    pub fn to_storage(&self) -> String {
        let items = self
            .entries
            .iter()
            .filter_map(|entry| {
                let path = entry.path.to_str()?;
                let folder = entry.kind == EntryKind::Folder;
                Some(serde_json::json!({ "path": path, "folder": folder }))
            })
            .collect();
        Value::Array(items).to_string()
    }

    /// The history [`Self::to_storage`] wrote. Unreadable text gives an
    /// empty history and unreadable entries are skipped; repeated paths and
    /// entries past [`MAX_ENTRIES`] are dropped, as [`Self::record`] would.
    #[must_use]
    pub fn from_storage(text: &str) -> Self {
        let mut history = Self::default();
        let Ok(Value::Array(items)) = serde_json::from_str(text) else {
            return history;
        };
        // Oldest first, so each `record` puts a newer entry in front.
        for item in items.iter().rev() {
            let Some(path) = item.get("path").and_then(Value::as_str) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            let kind = match item.get("folder").and_then(Value::as_bool) {
                Some(true) => EntryKind::Folder,
                _ => EntryKind::File,
            };
            history.record(PathBuf::from(path), kind);
        }
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn recorded(paths: &[&str]) -> History {
        let mut history = History::default();
        for path in paths {
            history.record(PathBuf::from(path), EntryKind::File);
        }
        history
    }

    fn paths(history: &History) -> Vec<&str> {
        history
            .entries()
            .iter()
            .map(|entry| entry.path.to_str().unwrap())
            .collect()
    }

    #[test]
    fn test_record_puts_the_newest_first() {
        let history = recorded(&["/a", "/b", "/c"]);
        assert_eq!(paths(&history), ["/c", "/b", "/a"]);
        assert!(!history.is_empty());
        assert!(History::default().is_empty());
    }

    #[test]
    fn test_reopening_a_path_moves_it_to_the_front_once() {
        let mut history = recorded(&["/a", "/b", "/c"]);
        history.record(PathBuf::from("/a"), EntryKind::File);
        assert_eq!(paths(&history), ["/a", "/c", "/b"]);
    }

    #[test]
    fn test_reopening_a_path_as_another_kind_keeps_the_latest_kind() {
        let mut history = recorded(&["/a"]);
        history.record(PathBuf::from("/a"), EntryKind::Folder);
        assert_eq!(
            history.entries(),
            [Entry {
                path: PathBuf::from("/a"),
                kind: EntryKind::Folder,
            }]
        );
    }

    #[test]
    fn test_history_is_capped_dropping_the_oldest() {
        let names: Vec<String> = (0..MAX_ENTRIES + 5).map(|i| format!("/f{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let history = recorded(&names);
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        let newest = format!("/f{}", MAX_ENTRIES + 4);
        assert_eq!(paths(&history)[0], newest);
        assert_eq!(
            paths(&history)[MAX_ENTRIES - 1],
            "/f5",
            "the oldest five went"
        );
    }

    #[test]
    fn test_remove_and_clear() {
        let mut history = recorded(&["/a", "/b"]);
        assert!(history.remove(Path::new("/a")));
        assert!(!history.remove(Path::new("/a")), "already gone");
        assert_eq!(paths(&history), ["/b"]);
        history.clear();
        assert!(history.is_empty());
    }

    #[test]
    fn test_storage_round_trip_keeps_order_and_kind() {
        let mut history = recorded(&["/a.rs", "/b.rs"]);
        history.record(PathBuf::from("/project"), EntryKind::Folder);
        let restored = History::from_storage(&history.to_storage());
        assert_eq!(restored, history);
        assert_eq!(restored.entries()[0].kind, EntryKind::Folder);
        assert_eq!(
            History::from_storage(&History::default().to_storage()),
            History::default()
        );
    }

    #[test]
    fn test_bad_storage_gives_an_empty_history() {
        for text in ["", "not json", "{}", "42", r#"{"path": "/a"}"#] {
            assert_eq!(History::from_storage(text), History::default(), "{text:?}");
        }
    }

    #[test]
    fn test_from_storage_skips_bad_entries_and_restores_the_rules() {
        let text = r#"[
            {"path": "/new", "folder": false},
            {"folder": true},
            {"path": 7},
            {"path": ""},
            "/loose",
            {"path": "/dir", "folder": true},
            {"path": "/new", "folder": true}
        ]"#;
        let history = History::from_storage(text);
        assert_eq!(paths(&history), ["/new", "/dir"]);
        assert_eq!(
            history.entries()[0].kind,
            EntryKind::File,
            "the newest copy wins"
        );
        assert_eq!(history.entries()[1].kind, EntryKind::Folder);

        let long: Vec<String> = (0..MAX_ENTRIES + 3)
            .map(|i| format!(r#"{{"path": "/f{i}"}}"#))
            .collect();
        let history = History::from_storage(&format!("[{}]", long.join(",")));
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        assert_eq!(paths(&history)[0], "/f0", "the first stored is the newest");
    }

    #[test]
    fn test_exists_checks_the_kind_too() {
        let dir = std::env::temp_dir().join("rred_history_exists");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        fs::write(&file, "x").unwrap();
        let entry = |path: &Path, kind| Entry {
            path: path.to_path_buf(),
            kind,
        };

        assert!(entry(&file, EntryKind::File).exists());
        assert!(entry(&dir, EntryKind::Folder).exists());
        assert!(
            !entry(&file, EntryKind::Folder).exists(),
            "a file is not a folder"
        );
        assert!(
            !entry(&dir, EntryKind::File).exists(),
            "a folder is not a file"
        );
        fs::remove_dir_all(&dir).unwrap();
        assert!(!entry(&file, EntryKind::File).exists());
        assert!(!entry(&dir, EntryKind::Folder).exists());
    }

    #[test]
    fn test_label_is_the_name_and_its_folder() {
        let sep = std::path::MAIN_SEPARATOR;
        let file = Entry {
            path: Path::new("/home/me/src").join("main.rs"),
            kind: EntryKind::File,
        };
        assert_eq!(
            file.label(),
            (
                "main.rs".to_owned(),
                Path::new("/home/me/src").display().to_string()
            )
        );
        let folder = Entry {
            path: Path::new("/home/me").join("project"),
            kind: EntryKind::Folder,
        };
        assert_eq!(
            folder.label(),
            (
                format!("project{sep}"),
                Path::new("/home/me").display().to_string()
            )
        );
        let root = Entry {
            path: PathBuf::from(sep.to_string()),
            kind: EntryKind::Folder,
        };
        assert_eq!(root.label(), (sep.to_string(), String::new()));
    }
}
