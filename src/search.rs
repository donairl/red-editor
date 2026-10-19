//! Find in Files: searching the open folder on a background thread, and the
//! results panel at the bottom of the window.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui::{self, Key};

use crate::find;
use crate::i18n::I18n;

/// Results kept before the search stops.
pub const MAX_HITS: usize = 2000;
/// Files larger than this are not searched.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Longest result text shown, in `char`s.
const MAX_TEXT_CHARS: usize = 200;
/// Folders that are never searched, besides hidden ones.
const SKIPPED_DIRS: [&str; 2] = ["target", "node_modules"];

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    pub text: String,
}

/// Calls `on_hit` for the first match on each matching line of every text
/// file under `root`, in name order. Stops when `on_hit` returns `false` or
/// `cancel` is set. Hidden entries, symlinks, huge files and files that are
/// not UTF-8 are skipped.
pub fn search(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(Hit) -> bool,
) {
    if query.is_empty() {
        return;
    }
    walk_files(root, cancel, |entry| {
        if entry.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            return true;
        }
        let path = entry.path();
        let Ok(text) = fs::read_to_string(&path) else { return true };
        for (index, line) in text.lines().enumerate() {
            if find::find_all(line, query, case_sensitive).is_empty() {
                continue;
            }
            let hit = Hit {
                path: path.clone(),
                line: index + 1,
                text: line.trim().chars().take(MAX_TEXT_CHARS).collect(),
            };
            if !on_hit(hit) {
                return false;
            }
        }
        true
    });
}

/// Calls `on_file` for every file under `root`, in name order, until it
/// returns `false` or `cancel` is set. Hidden entries, symlinks and the
/// [`SKIPPED_DIRS`] are skipped.
pub fn walk_files(
    root: &Path,
    cancel: &AtomicBool,
    mut on_file: impl FnMut(&fs::DirEntry) -> bool,
) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        // The stack pops last-in first, so push folders in reverse to visit
        // them in name order.
        let mut subdirs = Vec::new();
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if name.starts_with('.') || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_str()) {
                    subdirs.push(entry.path());
                }
                continue;
            }
            if !on_file(&entry) {
                return;
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
}

enum Message {
    Hit(Hit),
    Done { truncated: bool },
}

/// State of the results panel and of the search feeding it.
#[derive(Default)]
pub struct SearchState {
    pub visible: bool,
    pub query: String,
    pub case_sensitive: bool,
    /// Put the caret in the query field on the next frame.
    pub focus_query: bool,
    pub hits: Vec<Hit>,
    pub truncated: bool,
    /// A search was started since the panel opened.
    pub searched: bool,
    root: Option<PathBuf>,
    receiver: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl SearchState {
    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    /// Folder the shown results came from.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Stops any running search and forgets its results.
    pub fn clear(&mut self) {
        self.stop();
        self.hits.clear();
        self.truncated = false;
        self.searched = false;
    }

    fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
    }

    /// Starts searching `root` for the current query, replacing old results.
    pub fn start(&mut self, ctx: &egui::Context, root: PathBuf) {
        self.clear();
        if self.query.is_empty() {
            return;
        }
        self.searched = true;
        self.root = Some(root.clone());
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.receiver = Some(receiver);
        self.cancel = Some(cancel.clone());
        let (query, case_sensitive, ctx) = (self.query.clone(), self.case_sensitive, ctx.clone());
        std::thread::spawn(move || {
            let mut count = 0;
            let mut truncated = false;
            search(&root, &query, case_sensitive, &cancel, |hit| {
                count += 1;
                // A closed channel means the search was replaced.
                let open = sender.send(Message::Hit(hit)).is_ok();
                if count % 50 == 0 {
                    ctx.request_repaint();
                }
                truncated = count >= MAX_HITS;
                open && !truncated
            });
            let _ = sender.send(Message::Done { truncated });
            ctx.request_repaint();
        });
    }

    /// Moves what the search thread found so far into `hits`.
    pub fn poll(&mut self) {
        let Some(receiver) = &self.receiver else { return };
        let mut finished = false;
        while let Ok(message) = receiver.try_recv() {
            match message {
                Message::Hit(hit) => self.hits.push(hit),
                Message::Done { truncated } => {
                    self.truncated = truncated;
                    finished = true;
                }
            }
        }
        if finished {
            self.receiver = None;
            self.cancel = None;
        }
    }

    /// Number of distinct files among the results.
    pub fn file_count(&self) -> usize {
        let mut files = 0;
        let mut last: Option<&Path> = None;
        for hit in &self.hits {
            if last != Some(hit.path.as_path()) {
                files += 1;
                last = Some(&hit.path);
            }
        }
        files
    }
}

/// What the user asked of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchAction {
    Run,
    Open { path: PathBuf, line: usize },
    Close,
}

/// Draws the panel contents; `root` is the folder searches run in, if one
/// is open.
pub fn show_panel(
    ui: &mut egui::Ui,
    state: &mut SearchState,
    root: Option<&Path>,
    i18n: &I18n,
) -> Option<SearchAction> {
    let mut action = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let query = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .id(egui::Id::unique("search_query"))
                .hint_text(i18n.t("search.query"))
                .desired_width(260.0),
        );
        if std::mem::take(&mut state.focus_query) {
            query.request_focus();
        }
        if query.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            action = Some(SearchAction::Run);
            query.request_focus();
        }
        ui.toggle_value(&mut state.case_sensitive, "Aa")
            .on_hover_text(i18n.t("find.case"));
        if ui.button(i18n.t("search.run")).clicked() {
            action = Some(SearchAction::Run);
        }
        let status = if root.is_none() {
            i18n.t("search.no_folder").to_owned()
        } else if state.is_running() {
            i18n.t("search.searching").to_owned()
        } else if !state.searched {
            String::new()
        } else if state.hits.is_empty() {
            i18n.t("find.none").to_owned()
        } else if state.truncated {
            i18n.tf("search.limit", &[("count", &MAX_HITS.to_string())])
        } else {
            i18n.tf(
                "search.summary",
                &[
                    ("count", &state.hits.len().to_string()),
                    ("files", &state.file_count().to_string()),
                ],
            )
        };
        ui.label(status);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("\u{d7}").clicked() {
                action = Some(SearchAction::Close);
            }
        });
    });
    ui.separator();

    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, state.hits.len(), |ui, rows| {
            for hit in &state.hits[rows] {
                let shown = state
                    .root()
                    .and_then(|root| hit.path.strip_prefix(root).ok())
                    .unwrap_or(&hit.path);
                let label = format!("{}:{}   {}", shown.display(), hit.line, hit.text);
                let text = egui::RichText::new(label).monospace();
                if ui.selectable_label(false, text).clicked() {
                    action = Some(SearchAction::Open {
                        path: hit.path.clone(),
                        line: hit.line,
                    });
                }
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(root: &Path, query: &str, case: bool) -> Vec<(String, usize, String)> {
        let mut found = Vec::new();
        search(root, query, case, &AtomicBool::new(false), |hit| {
            let name = hit.path.strip_prefix(root).unwrap().display().to_string();
            found.push((name, hit.line, hit.text));
            true
        });
        found
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("red_search_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join("a.txt"), "one\n  Needle here\nthree needle needle\n").unwrap();
        fs::write(dir.join("sub/b.txt"), "no match\nneedle\n").unwrap();
        fs::write(dir.join(".git/c.txt"), "needle\n").unwrap();
        fs::write(dir.join("target/d.txt"), "needle\n").unwrap();
        fs::write(dir.join("bin.dat"), [0xff, 0xfe, b'n', b'e', b'e', b'd', b'l', b'e']).unwrap();
        dir
    }

    #[test]
    fn test_search_finds_lines_in_name_order() {
        let dir = fixture("order");
        let found = collect(&dir, "needle", false);
        assert_eq!(
            found,
            [
                ("a.txt".to_owned(), 2, "Needle here".to_owned()),
                ("a.txt".to_owned(), 3, "three needle needle".to_owned()),
                ("sub/b.txt".to_owned(), 2, "needle".to_owned()),
            ],
            "one hit per line; hidden, target and binary files skipped"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_files_lists_files_in_name_order() {
        let dir = fixture("walk");
        let mut names = Vec::new();
        walk_files(&dir, &AtomicBool::new(false), |entry| {
            let path = entry.path();
            names.push(path.strip_prefix(&dir).unwrap().display().to_string());
            true
        });
        assert_eq!(
            names,
            ["a.txt", "bin.dat", "sub/b.txt"],
            "hidden and target folders skipped"
        );
        let mut seen = 0;
        walk_files(&dir, &AtomicBool::new(false), |_| {
            seen += 1;
            false
        });
        assert_eq!(seen, 1, "on_file returning false stops the walk");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_honours_case_and_empty_query() {
        let dir = fixture("case");
        assert_eq!(collect(&dir, "Needle", true).len(), 1);
        assert!(collect(&dir, "", false).is_empty());
        assert!(collect(&dir, "zzz", false).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_stops_when_asked() {
        let dir = fixture("stop");
        let mut seen = 0;
        search(&dir, "needle", false, &AtomicBool::new(false), |_| {
            seen += 1;
            false
        });
        assert_eq!(seen, 1, "on_hit returning false stops the walk");
        let mut seen = 0;
        search(&dir, "needle", false, &AtomicBool::new(true), |_| {
            seen += 1;
            true
        });
        assert_eq!(seen, 0, "a set cancel flag stops it at once");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_state_collects_results_from_the_thread() {
        let dir = fixture("state");
        let ctx = egui::Context::default();
        let mut state = SearchState { query: "needle".into(), ..Default::default() };
        state.start(&ctx, dir.clone());
        assert!(state.is_running());
        for _ in 0..200 {
            state.poll();
            if !state.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!state.is_running());
        assert_eq!(state.hits.len(), 3);
        assert_eq!(state.file_count(), 2);
        assert!(!state.truncated);
        let _ = fs::remove_dir_all(&dir);
    }
}
