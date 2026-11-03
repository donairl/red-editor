//! The session: the folder and files open when the editor quit, reopened
//! at the next start.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::app::TextEditorApp;

/// eframe storage key of the session.
pub const SESSION_KEY: &str = "session";

/// What is reopened at start. Only files on disk are kept: an untitled
/// tab has nothing to reopen, and unsaved edits are never stored (quitting
/// asks to save them first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The Explorer's folder.
    pub folder: Option<PathBuf>,
    /// Files of the open tabs, in tab order.
    pub tabs: Vec<PathBuf>,
    /// Index into `tabs` of the active tab.
    pub active: usize,
}

impl Session {
    /// The session of `app` as it is now.
    pub fn from_app(app: &TextEditorApp) -> Self {
        let mut tabs = Vec::new();
        let mut active = 0;
        for (index, tab) in app.tabs.iter().enumerate() {
            let Some(path) = &tab.current_path else { continue };
            if index == app.active_tab_index {
                active = tabs.len();
            }
            tabs.push(path.clone());
        }
        Self {
            folder: app.explorer.as_ref().map(|e| e.root().to_path_buf()),
            tabs,
            active,
        }
    }

    pub fn to_storage(&self) -> String {
        let tabs: Vec<&str> = self.tabs.iter().filter_map(|p| p.to_str()).collect();
        json!({
            "folder": self.folder.as_ref().and_then(|p| p.to_str()),
            "tabs": tabs,
            "active": self.active,
        })
        .to_string()
    }

    /// Reads what [`Self::to_storage`] wrote; anything unreadable gives an
    /// empty session.
    pub fn from_storage(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let folder = value
            .get("folder")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let tabs = value
            .get("tabs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let active = value.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        Self { folder, tabs, active }
    }

    /// Reopens the folder and files that still exist, and selects the tab
    /// that was active (or the nearest one left). Leaves Open Recent and the
    /// status bar as they were: restoring is not the user opening things.
    pub fn restore(&self, app: &mut TextEditorApp) {
        let history = app.history.clone();
        let status = app.status_message.clone();
        if let Some(folder) = self.folder.as_ref().filter(|f| f.is_dir()) {
            app.open_folder(folder.clone());
        }
        let active_path = self.tabs.get(self.active);
        for path in self.tabs.iter().filter(|p| p.is_file()) {
            app.open_file_in_tab(path.clone());
        }
        if let Some(index) = active_path.and_then(|path| {
            app.tabs
                .iter()
                .position(|tab| tab.current_path.as_ref() == Some(path))
        }) {
            app.active_tab_index = index;
        }
        app.history = history;
        app.status_message = status;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_test_session_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tab_paths(app: &TextEditorApp) -> Vec<Option<PathBuf>> {
        app.tabs.iter().map(|t| t.current_path.clone()).collect()
    }

    #[test]
    fn test_storage_round_trip() {
        let session = Session {
            folder: Some(PathBuf::from("/project")),
            tabs: vec![PathBuf::from("/project/a.rs"), PathBuf::from("/project/b.md")],
            active: 1,
        };
        assert_eq!(Session::from_storage(&session.to_storage()), session);
        let empty = Session::default();
        assert_eq!(Session::from_storage(&empty.to_storage()), empty);
    }

    #[test]
    fn test_unreadable_storage_is_an_empty_session() {
        assert_eq!(Session::from_storage(""), Session::default());
        assert_eq!(Session::from_storage("not json"), Session::default());
        assert_eq!(Session::from_storage("[1, 2]"), Session::default());
        assert_eq!(
            Session::from_storage(r#"{"folder": 3, "tabs": [1, "", "/a"], "active": "x"}"#),
            Session { folder: None, tabs: vec![PathBuf::from("/a")], active: 0 }
        );
    }

    #[test]
    fn test_from_app_keeps_saved_tabs_and_maps_the_active_one() {
        let mut app = TextEditorApp::default();
        app.tabs[0].current_path = Some(PathBuf::from("/a.rs"));
        app.open_new_tab(); // untitled, left out
        app.open_new_tab();
        app.tabs[2].current_path = Some(PathBuf::from("/b.rs"));
        app.active_tab_index = 2;
        let session = Session::from_app(&app);
        assert_eq!(session.folder, None);
        assert_eq!(session.tabs, [PathBuf::from("/a.rs"), PathBuf::from("/b.rs")]);
        assert_eq!(session.active, 1);
    }

    #[test]
    fn test_from_app_with_only_untitled_tabs_is_empty() {
        let app = TextEditorApp::default();
        assert_eq!(Session::from_app(&app), Session::default());
    }

    #[test]
    fn test_restore_reopens_folder_and_files_and_selects_the_active_tab() {
        let dir = temp_dir("restore");
        let (a, b) = (dir.join("a.rs"), dir.join("b.md"));
        fs::write(&a, "fn a() {}").unwrap();
        fs::write(&b, "# b").unwrap();
        let session = Session { folder: Some(dir.clone()), tabs: vec![a.clone(), b.clone()], active: 1 };

        let mut app = TextEditorApp::default();
        session.restore(&mut app);

        assert_eq!(app.explorer.as_ref().map(|e| e.root().to_path_buf()), Some(dir.clone()));
        assert_eq!(tab_paths(&app), [Some(a), Some(b.clone())]);
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&b));
        assert_eq!(app.active_tab().text_content, "# b");
        assert!(!app.show_welcome);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_restore_skips_missing_paths_and_leaves_history_and_status_alone() {
        let dir = temp_dir("missing");
        let kept = dir.join("kept.rs");
        fs::write(&kept, "").unwrap();
        let session = Session {
            folder: Some(dir.join("gone")),
            tabs: vec![dir.join("gone.rs"), kept.clone()],
            active: 0, // the missing file: falls back to what opened
        };

        let mut app = TextEditorApp::default();
        let history = app.history.clone();
        session.restore(&mut app);

        assert!(app.explorer.is_none());
        assert_eq!(tab_paths(&app), [Some(kept)]);
        assert_eq!(app.active_tab_index, 0);
        assert_eq!(app.history, history);
        assert_eq!(app.status_message, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_restoring_nothing_keeps_the_welcome_page() {
        let mut app = TextEditorApp::default();
        Session::default().restore(&mut app);
        assert!(app.show_welcome);
        assert_eq!(tab_paths(&app), [None]);
    }
}
