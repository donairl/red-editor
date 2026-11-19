//! Files the agent reads and writes through the editor (stage 3): the tabs
//! as [`Buffers`], the list of files the agent wrote that are still
//! unsaved, and the Diff / Save actions of the panel's cards.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::acp::fs::Buffers;
use crate::app::{DocumentTab, TextEditorApp};
use crate::diff_view::Against;
use crate::highlighter::Language;

/// The index of the tab holding `path` (canonical), whatever way its own
/// path is spelled.
fn tab_index(tabs: &[DocumentTab], path: &Path) -> Option<usize> {
    let held = |tab: &DocumentTab| tab.current_path.as_deref().is_some_and(|p| p == path);
    tabs.iter().position(held).or_else(|| {
        tabs.iter().position(|tab| {
            tab.current_path.as_deref().and_then(|p| p.canonicalize().ok()).is_some_and(|p| p == path)
        })
    })
}

/// The editor's tabs, for the agent's file requests.
pub struct TabBuffers<'a> {
    pub tabs: &'a mut Vec<DocumentTab>,
}

impl Buffers for TabBuffers<'_> {
    fn text(&self, path: &Path) -> Option<String> {
        tab_index(self.tabs, path).map(|i| self.tabs[i].text_content.clone())
    }

    fn set_text(&mut self, path: &Path, text: &str) -> Result<(), String> {
        let index = match tab_index(self.tabs, path) {
            Some(index) => index,
            None => {
                // Opened from disk first, so the tab is dirty exactly when
                // the agent's text differs from the file. A new file is an
                // empty saved text: dirty, and Save creates it.
                let tab = if path.exists() {
                    DocumentTab::from_file(path.to_owned()).map_err(|e| format!("{}: {e}", path.display()))?
                } else {
                    let mut tab = DocumentTab::default_new();
                    tab.active_language = Language::from_path(path);
                    tab.current_path = Some(path.to_owned());
                    tab
                };
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        let tab = &mut self.tabs[index];
        tab.text_content = text.to_owned();
        tab.caret = tab.caret.min(text.chars().count());
        Ok(())
    }
}

impl TextEditorApp {
    /// Answers the agent's file requests with the editor's tabs. Once per
    /// frame, after polling the client.
    pub(crate) fn serve_agent_fs(&mut self) {
        let Some(client) = &mut self.agent.client else { return };
        if !client.has_fs_requests() {
            return;
        }
        let written = client.serve_fs(&mut TabBuffers { tabs: &mut self.tabs });
        for path in written {
            if !self.agent.edits.contains(&path) {
                self.agent.edits.push(path);
            }
        }
    }

    /// The files the agent wrote that are still unsaved. Saved (or closed)
    /// ones leave the list.
    pub(crate) fn agent_unsaved_paths(&mut self) -> Vec<PathBuf> {
        let tabs = &self.tabs;
        self.agent
            .edits
            .retain(|path| tab_index(tabs, path).is_some_and(|i| tabs[i].is_dirty()));
        self.agent.edits.clone()
    }

    /// Saves the tab of a file the agent wrote.
    pub fn save_agent_edit(&mut self, path: &Path) {
        if let Some(index) = tab_index(&self.tabs, path) {
            self.save_tab(index);
        }
    }

    /// Saves every file the agent wrote that is still unsaved.
    pub fn save_all_agent_edits(&mut self) {
        for path in self.agent_unsaved_paths() {
            self.save_agent_edit(&path);
        }
    }

    /// Shows a file the agent wrote next to HEAD's copy; outside a
    /// repository, next to the file on disk (empty when it is new).
    pub fn diff_agent_edit(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.open_agent_location(path.clone(), None);
        if self.active_tab().current_path.as_ref() != Some(&path) {
            return;
        }
        if self.git.repo().is_some_and(|repo| repo.contains(&path)) {
            self.toggle_diff_view(ctx, Against::Head);
        } else if path.exists() {
            self.diff_with_file(ctx, path);
        } else {
            self.toggle_diff_view(ctx, Against::File { path, text: String::new() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::fs::tests::temp_dir;

    #[test]
    fn test_write_to_an_open_tab_leaves_it_dirty_and_the_disk_alone() {
        let root = temp_dir("tabs_open");
        let file = root.join("f.txt");
        std::fs::write(&file, "old\n").unwrap();
        let mut tabs = vec![DocumentTab::from_file(file.clone()).unwrap()];
        assert!(!tabs[0].is_dirty());
        let mut buffers = TabBuffers { tabs: &mut tabs };
        buffers.set_text(&file, "new\n").unwrap();
        assert_eq!(buffers.text(&file).as_deref(), Some("new\n"));
        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].is_dirty());
        assert_eq!(tabs[0].saved_content, "old\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\n");
    }

    #[test]
    fn test_write_to_a_closed_file_opens_a_dirty_tab() {
        let root = temp_dir("tabs_closed");
        let file = root.join("f.txt");
        std::fs::write(&file, "old\n").unwrap();
        let mut tabs = vec![DocumentTab::default_new()];
        TabBuffers { tabs: &mut tabs }.set_text(&file, "new\n").unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[1].current_path.as_ref(), Some(&file));
        assert!(tabs[1].is_dirty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\n");
        // Written back to the same text: not dirty any more.
        TabBuffers { tabs: &mut tabs }.set_text(&file, "old\n").unwrap();
        assert!(!tabs[1].is_dirty());
    }

    #[test]
    fn test_write_of_a_new_file_makes_a_dirty_tab_and_no_file() {
        let root = temp_dir("tabs_new");
        let file = root.join("new.rs");
        let mut tabs = vec![DocumentTab::default_new()];
        TabBuffers { tabs: &mut tabs }.set_text(&file, "fn main() {}\n").unwrap();
        assert!(tabs[1].is_dirty());
        assert_eq!(tabs[1].active_language, Language::from_path(&file));
        assert!(!file.exists());
    }

    #[test]
    fn test_read_sees_the_unsaved_tab_text() {
        let root = temp_dir("tabs_read");
        let file = root.join("f.txt");
        std::fs::write(&file, "disk\n").unwrap();
        let mut tabs = vec![DocumentTab::from_file(file.clone()).unwrap()];
        tabs[0].text_content = "typed\n".into();
        let buffers = TabBuffers { tabs: &mut tabs };
        assert_eq!(buffers.text(&file).as_deref(), Some("typed\n"));
        assert_eq!(buffers.text(&root.join("other.txt")), None);
    }
}
