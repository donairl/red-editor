//! Files changed by other programs: polling the disk, reloading tabs
//! without unsaved edits, and the conflict prompt's choice for those with.

use eframe::egui;
use std::time::Duration;

use crate::app::{DISK_CHECK_SECS, DocumentTab, TextEditorApp, disk_stamp, read_document};
use crate::modals::DiskChoice;

impl DocumentTab {
    /// Whether another program changed the file since this tab last read or
    /// wrote it. A file that cannot be read (deleted, moved) is no change.
    pub fn changed_on_disk(&self) -> bool {
        self.current_path
            .as_deref()
            .and_then(disk_stamp)
            .is_some_and(|stamp| Some(stamp) != self.disk_stamp)
    }

    /// Replaces the text, unsaved edits included, with the file's contents.
    pub fn reload(&mut self) -> Result<(), std::io::Error> {
        let path = self.current_path.as_deref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No file path specified")
        })?;
        let (content, disk_stamp) = read_document(path)?;
        self.text_content = content.clone();
        self.saved_content = content;
        self.disk_stamp = disk_stamp;
        self.disk_conflict = false;
        Ok(())
    }
}

impl TextEditorApp {
    /// Compares every open file with the disk: a tab without unsaved edits
    /// reloads the new contents, one with edits is flagged so the user can
    /// choose.
    pub fn check_files_on_disk(&mut self) {
        let mut reloaded = None;
        for tab in &mut self.tabs {
            if tab.disk_conflict || !tab.changed_on_disk() {
                continue;
            }
            if tab.is_dirty() {
                tab.disk_conflict = true;
            } else if tab.reload().is_ok() {
                reloaded = Some(tab.file_display_name(""));
            }
        }
        if let Some(name) = reloaded {
            self.status_message = Some((self.i18n.tf("msg.reloaded", &[("name", &name)]), false));
            self.git.request_refresh();
        }
    }

    /// Runs [`Self::check_files_on_disk`] when the window gets the focus
    /// back and every [`DISK_CHECK_SECS`], waking egui for it.
    pub fn poll_disk(&mut self, ctx: &egui::Context) {
        let (now, refocused) = ctx.input(|i| {
            let refocused = i
                .events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(true)));
            (i.time, refocused)
        });
        if refocused || now - self.last_disk_check >= DISK_CHECK_SECS {
            self.last_disk_check = now;
            self.check_files_on_disk();
            // Back from another program: files may have changed under git
            // without touching its index.
            if refocused {
                self.git.refresh(ctx);
            } else {
                self.git.refresh_if_changed(ctx);
            }
        }
        ctx.request_repaint_after(Duration::from_secs_f64(DISK_CHECK_SECS));
    }

    /// Applies the conflict dialog's choice to the active tab.
    pub fn resolve_disk_conflict(&mut self, choice: DiskChoice) {
        let tab = self.active_tab_mut();
        tab.disk_conflict = false;
        let result = match choice {
            DiskChoice::Reload => tab.reload(),
            DiskChoice::Keep => {
                tab.disk_stamp = tab.current_path.as_deref().and_then(disk_stamp);
                Ok(())
            }
        };
        if let Err(err) = result {
            self.status_message = Some((self.open_failed_message(&err), true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MAX_OPEN_BYTES;
    use std::fs::{self, File};
    use std::path::PathBuf;

    /// An open file that grows past the limit is not reloaded.
    #[test]
    fn test_reload_refuses_a_file_grown_past_the_size_limit() {
        let path = std::env::temp_dir().join("red_test_grows.log");
        fs::write(&path, "small").unwrap();
        let mut tab = DocumentTab::from_file(path.clone()).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_OPEN_BYTES + 1)
            .unwrap();
        let error = tab.reload().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
        assert_eq!(tab.text_content, "small");
        let _ = fs::remove_file(&path);
    }

    /// A fresh file `rred_disk_<name>.txt` in the temp directory.
    fn disk_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rred_disk_{name}.txt"));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_tab_notices_its_file_changing_on_disk() {
        let path = disk_file("notice", "one");
        let mut tab = DocumentTab::from_file(path.clone()).unwrap();
        assert!(!tab.changed_on_disk());
        fs::write(&path, "one two").unwrap();
        assert!(tab.changed_on_disk());
        tab.reload().unwrap();
        assert_eq!(tab.text_content, "one two");
        assert!(!tab.is_dirty() && !tab.changed_on_disk());
        fs::remove_file(&path).unwrap();
        assert!(!tab.changed_on_disk(), "a deleted file is nothing to reload");
    }

    #[test]
    fn test_disk_check_reloads_clean_tabs_and_flags_dirty_ones() {
        let clean = disk_file("check_clean", "clean");
        let dirty = disk_file("check_dirty", "dirty");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(clean.clone());
        app.open_file_in_tab(dirty.clone());
        app.active_tab_mut().text_content.push_str(" edited");
        fs::write(&clean, "clean, changed outside").unwrap();
        fs::write(&dirty, "dirty, changed outside").unwrap();

        app.check_files_on_disk();
        assert_eq!(app.tabs[0].text_content, "clean, changed outside");
        assert!(!app.tabs[0].is_dirty() && !app.tabs[0].disk_conflict);
        assert_eq!(app.tabs[1].text_content, "dirty edited", "unsaved edits stay");
        assert!(app.tabs[1].disk_conflict, "the user is asked instead");
        let _ = fs::remove_file(&clean);
        let _ = fs::remove_file(&dirty);
    }

    /// Saving over a file changed on disk waits for the user's choice;
    /// keeping their version lets the save through, reloading drops it.
    #[test]
    fn test_disk_conflict_choices() {
        let path = disk_file("choice", "disk v1");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(path.clone());
        app.active_tab_mut().text_content = "mine".into();
        fs::write(&path, "disk v2, longer").unwrap();

        assert!(!app.save(), "no silent overwrite");
        assert_eq!(fs::read_to_string(&path).unwrap(), "disk v2, longer");
        assert!(app.active_tab().disk_conflict);

        app.resolve_disk_conflict(DiskChoice::Keep);
        assert!(!app.active_tab().disk_conflict);
        app.check_files_on_disk();
        assert!(!app.active_tab().disk_conflict, "no second prompt for a kept change");
        assert!(app.save());
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");

        fs::write(&path, "disk v3, even longer").unwrap();
        app.active_tab_mut().text_content = "mine again".into();
        app.check_files_on_disk();
        assert!(app.active_tab().disk_conflict);
        app.resolve_disk_conflict(DiskChoice::Reload);
        assert_eq!(app.active_tab().text_content, "disk v3, even longer");
        assert!(!app.active_tab().is_dirty() && !app.active_tab().disk_conflict);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_disk_is_checked_on_focus_and_every_few_seconds() {
        let path = disk_file("poll", "v1");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(path.clone());
        let ctx = egui::Context::default();
        let frame = |app: &mut TextEditorApp, time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.poll_disk(ui.ctx()));
            output.textures_delta.clear();
        };
        let text = |app: &TextEditorApp| app.active_tab().text_content.clone();

        frame(&mut app, 0.0, Vec::new());
        fs::write(&path, "v2 changed").unwrap();
        frame(&mut app, 1.0, Vec::new());
        assert_eq!(text(&app), "v1", "not due yet");
        frame(&mut app, 1.5, vec![egui::Event::WindowFocused(true)]);
        assert_eq!(text(&app), "v2 changed", "the focus coming back checks at once");
        fs::write(&path, "v3 changed again").unwrap();
        frame(&mut app, 3.0, Vec::new());
        assert_eq!(text(&app), "v2 changed");
        frame(&mut app, 3.6, Vec::new());
        assert_eq!(text(&app), "v3 changed again", "checked again after the interval");
        let _ = fs::remove_file(&path);
    }
}
