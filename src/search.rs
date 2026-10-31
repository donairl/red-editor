//! Find in Files: searching the open folder on a background thread, and the
//! results panel at the bottom of the window.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui::{self, Align2, CornerRadius, FontId, Key, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::file_browser;
use crate::find;
use crate::highlighter::Language;
use crate::i18n::I18n;
use crate::{symbols, tab_bar, theme};

/// Results kept before the search stops.
pub const MAX_HITS: usize = 2000;
/// Files larger than this are not searched.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Longest result text shown, in `char`s.
const MAX_TEXT_CHARS: usize = 200;
/// Characters kept before the match when a long line is cut around it.
const CONTEXT_BEFORE: usize = 30;
/// Height of a result row and of a file header.
const ROW_HEIGHT: f32 = 22.0;
const INDENT: f32 = 16.0;
/// Folders that are never searched, besides hidden ones.
const SKIPPED_DIRS: [&str; 2] = ["target", "node_modules"];

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    pub text: String,
    /// The matched `char` range of `text`, when one is known.
    pub range: Option<find::Range>,
}

/// `line` trimmed and cut to [`MAX_TEXT_CHARS`] around `range`, with the
/// range moved along. A cut is marked with `…`.
fn excerpt(line: &str, range: Option<find::Range>) -> (String, Option<find::Range>) {
    let leading = line.chars().take_while(|c| c.is_whitespace()).count();
    let trimmed: Vec<char> = line.trim().chars().collect();
    let range = range.map(|(s, e)| (s.saturating_sub(leading), e.saturating_sub(leading)));
    if trimmed.len() <= MAX_TEXT_CHARS {
        return (trimmed.into_iter().collect(), range);
    }
    let start = match range {
        Some((s, _)) if s > CONTEXT_BEFORE => (s - CONTEXT_BEFORE).min(trimmed.len() - MAX_TEXT_CHARS + 1),
        _ => 0,
    };
    let end = (start + MAX_TEXT_CHARS).min(trimmed.len());
    let mut text = String::new();
    let mut shift = start;
    if start > 0 {
        text.push('…');
        shift -= 1;
    }
    text.extend(&trimmed[start..end]);
    if end < trimmed.len() {
        text.push('…');
    }
    let shown = text.chars().count();
    let range = range.map(|(s, e)| {
        let s = s.saturating_sub(shift).min(shown);
        (s, e.saturating_sub(shift).clamp(s, shown))
    });
    (text, range)
}

/// Which files a search looks at: comma-separated globs such as
/// `*.rs, src/**, docs/*.md`. Empty means every file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFilter {
    patterns: Vec<String>,
}

impl FileFilter {
    pub fn parse(text: &str) -> FileFilter {
        FileFilter {
            patterns: text
                .split(',')
                .map(|p| p.trim().trim_start_matches("./").to_owned())
                .filter(|p| !p.is_empty())
                .collect(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether the file at `rel` (relative to the root, any separator)
    /// passes. A pattern without `/` is matched against the file name, one
    /// with `/` against the whole relative path.
    pub fn accepts(&self, rel: &Path) -> bool {
        if self.patterns.is_empty() {
            return true;
        }
        let path: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let full = path.join("/");
        let name = path.last().map(String::as_str).unwrap_or("");
        self.patterns.iter().any(|pattern| {
            if pattern.contains('/') {
                glob_match(pattern.trim_end_matches('/'), &full)
                    // `src/` or `src/**` also means everything under it.
                    || glob_match(&format!("{}/**", pattern.trim_end_matches('/')), &full)
            } else {
                glob_match(pattern, name)
            }
        })
    }
}

/// Glob matching with `*` (within one path segment), `**` (across
/// segments) and `?`. Case-sensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p {
            [] => t.is_empty(),
            ['*', '*', rest @ ..] => {
                let rest = rest.strip_prefix(&['/']).unwrap_or(rest);
                (0..=t.len()).any(|i| go(rest, &t[i..]))
            }
            ['*', rest @ ..] => (0..=t.len())
                .take_while(|&i| i == 0 || t[i - 1] != '/')
                .any(|i| go(rest, &t[i..])),
            ['?', rest @ ..] => !t.is_empty() && t[0] != '/' && go(rest, &t[1..]),
            [c, rest @ ..] => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
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
    filter: &FileFilter,
    cancel: &AtomicBool,
    mut on_hit: impl FnMut(Hit) -> bool,
) {
    if query.is_empty() {
        return;
    }
    walk_files(root, cancel, |entry| {
        let path = entry.path();
        if !path.strip_prefix(root).is_ok_and(|rel| filter.accepts(rel)) {
            return true;
        }
        let Some(text) = read_small(entry) else { return true };
        for (index, line) in text.lines().enumerate() {
            let matches = find::find_all(line, query, case_sensitive);
            let found = if whole_word {
                matches.into_iter().find(|&range| is_whole_word(line, range))
            } else {
                matches.into_iter().next()
            };
            let Some(range) = found else { continue };
            let (text, range) = excerpt(line, Some(range));
            let hit = Hit {
                path: path.clone(),
                line: index + 1,
                text,
                range,
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
            let range = find::find_all(line, name, true)
                .into_iter()
                .find(|&r| is_whole_word(line, r));
            let (text, range) = excerpt(line, range);
            let hit = Hit {
                path: path.clone(),
                line: symbol.line,
                text,
                range,
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
    /// Globs of the files to search, as typed.
    pub include: String,
    /// Put the caret in the query field on the next frame.
    pub focus_query: bool,
    pub hits: Vec<Hit>,
    /// The result the keyboard is on, and the last one opened.
    pub selected: Option<usize>,
    /// Files whose results are folded away.
    collapsed: HashSet<PathBuf>,
    /// Scroll the list to `selected` on the next frame.
    scroll_to_selected: bool,
    /// The query the results came from, so Enter knows whether to search
    /// again or open the selected result.
    searched_query: String,
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
        self.selected = None;
        self.collapsed.clear();
    }

    /// Stops a running search, keeping what it found so far.
    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
    }

    /// Starts searching `root` for the current query, replacing old results.
    pub fn start(&mut self, ctx: &egui::Context, root: PathBuf) {
        let (query, case_sensitive, whole_word) =
            (self.query.clone(), self.case_sensitive, self.whole_word);
        let filter = FileFilter::parse(&self.include);
        self.spawn(ctx, root, move |root, cancel, on_hit| {
            search(root, &query, case_sensitive, whole_word, &filter, cancel, on_hit)
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
        self.searched_query = self.query.clone();
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

/// One visible row of the results list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// The header of the file whose hits start at this index.
    File { first: usize, count: usize },
    Hit(usize),
}

/// The list as rows: a header per file, then its hits unless folded.
fn rows(hits: &[Hit], collapsed: &HashSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < hits.len() {
        let path = &hits[i].path;
        let count = hits[i..].iter().take_while(|h| &h.path == path).count();
        rows.push(Row::File { first: i, count });
        if !collapsed.contains(path) {
            rows.extend((i..i + count).map(Row::Hit));
        }
        i += count;
    }
    rows
}

/// Draws the panel contents; `root` is the folder searches run in, if one
/// is open.
pub fn show_panel(
    ui: &mut egui::Ui,
    state: &mut SearchState,
    root: Option<&Path>,
    i18n: &I18n,
) -> Option<SearchAction> {
    let palette = theme::active(ui.ctx()).sidebar;
    let mut action = None;
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    ui.add_space(6.0);

    // Toolbar: query and options on the left, status and panel buttons on
    // the right.
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (icon_rect, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
        file_browser::paint_search_icon(ui.painter(), icon_rect.center(), palette.icon_inactive);
        let query = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .id(egui::Id::unique("search_query"))
                .hint_text(i18n.t("search.query"))
                .desired_width(280.0),
        );
        if std::mem::take(&mut state.focus_query) {
            query.request_focus();
        }
        let in_query = query.has_focus() || query.lost_focus();
        let unchanged = state.searched && state.query == state.searched_query;
        if in_query && !state.hits.is_empty() && unchanged {
            // Up and Down walk the results; Enter opens the one reached.
            let (up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                )
            });
            if up || down {
                let last = state.hits.len() - 1;
                state.selected = Some(match (state.selected, down) {
                    (None, true) => 0,
                    (None, false) => last,
                    (Some(i), true) => (i + 1).min(last),
                    (Some(i), false) => i.saturating_sub(1),
                });
                state.scroll_to_selected = true;
                query.request_focus();
            }
        }
        if query.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            action = Some(match state.selected.filter(|_| unchanged) {
                Some(i) => SearchAction::Open {
                    path: state.hits[i].path.clone(),
                    line: state.hits[i].line,
                },
                None => SearchAction::Run,
            });
            query.request_focus();
        }
        option_toggle(ui, &mut state.case_sensitive, "Aa", i18n.t("find.case"), &palette);
        option_toggle(ui, &mut state.whole_word, "ab", i18n.t("find.whole_word"), &palette);
        if ui.button(i18n.t("search.run")).clicked() {
            action = Some(SearchAction::Run);
        }
        if state.is_running() {
            ui.spinner();
            if ui.button(i18n.t("search.cancel")).clicked() {
                state.stop();
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            if ui
                .add(egui::Button::new("×").frame(false))
                .on_hover_text(i18n.t("search.close"))
                .clicked()
            {
                action = Some(SearchAction::Close);
            }
            if !state.hits.is_empty() {
                let all_folded = state.hits.iter().all(|h| state.collapsed.contains(&h.path));
                let key = if all_folded { "search.expand_all" } else { "search.collapse_all" };
                if ui.add(egui::Button::new(if all_folded { "⊞" } else { "⊟" }).frame(false))
                    .on_hover_text(i18n.t(key))
                    .clicked()
                {
                    if all_folded {
                        state.collapsed.clear();
                    } else {
                        state.collapsed = state.hits.iter().map(|h| h.path.clone()).collect();
                    }
                }
            }
            let status = if root.is_none() {
                i18n.t("search.no_folder").to_owned()
            } else if state.is_running() {
                i18n.tf("search.found_so_far", &[("count", &state.hits.len().to_string())])
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
            ui.label(RichText::new(status).color(palette.icon_inactive));
        });
    });
    ui.horizontal(|ui| {
        ui.add_space(28.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.include)
                .id(egui::Id::unique("search_include"))
                .hint_text(i18n.t("search.include_hint"))
                .desired_width(280.0),
        )
        .on_hover_text(i18n.t("search.include_tooltip"));
    });
    ui.add_space(2.0);
    let line_y = ui.cursor().top() + 0.5;
    ui.painter().hline(ui.max_rect().x_range(), line_y, Stroke::new(1.0, palette.border));
    ui.add_space(3.0);

    // The results, grouped by file.
    let rows = rows(&state.hits, &state.collapsed);
    let scroll_to = std::mem::take(&mut state.scroll_to_selected)
        .then_some(state.selected)
        .flatten()
        .and_then(|i| rows.iter().position(|r| *r == Row::Hit(i)));
    let digits = state.hits.iter().map(|h| h.line).max().unwrap_or(1).max(1).ilog10() as usize + 1;
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let number_width = ui.fonts_mut(|f| f.glyph_width(&mono, '0')) * digits.max(2) as f32 + 12.0;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("search_results")
        .auto_shrink([false, false]);
    if let Some(row) = scroll_to {
        scroll = scroll.vertical_scroll_offset((row as f32 * ROW_HEIGHT - 2.0 * ROW_HEIGHT).max(0.0));
    }
    // Folders are shown relative to the folder searched, or the open one.
    let root_path = state.root().or(root).map(Path::to_path_buf);
    let mut toggle = None;
    scroll.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        for row in &rows[range] {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            match *row {
                Row::File { first, count } => {
                    let hit = &state.hits[first];
                    let folded = state.collapsed.contains(&hit.path);
                    paint_file_header(ui, &palette, rect, hit, count, folded, root_path.as_deref(), response.hovered());
                    if response.clicked() {
                        toggle = Some(hit.path.clone());
                    }
                }
                Row::Hit(i) => {
                    let hit = &state.hits[i];
                    let selected = state.selected == Some(i);
                    paint_hit(ui, &palette, rect, hit, number_width, &mono, response.hovered(), selected);
                    let response = response.on_hover_text(format!("{}:{}", hit.path.display(), hit.line));
                    if response.clicked() {
                        state.selected = Some(i);
                        action = Some(SearchAction::Open {
                            path: hit.path.clone(),
                            line: hit.line,
                        });
                    }
                }
            }
        }
    });
    if let Some(path) = toggle
        && !state.collapsed.remove(&path)
    {
        state.collapsed.insert(path);
    }
    action
}

/// A pill-shaped on/off option such as `Aa`.
fn option_toggle(ui: &mut egui::Ui, on: &mut bool, label: &str, tooltip: &str, palette: &file_browser::SidebarPalette) {
    let text = if *on {
        RichText::new(label).strong().color(palette.icon_active)
    } else {
        RichText::new(label).color(palette.icon_inactive)
    };
    let button = egui::Button::new(text)
        .corner_radius(6)
        .selected(*on)
        .min_size(vec2(28.0, 22.0));
    if ui.add(button).on_hover_text(tooltip).clicked() {
        *on = !*on;
    }
}

/// `▾ BADGE name  folder` with the number of hits in a pill at the right.
#[allow(clippy::too_many_arguments)]
fn paint_file_header(
    ui: &egui::Ui,
    palette: &file_browser::SidebarPalette,
    rect: Rect,
    hit: &Hit,
    count: usize,
    folded: bool,
    root: Option<&Path>,
    hovered: bool,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, if hovered { palette.row_hover } else { palette.background });
    let mut x = rect.left() + 8.0;
    file_browser::paint_chevron(painter, pos2(x + 6.0, rect.center().y), !folded, palette.foreground);
    x += INDENT;
    let (badge, color) = tab_bar::badge(Language::from_path(&hit.path));
    let badge = painter.layout_no_wrap(badge.to_owned(), FontId::monospace(10.0), color.unwrap_or(palette.icon_inactive));
    let badge_width = badge.size().x;
    painter.galley(pos2(x, rect.center().y - badge.size().y / 2.0), badge, palette.foreground);
    x += badge_width + 6.0;

    // The count pill first, so the name knows how much room is left.
    let pill_text = painter.layout_no_wrap(count.to_string(), FontId::proportional(11.0), palette.icon_active);
    let pill = Rect::from_center_size(
        pos2(rect.right() - 8.0 - (pill_text.size().x + 10.0) / 2.0, rect.center().y),
        vec2(pill_text.size().x + 10.0, 16.0),
    );
    painter.rect_filled(pill, CornerRadius::same(8), palette.row_selected);
    painter.galley(Align2::CENTER_CENTER.anchor_size(pill.center(), pill_text.size()).min, pill_text, palette.icon_active);
    let right = pill.left() - 8.0;

    let name = hit.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = file_browser::elided(ui, name, FontId::proportional(13.0), palette.icon_active, right - x);
    let name_width = name.size().x;
    painter.galley(pos2(x, rect.center().y - name.size().y / 2.0), name, palette.icon_active);
    x += name_width + 8.0;
    let folder = hit
        .path
        .parent()
        .map(|dir| root.and_then(|r| dir.strip_prefix(r).ok()).unwrap_or(dir))
        .map(|dir| dir.display().to_string())
        .filter(|dir| !dir.is_empty());
    if let Some(folder) = folder
        && right - x > 16.0
    {
        let folder = file_browser::elided(ui, folder, FontId::proportional(12.0), palette.icon_inactive, right - x);
        painter.galley(pos2(x, rect.center().y - folder.size().y / 2.0), folder, palette.icon_inactive);
    }
}

/// The line number, then the line with the match on a lit ground.
#[allow(clippy::too_many_arguments)]
fn paint_hit(
    ui: &egui::Ui,
    palette: &file_browser::SidebarPalette,
    rect: Rect,
    hit: &Hit,
    number_width: f32,
    mono: &FontId,
    hovered: bool,
    selected: bool,
) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_selected);
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
    }
    let x = rect.left() + 8.0 + 2.0 * INDENT;
    painter.text(
        pos2(x + number_width - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        hit.line.to_string(),
        mono.clone(),
        palette.icon_inactive,
    );
    let text_x = x + number_width + 4.0;
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::truncate_at_width((rect.right() - 8.0 - text_x).max(0.0)),
        ..Default::default()
    };
    let plain = egui::TextFormat::simple(mono.clone(), palette.foreground);
    let lit = egui::TextFormat {
        background: palette.accent.gamma_multiply(0.35),
        color: palette.icon_active,
        ..plain.clone()
    };
    match hit.range {
        Some((start, end)) if start < end => {
            let at = |i: usize| hit.text.char_indices().nth(i).map_or(hit.text.len(), |(b, _)| b);
            let (s, e) = (at(start), at(end));
            job.append(&hit.text[..s], 0.0, plain.clone());
            job.append(&hit.text[s..e], 0.0, lit);
            job.append(&hit.text[e..], 0.0, plain);
        }
        _ => job.append(&hit.text, 0.0, plain),
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, palette.foreground);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(root: &Path, query: &str, case: bool) -> Vec<(String, usize, String)> {
        let mut found = Vec::new();
        search(root, query, case, false, &FileFilter::default(), &AtomicBool::new(false), |hit| {
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
    fn test_hits_carry_the_match_range_and_cut_long_lines() {
        let dir = fixture("range");
        let long = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        fs::write(dir.join("long.txt"), &long).unwrap();
        let mut hits = Vec::new();
        search(&dir, "Needle", false, false, &FileFilter::default(), &AtomicBool::new(false), |hit| {
            hits.push(hit);
            true
        });
        let first = &hits[0];
        assert_eq!((first.text.as_str(), first.range), ("Needle here", Some((0, 6))), "leading spaces dropped");
        let cut = hits.iter().find(|h| h.path.ends_with("long.txt")).unwrap();
        let (s, e) = cut.range.unwrap();
        assert_eq!(cut.text.chars().count(), MAX_TEXT_CHARS + 2, "cut, with … at both ends");
        assert!(cut.text.starts_with('…') && cut.text.ends_with('…'));
        let shown: String = cut.text.chars().skip(s).take(e - s).collect();
        assert_eq!(shown, "needle");
        assert_eq!(s, CONTEXT_BEFORE + 1, "the … counts");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_glob_filter() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.rsx"));
        assert!(!glob_match("*.rs", "src/main.rs"), "`*` stays within a segment");
        assert!(glob_match("**/*.rs", "src/ui/main.rs"));
        assert!(glob_match("**/*.rs", "main.rs"));
        assert!(glob_match("src/**", "src/a/b.rs"));
        assert!(glob_match("a?c", "abc") && !glob_match("a?c", "a/c"));

        let filter = FileFilter::parse(" *.rs, docs/, ./tests/**/*.txt ");
        assert!(filter.accepts(Path::new("src/main.rs")));
        assert!(filter.accepts(Path::new("docs/guide.md")));
        assert!(filter.accepts(Path::new("tests/a/b/c.txt")));
        assert!(!filter.accepts(Path::new("README.md")));
        assert!(FileFilter::parse("  ,").is_empty());
        assert!(FileFilter::default().accepts(Path::new("anything")));

        let dir = fixture("filter");
        let mut names = Vec::new();
        search(&dir, "needle", false, false, &FileFilter::parse("sub/*"), &AtomicBool::new(false), |hit| {
            names.push(hit.path.strip_prefix(&dir).unwrap().display().to_string());
            true
        });
        assert_eq!(names, ["sub/b.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// One frame of the panel in a 700 × 300 window; the texts painted and
    /// the action.
    fn panel_frame(
        ctx: &egui::Context,
        state: &mut SearchState,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, Rect)>, Option<SearchAction>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(700.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            action = show_panel(ui, state, Some(Path::new("/r")), &I18n::builtin());
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect();
        (texts, action)
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn test_panel_groups_hits_under_file_headers_that_fold() {
        let ctx = egui::Context::default();
        let make = |p: &str, line, text: &str| Hit {
            path: PathBuf::from(p),
            line,
            text: text.into(),
            range: Some((0, 3)),
        };
        let mut state = SearchState {
            hits: vec![
                make("/r/src/a.rs", 3, "foo one"),
                make("/r/src/a.rs", 12, "foo two"),
                make("/r/b.rs", 1, "foo"),
            ],
            searched: true,
            ..Default::default()
        };
        let (texts, _) = panel_frame(&ctx, &mut state, Vec::new());
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(names.contains(&"a.rs") && names.contains(&"b.rs"), "{names:?}");
        assert!(names.contains(&"src"), "folder beside the name");
        assert!(names.contains(&"12") && names.contains(&"3"), "line numbers");
        assert!(names.contains(&"foo one") && names.contains(&"foo"), "{names:?}");
        assert!(names.iter().any(|t| t.contains("3 hasil")), "summary: {names:?}");

        // Clicking a header folds its hits.
        let (_, header) = texts.iter().find(|(t, _)| t == "a.rs").unwrap();
        let (_, action) = panel_frame(&ctx, &mut state, click(header.center()));
        assert_eq!(action, None);
        let (texts, _) = panel_frame(&ctx, &mut state, Vec::new());
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(!names.contains(&"foo one") && names.contains(&"foo"), "{names:?}");

        // Clicking a hit opens it and keeps it selected.
        let (_, row) = texts.iter().find(|(t, _)| t == "foo").unwrap();
        let (_, action) = panel_frame(&ctx, &mut state, click(row.center()));
        assert_eq!(action, Some(SearchAction::Open { path: PathBuf::from("/r/b.rs"), line: 1 }));
        assert_eq!(state.selected, Some(2));
    }

    #[test]
    fn test_rows_group_hits_by_file_and_fold() {
        let hit = |p: &str, line| Hit { path: PathBuf::from(p), line, text: String::new(), range: None };
        let hits = [hit("a", 1), hit("a", 2), hit("b", 1)];
        assert_eq!(
            rows(&hits, &HashSet::new()),
            [Row::File { first: 0, count: 2 }, Row::Hit(0), Row::Hit(1), Row::File { first: 2, count: 1 }, Row::Hit(2)]
        );
        let folded: HashSet<PathBuf> = [PathBuf::from("a")].into();
        assert_eq!(
            rows(&hits, &folded),
            [Row::File { first: 0, count: 2 }, Row::File { first: 2, count: 1 }, Row::Hit(2)]
        );
    }

    #[test]
    fn test_search_stops_when_asked() {
        let dir = fixture("stop");
        let mut seen = 0;
        search(&dir, "needle", false, false, &FileFilter::default(), &AtomicBool::new(false), |_| {
            seen += 1;
            false
        });
        assert_eq!(seen, 1, "on_hit returning false stops the walk");
        let mut seen = 0;
        search(&dir, "needle", false, false, &FileFilter::default(), &AtomicBool::new(true), |_| {
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
        search(&dir, "run", true, true, &FileFilter::default(), &AtomicBool::new(false), |hit| {
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
