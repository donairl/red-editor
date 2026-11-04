//! VS Code-style Explorer: the folder tree in the sidebar.
//!
//! [`Explorer`] remembers which folders are open and caches what is in them;
//! [`show`] paints the sidebar and applies the clicks it receives. Typing in
//! the filter below the folder header swaps the tree for the files whose
//! names match.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::git::Kind;
use crate::git_worker::Decorations;
use crate::file_ops::{self, NameError};
use crate::highlighter::Language;
use crate::i18n::I18n;
use crate::{find, search, tab_bar, theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub name: String,
}

pub fn scan_directory(dir: &Path) -> Vec<FsEntry> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = match path.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            // Hidden files are listed; only the repository's own `.git`
            // folder is left out, as VS Code does.
            if name == ".git" {
                continue;
            }

            // The listing already knows the type, so most entries cost no
            // extra call to the file system; only a link is followed.
            let is_dir = match entry.file_type() {
                Ok(kind) if !kind.is_symlink() => kind.is_dir(),
                _ => path.is_dir(),
            };
            entries.push(FsEntry { path, is_dir, name });
        }
    }

    // Sort: directories first, then alphabetical by name (case-insensitive).
    // Each name is lowered once, not at every comparison.
    entries.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));

    entries
}

/// Colours of the Activity Bar and the sidebar. `DARK` and `LIGHT` are VS
/// Code's "Dark Modern" and "Light Modern"; the other themes derive theirs
/// in `theme.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarPalette {
    pub background: Color32,
    pub border: Color32,
    pub icon_active: Color32,
    pub icon_inactive: Color32,
    /// Marks the Activity Bar item whose view is open.
    pub accent: Color32,
    pub foreground: Color32,
    pub row_hover: Color32,
    /// The row of the file in the active tab.
    pub row_selected: Color32,
    pub indent_guide: Color32,
    /// Names of files git sees as new (staged or untracked).
    pub git_added: Color32,
    pub git_modified: Color32,
    pub git_deleted: Color32,
    pub git_conflict: Color32,
}

impl SidebarPalette {
    pub const DARK: SidebarPalette = SidebarPalette {
        background: Color32::from_rgb(0x18, 0x18, 0x18),
        border: Color32::from_rgb(0x2B, 0x2B, 0x2B),
        icon_active: Color32::from_rgb(0xD7, 0xD7, 0xD7),
        icon_inactive: Color32::from_rgb(0x86, 0x86, 0x86),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        foreground: Color32::from_rgb(0xCC, 0xCC, 0xCC),
        row_hover: Color32::from_rgb(0x2A, 0x2D, 0x2E),
        row_selected: Color32::from_rgb(0x37, 0x37, 0x3D),
        indent_guide: Color32::from_rgb(0x40, 0x40, 0x40),
        git_added: Color32::from_rgb(0x73, 0xC9, 0x91),
        git_modified: Color32::from_rgb(0xE2, 0xC0, 0x8D),
        git_deleted: Color32::from_rgb(0xC7, 0x4E, 0x39),
        git_conflict: Color32::from_rgb(0xE4, 0x67, 0x6B),
    };

    pub const LIGHT: SidebarPalette = SidebarPalette {
        background: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        icon_active: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        icon_inactive: Color32::from_rgb(0x61, 0x61, 0x61),
        accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
        foreground: Color32::from_rgb(0x3B, 0x3B, 0x3B),
        row_hover: Color32::from_rgb(0xF2, 0xF2, 0xF2),
        row_selected: Color32::from_rgb(0xE4, 0xE6, 0xF1),
        indent_guide: Color32::from_rgb(0xD3, 0xD3, 0xD3),
        git_added: Color32::from_rgb(0x58, 0x7C, 0x0C),
        git_modified: Color32::from_rgb(0x89, 0x55, 0x03),
        git_deleted: Color32::from_rgb(0xAD, 0x07, 0x07),
        git_conflict: Color32::from_rgb(0xAD, 0x07, 0x07),
    };

    /// The colour of a file with `kind` of change.
    pub fn git(&self, kind: Kind) -> Color32 {
        match kind {
            Kind::Added | Kind::Untracked => self.git_added,
            Kind::Modified | Kind::Renamed => self.git_modified,
            Kind::Deleted => self.git_deleted,
            Kind::Conflicted => self.git_conflict,
        }
    }
}

/// One visible line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub entry: FsEntry,
    /// 0 for the entries directly inside the root folder.
    pub depth: usize,
    /// Only ever `true` for folders.
    pub expanded: bool,
    /// Drawn as a name field: the entry being renamed, or the placeholder
    /// row of a new entry (its `name` is empty, its `path` the parent).
    pub edit: bool,
}

/// What the inline name field is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditTarget {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { path: PathBuf },
}

impl EditTarget {
    /// The folder the typed name goes in.
    fn dir(&self) -> &Path {
        match self {
            EditTarget::NewFile { parent } | EditTarget::NewFolder { parent } => parent,
            EditTarget::Rename { path } => path.parent().unwrap_or(path),
        }
    }
}

/// The name being typed in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineEdit {
    pub target: EditTarget,
    pub name: String,
    /// Why the last Enter was refused; shown below the row.
    pub error: Option<NameError>,
    /// Focus the field (and, for a rename, select the name before its
    /// extension) on the next frame it is drawn.
    focus: bool,
}

/// A file operation the Explorer asks the app to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerAction {
    Create { dir: PathBuf, name: String, folder: bool },
    Rename { from: PathBuf, name: String },
    Delete(PathBuf),
    Reveal(PathBuf),
}

/// The tree under one root folder.
pub struct Explorer {
    root: PathBuf,
    /// The `▾ NAME` section that holds the whole tree.
    section_open: bool,
    expanded: HashSet<PathBuf>,
    /// What each folder held when it was last read from disk.
    children: HashMap<PathBuf, Vec<FsEntry>>,
    /// The visible tree, kept until a folder opens or closes.
    rows: Option<Vec<TreeRow>>,
    /// The file-name filter typed below the folder header.
    query: String,
    /// Every file under the root, gathered the first time the filter is used.
    index: Option<FileIndex>,
    /// The last filter applied, and what it matched.
    found: Option<Found>,
    /// The name field open in the tree, if any.
    editing: Option<InlineEdit>,
    /// How many times the visible tree was worked out.
    #[cfg(test)]
    row_builds: usize,
}

/// Most files the filter looks through, so a huge folder cannot take
/// forever to index.
const MAX_INDEXED_FILES: usize = 50_000;

/// Files the indexing thread hands over at a time.
const INDEX_BATCH: usize = 512;

/// Every file under a folder, gathered on a background thread: walking a
/// big project takes seconds, and the window must not freeze meanwhile.
struct FileIndex {
    files: Vec<FsEntry>,
    /// Batches still to come; `None` once the thread is done.
    batches: Option<Receiver<Vec<FsEntry>>>,
}

impl FileIndex {
    /// Indexes every file under `root` that Find in Files would search, up
    /// to [`MAX_INDEXED_FILES`].
    fn start(ctx: &egui::Context, root: PathBuf) -> Self {
        Self::spawn(ctx, move |found| {
            let mut batch = Vec::new();
            let mut total = 0;
            search::walk_files(&root, &AtomicBool::new(false), |entry| {
                batch.push(FsEntry {
                    path: entry.path(),
                    is_dir: false,
                    name: entry.file_name().to_string_lossy().into_owned(),
                });
                total += 1;
                // `found` fails once the index is dropped: stop walking.
                if batch.len() == INDEX_BATCH && !found(std::mem::take(&mut batch)) {
                    return false;
                }
                total < MAX_INDEXED_FILES
            });
            if !batch.is_empty() {
                found(batch);
            }
        })
    }

    /// Runs `walk` on a new thread. It passes what it finds to `found`, which
    /// returns `false` once nobody listens any more, and wakes `ctx` each time.
    fn spawn(
        ctx: &egui::Context,
        walk: impl FnOnce(&mut dyn FnMut(Vec<FsEntry>) -> bool) + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            walk(&mut |batch| {
                let open = sender.send(batch).is_ok();
                ctx.request_repaint();
                open
            });
            drop(sender);
            ctx.request_repaint();
        });
        Self {
            files: Vec::new(),
            batches: Some(receiver),
        }
    }

    /// Takes in what the thread found since the last call.
    fn poll(&mut self) {
        let Some(batches) = &self.batches else { return };
        loop {
            match batches.try_recv() {
                Ok(batch) => self.files.extend(batch),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.batches = None;
                    return;
                }
            }
        }
    }

    fn is_done(&self) -> bool {
        self.batches.is_none()
    }
}

/// What a filter matched among the files indexed so far.
struct Found {
    query: String,
    /// How many of the index's files have been looked at.
    checked: usize,
    /// Best first.
    files: Vec<FsEntry>,
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            section_open: true,
            expanded: HashSet::new(),
            children: HashMap::new(),
            rows: None,
            query: String::new(),
            index: None,
            found: None,
            editing: None,
            #[cfg(test)]
            row_builds: 0,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_section_open(&self) -> bool {
        self.section_open
    }

    /// Expands a collapsed folder, or collapses an expanded one.
    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rows = None;
    }

    pub fn toggle_section(&mut self) {
        self.section_open = !self.section_open;
        self.rows = None;
    }

    /// Collapses every folder; the section stays open.
    pub fn collapse_all(&mut self) {
        self.editing = None;
        self.expanded.clear();
        self.rows = None;
    }

    /// Forgets what every folder held, so the next [`Self::rows`] and
    /// [`Self::matches`] read them from disk again. Expanded folders stay
    /// expanded.
    pub fn refresh(&mut self) {
        self.editing = None;
        self.children.clear();
        self.rows = None;
        self.index = None;
        self.found = None;
    }

    /// Whether the filter holds a word, so the sidebar lists
    /// [`Self::matches`] instead of the tree.
    pub fn is_filtering(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Every file under the root found so far, and whether the walk is
    /// done. Starts the background index the first time; never waits for
    /// it (`ctx` is woken as more files arrive).
    pub fn indexed_files(&mut self, ctx: &egui::Context) -> (&[FsEntry], bool) {
        let index = self
            .index
            .get_or_insert_with(|| FileIndex::start(ctx, self.root.clone()));
        index.poll();
        (&index.files, index.is_done())
    }

    /// Brings [`Self::matches`] up to date with the filter and with what the
    /// index found since the last call, starting the index on first use.
    /// Never waits for the index: `ctx` is woken when more files arrive.
    pub fn update_matches(&mut self, ctx: &egui::Context) {
        let query = self.query.trim();
        if query.is_empty() {
            return;
        }
        let index = self
            .index
            .get_or_insert_with(|| FileIndex::start(ctx, self.root.clone()));
        index.poll();
        let found = match &mut self.found {
            Some(found) if found.query == query => found,
            _ => self.found.insert(Found {
                query: query.to_owned(),
                checked: 0,
                files: Vec::new(),
            }),
        };
        if found.checked == index.files.len() {
            return;
        }
        // Only the files new since the last call are looked at.
        let words = |file: &FsEntry| find::words_found(&file.name, query, false);
        found.files.extend(
            index.files[found.checked..]
                .iter()
                .filter(|file| words(file) > 0)
                .cloned(),
        );
        found.checked = index.files.len();
        found.files.sort_by_cached_key(|file| {
            (
                std::cmp::Reverse(words(file)),
                file.name.to_lowercase(),
                file.path.clone(),
            )
        });
    }

    /// Files anywhere under the root whose names hold at least one word of
    /// the filter, ignoring case: those holding the most words first, then
    /// by name. Hidden files and the folders Find in Files skips are left
    /// out. Holds what [`Self::update_matches`] found so far.
    pub fn matches(&self) -> &[FsEntry] {
        match &self.found {
            Some(found) if found.query == self.query.trim() => &found.files,
            _ => &[],
        }
    }

    /// Whether every file under the root has been indexed, so
    /// [`Self::matches`] is complete.
    pub fn is_indexed(&self) -> bool {
        self.index.as_ref().is_some_and(FileIndex::is_done)
    }

    /// The visible tree in display order, reading any folder not yet cached.
    pub fn rows(&mut self) -> &[TreeRow] {
        if self.rows.is_none() {
            #[cfg(test)]
            {
                self.row_builds += 1;
            }
            let mut rows = Vec::new();
            if self.section_open {
                let root = self.root.clone();
                self.push_rows(&root, 0, &mut rows);
            }
            self.rows = Some(rows);
        }
        self.rows.as_deref().unwrap_or_default()
    }

    /// Expands `dir` if it is collapsed.
    pub fn expand(&mut self, dir: &Path) {
        if self.expanded.insert(dir.to_path_buf()) {
            self.rows = None;
        }
    }

    /// Opens a name field for a new file (or `folder`) as the first entry
    /// of `parent`, expanding it. Replaces any field already open.
    pub fn begin_new(&mut self, parent: PathBuf, folder: bool) {
        if parent != self.root {
            self.expanded.insert(parent.clone());
        }
        let target = if folder {
            EditTarget::NewFolder { parent }
        } else {
            EditTarget::NewFile { parent }
        };
        self.start_edit(target, String::new());
    }

    /// Turns the row of `path` into a name field holding its current name.
    pub fn begin_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.start_edit(EditTarget::Rename { path }, name);
    }

    fn start_edit(&mut self, target: EditTarget, name: String) {
        self.editing = Some(InlineEdit {
            target,
            name,
            error: None,
            focus: true,
        });
        self.rows = None;
    }

    pub fn cancel_edit(&mut self) {
        if self.editing.take().is_some() {
            self.rows = None;
        }
    }

    pub fn editing(&self) -> Option<&InlineEdit> {
        self.editing.as_ref()
    }

    /// The text of the open name field.
    pub fn edit_name_mut(&mut self) -> Option<&mut String> {
        self.editing.as_mut().map(|edit| &mut edit.name)
    }

    /// Closes the name field and returns what it asks for. An empty or
    /// unchanged name just closes it; a name that cannot be used keeps it
    /// open with [`InlineEdit::error`] set.
    pub fn commit_edit(&mut self) -> Option<ExplorerAction> {
        let edit = self.editing.as_ref()?;
        let current = match &edit.target {
            EditTarget::Rename { path } => Some(path.as_path()),
            _ => None,
        };
        let action = match file_ops::validate_name(edit.target.dir(), &edit.name, current) {
            Err(NameError::Empty) => None,
            Err(error) => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.error = Some(error);
                    edit.focus = true;
                }
                return None;
            }
            Ok(name) => match &edit.target {
                EditTarget::NewFile { parent } => Some(ExplorerAction::Create {
                    dir: parent.clone(),
                    name,
                    folder: false,
                }),
                EditTarget::NewFolder { parent } => Some(ExplorerAction::Create {
                    dir: parent.clone(),
                    name,
                    folder: true,
                }),
                EditTarget::Rename { path } => (path.file_name() != Some(std::ffi::OsStr::new(&name)))
                    .then(|| ExplorerAction::Rename {
                        from: path.clone(),
                        name,
                    }),
            },
        };
        self.cancel_edit();
        action
    }

    /// After `from` was renamed to `to`: folders that were expanded at or
    /// under `from` stay expanded under `to`.
    pub fn rebase_expanded(&mut self, from: &Path, to: &Path) {
        self.expanded = self
            .expanded
            .drain()
            .map(|dir| file_ops::rebase(&dir, from, to).unwrap_or(dir))
            .collect();
        self.rows = None;
    }

    /// Whether a new entry's field goes first in `dir`, and if so whether
    /// it is a folder.
    fn new_entry_in(&self, dir: &Path) -> Option<bool> {
        match &self.editing.as_ref()?.target {
            EditTarget::NewFile { parent } if parent == dir => Some(false),
            EditTarget::NewFolder { parent } if parent == dir => Some(true),
            _ => None,
        }
    }

    /// The entry being renamed.
    fn renaming(&self) -> Option<&Path> {
        match &self.editing.as_ref()?.target {
            EditTarget::Rename { path } => Some(path),
            _ => None,
        }
    }

    fn push_rows(&mut self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        if let Some(folder) = self.new_entry_in(dir) {
            rows.push(TreeRow {
                entry: FsEntry {
                    path: dir.to_path_buf(),
                    is_dir: folder,
                    name: String::new(),
                },
                depth,
                expanded: false,
                edit: true,
            });
        }
        let renaming = self.renaming().map(Path::to_path_buf);
        let entries = self
            .children
            .entry(dir.to_path_buf())
            .or_insert_with(|| scan_directory(dir))
            .clone();
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let path = entry.path.clone();
            let edit = renaming.as_deref() == Some(path.as_path());
            rows.push(TreeRow {
                entry,
                depth,
                expanded,
                edit,
            });
            if expanded {
                self.push_rows(&path, depth + 1, rows);
            }
        }
    }
}

/// Height of the `EXPLORER` title row, level with the tab strip.
pub(crate) const TITLE_HEIGHT: f32 = tab_bar::TAB_HEIGHT;
pub(crate) const TITLE_PADDING_X: f32 = 20.0;
/// Height of the section header and of every tree row.
pub(crate) const ROW_HEIGHT: f32 = 22.0;
pub(crate) const ROW_PADDING_X: f32 = 8.0;
/// Extra left offset per depth level.
const INDENT: f32 = 8.0;
pub(crate) const CHEVRON_WIDTH: f32 = 16.0;
pub(crate) const BADGE_GAP: f32 = 6.0;
/// Room for a git status letter at the end of a row.
const GIT_MARK_WIDTH: f32 = 10.0;
/// Side of the Search, Refresh and Collapse All buttons.
pub(crate) const ACTION_SIZE: f32 = 22.0;

/// One frame of the sidebar: the file to open, and where each part was drawn.
#[derive(Clone, Debug)]
pub struct ExplorerResponse {
    pub open_file: Option<PathBuf>,
    /// The file whose commit history was asked for (right-click menu).
    pub file_history: Option<PathBuf>,
    /// The Search button was clicked: the caller opens Find in Files.
    pub find_in_files: bool,
    /// A file operation picked from a menu or typed in a name field.
    pub action: Option<ExplorerAction>,
    // Only the tests read where things were drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub section_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub search_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub refresh_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub collapse_all_rect: Rect,
    /// [`Rect::NOTHING`] while the section is closed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub filter_rect: Rect,
    /// The tree rows in view, or the matches in view while filtering.
    #[cfg_attr(not(test), allow(dead_code))]
    pub rows: Vec<(PathBuf, Rect)>,
}

/// Draws the sidebar over the available width and applies folder, header,
/// Refresh and Collapse All clicks to `explorer`. A clicked file is returned
/// in [`ExplorerResponse::open_file`] for the caller to open, and a Search
/// click in [`ExplorerResponse::find_in_files`].
pub fn show(
    ui: &mut egui::Ui,
    explorer: &mut Explorer,
    active_file: Option<&Path>,
    git: Option<&Decorations>,
    i18n: &I18n,
) -> ExplorerResponse {
    let palette = theme::active(ui.ctx()).sidebar;
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let width = ui.available_width();

    // Right border, over the full height of the panel.
    let panel = ui.max_rect();
    ui.painter().vline(
        panel.right() - 0.5,
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );

    let (title_rect, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(title_rect.left() + TITLE_PADDING_X, title_rect.center().y),
        Align2::LEFT_CENTER,
        i18n.t("explorer.title"),
        FontId::proportional(11.0),
        palette.foreground,
    );

    let (section_rect, section) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    let collapse_all_rect = Rect::from_min_size(
        pos2(section_rect.right() - ACTION_SIZE, section_rect.top()),
        Vec2::splat(ACTION_SIZE),
    );
    let refresh_rect = collapse_all_rect.translate(vec2(-ACTION_SIZE, 0.0));
    let search_rect = refresh_rect.translate(vec2(-ACTION_SIZE, 0.0));
    // Registered after the header, so they win clicks inside their rects.
    let search = ui
        .interact(search_rect, section.id.with("search"), Sense::click())
        .on_hover_text(i18n.t("edit.find_in_files"));
    let refresh = ui
        .interact(refresh_rect, section.id.with("refresh"), Sense::click())
        .on_hover_text(i18n.t("explorer.refresh"));
    let collapse_all = ui
        .interact(
            collapse_all_rect,
            section.id.with("collapse_all"),
            Sense::click(),
        )
        .on_hover_text(i18n.t("explorer.collapse_all"));
    paint_section_header(
        ui,
        &palette,
        explorer,
        section_rect,
        section.hovered() || search.hovered() || refresh.hovered() || collapse_all.hovered(),
        [
            (search_rect, search.hovered()),
            (refresh_rect, refresh.hovered()),
            (collapse_all_rect, collapse_all.hovered()),
        ],
    );
    if refresh.clicked() {
        explorer.refresh();
    } else if collapse_all.clicked() {
        explorer.collapse_all();
    } else if section.clicked() && !search.clicked() {
        explorer.toggle_section();
    }

    let mut filter_rect = Rect::NOTHING;
    if explorer.is_section_open() {
        filter_rect = egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut explorer.query)
                        .id_salt("explorer_filter")
                        .hint_text(i18n.t("explorer.filter"))
                        .desired_width(f32::INFINITY),
                )
                .rect
            })
            .inner;
    }

    let mut response = ExplorerResponse {
        open_file: None,
        file_history: None,
        find_in_files: search.clicked(),
        action: None,
        section_rect,
        search_rect,
        refresh_rect,
        collapse_all_rect,
        filter_rect,
        rows: Vec::new(),
    };
    if explorer.is_section_open() && explorer.is_filtering() {
        show_matches(ui, &palette, explorer, active_file, git, i18n, &mut response);
        return response;
    }

    // Only the rows in view are laid out: a folder can hold thousands.
    // The rows are built first, while the name field is still in place,
    // then the field is taken out so it can be edited as the rows draw.
    explorer.rows();
    let mut edit = explorer.editing.take();
    let mut outcome = EditOutcome::Keep;
    let mut choice = None;
    let mut toggled = None;
    let mut blank_top = None;
    let root = explorer.root().to_path_buf();
    let rows = explorer.rows();
    let scroll = egui::ScrollArea::vertical()
        .id_salt("explorer_tree")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let last = range.end == rows.len();
            for row in &rows[range] {
                let (rect, row_response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if let (true, Some(edit)) = (row.edit, edit.as_mut()) {
                    outcome = show_edit_row(ui, &palette, row, rect, edit, i18n);
                    if !row.entry.name.is_empty() {
                        response.rows.push((row.entry.path.clone(), rect));
                    }
                    continue;
                }
                let selected = active_file == Some(row.entry.path.as_path());
                paint_row(
                    ui,
                    &palette,
                    row,
                    None,
                    git_kind(git, &row.entry),
                    rect,
                    row_response.hovered(),
                    selected,
                );
                if row_response.clicked() {
                    if row.entry.is_dir {
                        toggled = Some(row.entry.path.clone());
                    } else {
                        response.open_file = Some(row.entry.path.clone());
                    }
                }
                entry_menu(&row_response, &row.entry, true, i18n, &mut choice);
                response.rows.push((row.entry.path.clone(), rect));
            }
            if last {
                blank_top = Some(ui.min_rect().bottom());
            }
        });
    // The empty space below the last row: New File / New Folder in the root.
    if let Some(top) = blank_top.filter(|top| *top < scroll.inner_rect.bottom()) {
        let blank = Rect::from_min_max(
            pos2(scroll.inner_rect.left(), top),
            scroll.inner_rect.max,
        );
        let blank = ui.interact(blank, ui.scope_id().with("explorer_blank"), Sense::click());
        blank_menu(&blank, &root, i18n, &mut choice);
    }

    explorer.editing = edit;
    match outcome {
        EditOutcome::Commit => response.action = explorer.commit_edit(),
        EditOutcome::Cancel => explorer.cancel_edit(),
        EditOutcome::Keep => {}
    }
    if let Some(dir) = toggled {
        explorer.toggle(&dir);
    }
    if let Some(choice) = choice {
        apply_choice(explorer, choice, &mut response);
    }
    response
}

/// The files matching the filter as a flat list, each followed by its
/// folder, or a note that none match. A clicked file goes in
/// `response.open_file`.
fn show_matches(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    explorer: &mut Explorer,
    active_file: Option<&Path>,
    git: Option<&Decorations>,
    i18n: &I18n,
    response: &mut ExplorerResponse,
) {
    let root = explorer.root().to_path_buf();
    explorer.update_matches(ui.ctx());
    let mut edit = explorer.editing.take();
    let mut outcome = EditOutcome::Keep;
    let mut choice = None;
    let renaming = edit.as_ref().and_then(|e| match &e.target {
        EditTarget::Rename { path } => Some(path.clone()),
        _ => None,
    });
    let found = explorer.matches();
    if found.is_empty() {
        let message = if explorer.is_indexed() {
            "explorer.no_matches"
        } else {
            "explorer.indexing"
        };
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        ui.painter().text(
            pos2(rect.left() + TITLE_PADDING_X, rect.center().y),
            Align2::LEFT_CENTER,
            i18n.t(message),
            FontId::proportional(13.0),
            palette.icon_inactive,
        );
        explorer.editing = edit;
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("explorer_matches")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, found.len(), |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for entry in &found[range] {
                let (rect, row_response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                let row = TreeRow {
                    entry: entry.clone(),
                    depth: 0,
                    expanded: false,
                    edit: renaming.as_ref() == Some(&entry.path),
                };
                if let (true, Some(edit)) = (row.edit, edit.as_mut()) {
                    outcome = show_edit_row(ui, palette, &row, rect, edit, i18n);
                    response.rows.push((entry.path.clone(), rect));
                    continue;
                }
                let folder = entry
                    .path
                    .parent()
                    .and_then(|dir| dir.strip_prefix(&root).ok())
                    .map(|dir| dir.display().to_string())
                    .filter(|dir| !dir.is_empty());
                let selected = active_file == Some(entry.path.as_path());
                paint_row(
                    ui,
                    palette,
                    &row,
                    folder.as_deref(),
                    git_kind(git, entry),
                    rect,
                    row_response.hovered(),
                    selected,
                );
                if row_response.clicked() {
                    response.open_file = Some(entry.path.clone());
                }
                entry_menu(&row_response, entry, false, i18n, &mut choice);
                response.rows.push((entry.path.clone(), rect));
            }
        });
    explorer.editing = edit;
    match outcome {
        EditOutcome::Commit => response.action = explorer.commit_edit(),
        EditOutcome::Cancel => explorer.cancel_edit(),
        EditOutcome::Keep => {}
    }
    if let Some(choice) = choice {
        apply_choice(explorer, choice, response);
    }
}

/// `▾ NAME`, plus the Search, Refresh and Collapse All buttons while
/// `hovered`.
fn paint_section_header(
    ui: &egui::Ui,
    palette: &SidebarPalette,
    explorer: &Explorer,
    rect: Rect,
    hovered: bool,
    actions: [(Rect, bool); 3],
) {
    let painter = ui.painter();
    let chevron = Rect::from_min_size(
        pos2(rect.left() + ROW_PADDING_X, rect.top()),
        vec2(CHEVRON_WIDTH, ROW_HEIGHT),
    );
    paint_chevron(
        painter,
        chevron.center(),
        explorer.is_section_open(),
        palette.foreground,
    );

    let name = explorer
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| explorer.root().display().to_string());
    let right = if hovered {
        actions[0].0.left()
    } else {
        rect.right()
    };
    let galley = elided(
        ui,
        name,
        FontId::proportional(11.0),
        palette.foreground,
        right - chevron.right() - ROW_PADDING_X,
    );
    let pos = pos2(chevron.right(), rect.center().y - galley.size().y / 2.0);
    // egui bundles no bold face: a second pass half a point to the right
    // thickens the strokes instead.
    painter.galley(pos, galley.clone(), palette.foreground);
    painter.galley(pos + vec2(0.5, 0.0), galley, palette.foreground);

    if hovered {
        for (action, action_hovered) in actions {
            if action_hovered {
                painter.rect_filled(action.shrink(2.0), CornerRadius::same(4), palette.row_hover);
            }
        }
        let [(search, _), (refresh, _), (collapse_all, _)] = actions;
        paint_search_icon(painter, search.center(), palette.foreground);
        paint_refresh_icon(painter, refresh.center(), palette.foreground);
        paint_collapse_all_icon(painter, collapse_all.center(), palette.foreground);
    }
}

/// A right-click menu entry, applied once the tree is drawn (the menus run
/// while the rows still borrow the Explorer).
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuChoice {
    New { parent: PathBuf, folder: bool },
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    History(PathBuf),
}

/// The right-click menu of a file or folder row. `offer_new` is false in
/// the flat list of filter matches, which has no obvious target folder.
fn entry_menu(
    row: &egui::Response,
    entry: &FsEntry,
    offer_new: bool,
    i18n: &I18n,
    choice: &mut Option<MenuChoice>,
) {
    row.context_menu(|ui| {
        let mut pick = |ui: &mut egui::Ui, key: &str, value: MenuChoice| {
            if ui.button(i18n.t(key)).clicked() {
                *choice = Some(value);
                ui.close();
            }
        };
        if offer_new {
            let parent = if entry.is_dir {
                entry.path.clone()
            } else {
                entry.path.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            pick(ui, "explorer.new_file", MenuChoice::New { parent: parent.clone(), folder: false });
            pick(ui, "explorer.new_folder", MenuChoice::New { parent, folder: true });
            ui.separator();
        }
        pick(ui, "explorer.rename", MenuChoice::Rename(entry.path.clone()));
        pick(ui, "explorer.delete", MenuChoice::Delete(entry.path.clone()));
        ui.separator();
        pick(ui, "explorer.reveal", MenuChoice::Reveal(entry.path.clone()));
        if !entry.is_dir {
            pick(ui, "history.file_history", MenuChoice::History(entry.path.clone()));
        }
    });
}

/// The right-click menu of the empty space below the tree: new entries in
/// the root.
fn blank_menu(blank: &egui::Response, root: &Path, i18n: &I18n, choice: &mut Option<MenuChoice>) {
    blank.context_menu(|ui| {
        for (key, folder) in [("explorer.new_file", false), ("explorer.new_folder", true)] {
            if ui.button(i18n.t(key)).clicked() {
                *choice = Some(MenuChoice::New { parent: root.to_path_buf(), folder });
                ui.close();
            }
        }
    });
}

/// Applies a menu entry: new and rename open a name field; the rest go to
/// the app through `response`.
fn apply_choice(explorer: &mut Explorer, choice: MenuChoice, response: &mut ExplorerResponse) {
    match choice {
        MenuChoice::New { parent, folder } => explorer.begin_new(parent, folder),
        MenuChoice::Rename(path) => explorer.begin_rename(path),
        MenuChoice::Delete(path) => response.action = Some(ExplorerAction::Delete(path)),
        MenuChoice::Reveal(path) => response.action = Some(ExplorerAction::Reveal(path)),
        MenuChoice::History(path) => response.file_history = Some(path),
    }
}

/// The git change an Explorer entry is marked with.
fn git_kind(git: Option<&Decorations>, entry: &FsEntry) -> Option<Kind> {
    let git = git?;
    if entry.is_dir {
        git.dir(&entry.path)
    } else {
        git.file(&entry.path)
    }
}

/// Background, indent guides, chevron or badge, and the name of one row,
/// followed by `detail` in a dimmer colour. Returns where the name starts.
#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &egui::Ui,
    palette: &SidebarPalette,
    row: &TreeRow,
    detail: Option<&str>,
    git: Option<Kind>,
    rect: Rect,
    hovered: bool,
    selected: bool,
) -> f32 {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_selected);
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
    }

    // One guide per ancestor, through the middle of its chevron column.
    let guide = Stroke::new(1.0, palette.indent_guide);
    for level in 0..row.depth {
        let x = rect.left() + ROW_PADDING_X + level as f32 * INDENT + CHEVRON_WIDTH / 2.0;
        painter.vline(x.round() + 0.5, rect.y_range(), guide);
    }

    let chevron = Rect::from_min_size(
        pos2(
            rect.left() + ROW_PADDING_X + row.depth as f32 * INDENT,
            rect.top(),
        ),
        vec2(CHEVRON_WIDTH, ROW_HEIGHT),
    );
    let mut x = chevron.right();
    if row.entry.is_dir {
        paint_chevron(painter, chevron.center(), row.expanded, palette.foreground);
    } else {
        let (text, color) = tab_bar::badge(Language::from_path(&row.entry.path));
        let badge = painter.layout_no_wrap(
            text.to_owned(),
            FontId::monospace(10.0),
            color.unwrap_or(palette.icon_inactive),
        );
        let badge_width = badge.size().x;
        painter.galley(
            pos2(x, rect.center().y - badge.size().y / 2.0),
            badge,
            palette.foreground,
        );
        x += badge_width + BADGE_GAP;
    }
    let name_x = x;

    // A changed file ends with its status letter, a folder holding one
    // with a dot, both in the change's colour, as is the name.
    let mut right = rect.right() - ROW_PADDING_X;
    let mut name_color = palette.foreground;
    if let Some(kind) = git {
        let color = palette.git(kind);
        name_color = color;
        let center = pos2(right - GIT_MARK_WIDTH / 2.0, rect.center().y);
        if row.entry.is_dir {
            painter.circle_filled(center, 3.0, color);
        } else {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                kind.letter(),
                FontId::proportional(12.0),
                color,
            );
        }
        right -= GIT_MARK_WIDTH + BADGE_GAP;
    }

    let name = elided(
        ui,
        row.entry.name.clone(),
        FontId::proportional(13.0),
        name_color,
        right - x,
    );
    let name_width = name.size().x;
    painter.galley(
        pos2(x, rect.center().y - name.size().y / 2.0),
        name,
        name_color,
    );

    if let Some(detail) = detail {
        let x = x + name_width + BADGE_GAP;
        let width = right - x;
        // Narrower than this, only the `…` would show.
        if width > 16.0 {
            let detail = elided(
                ui,
                detail.to_owned(),
                FontId::proportional(12.0),
                palette.icon_inactive,
                width,
            );
            painter.galley(
                pos2(x, rect.center().y - detail.size().y / 2.0),
                detail,
                palette.icon_inactive,
            );
        }
    }
    name_x
}

/// How the name field ended this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditOutcome {
    Keep,
    Commit,
    Cancel,
}

/// Characters of `name` before its extension: what a rename selects.
/// Folders and dot-files (`.gitignore`) select the whole name.
fn stem_chars(name: &str, is_dir: bool) -> usize {
    let end = if is_dir { None } else { name.rfind('.').filter(|&i| i > 0) };
    name[..end.unwrap_or(name.len())].chars().count()
}

/// Draws `row` with a single-line field over its name. Enter or a click
/// elsewhere commits, Escape cancels; a refused name is shown below.
fn show_edit_row(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    row: &TreeRow,
    rect: Rect,
    edit: &mut InlineEdit,
    i18n: &I18n,
) -> EditOutcome {
    // Indent, chevron or badge as for a normal row, the badge after the
    // typed name; the name itself is the field's.
    let shown = TreeRow {
        entry: FsEntry {
            path: if row.entry.name.is_empty() {
                row.entry.path.join(&edit.name)
            } else {
                row.entry.path.clone()
            },
            is_dir: row.entry.is_dir,
            name: String::new(),
        },
        ..row.clone()
    };
    let name_x = paint_row(ui, palette, &shown, None, None, rect, false, true);

    let id = ui.scope_id().with("explorer_edit");
    if edit.focus {
        edit.focus = false;
        ui.memory_mut(|m| m.request_focus(id));
        let end = if matches!(edit.target, EditTarget::Rename { .. }) {
            stem_chars(&edit.name, row.entry.is_dir)
        } else {
            edit.name.chars().count()
        };
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(end),
        )));
        state.store(ui.ctx(), id);
    }
    let field_rect = Rect::from_min_max(
        pos2(name_x - 2.0, rect.top() + 1.0),
        pos2(rect.right() - ROW_PADDING_X, rect.bottom() - 1.0),
    );
    let field = ui.put(
        field_rect,
        egui::TextEdit::singleline(&mut edit.name)
            .id(id)
            .font(FontId::proportional(13.0))
            .margin(vec2(2.0, 1.0)),
    );
    if field.changed() {
        edit.error = None;
    }
    if let Some(error) = edit.error {
        paint_name_error(ui, palette, field_rect, &error_text(error, &edit.name, i18n));
    }
    if field.lost_focus() {
        return if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            EditOutcome::Cancel
        } else {
            EditOutcome::Commit
        };
    }
    EditOutcome::Keep
}

/// The message for a refused name.
fn error_text(error: NameError, name: &str, i18n: &I18n) -> String {
    let name = name.trim();
    match error {
        NameError::Empty => String::new(),
        NameError::Reserved => i18n.tf("explorer.name_reserved", &[("name", name)]),
        NameError::Separator => i18n.t("explorer.name_separator").to_owned(),
        NameError::Exists => i18n.tf("explorer.name_exists", &[("name", name)]),
    }
}

/// A red-bordered note just below `field`, over the rows beneath it.
fn paint_name_error(ui: &egui::Ui, palette: &SidebarPalette, field: Rect, message: &str) {
    field_outline(ui, field, palette.git_deleted);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("explorer_edit_error"),
    ));
    let galley = painter.layout(
        message.to_owned(),
        FontId::proportional(12.0),
        palette.foreground,
        field.width() - 8.0,
    );
    let note = Rect::from_min_size(
        field.left_bottom(),
        vec2(field.width(), galley.size().y + 8.0),
    );
    painter.rect(
        note,
        CornerRadius::ZERO,
        palette.background,
        Stroke::new(1.0, palette.git_deleted),
        StrokeKind::Inside,
    );
    painter.galley(note.min + vec2(4.0, 4.0), galley, palette.foreground);
}

fn field_outline(ui: &egui::Ui, field: Rect, color: Color32) {
    ui.painter()
        .rect_stroke(field, CornerRadius::same(2), Stroke::new(1.0, color), StrokeKind::Outside);
}

/// `text` on one line, cut with `…` where it would pass `max_width`.
pub(crate) fn elided(
    ui: &egui::Ui,
    text: String,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font_id, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// `›` pointing right when collapsed, down when expanded. Drawn as lines,
/// so it never depends on the bundled fonts.
pub(crate) fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, expanded: bool, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let points = if expanded {
        [vec2(-4.0, -2.0), vec2(0.0, 2.0), vec2(4.0, -2.0)]
    } else {
        [vec2(-2.0, -4.0), vec2(2.0, 0.0), vec2(-2.0, 4.0)]
    };
    painter.line_segment([center + points[0], center + points[1]], stroke);
    painter.line_segment([center + points[1], center + points[2]], stroke);
}

/// A magnifying glass: a ring with its handle to the bottom right.
pub(crate) fn paint_search_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let radius = 4.0;
    let ring = center + vec2(-1.5, -1.5);
    painter.circle_stroke(ring, radius, stroke);
    let handle = vec2(1.0, 1.0).normalized();
    painter.line_segment([ring + handle * radius, center + vec2(5.0, 5.0)], stroke);
}

/// A circular arrow, open at the top right.
pub(crate) fn paint_refresh_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    use std::f32::consts::PI;
    let stroke = Stroke::new(1.2, color);
    let radius = 5.0;
    // From the top (-90°) clockwise round to the right (≈ -20°).
    let start = -PI / 2.0;
    let end = start + 2.0 * PI * 0.8;
    let points: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let angle = start + (end - start) * i as f32 / 24.0;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect();
    let tip = points[0];
    painter.line(points, stroke);
    // Arrow head at the start of the arc, pointing back into the gap.
    painter.line_segment([tip, tip + vec2(3.0, -3.0)], stroke);
    painter.line_segment([tip, tip + vec2(3.0, 3.0)], stroke);
}

/// A box with a minus, in front of a second box: VS Code's "collapse all".
fn paint_collapse_all_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let front = Rect::from_center_size(center + vec2(1.0, 1.0), Vec2::splat(9.0));
    painter.rect_stroke(front, CornerRadius::ZERO, stroke, StrokeKind::Middle);
    painter.line_segment(
        [
            pos2(front.left() + 2.5, front.center().y),
            pos2(front.right() - 2.5, front.center().y),
        ],
        stroke,
    );
    // Top and left edges of the box behind.
    let back = front.translate(vec2(-2.5, -2.5));
    painter.line_segment([back.left_bottom(), back.left_top()], stroke);
    painter.line_segment([back.left_top(), back.right_top()], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn test_scan_directory_sorting_and_hidden_files() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_dir");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("sub_folder")).unwrap();
        fs::create_dir_all(temp_dir.join(".hidden_folder")).unwrap();
        fs::create_dir_all(temp_dir.join(".git")).unwrap();
        File::create(temp_dir.join("a_file.txt")).unwrap();
        File::create(temp_dir.join("b_file.txt")).unwrap();
        File::create(temp_dir.join(".hidden_file.txt")).unwrap();

        let listed: Vec<(String, bool)> = scan_directory(&temp_dir)
            .into_iter()
            .map(|entry| (entry.name, entry.is_dir))
            .collect();
        // Folders first, then files; hidden entries shown, `.git` skipped.
        assert_eq!(
            listed,
            [
                (".hidden_folder".to_owned(), true),
                ("sub_folder".to_owned(), true),
                (".hidden_file.txt".to_owned(), false),
                ("a_file.txt".to_owned(), false),
                ("b_file.txt".to_owned(), false),
            ]
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_scan_directory_case_insensitive_sorting() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_case");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        File::create(temp_dir.join("b_file.txt")).unwrap();
        File::create(temp_dir.join("A_file.txt")).unwrap();
        File::create(temp_dir.join("c_file.txt")).unwrap();

        let entries = scan_directory(&temp_dir);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "A_file.txt");
        assert_eq!(entries[1].name, "b_file.txt");
        assert_eq!(entries[2].name, "c_file.txt");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    /// A link is listed as what it points to, so a linked folder opens.
    #[cfg(unix)]
    #[test]
    fn test_scan_directory_follows_symlinks() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_links");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("real_dir")).unwrap();
        File::create(temp_dir.join("real_file.txt")).unwrap();
        std::os::unix::fs::symlink(temp_dir.join("real_dir"), temp_dir.join("link_dir")).unwrap();
        std::os::unix::fs::symlink(
            temp_dir.join("real_file.txt"),
            temp_dir.join("link_file.txt"),
        )
        .unwrap();
        std::os::unix::fs::symlink(temp_dir.join("gone"), temp_dir.join("broken")).unwrap();

        let listed: Vec<(String, bool)> = scan_directory(&temp_dir)
            .into_iter()
            .map(|entry| (entry.name, entry.is_dir))
            .collect();
        assert_eq!(
            listed,
            [
                ("link_dir".to_owned(), true),
                ("real_dir".to_owned(), true),
                ("broken".to_owned(), false),
                ("link_file.txt".to_owned(), false),
                ("real_file.txt".to_owned(), false),
            ]
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_palettes_match_vscode_modern_themes() {
        let d = SidebarPalette::DARK;
        assert_eq!(d.background, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(d.border, Color32::from_rgb(0x2B, 0x2B, 0x2B));
        assert_eq!(d.icon_active, Color32::from_rgb(0xD7, 0xD7, 0xD7));
        assert_eq!(d.icon_inactive, Color32::from_rgb(0x86, 0x86, 0x86));
        assert_eq!(d.accent, Color32::from_rgb(0x00, 0x78, 0xD4));
        assert_eq!(d.foreground, Color32::from_rgb(0xCC, 0xCC, 0xCC));
        assert_eq!(d.row_hover, Color32::from_rgb(0x2A, 0x2D, 0x2E));
        assert_eq!(d.row_selected, Color32::from_rgb(0x37, 0x37, 0x3D));
        assert_eq!(d.indent_guide, Color32::from_rgb(0x40, 0x40, 0x40));

        let l = SidebarPalette::LIGHT;
        assert_eq!(l.background, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(l.border, Color32::from_rgb(0xE5, 0xE5, 0xE5));
        assert_eq!(l.icon_active, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(l.icon_inactive, Color32::from_rgb(0x61, 0x61, 0x61));
        assert_eq!(l.accent, Color32::from_rgb(0x00, 0x5F, 0xB8));
        assert_eq!(l.foreground, Color32::from_rgb(0x3B, 0x3B, 0x3B));
        assert_eq!(l.row_hover, Color32::from_rgb(0xF2, 0xF2, 0xF2));
        assert_eq!(l.row_selected, Color32::from_rgb(0xE4, 0xE6, 0xF1));
        assert_eq!(l.indent_guide, Color32::from_rgb(0xD3, 0xD3, 0xD3));
    }

    /// A fresh folder named `rred_explorer_<name>` in the temp directory:
    ///
    /// ```text
    /// docs/guide.md
    /// src/main.rs
    /// src/nested/deep.rs
    /// Cargo.toml
    /// .git/HEAD
    /// ```
    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rred_explorer_{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        File::create(root.join("docs/guide.md")).unwrap();
        File::create(root.join("src/main.rs")).unwrap();
        File::create(root.join("src/nested/deep.rs")).unwrap();
        File::create(root.join("Cargo.toml")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        File::create(root.join(".git/HEAD")).unwrap();
        root
    }

    /// `(name, depth)` of every visible row.
    fn outline(explorer: &mut Explorer) -> Vec<(String, usize)> {
        explorer
            .rows()
            .iter()
            .map(|row| (row.entry.name.clone(), row.depth))
            .collect()
    }

    fn owned(rows: &[(&str, usize)]) -> Vec<(String, usize)> {
        rows.iter().map(|(n, d)| (n.to_string(), *d)).collect()
    }

    #[test]
    fn test_rows_list_root_entries_folders_first() {
        let root = fixture("root_rows");
        let mut explorer = Explorer::new(root.clone());
        let rows = explorer.rows().to_vec();
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        assert!(rows.iter().all(|row| !row.expanded));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_toggle_expands_and_collapses_folders() {
        let root = fixture("toggle");
        let mut explorer = Explorer::new(root.clone());

        explorer.toggle(&root.join("src"));
        let rows = explorer.rows();
        assert!(rows[1].expanded, "src is expanded");
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );

        explorer.toggle(&root.join("src/nested"));
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("deep.rs", 2),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );

        explorer.toggle(&root.join("src"));
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_collapse_all_leaves_root_entries() {
        let root = fixture("collapse_all");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.toggle(&root.join("src/nested"));
        explorer.toggle(&root.join("docs"));

        explorer.collapse_all();
        assert!(explorer.is_section_open(), "the section stays open");
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_folders_are_cached_until_refresh() {
        let root = fixture("refresh");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.rows();

        File::create(root.join("src/added.rs")).unwrap();
        let names = |explorer: &mut Explorer| -> Vec<String> {
            explorer
                .rows()
                .iter()
                .map(|row| row.entry.name.clone())
                .collect()
        };
        assert!(
            !names(&mut explorer).contains(&"added.rs".to_string()),
            "a cached folder is not read again"
        );

        explorer.refresh();
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("added.rs", 1),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ]),
            "refresh reads folders again and keeps src expanded"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_closed_section_has_no_rows() {
        let root = fixture("section");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle_section();
        assert!(!explorer.is_section_open());
        assert!(explorer.rows().is_empty());
        explorer.toggle_section();
        assert_eq!(explorer.rows().len(), 3);
        let _ = fs::remove_dir_all(&root);
    }

    /// One headless frame of the sidebar in a 240 × 600 window; returns its
    /// response and the painted shapes.
    fn run(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        explorer: &mut Explorer,
        active_file: Option<&Path>,
    ) -> (ExplorerResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(240.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| {
            response = Some(show(ui, explorer, active_file, None, &I18n::builtin()));
        });
        output.textures_delta.clear();
        (response.expect("show ran"), output.shapes)
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

    fn row_rect(response: &ExplorerResponse, path: &Path) -> Rect {
        response
            .rows
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("no row for {path:?}"))
    }

    /// Rects painted with `fill`.
    fn filled(shapes: &[egui::epaint::ClippedShape], fill: Color32) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_clicking_a_folder_row_expands_it() {
        let root = fixture("click_folder");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        let src = row_rect(&response, &root.join("src"));

        let (response, _) = run(&ctx, click(src.center()), &mut explorer, None);
        assert_eq!(response.open_file, None, "a folder is not opened as a file");
        assert!(explorer.rows()[1].expanded, "clicking src expands it");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_clicking_a_file_row_opens_it() {
        let root = fixture("click_file");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let file = root.join("Cargo.toml");
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);

        let (response, _) = run(
            &ctx,
            click(row_rect(&response, &file).center()),
            &mut explorer,
            None,
        );
        assert_eq!(response.open_file, Some(file));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_active_file_row_is_selected() {
        let root = fixture("selected");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let active = root.join("Cargo.toml");
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, Some(&active));

        let palette = theme::active(&ctx).sidebar;
        assert_eq!(
            filled(&shapes, palette.row_selected),
            vec![row_rect(&response, &active)],
            "only the active file's row is selected"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_section_header_buttons() {
        let root = fixture("header_buttons");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        File::create(root.join("added.txt")).unwrap();

        run(
            &ctx,
            click(response.refresh_rect.center()),
            &mut explorer,
            None,
        );
        let names: Vec<String> = explorer
            .rows()
            .iter()
            .map(|r| r.entry.name.clone())
            .collect();
        assert!(
            names.contains(&"added.txt".to_string()),
            "Refresh reads the folder again"
        );
        assert!(
            names.contains(&"main.rs".to_string()),
            "Refresh keeps src expanded"
        );

        run(
            &ctx,
            click(response.collapse_all_rect.center()),
            &mut explorer,
            None,
        );
        assert!(
            explorer.is_section_open(),
            "the buttons do not toggle the section"
        );
        assert!(
            explorer.rows().iter().all(|row| row.depth == 0),
            "Collapse All"
        );

        run(
            &ctx,
            click(response.section_rect.left_center() + vec2(30.0, 0.0)),
            &mut explorer,
            None,
        );
        assert!(
            !explorer.is_section_open(),
            "clicking the header closes the section"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_search_button_sits_left_of_refresh() {
        let root = fixture("search_button");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        assert!(!response.find_in_files, "nothing was clicked");
        assert_eq!(
            response.search_rect.right(),
            response.refresh_rect.left(),
            "Search sits just left of Refresh"
        );
        assert_eq!(
            response.refresh_rect.right(),
            response.collapse_all_rect.left(),
            "Refresh stays just left of Collapse All"
        );
        assert_eq!(
            response.collapse_all_rect.right(),
            response.section_rect.right()
        );

        let (clicked, _) = run(
            &ctx,
            click(response.search_rect.center()),
            &mut explorer,
            None,
        );
        assert!(
            clicked.find_in_files,
            "clicking Search asks for Find in Files"
        );
        assert!(
            explorer.is_section_open(),
            "the button does not toggle the section"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A fresh folder named `rred_explorer_<name>` holding PHP files:
    ///
    /// ```text
    /// app/SMSForm.php
    /// app/SMSLog.php
    /// app/UserForm.php
    /// lib/SMSSender.php
    /// target/SMSBuild.php
    /// Kirim.php
    /// .SMSHidden.php
    /// ```
    fn sms_fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rred_explorer_{name}"));
        let _ = fs::remove_dir_all(&root);
        for dir in ["app", "lib", "target"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "app/SMSForm.php",
            "app/SMSLog.php",
            "app/UserForm.php",
            "lib/SMSSender.php",
            "target/SMSBuild.php",
            "Kirim.php",
            ".SMSHidden.php",
        ] {
            File::create(root.join(file)).unwrap();
        }
        root
    }

    /// Calls `done` every millisecond until it holds; fails after 10 s.
    fn wait_until(mut done: impl FnMut() -> bool) {
        let start = std::time::Instant::now();
        while !done() {
            assert!(start.elapsed().as_secs() < 10, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn names(explorer: &Explorer) -> Vec<String> {
        explorer.matches().iter().map(|e| e.name.clone()).collect()
    }

    /// The names matching `query` once the whole folder is indexed.
    fn match_names(explorer: &mut Explorer, query: &str) -> Vec<String> {
        let ctx = egui::Context::default();
        explorer.query = query.to_owned();
        wait_until(|| {
            explorer.update_matches(&ctx);
            !explorer.is_filtering() || explorer.is_indexed()
        });
        names(explorer)
    }

    /// Quick Open reads the index the Explorer filter builds, as it fills.
    #[test]
    fn test_indexed_files_starts_the_index_and_reports_when_done() {
        let dir = std::env::temp_dir().join("rred_test_indexed_files");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join("b.rs")).unwrap();
        File::create(dir.join("a.rs")).unwrap();
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(dir.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (_, done) = explorer.indexed_files(&ctx);
            if done {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "index never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (files, done) = explorer.indexed_files(&ctx);
        assert!(done);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The filter never waits for the folder to be indexed: it lists what
    /// was found so far, and the rest as it arrives.
    #[test]
    fn test_filter_lists_matches_while_the_folder_is_still_indexed() {
        let ctx = egui::Context::default();
        let root = PathBuf::from("project");
        let entry = |name: &str| FsEntry {
            path: root.join(name),
            is_dir: false,
            name: name.to_owned(),
        };
        let first = vec![entry("b_match.txt"), entry("other.txt")];
        let second = vec![entry("a_match.txt")];
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let mut explorer = Explorer::new(root.clone());
        explorer.index = Some(FileIndex::spawn(&ctx, move |found| {
            found(first);
            let _ = gate.recv();
            found(second);
        }));
        explorer.query = "match".into();

        wait_until(|| {
            explorer.update_matches(&ctx);
            !explorer.matches().is_empty()
        });
        assert_eq!(names(&explorer), ["b_match.txt"]);
        assert!(!explorer.is_indexed());

        release.send(()).unwrap();
        wait_until(|| {
            explorer.update_matches(&ctx);
            explorer.is_indexed()
        });
        assert_eq!(names(&explorer), ["a_match.txt", "b_match.txt"]);
    }

    /// Until the first match turns up, the sidebar says it is still
    /// looking rather than that nothing matches.
    #[test]
    fn test_filter_says_it_is_indexing_until_done() {
        let ctx = egui::Context::default();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let mut explorer = Explorer::new(PathBuf::from("project"));
        explorer.index = Some(FileIndex::spawn(&ctx, move |_| {
            let _ = gate.recv();
        }));
        explorer.query = "zzz".into();
        let i18n = I18n::builtin();
        let (indexing, none) = (i18n.t("explorer.indexing"), i18n.t("explorer.no_matches"));

        let (_, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        assert!(painted_texts(&shapes).contains(&indexing.to_owned()));

        release.send(()).unwrap();
        wait_until(|| {
            run(&ctx, Vec::new(), &mut explorer, None);
            explorer.is_indexed()
        });
        let (_, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        assert!(painted_texts(&shapes).contains(&none.to_owned()));
    }

    #[test]
    fn test_matches_hold_any_query_word_ignoring_case() {
        let root = sms_fixture("matches");
        let mut explorer = Explorer::new(root.clone());
        assert_eq!(
            match_names(&mut explorer, "SMS keluar"),
            ["SMSForm.php", "SMSLog.php", "SMSSender.php"],
            "hidden files and target are skipped"
        );
        assert_eq!(
            match_names(&mut explorer, "sms log"),
            ["SMSLog.php", "SMSForm.php", "SMSSender.php"],
            "names holding more words come first"
        );
        assert_eq!(match_names(&mut explorer, "smslog"), ["SMSLog.php"]);
        assert_eq!(
            match_names(&mut explorer, "form"),
            ["SMSForm.php", "UserForm.php"]
        );
        assert!(match_names(&mut explorer, "zzz").is_empty());
        assert!(match_names(&mut explorer, "   ").is_empty());
        assert!(!explorer.is_filtering(), "a blank query shows the tree");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_matches_are_cached_until_refresh() {
        let root = sms_fixture("matches_refresh");
        let mut explorer = Explorer::new(root.clone());
        assert_eq!(match_names(&mut explorer, "sender").len(), 1);
        File::create(root.join("app/MailSender.php")).unwrap();
        assert_eq!(match_names(&mut explorer, "sender").len(), 1);
        explorer.refresh();
        assert_eq!(
            match_names(&mut explorer, "sender"),
            ["MailSender.php", "SMSSender.php"]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_git_changes_mark_files_and_folders() {
        use crate::git::{FileChange, RepoStatus};
        let root = std::env::temp_dir().join(format!("red_fb_git_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();
        let status = RepoStatus {
            files: vec![
                FileChange {
                    path: root.join("src/main.rs"),
                    orig_path: None,
                    staged: None,
                    unstaged: Some(Kind::Modified),
                },
                FileChange {
                    path: root.join("notes.txt"),
                    orig_path: None,
                    staged: None,
                    unstaged: Some(Kind::Untracked),
                },
            ],
            ..RepoStatus::default()
        };
        let decorations = Decorations::new(&root, &status);
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(240.0, 600.0))),
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input(), |ui| {
                show(ui, &mut explorer, None, Some(&decorations), &I18n::builtin());
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }
        let texts = painted_texts(&shapes);
        assert!(texts.iter().any(|t| t == "M"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "U"), "{texts:?}");
        let palette = SidebarPalette::DARK;
        let dot = shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Circle(c) if c.fill == palette.git_modified
                || c.fill == SidebarPalette::LIGHT.git_modified)
        });
        assert!(dot, "the src folder holding a change gets a dot");
        let _ = fs::remove_dir_all(&root);
    }

    /// The text of every painted text shape.
    fn painted_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_typing_in_the_filter_lists_matching_files() {
        let root = sms_fixture("filter_typing");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        assert!(
            response.filter_rect.top() >= response.section_rect.bottom(),
            "the filter sits below the folder header"
        );
        run(
            &ctx,
            click(response.filter_rect.center()),
            &mut explorer,
            None,
        );
        let typed = vec![egui::Event::Text("SMS keluar".into())];
        run(&ctx, typed, &mut explorer, None);
        assert_eq!(explorer.query, "SMS keluar");
        wait_until(|| {
            run(&ctx, Vec::new(), &mut explorer, None);
            explorer.is_indexed()
        });
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        let listed: Vec<&Path> = response.rows.iter().map(|(p, _)| p.as_path()).collect();
        assert_eq!(
            listed,
            [
                root.join("app/SMSForm.php"),
                root.join("app/SMSLog.php"),
                root.join("lib/SMSSender.php"),
            ]
        );
        let texts = painted_texts(&shapes);
        assert!(
            texts.contains(&"lib".to_owned()),
            "each match shows its folder"
        );

        let sender = response.rows[2].1;
        let (response, _) = run(&ctx, click(sender.center()), &mut explorer, None);
        assert_eq!(response.open_file, Some(root.join("lib/SMSSender.php")));
        let _ = fs::remove_dir_all(&root);
    }

    /// The visible tree is worked out once and kept until a folder opens or
    /// closes, the section folds, or the tree is refreshed: building it
    /// every frame cloned every listed entry.
    #[test]
    fn test_rows_are_built_once_until_the_tree_changes() {
        let root = fixture("rows_cached");
        let mut explorer = Explorer::new(root.clone());
        explorer.rows();
        explorer.rows();
        assert_eq!(explorer.row_builds, 1);

        explorer.toggle(&root.join("src"));
        assert_eq!(explorer.rows().len(), 5);
        explorer.collapse_all();
        assert_eq!(explorer.rows().len(), 3);
        explorer.toggle_section();
        assert!(explorer.rows().is_empty());
        explorer.toggle_section();
        File::create(root.join("added.txt")).unwrap();
        explorer.refresh();
        assert_eq!(explorer.rows().len(), 4);
        assert_eq!(explorer.row_builds, 5);
        let _ = fs::remove_dir_all(&root);
    }

    /// A folder of thousands of entries costs only the rows in view.
    #[test]
    fn test_only_the_rows_in_view_are_laid_out() {
        let root = std::env::temp_dir().join("rred_explorer_many_rows");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for i in 0..500 {
            File::create(root.join(format!("file_{i:03}.txt"))).unwrap();
        }
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        // A 600-point window holds fewer than 30 rows of 22 points.
        assert!(
            response.rows.len() < 30,
            "{} rows laid out",
            response.rows.len()
        );
        assert_eq!(response.rows[0].0, root.join("file_000.txt"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_no_matches_shows_a_message() {
        let root = sms_fixture("no_matches");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.query = "zzz".into();
        wait_until(|| {
            run(&ctx, Vec::new(), &mut explorer, None);
            explorer.is_indexed()
        });
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        assert!(response.rows.is_empty());
        let message = I18n::builtin().t("explorer.no_matches").to_owned();
        assert!(painted_texts(&shapes).contains(&message));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_long_names_are_cut_to_the_sidebar_width() {
        let root = fixture("long_name");
        let long = "a_file_name_far_too_long_to_fit_in_a_narrow_sidebar.txt";
        File::create(root.join(long)).unwrap();
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        let row = row_rect(&response, &root.join(long));

        let name = shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == long => Some(text),
                _ => None,
            })
            .expect("the long name is painted");
        assert!(name.galley.elided, "the name is cut with …");
        assert!(
            name.pos.x + name.galley.size().x <= row.right(),
            "the name stays inside its row"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `(name, depth, edit)` of every visible row.
    fn edit_outline(explorer: &mut Explorer) -> Vec<(String, usize, bool)> {
        explorer
            .rows()
            .iter()
            .map(|row| (row.entry.name.clone(), row.depth, row.edit))
            .collect()
    }

    #[test]
    fn test_new_file_edit_row_is_first_under_its_expanded_parent() {
        let root = fixture("edit_new");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.join("src"), false);
        assert_eq!(
            edit_outline(&mut explorer),
            vec![
                ("docs".into(), 0, false),
                ("src".into(), 0, false),
                ("".into(), 1, true),
                ("nested".into(), 1, false),
                ("main.rs".into(), 1, false),
                ("Cargo.toml".into(), 0, false),
            ]
        );
        explorer.cancel_edit();
        assert!(explorer.editing().is_none());
        assert!(edit_outline(&mut explorer).iter().all(|(_, _, edit)| !edit));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_new_entry_in_the_root_is_the_first_row() {
        let root = fixture("edit_new_root");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), true);
        let rows = explorer.rows();
        assert!(rows[0].edit && rows[0].entry.is_dir && rows[0].depth == 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rename_marks_the_row_and_fills_the_name() {
        let root = fixture("edit_rename");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        assert_eq!(explorer.editing().unwrap().name, "Cargo.toml");
        let marked: Vec<_> = explorer.rows().iter().filter(|r| r.edit).map(|r| r.entry.name.clone()).collect();
        assert_eq!(marked, vec!["Cargo.toml".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_turns_a_valid_name_into_an_action() {
        let root = fixture("edit_commit");
        let mut explorer = Explorer::new(root.clone());

        explorer.begin_new(root.join("src"), false);
        explorer.edit_name_mut().unwrap().push_str(" lib.rs ");
        assert_eq!(
            explorer.commit_edit(),
            Some(ExplorerAction::Create { dir: root.join("src"), name: "lib.rs".into(), folder: false })
        );
        assert!(explorer.editing().is_none());

        explorer.begin_rename(root.join("Cargo.toml"));
        *explorer.edit_name_mut().unwrap() = "Cargo.lock".into();
        assert_eq!(
            explorer.commit_edit(),
            Some(ExplorerAction::Rename { from: root.join("Cargo.toml"), name: "Cargo.lock".into() })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_keeps_the_edit_open_on_a_bad_name() {
        let root = fixture("edit_bad");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), false);
        explorer.edit_name_mut().unwrap().push_str("Cargo.toml");
        assert_eq!(explorer.commit_edit(), None);
        assert_eq!(explorer.editing().unwrap().error, Some(NameError::Exists));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_cancels_an_empty_or_unchanged_name() {
        let root = fixture("edit_noop");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), true);
        assert_eq!(explorer.commit_edit(), None);
        assert!(explorer.editing().is_none(), "empty name cancels");

        explorer.begin_rename(root.join("Cargo.toml"));
        assert_eq!(explorer.commit_edit(), None);
        assert!(explorer.editing().is_none(), "same name cancels");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_refresh_and_collapse_all_cancel_the_edit() {
        let root = fixture("edit_refresh");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        explorer.refresh();
        assert!(explorer.editing().is_none());
        explorer.begin_rename(root.join("Cargo.toml"));
        explorer.collapse_all();
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rebase_expanded_keeps_a_renamed_folder_open() {
        let root = fixture("edit_rebase");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.toggle(&root.join("src/nested"));
        fs::rename(root.join("src"), root.join("lib")).unwrap();
        explorer.rebase_expanded(&root.join("src"), &root.join("lib"));
        explorer.refresh();
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("lib", 0),
                ("nested", 1),
                ("deep.rs", 2),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn test_typing_a_name_and_enter_asks_to_create_it() {
        let root = fixture("ui_new");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), false);

        run(&ctx, Vec::new(), &mut explorer, None); // focuses the field
        run(&ctx, vec![egui::Event::Text(String::from("notes.md"))], &mut explorer, None);
        let (response, _) = run(&ctx, vec![key(egui::Key::Enter)], &mut explorer, None);
        assert_eq!(
            response.action,
            Some(ExplorerAction::Create { dir: root.clone(), name: "notes.md".into(), folder: false })
        );
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_escape_closes_the_name_field_without_an_action() {
        let root = fixture("ui_escape");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));

        run(&ctx, Vec::new(), &mut explorer, None);
        run(&ctx, vec![egui::Event::Text("x".into())], &mut explorer, None);
        let (response, _) = run(&ctx, vec![key(egui::Key::Escape)], &mut explorer, None);
        assert_eq!(response.action, None);
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rename_field_starts_with_the_stem_selected() {
        let root = fixture("ui_stem");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        run(&ctx, Vec::new(), &mut explorer, None);
        // Typing replaces the selected stem and keeps `.toml`.
        run(&ctx, vec![egui::Event::Text("Build".into())], &mut explorer, None);
        assert_eq!(explorer.editing().unwrap().name, "Build.toml");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_stem_chars_stops_before_the_extension() {
        assert_eq!(stem_chars("main.rs", false), 4);
        assert_eq!(stem_chars("archive.tar.gz", false), 11);
        assert_eq!(stem_chars(".gitignore", false), 10);
        assert_eq!(stem_chars("src.old", true), 7);
        assert_eq!(stem_chars("héllo.rs", false), 5);
    }
}
