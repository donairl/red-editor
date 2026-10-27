//! Find in Files: searching the open folder on a background thread, and the
//! results panel at the bottom of the window.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui::{self, Key};

use crate::find;
use crate::highlighter::Language;
use crate::i18n::I18n;
use crate::symbols;

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

/// Whether the `char` range `(start, end)` of `line` is a whole word: no
/// letter, digit or `_` right before or after it.
fn is_whole_word(line: &str, (start, end): find::Range) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let before = start.checked_sub(1).and_then(|i| line.chars().nth(i));
    let after = line.chars().nth(end);
    !before.is_some_and(word) && !after.is_some_and(word)
}

/// Reads `entry` if it is a small UTF-8 file.
fn read_small(entry: &fs::DirEntry) -> Option<String> {
    if entry.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
        return None;
    }
    fs::read_to_string(entry.path()).ok()
}

/// Calls `on_hit` for the first match on each matching line of every text
/// file under `root`, in name order. With `whole_word`, a match must not sit
/// inside a longer word. Stops when `on_hit` returns `false` or `cancel` is
/// set. Hidden entries, symlinks, huge files and files that are not UTF-8
/// are skipped.
pub fn search(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(Hit) -> bool,
) {
    if query.is_empty() {
        return;
    }
    walk_files(root, cancel, |entry| {
        let Some(text) = read_small(entry) else { return true };
        let path = entry.path();
        for (index, line) in text.lines().enumerate() {
            let matches = find::find_all(line, query, case_sensitive);
            let found = if whole_word {
                matches.into_iter().any(|range| is_whole_word(line, range))
            } else {
                !matches.is_empty()
            };
            if !found {
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

/// Calls `on_hit` for every definition of `name` in the `language` files
/// under `root`, as [`symbols::symbols`] finds them, in name order. Stops
/// like [`search`].
pub fn find_definitions(
    root: &Path,
    name: &str,
    language: Language,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(Hit) -> bool,
) {
    walk_files(root, cancel, |entry| {
        let path = entry.path();
        if Language::from_path(&path) != language {
            return true;
        }
        let Some(text) = read_small(entry) else { return true };
        for symbol in symbols::symbols(&text, language) {
            if !symbol.defines(name) {
                continue;
            }
            let line = text.lines().nth(symbol.line - 1).unwrap_or_default();
            let hit = Hit {
                path: path.clone(),
                line: symbol.line,
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
    pub whole_word: bool,
    /// Put the caret in the query field on the next frame.
    pub focus_query: bool,
    pub hits: Vec<Hit>,
    pub truncated: bool,
    /// A search was started since the panel opened.
    pub searched: bool,
    /// Go to Definition is running: a single result is opened directly.
    open_single: bool,
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
        self.open_single = false;
    }

    fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
    }

    /// Starts searching `root` for the current query, replacing old results.
    pub fn start(&mut self, ctx: &egui::Context, root: PathBuf) {
        let (query, case_sensitive, whole_word) =
            (self.query.clone(), self.case_sensitive, self.whole_word);
        self.spawn(ctx, root, move |root, cancel, on_hit| {
            search(root, &query, case_sensitive, whole_word, cancel, on_hit)
        });
    }

    /// Starts looking for the definitions of `name` in the `language` files
    /// of `root`. [`Self::take_single`] hands over a lone result.
    pub fn start_definitions(
        &mut self,
        ctx: &egui::Context,
        root: PathBuf,
        name: String,
        language: Language,
    ) {
        self.query = name.clone();
        self.case_sensitive = true;
        self.whole_word = true;
        self.spawn(ctx, root, move |root, cancel, on_hit| {
            find_definitions(root, &name, language, cancel, on_hit)
        });
        self.open_single = self.searched;
    }

    /// The only result of a finished Go to Definition, which is opened
    /// instead of listed.
    pub fn take_single(&mut self) -> Option<Hit> {
        if !self.open_single || self.is_running() {
            return None;
        }
        self.open_single = false;
        match self.hits.as_slice() {
            [hit] => Some(hit.clone()),
            _ => None,
        }
    }

    /// Runs `job` on a thread, replacing old results with what it finds.
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        root: PathBuf,
        job: impl FnOnce(&Path, &AtomicBool, &mut dyn FnMut(Hit) -> bool) + Send + 'static,
    ) {
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
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut count = 0;
            let mut truncated = false;
            job(&root, &cancel, &mut |hit| {
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
        ui.toggle_value(&mut state.whole_word, egui::RichText::new("ab").underline())
            .on_hover_text(i18n.t("find.whole_word"));
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
        search(root, query, case, false, &AtomicBool::new(false), |hit| {
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
        search(&dir, "needle", false, false, &AtomicBool::new(false), |_| {
            seen += 1;
            false
        });
        assert_eq!(seen, 1, "on_hit returning false stops the walk");
        let mut seen = 0;
        search(&dir, "needle", false, false, &AtomicBool::new(true), |_| {
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

    #[test]
    fn test_whole_word_skips_matches_inside_words() {
        assert!(is_whole_word("a needle b", (2, 8)));
        assert!(is_whole_word("needle", (0, 6)));
        assert!(!is_whole_word("needles", (0, 6)));
        assert!(!is_whole_word("my_needle", (3, 9)));
        assert!(is_whole_word("x.needle()", (2, 8)));

        let dir = std::env::temp_dir().join(format!("red_search_word_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.rs"), "run();\nrunner();\nrun_all(); run();\n").unwrap();
        let mut lines = Vec::new();
        search(&dir, "run", true, true, &AtomicBool::new(false), |hit| {
            lines.push(hit.line);
            true
        });
        assert_eq!(lines, [1, 3]);
        let _ = fs::remove_dir_all(&dir);
    }

    fn wait(state: &mut SearchState) {
        for _ in 0..200 {
            state.poll();
            if !state.is_running() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the search did not finish");
    }

    fn definitions_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("red_search_def_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "fn main() {\n    start();\n}\n").unwrap();
        fs::write(dir.join("src/server.rs"), "pub struct Server;\n\npub fn start() {}\n").unwrap();
        fs::write(dir.join("server.py"), "def start():\n    pass\n").unwrap();
        dir
    }

    #[test]
    fn test_find_definitions_reads_only_the_language() {
        let dir = definitions_fixture("lang");
        let mut found = Vec::new();
        find_definitions(&dir, "start", Language::Rust, &AtomicBool::new(false), |hit| {
            let name = hit.path.strip_prefix(&dir).unwrap().display().to_string();
            found.push((name, hit.line, hit.text));
            true
        });
        assert_eq!(found, [("src/server.rs".to_owned(), 3, "pub fn start() {}".to_owned())]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_state_hands_over_a_single_definition() {
        let dir = definitions_fixture("single");
        let ctx = egui::Context::default();
        let mut state = SearchState::default();
        state.start_definitions(&ctx, dir.clone(), "start".into(), Language::Rust);
        assert_eq!(state.query, "start");
        assert!(state.take_single().is_none(), "nothing while it runs");
        wait(&mut state);
        let hit = state.take_single().expect("one definition");
        assert_eq!(hit.line, 3);
        assert!(state.take_single().is_none(), "handed over once");

        state.start_definitions(&ctx, dir.clone(), "missing".into(), Language::Rust);
        wait(&mut state);
        assert!(state.take_single().is_none());
        assert!(state.hits.is_empty());

        state.query = "start".into();
        state.start(&ctx, dir.clone());
        wait(&mut state);
        assert!(state.take_single().is_none(), "a plain search lists its results");
        let _ = fs::remove_dir_all(&dir);
    }
}
