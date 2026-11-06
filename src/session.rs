//! The session: the folder and files open when the editor quit, reopened
//! at the next start, each file at the caret and scroll offset it was left
//! at.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::app::TextEditorApp;

/// eframe storage key of the session.
pub const SESSION_KEY: &str = "session";

/// One tab of the session: the file to reopen, and where the view was in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionTab {
    /// The file, as it was on disk.
    pub path: PathBuf,
    /// The caret, as a `char` index into the file.
    pub caret: usize,
    /// The editor's vertical scroll offset, in points.
    pub scroll: f32,
}

/// What is reopened at start. Only files on disk are kept: an untitled
/// tab has nothing to reopen, and unsaved edits are never stored (quitting
/// asks to save them first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// The Explorer's folder.
    pub folder: Option<PathBuf>,
    /// Files of the open tabs, in tab order, each with its view position.
    pub tabs: Vec<SessionTab>,
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
            tabs.push(SessionTab {
                path: path.clone(),
                caret: tab.caret,
                scroll: tab.scroll,
            });
        }
        Self {
            folder: app.explorer.as_ref().map(|e| e.root().to_path_buf()),
            tabs,
            active,
        }
    }

    pub fn to_storage(&self) -> String {
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let path = tab.path.to_str()?;
                Some(json!({
                    "path": path,
                    "caret": tab.caret,
                    "scroll": scroll_value(tab.scroll),
                }))
            })
            .collect();
        json!({
            "folder": self.folder.as_ref().and_then(|p| p.to_str()),
            "tabs": tabs,
            "active": self.active,
        })
        .to_string()
    }

    /// Reads what [`Self::to_storage`] wrote; anything unreadable gives an
    /// empty session.
    ///
    /// A `tabs` entry that is a bare path comes from an older version that
    /// kept no positions: its tab opens at the top, as it would have then.
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
            .map(|items| items.iter().filter_map(tab_from_value).collect())
            .unwrap_or_default();
        let active = value.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        Self { folder, tabs, active }
    }

    /// Reopens the folder and files that still exist, puts each caret and
    /// view back where it was left, and selects the tab that was active (or
    /// the nearest one left). Leaves Open Recent and the status bar as they
    /// were: restoring is not the user opening things.
    pub fn restore(&self, app: &mut TextEditorApp) {
        let history = app.history.clone();
        let status = app.status_message.clone();
        if let Some(folder) = self.folder.as_ref().filter(|f| f.is_dir()) {
            app.open_folder(folder.clone());
        }
        let active_path = self.tabs.get(self.active).map(|tab| &tab.path);
        for wanted in self.tabs.iter().filter(|tab| tab.path.is_file()) {
            app.open_file_in_tab(wanted.path.clone());
            // A file that shrank since the session was saved cannot put the
            // caret back where it was: it opens at its new end.
            if let Some(tab) = app
                .tabs
                .iter_mut()
                .find(|tab| tab.current_path.as_ref() == Some(&wanted.path))
            {
                tab.caret = wanted.caret.min(tab.text_content.chars().count());
                tab.scroll = wanted.scroll;
                tab.restore_view = true;
            }
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

/// The scroll offset as storage keeps it: never negative, and never a number
/// JSON cannot hold.
fn scroll_value(scroll: f32) -> f32 {
    if scroll.is_finite() { scroll.max(0.0) } else { 0.0 }
}

/// One `tabs` entry: an object as this version writes it, or a bare path from
/// one that kept no positions.
fn tab_from_value(item: &Value) -> Option<SessionTab> {
    if let Some(path) = item.as_str() {
        return (!path.is_empty()).then(|| SessionTab {
            path: PathBuf::from(path),
            ..Default::default()
        });
    }
    let path = item.get("path").and_then(Value::as_str).filter(|p| !p.is_empty())?;
    Some(SessionTab {
        path: PathBuf::from(path),
        caret: item.get("caret").and_then(Value::as_u64).unwrap_or(0) as usize,
        scroll: item
            .get("scroll")
            .and_then(Value::as_f64)
            .map_or(0.0, |scroll| scroll_value(scroll as f32)),
    })
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

    fn placed(path: &str, caret: usize, scroll: f32) -> SessionTab {
        SessionTab { path: PathBuf::from(path), caret, scroll }
    }

    #[test]
    fn test_storage_round_trip() {
        let session = Session {
            folder: Some(PathBuf::from("/project")),
            tabs: vec![placed("/project/a.rs", 12, 340.5), placed("/project/b.md", 0, 0.0)],
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
            Session {
                folder: None,
                tabs: vec![placed("/a", 0, 0.0)],
                active: 0,
            }
        );
    }

    /// A session saved by a version that kept no positions still opens its
    /// files, at the top.
    #[test]
    fn test_the_old_path_only_format_is_still_read() {
        let text = r#"{"folder": "/project", "tabs": ["/project/a.rs"], "active": 0}"#;
        assert_eq!(
            Session::from_storage(text),
            Session {
                folder: Some(PathBuf::from("/project")),
                tabs: vec![placed("/project/a.rs", 0, 0.0)],
                active: 0,
            }
        );
    }

    #[test]
    fn test_a_position_storage_cannot_hold_is_read_as_the_top() {
        let text = r#"{"tabs": [{"path": "/a", "caret": 5, "scroll": null}], "active": 0}"#;
        assert_eq!(Session::from_storage(text).tabs, [placed("/a", 5, 0.0)]);
        let text = r#"{"tabs": [{"path": "/a", "caret": 5, "scroll": -3}]}"#;
        assert_eq!(Session::from_storage(text).tabs, [placed("/a", 5, 0.0)]);
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
        assert_eq!(
            session.tabs,
            [placed("/a.rs", 0, 0.0), placed("/b.rs", 0, 0.0)]
        );
        assert_eq!(session.active, 1);
    }

    /// The positions recorded for the tabs are the ones written.
    #[test]
    fn test_from_app_keeps_the_caret_and_scroll_of_each_tab() {
        let mut app = TextEditorApp::default();
        app.tabs[0].current_path = Some(PathBuf::from("/a.rs"));
        app.tabs[0].caret = 12;
        app.tabs[0].scroll = 340.5;
        app.open_new_tab();
        app.tabs[1].current_path = Some(PathBuf::from("/b.rs"));
        app.tabs[1].caret = 3;
        let session = Session::from_app(&app);
        assert_eq!(session.tabs, [placed("/a.rs", 12, 340.5), placed("/b.rs", 3, 0.0)]);
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
        let session = Session {
            folder: Some(dir.clone()),
            tabs: vec![placed(&a.to_string_lossy(), 2, 40.0), placed(&b.to_string_lossy(), 0, 0.0)],
            active: 1,
        };

        let mut app = TextEditorApp::default();
        session.restore(&mut app);

        assert_eq!(app.explorer.as_ref().map(|e| e.root().to_path_buf()), Some(dir.clone()));
        assert_eq!(tab_paths(&app), [Some(a), Some(b.clone())]);
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&b));
        assert_eq!(app.active_tab().text_content, "# b");
        assert!(!app.show_welcome);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Restoring puts each tab's caret and scroll back, and marks the tab so
    /// the editor applies them the first time it draws.
    #[test]
    fn test_restore_puts_the_caret_and_scroll_back() {
        let dir = temp_dir("positions");
        let (a, b) = (dir.join("a.rs"), dir.join("b.rs"));
        fs::write(&a, "fn a() {}\nfn b() {}\n").unwrap();
        fs::write(&b, "x").unwrap();
        let session = Session {
            folder: Some(dir.clone()),
            tabs: vec![placed(&a.to_string_lossy(), 4, 120.0), placed(&b.to_string_lossy(), 9, 0.0)],
            active: 0,
        };

        let mut app = TextEditorApp::default();
        session.restore(&mut app);

        let first = &app.tabs[0];
        assert_eq!(first.caret, 4, "the caret is where it was left");
        assert_eq!(first.scroll, 120.0, "the view is where it was left");
        assert!(first.restore_view, "the editor has not applied them yet");
        // "x" holds one char: the caret cannot be put past its end.
        assert_eq!(app.tabs[1].caret, 1, "a shorter file clamps the caret");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_restore_skips_missing_paths_and_leaves_history_and_status_alone() {
        let dir = temp_dir("missing");
        let kept = dir.join("kept.rs");
        fs::write(&kept, "").unwrap();
        let session = Session {
            folder: Some(dir.join("gone")),
            tabs: vec![placed(&dir.join("gone.rs").to_string_lossy(), 3, 20.0), placed(&kept.to_string_lossy(), 0, 0.0)],
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
