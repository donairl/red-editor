//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::about::{self, AboutState};
use crate::activity_bar;
use crate::blame::{self, Blame};
use crate::bookmarks::Bookmarks;
use crate::brackets;
use crate::file_browser::{self, Explorer, ExplorerAction};
use crate::file_ops;
use crate::git::{FileChange, GitError};
use crate::diff_view::{self, Against, DiffViewAction, DiffViewUi};
use crate::drop;
use crate::git_graph::{self, GitGraphUi, GraphAction};
use crate::git_worker::{Base, DiffSource, GitState, Op};
use crate::editor_view::{self, FoldOutput, render_editor_with, text_content_exceeds_highlight_limit};
#[cfg(test)]
use crate::editor_view::render_editor;
use crate::highlighter::{DocumentHighlightCache, Language, SyntaxHighlighter};
use crate::history::{Entry, EntryKind, History};
use crate::line_ops;
use crate::i18n::{self, I18n};
use crate::layout_cache::LayoutCache;
use crate::line_diff::{self, LineMarks};
use crate::markdown_preview::{MarkdownPreview, ScrollPos, ScrollSync};
use crate::modals::{self, DiskChoice, ModalResponse, PendingAction};
use crate::prettify;
use crate::find::{self, FindAction, FindState, GotoAction, GotoState};
use crate::folding::{self, Folds};
use crate::search::{self, SearchAction, SearchState};
use crate::session::{SESSION_KEY, Session};
use crate::settings::EditorSettings;
use crate::source_control::{self, DiffAction, ScmAction, ScmUi};
use crate::symbols::{self, PickerAction, SymbolPicker};
use crate::tab_bar;
use crate::palette::{self, Item, Palette, PaletteAction, PaletteMode};
use crate::git_log::GitLogPanel;
use crate::terminal::{self, Terminal};
use crate::theme::{self, ThemeId};
use crate::shortcuts::{self, ShortcutAction, consume_shortcut};
use crate::welcome::{self, WelcomeAction};
use crate::zoom;

/// Storage key under which eframe remembers the interface language.
const UI_LANGUAGE_KEY: &str = "ui_language";

/// Storage key under which eframe remembers the recently opened files and
/// folders.
const HISTORY_KEY: &str = "recent_paths";

/// Storage key under which eframe remembers the chosen color theme.
const THEME_KEY: &str = "theme";

/// Storage key under which eframe remembers the editor's font size.
const FONT_SIZE_KEY: &str = "font_size";

/// Storage key under which eframe remembers whether auto-save is on.
const AUTO_SAVE_KEY: &str = "auto_save";
/// eframe storage key of the editor settings (a JSON object).
const EDITOR_SETTINGS_KEY: &str = "editor_settings";

/// Seconds between checks for open files changed by other programs.
const DISK_CHECK_SECS: f64 = 2.0;

/// How long the typing must pause before auto-save writes the open files.
const AUTOSAVE_SECS: f64 = 1.0;

/// The width of the seam between the two editor views, in points.
const SPLIT_GAP: f32 = 1.0;

/// Least time between two updates of the gutter's git change markers.
const MARKS_INTERVAL: Duration = Duration::from_millis(250);

/// Largest file the editor opens. egui lays out the whole document, on every
/// edit: past this size each keystroke takes well over a frame.
const MAX_OPEN_BYTES: u64 = 10 * 1024 * 1024;

/// Under the editor's id, the `char` index it was last right-clicked at.
pub(crate) const CONTEXT_CHAR: &str = "context_char";

/// Least room worth giving an Open Recent entry's folder; with less, only
/// the row's hover text shows it.
pub(crate) const MIN_RECENT_FOLDER_WIDTH: f32 = 48.0;

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentTab {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub active_language: Language,
    pub language_override: Option<Language>,
    /// Line-by-line highlight state, so an edit only re-parses the lines it touched.
    pub highlight_cache: DocumentHighlightCache,
    /// The file as this tab last read or wrote it, to notice other programs
    /// changing it.
    pub disk_stamp: Option<DiskStamp>,
    /// The file changed on disk while this tab had unsaved edits; the user
    /// is asked which version to keep.
    pub disk_conflict: bool,
    /// Lines the user bookmarked; they follow edits.
    pub bookmarks: Bookmarks,
    /// HEAD's copy of the file, once git delivered it.
    pub git_base: Option<Base>,
    /// The lines that differ from `git_base`, for the gutter.
    pub line_marks: LineMarks,
    /// The text `line_marks` were computed for.
    marks_text: Option<String>,
    /// The caret, as a `char` index, and the editor's vertical scroll offset
    /// in points, as of the last frame the tab was drawn. Written to the
    /// session, so the tab reopens where it was left.
    pub caret: usize,
    pub scroll: f32,
    /// Set on a tab the session restored: its caret and scroll are put back
    /// the first time the tab is drawn, then this is cleared.
    pub restore_view: bool,
    /// The folded blocks; per tab, not saved.
    pub folds: Folds,
    /// The text `folds` and `foldable` were last checked against.
    fold_text: Option<String>,
    /// The foldable blocks of `fold_text`, as `folding::foldable` gives them.
    foldable: Vec<(usize, usize)>,
    /// The hidden `char` ranges of `fold_text`, and the folds they are for.
    hidden: Option<(Folds, Vec<(usize, usize)>)>,
}

/// Modified time and size of a file: when either moves, the file changed.
pub type DiskStamp = (SystemTime, u64);

/// The [`DiskStamp`] of the file at `path`, if it can be read.
fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The contents of the file at `path` and its [`DiskStamp`]. A file over
/// [`MAX_OPEN_BYTES`] is refused with [`std::io::ErrorKind::FileTooLarge`]
/// before any of it is read.
fn read_document(path: &Path) -> Result<(String, Option<DiskStamp>), std::io::Error> {
    // Stamped before reading: a write in between shows as a change.
    let stamp = disk_stamp(path);
    if fs::metadata(path)?.len() > MAX_OPEN_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "file too large to open",
        ));
    }
    Ok((fs::read_to_string(path)?, stamp))
}

impl Default for DocumentTab {
    fn default() -> Self {
        Self::default_new()
    }
}

impl DocumentTab {
    pub fn default_new() -> Self {
        Self {
            text_content: String::new(),
            saved_content: String::new(),
            current_path: None,
            active_language: Language::PlainText,
            language_override: None,
            highlight_cache: DocumentHighlightCache::default(),
            disk_stamp: None,
            disk_conflict: false,
            bookmarks: Bookmarks::default(),
            git_base: None,
            line_marks: LineMarks::default(),
            marks_text: None,
            caret: 0,
            scroll: 0.0,
            restore_view: false,
            folds: Folds::default(),
            fold_text: None,
            foldable: Vec::new(),
            hidden: None,
        }
    }

    pub fn from_file(path: PathBuf) -> Result<Self, std::io::Error> {
        let (content, disk_stamp) = read_document(&path)?;
        let active_language = Language::from_path(&path);
        Ok(Self {
            text_content: content.clone(),
            saved_content: content,
            current_path: Some(path),
            active_language,
            language_override: None,
            highlight_cache: DocumentHighlightCache::default(),
            disk_stamp,
            disk_conflict: false,
            bookmarks: Bookmarks::default(),
            git_base: None,
            line_marks: LineMarks::default(),
            marks_text: None,
            caret: 0,
            scroll: 0.0,
            restore_view: false,
            folds: Folds::default(),
            fold_text: None,
            foldable: Vec::new(),
            hidden: None,
        })
    }

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

    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
    }

    /// Moves the bookmarks along with the edits made since the last call.
    pub fn sync_bookmarks(&mut self) {
        self.bookmarks.sync(&self.text_content);
    }

    /// Moves the folds along with the edits made since the last call, and
    /// finds the foldable blocks again. Folding is off, and every fold
    /// dropped, in a document too large to highlight.
    pub fn sync_folds(&mut self) {
        if self.fold_text.as_ref() == Some(&self.text_content) {
            return;
        }
        if text_content_exceeds_highlight_limit(&self.text_content) {
            self.folds.clear();
            self.foldable.clear();
            self.fold_text = None;
            self.hidden = None;
            return;
        }
        if let Some(old) = &self.fold_text {
            self.folds.follow_edit(old, &self.text_content);
        }
        self.foldable = folding::foldable(&self.text_content);
        self.fold_text = Some(self.text_content.clone());
        self.hidden = None;
    }

    /// The `char` ranges the folds hide, sorted and apart, worked out again
    /// only when the text or the folds changed.
    pub fn hidden_ranges(&mut self) -> &[(usize, usize)] {
        self.sync_folds();
        if self.hidden.as_ref().is_none_or(|(folds, _)| *folds != self.folds) {
            let ranges = folding::hidden_ranges_of(&self.text_content, &self.foldable, &self.folds);
            self.hidden = Some((self.folds.clone(), ranges));
        }
        self.hidden.as_ref().map_or(&[], |(_, ranges)| ranges)
    }

    pub fn is_pristine(&self) -> bool {
        self.current_path.is_none() && !self.is_dirty() && self.text_content.is_empty()
    }

    /// The file name, or `untitled` for a document never saved.
    pub fn file_display_name(&self, untitled: &str) -> String {
        self.current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| untitled.to_owned())
    }

    pub fn effective_language(&self) -> Language {
        self.language_override.unwrap_or(self.active_language)
    }

    /// How this document appears in the tab strip.
    pub fn tab_view(&self, untitled: &str) -> tab_bar::TabView {
        tab_bar::TabView {
            title: self.file_display_name(untitled),
            tooltip: self.current_path.as_ref().map(|p| p.display().to_string()),
            language: self.effective_language(),
            dirty: self.is_dirty(),
        }
    }

    pub fn save(&mut self) -> Result<(), std::io::Error> {
        if let Some(ref path) = self.current_path {
            fs::write(path, &self.text_content)?;
            self.saved_content = self.text_content.clone();
            self.disk_stamp = disk_stamp(path);
            self.disk_conflict = false;
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No file path specified",
            ))
        }
    }
}

/// Which editor pane holds the keyboard, when the document is shown twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The editor above the split; the only one while the split is off.
    #[default]
    Main,
    /// The second view of the same document, on the right.
    Split,
}

/// What the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarView {
    #[default]
    Explorer,
    SourceControl,
}

/// A git command that waits for the user to confirm it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitConfirm {
    /// Throwing work-tree changes away cannot be undone.
    Discard(Vec<FileChange>),
    /// Nothing is staged: stage everything, then commit this message.
    CommitAll(String),
    /// Check a commit out, detaching HEAD.
    Checkout(String),
    /// Make a file what it was in a commit.
    RestoreFile { hash: String, path: PathBuf },
    /// Delete a local branch.
    DeleteBranch(String),
}

pub struct TextEditorApp {
    pub tabs: Vec<DocumentTab>,
    pub active_tab_index: usize,
    /// The open folder's tree; `None` until a folder is opened.
    pub explorer: Option<Explorer>,
    /// Whether the sidebar is shown.
    pub show_sidebar: bool,
    /// Which view the sidebar shows.
    pub sidebar_view: SidebarView,
    pub pending_action: Option<PendingAction>,
    pub show_about_modal: bool,
    pub about: AboutState,
    pub status_message: Option<(String, bool)>, // (message, is_error)
    /// The caret's line and column as the status bar last showed them.
    pub(crate) status_position: Option<(usize, usize)>,
    /// The bookmark count the status bar last showed.
    pub(crate) status_bookmarks: usize,
    /// Show the window listing the active document's bookmarks.
    pub show_bookmarks: bool,
    /// egui's clock when the open files were last compared with the disk.
    pub(crate) last_disk_check: f64,
    pub highlighter: SyntaxHighlighter,
    /// The editor's last layout, reused while the text does not change.
    /// Kept for the active tab only: a galley takes tens of bytes per
    /// character.
    pub(crate) editor_layout: LayoutCache,
    /// The split view's layout; separate, because the two panes have
    /// different widths to wrap at.
    pub(crate) editor_layout_split: LayoutCache,
    /// Show the document in a second view beside the editor. `View >
    /// Split Editor`.
    pub split: bool,
    /// The pane the keyboard was last in.
    pub focused_pane: Pane,
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
    /// Show the line-number gutter left of the text. On by default, as in
    /// VS Code.
    pub show_line_numbers: bool,
    /// Draw a thin line at each indentation level. On by default, as in VS
    /// Code.
    pub show_indent_guides: bool,
    /// Draw a dot for every space and an arrow for every tab. Off by
    /// default: it is an aid, not something to read code through.
    pub show_whitespace: bool,
    /// Save every dirty file with a path of its own once the typing pauses,
    /// and when the window loses focus. Off by default.
    pub auto_save: bool,
    /// Indentation and on-save settings, edited in the Settings window.
    pub editor_settings: EditorSettings,
    /// egui's clock when auto-save fires; `None` while nothing waits.
    pub(crate) autosave_due: Option<f64>,
    /// Whether any tab was dirty last frame, to notice an edit that did not
    /// come through the editor (Find and Replace, Prettify).
    pub(crate) autosave_dirty: bool,
    /// Whether the window had the keyboard last frame, to save when it goes.
    pub(crate) was_focused: bool,
    /// Show who last changed each line, in a column left of the gutter.
    pub show_blame: bool,
    /// Distraction-free mode: fullscreen, with only the editor shown.
    pub zen_mode: bool,
    /// Show the rendered Markdown pane beside the editor. Only takes effect
    /// on a Markdown tab; the choice survives switching to another tab.
    pub show_markdown_preview: bool,
    pub markdown_preview: MarkdownPreview,
    pub scroll_sync: ScrollSync,
    /// Interface translations and the language in use.
    pub i18n: I18n,
    pub find: FindState,
    pub goto: GotoState,
    pub symbol_picker: SymbolPicker,
    pub search: SearchState,
    /// One-shot asks for the editor widget, applied on its next frame.
    pub editor_request: EditorRequest,
    /// The editor a click on the menu bar took the focus from. It gets the
    /// focus and its selection back once every menu has closed.
    pub menu_focus: Option<egui::Id>,
    /// Show the Console panel (the integrated terminal) below the editor.
    pub show_console: bool,
    /// The shell of the Console panel, started the first time it is shown.
    /// Hiding the panel keeps it running.
    pub console: Option<Terminal>,
    /// Show the Git Log panel below the editor: every git command the
    /// editor ran, with its output.
    pub show_git_log: bool,
    pub git_log: GitLogPanel,
    /// The Command Palette / Quick Open popup.
    pub palette: Palette,
    /// Recently opened files and folders, for File > Open Recent.
    pub history: History,
    /// The color theme picked in View > Theme; `System` follows the OS.
    pub theme: ThemeId,
    /// The size of the editor's font, in points. One number for the editor,
    /// the gutter, the diff view and the terminal.
    pub font_size: f32,
    /// Git status of the open folder, or of the active file's folder.
    pub git: GitState,
    /// The name typed in the status bar's branch menu.
    pub new_branch_name: String,
    /// The Source Control view's commit message and open lists.
    pub scm: ScmUi,
    /// A git command asking for confirmation.
    pub git_confirm: Option<GitConfirm>,
    /// An Explorer entry waiting for the user to confirm its move to the Trash.
    pub pending_delete: Option<PathBuf>,
    /// The Git Graph in place of the editor while `Some`.
    pub git_graph: Option<GitGraphUi>,
    /// The side-by-side diff of the active document in place of the
    /// editor while `Some`.
    pub diff_view: Option<DiffViewUi>,
    /// The start page in place of the editor; back whenever no tab is left.
    pub show_welcome: bool,
    /// When the gutter's change markers were last computed.
    pub(crate) last_marks_at: Option<Instant>,
    /// The popup a click on a gutter change marker opened.
    pub hunk_popup: Option<HunkPopup>,
}

/// A gutter change marker's popup: what the lines were before, with Revert
/// and Stage.
#[derive(Clone, Debug, PartialEq)]
pub struct HunkPopup {
    /// The tab it was opened on; it closes when another tab is shown.
    pub tab: usize,
    /// The 1-based line whose marker was clicked.
    pub line: usize,
    /// Where it opens, beside the marker.
    pub pos: egui::Pos2,
}

/// What the hunk popup asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HunkAction {
    Close,
    Revert,
    Stage,
}

/// What the app wants done to the editor widget, which only `render_editor`
/// can reach.
#[derive(Clone, Default)]
pub struct EditorRequest {
    /// Give the editor keyboard focus.
    pub focus: bool,
    /// Press `Ctrl+<key>` in the editor (undo and redo).
    pub key: Option<egui::Key>,
    /// Put the caret at this `char` index and scroll to it.
    pub goto: Option<usize>,
    /// Select this `char` range before the editor handles this frame's
    /// input (restores the selection a menu took).
    pub select: Option<find::Range>,
    /// Paint this `char` range as selected while egui shows no selection of
    /// its own (the editor has lost focus to a menu). Set every frame.
    pub inactive_selection: Option<find::Range>,
    /// Scroll this `char` range into view.
    pub reveal: Option<find::Range>,
    /// Paint this `char` range as the current find match. Set every frame.
    pub highlight: Option<find::Range>,
    /// Auto-indent on Enter and auto-close brackets and quotes, by the rules
    /// of this language. `None` leaves typing to `TextEdit`.
    pub auto_edit: Option<Language>,
    /// Indentation settings for auto-indent and Tab / Shift+Tab.
    pub editor_settings: EditorSettings,
    /// Box the bracket at the caret and its partner.
    pub match_brackets: bool,
    /// The `char` ranges folded away (sorted, apart), laid out at zero
    /// height. Set every frame.
    pub hidden: Vec<(usize, usize)>,
    /// Line ranges `(header, last)` of the foldable blocks, for the
    /// gutter's chevrons. Empty while folding is off. Set every frame.
    pub foldable: Vec<(usize, usize)>,
}

impl Default for TextEditorApp {
    fn default() -> Self {
        Self {
            tabs: vec![DocumentTab::default_new()],
            active_tab_index: 0,
            explorer: None,
            show_sidebar: false,
            sidebar_view: SidebarView::Explorer,
            pending_action: None,
            show_about_modal: false,
            about: AboutState::default(),
            status_message: None,
            status_position: None,
            status_bookmarks: 0,
            show_bookmarks: false,
            last_disk_check: 0.0,
            highlighter: SyntaxHighlighter::new(),
            editor_layout: LayoutCache::default(),
            editor_layout_split: LayoutCache::default(),
            split: false,
            focused_pane: Pane::Main,
            word_wrap: false,
            show_line_numbers: true,
            show_indent_guides: true,
            show_whitespace: false,
            auto_save: false,
            editor_settings: EditorSettings::default(),
            autosave_due: None,
            autosave_dirty: false,
            was_focused: true,
            show_blame: false,
            zen_mode: false,
            show_markdown_preview: false,
            markdown_preview: MarkdownPreview::default(),
            scroll_sync: ScrollSync::default(),
            i18n: I18n::builtin(),
            find: FindState::default(),
            goto: GotoState::default(),
            symbol_picker: SymbolPicker::default(),
            search: SearchState::default(),
            editor_request: EditorRequest::default(),
            menu_focus: None,
            show_console: false,
            console: None,
            show_git_log: false,
            git_log: GitLogPanel::default(),
            palette: Palette::default(),
            history: History::default(),
            theme: ThemeId::System,
            font_size: zoom::DEFAULT_SIZE,
            git: GitState::default(),
            new_branch_name: String::new(),
            scm: ScmUi::default(),
            git_confirm: None,
            pending_delete: None,
            git_graph: None,
            diff_view: None,
            show_welcome: true,
            last_marks_at: None,
            hunk_popup: None,
        }
    }
}

/// egui's bundled fonts have no CJK glyphs; borrow one from the system so
/// Chinese text renders instead of boxes. Silently skipped when none is found.
fn install_cjk_fallback_font(ctx: &egui::Context) {
    const CANDIDATES: [&str; 6] = [
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

impl TextEditorApp {
    /// The app as eframe starts it: language files from disk, and the saved
    /// or the system interface language.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_fallback_font(&cc.egui_ctx);
        // egui zooms the whole window on `Ctrl++` and `Ctrl+-` by itself.
        // The editor has its own font size, which those keys drive instead,
        // so egui's zoom is turned off.
        cc.egui_ctx.options_mut(|options| options.zoom_with_keyboard = false);
        let (mut i18n, errors) = I18n::load(&i18n::search_dirs());
        let saved = cc
            .storage
            .and_then(|storage| storage.get_string(UI_LANGUAGE_KEY));
        i18n.choose_initial(saved.as_deref(), i18n::system_language().as_deref());
        let status_message = errors.first().map(|e| {
            let message = i18n.tf(
                "msg.language_load_failed",
                &[("file", &e.file), ("error", &e.error)],
            );
            (message, true)
        });
        let mut app = Self {
            i18n,
            status_message,
            history: saved_history(cc.storage),
            theme: saved_theme(cc.storage),
            font_size: saved_font_size(cc.storage),
            auto_save: saved_auto_save(cc.storage),
            editor_settings: saved_editor_settings(cc.storage),
            ..Default::default()
        };
        saved_session(cc.storage).restore(&mut app);
        app
    }

    pub fn active_tab(&self) -> &DocumentTab {
        &self.tabs[self.active_tab_index]
    }

    pub fn active_tab_mut(&mut self) -> &mut DocumentTab {
        &mut self.tabs[self.active_tab_index]
    }

    pub fn is_dirty(&self) -> bool {
        self.active_tab().is_dirty()
    }

    pub fn any_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.is_dirty())
    }

    pub fn file_display_name(&self) -> String {
        self.active_tab()
            .file_display_name(self.i18n.t("doc.untitled"))
    }

    /// Name of `lang` in the menus: translated for plain text, the
    /// language's own name otherwise.
    pub fn language_label(&self, lang: Language) -> String {
        match lang {
            Language::PlainText => self.i18n.t("language.plain_text").to_owned(),
            other => other.display_name().to_owned(),
        }
    }

    pub fn window_title(&self) -> String {
        if self.show_welcome {
            return "Red Text Editor".to_owned();
        }
        let dirty_flag = if self.is_dirty() { " *" } else { "" };
        format!("{}{dirty_flag} - Red Text Editor", self.file_display_name())
    }

    /// The language actually used for highlighting: the manual override if set,
    /// otherwise the language auto-detected from the current file.
    pub fn effective_language(&self) -> Language {
        self.active_tab().effective_language()
    }

    /// Adds or removes the bookmark on `line` of the active document.
    pub fn toggle_bookmark(&mut self, line: usize) {
        let tab = self.active_tab_mut();
        tab.bookmarks.toggle(line, &tab.text_content);
    }

    /// The hunk the open popup is about, while its tab is shown and git
    /// still marks a change there.
    fn popup_hunk(&self) -> Option<line_diff::Hunk> {
        let popup = self.hunk_popup.as_ref()?;
        if popup.tab != self.active_tab_index
            || self.zen_mode
            || self.show_welcome
            || self.git_graph.is_some()
            || self.diff_view.is_some()
        {
            return None;
        }
        let tab = self.tabs.get(popup.tab)?;
        let base = tab.git_base.as_ref()?;
        let base_text = base.text.as_deref().unwrap_or("");
        line_diff::hunk_at(base_text, &tab.text_content, popup.line)
    }

    /// The popup of a clicked gutter change marker: the lines as HEAD has
    /// them, the lines now, and Revert / Stage.
    fn show_hunk_popup(&mut self, ctx: &egui::Context) {
        if self.hunk_popup.is_none() {
            return;
        }
        let Some(hunk) = self.popup_hunk() else {
            self.hunk_popup = None;
            return;
        };
        let popup = self.hunk_popup.clone().expect("checked above");
        let palette = theme::active(ctx);
        let removed_bg = palette.gutter.deleted.gamma_multiply(0.18);
        let added_bg = palette.gutter.added.gamma_multiply(0.18);
        let mut action = None;
        let area = egui::Area::new(egui::Id::unique("hunk_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ui.horizontal(|ui| {
                        let lines = &hunk.new_line_range;
                        ui.label(egui::RichText::new(format!("{}–{}", lines.start(), lines.end())).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new("×").frame(false))
                                .on_hover_text(self.i18n.t("scm.diff_close"))
                                .clicked()
                            {
                                action = Some(HunkAction::Close);
                            }
                            if ui.small_button(self.i18n.t("scm.stage_hunk")).clicked() {
                                action = Some(HunkAction::Stage);
                            }
                            if ui.small_button(self.i18n.t("scm.revert_hunk")).clicked() {
                                action = Some(HunkAction::Revert);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let row = |ui: &mut egui::Ui, sign: &str, text: &str, fill: egui::Color32| {
                            egui::Frame::new().fill(fill).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.label(egui::RichText::new(format!("{sign} {text}")).monospace());
                            });
                        };
                        for line in &hunk.old_lines {
                            row(ui, "-", line, removed_bg);
                        }
                        for line in &hunk.new_lines {
                            row(ui, "+", line, added_bg);
                        }
                    });
                });
            });
        // A click elsewhere, or Escape, closes it; the click that opened it
        // lands outside the area on the marker and must not.
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && !ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| {
                area.response.rect.contains(p) || (p - popup.pos).length() < 16.0
            });
        if action.is_none() && (clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            action = Some(HunkAction::Close);
        }
        match action {
            None => {}
            Some(HunkAction::Close) => self.hunk_popup = None,
            Some(HunkAction::Revert) => {
                self.revert_hunk(&hunk);
                self.hunk_popup = None;
            }
            Some(HunkAction::Stage) => {
                let tab = self.active_tab();
                if let Some(path) = tab.current_path.clone() {
                    let op = Op::StageHunk { path, text: tab.text_content.clone(), line: popup.line };
                    self.run_git(ctx, op);
                }
                self.hunk_popup = None;
            }
        }
    }

    /// Puts HEAD's lines back in place of `hunk` in the active document.
    /// The file is not saved.
    fn revert_hunk(&mut self, hunk: &line_diff::Hunk) {
        let tab = self.active_tab_mut();
        let byte = |text: &str, at: usize| text.char_indices().nth(at).map_or(text.len(), |(b, _)| b);
        let (start, end) = (byte(&tab.text_content, hunk.char_range.0), byte(&tab.text_content, hunk.char_range.1));
        tab.text_content.replace_range(start..end, &hunk.revert_replacement);
        let at = hunk.char_range.0;
        self.editor_request.goto = Some(at);
        self.editor_request.focus = true;
    }

    /// The list of the active document's bookmarks, while it is open;
    /// picking one moves the caret there.
    fn show_bookmarks_window(&mut self, ctx: &egui::Context) {
        if !self.show_bookmarks || self.zen_mode {
            return;
        }
        let rows = self.bookmarked_rows();
        let caret_line = self.caret_position(ctx).map(|(line, _)| line);
        let mut open = true;
        let picked = modals::show_bookmarks_panel(ctx, &mut open, &rows, caret_line, &self.i18n);
        self.show_bookmarks = open;
        if let Some(line) = picked {
            self.goto_line(line);
        }
    }

    /// The active document's bookmarked lines, with their text.
    fn bookmarked_rows(&self) -> Vec<(usize, &str)> {
        let tab = self.active_tab();
        let mut texts = tab.text_content.split('\n').zip(1..);
        tab.bookmarks
            .lines()
            .filter_map(|line| {
                texts
                    .find(|&(_, n)| n == line)
                    .map(|(text, _)| (line, text))
            })
            .collect()
    }

    /// Re-indents the active document in place, reporting the outcome in the
    /// status bar.
    pub fn prettify_document(&mut self) {
        let language = self.effective_language();
        let message = if !prettify::supports(language) {
            (self.i18n.t("msg.prettify_unsupported").to_owned(), true)
        } else {
            match prettify::prettify(language, &self.active_tab().text_content) {
                Ok(text) => {
                    if text != self.active_tab().text_content {
                        self.active_tab_mut().text_content = text;
                    }
                    (self.i18n.t("msg.prettified").to_owned(), false)
                }
                Err(error) => (self.i18n.tf("msg.prettify_failed", &[("error", &error)]), true),
            }
        };
        self.status_message = Some(message);
    }

    pub(crate) fn send_editor_key(&mut self, key: egui::Key) {
        self.editor_request.focus = true;
        self.editor_request.key = Some(key);
    }

    /// Rewrites the active document around its selection and hands the
    /// editor the selection to show next frame. A caret the editor has not
    /// placed yet counts as the start of the document.
    fn edit_lines(
        &mut self,
        ctx: &egui::Context,
        edit: impl FnOnce(&str, line_ops::Range) -> Option<line_ops::Edit>,
    ) {
        let selection = self.editor_selection(ctx).unwrap_or((0, 0));
        let Some(edit) = edit(&self.active_tab().text_content, selection) else {
            return;
        };
        self.active_tab_mut().text_content = edit.text;
        self.editor_request.focus = true;
        self.editor_request.select = Some(edit.selection);
        self.editor_request.reveal = Some(edit.selection);
    }

    /// Edit > Toggle Line Comment, in the active document's language.
    pub fn toggle_comment(&mut self, ctx: &egui::Context) {
        match line_ops::comment_style(self.effective_language()) {
            Some(style) => {
                self.edit_lines(ctx, |text, sel| line_ops::toggle_comment(text, sel, style));
            }
            None => {
                self.status_message =
                    Some((self.i18n.t("msg.comment_unsupported").to_owned(), true));
            }
        }
    }

    /// Edit > Select Next Match: the word at the caret, then each further
    /// occurrence of the selection.
    pub fn select_next_match(&mut self, ctx: &egui::Context) {
        let selection = self.editor_selection(ctx).unwrap_or((0, 0));
        if let Some(range) = line_ops::select_next_match(&self.active_tab().text_content, selection)
        {
            self.editor_request.focus = true;
            self.editor_request.select = Some(range);
            self.editor_request.reveal = Some(range);
        }
    }

    /// Edit > Go to Bracket: the caret jumps by the partner of the bracket
    /// it touches. A selection counts from its start, as in VS Code.
    pub fn go_to_bracket(&mut self, ctx: &egui::Context) {
        let (start, _) = self.editor_selection(ctx).unwrap_or((0, 0));
        if let Some(at) = brackets::jump_target(&self.active_tab().text_content, start) {
            self.editor_request.focus = true;
            self.editor_request.select = Some((at, at));
            self.editor_request.reveal = Some((at, at));
        }
    }

    /// Fold: folds the innermost unfolded block holding the caret's line,
    /// its header included. Unfold (`fold` false): unfolds the innermost
    /// folded one. As VS Code's Fold and Unfold.
    pub fn fold_at_caret(&mut self, ctx: &egui::Context, fold: bool) {
        let (caret, _) = self.editor_selection(ctx).unwrap_or((0, 0));
        let tab = self.active_tab_mut();
        tab.sync_folds();
        let line = find::line_col(&tab.text_content, caret).0 - 1;
        let header = folding::enclosing(&tab.foldable, line)
            .into_iter()
            .find(|&header| tab.folds.is_folded(header) != fold);
        match header {
            Some(header) if fold => tab.folds.fold(header),
            Some(header) => tab.folds.unfold(header),
            None => return,
        }
        self.keep_caret_visible(ctx);
    }

    /// A caret that a fold just hid moves to the end of the fold's header,
    /// as in VS Code; left there, the editor would unfold it again.
    fn keep_caret_visible(&mut self, ctx: &egui::Context) {
        let (caret, _) = self.editor_selection(ctx).unwrap_or((0, 0));
        let hidden = self.active_tab_mut().hidden_ranges();
        let at = hidden.partition_point(|&(_, end)| end < caret);
        if let Some(&(start, _)) = hidden.get(at).filter(|&&(start, _)| start < caret) {
            self.editor_request.focus = true;
            self.editor_request.select = Some((start, start));
        }
    }

    fn saved_selection_id(&self) -> egui::Id {
        self.editor_id().with("saved_selection")
    }

    /// The id of the editor the app acts on: the pane with the keyboard,
    /// the only one while the split is off.
    pub(crate) fn editor_id(&self) -> egui::Id {
        let pane = if self.split { self.focused_pane } else { Pane::Main };
        self.editor_id_of(pane)
    }

    /// The widget id of one editor pane, for the active tab.
    pub(crate) fn editor_id_of(&self, pane: Pane) -> egui::Id {
        match pane {
            Pane::Main => egui::Id::unique(("editor", self.active_tab_index)),
            Pane::Split => egui::Id::unique(("editor-split", self.active_tab_index)),
        }
    }

    /// The non-empty selection the editor had on its last focused frame.
    fn saved_selection(&self, ctx: &egui::Context) -> Option<find::Range> {
        ctx.data(|d| d.get_temp::<find::Range>(self.saved_selection_id()))
    }

    /// The editor's selection as sorted `char` indices, or just the caret.
    fn editor_selection(&self, ctx: &egui::Context) -> Option<find::Range> {
        // The start page hides the editor; its old selection means nothing.
        if self.show_welcome {
            return None;
        }
        // egui collapses the selection once the editor loses focus, so while
        // a menu holds that focus use the one remembered from before.
        if self.menu_focus == Some(self.editor_id())
            && let Some(range) = self.saved_selection(ctx)
        {
            return Some(range);
        }
        let state = egui::TextEdit::load_state(ctx, self.editor_id())?;
        let range = state.cursor.char_range()?;
        let (a, b) = (range.primary.index.0, range.secondary.index.0);
        Some((a.min(b), a.max(b)))
    }

    /// The selected text, if any. Read by the app, not the editor: the menu
    /// holds the focus, so the editor cannot answer a copy itself.
    fn selected_text(&self, ctx: &egui::Context) -> Option<(find::Range, String)> {
        let (a, b) = self.editor_selection(ctx).filter(|(a, b)| a < b)?;
        let text = self.active_tab().text_content.chars().skip(a).take(b - a).collect();
        Some(((a, b), text))
    }

    /// Edit > Copy: puts the selection on the clipboard. The selection stays;
    /// see [`Self::return_focus_after_menu`].
    pub(crate) fn copy_selection(&mut self, ctx: &egui::Context) {
        if let Some((_, text)) = self.selected_text(ctx) {
            ctx.copy_text(text);
        }
        self.editor_request.focus = true;
    }

    /// Edit > Cut: copies the selection, then deletes it.
    pub(crate) fn cut_selection(&mut self, ctx: &egui::Context) {
        if let Some(((a, b), text)) = self.selected_text(ctx) {
            ctx.copy_text(text);
            let content = &mut self.active_tab_mut().text_content;
            let start = content.char_indices().nth(a).map_or(content.len(), |(i, _)| i);
            let end = content.char_indices().nth(b).map_or(content.len(), |(i, _)| i);
            content.replace_range(start..end, "");
            self.editor_request.goto = Some(a);
            ctx.data_mut(|d| d.remove::<find::Range>(self.saved_selection_id()));
        }
        self.editor_request.focus = true;
    }

    /// Edit > Paste: the editor pastes at the caret, over the selection once
    /// it is restored (see [`Self::return_focus_after_menu`]).
    pub(crate) fn paste_clipboard(&mut self, ctx: &egui::Context) {
        self.editor_request.focus = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }

    /// Notes that a click on the menu bar (`menu_bar`) took the focus from
    /// the editor, which had it when the frame began.
    fn note_menu_click(&mut self, ctx: &egui::Context, editor_had_focus: bool, menu_bar: egui::Rect) {
        let clicked_menu_bar = ctx.input(|i| {
            i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| menu_bar.contains(p))
        });
        if editor_had_focus && clicked_menu_bar {
            self.menu_focus = Some(self.editor_id());
        }
    }

    /// Once every menu has closed, gives the editor back the focus and the
    /// selection a menu click took, unless something else took the focus
    /// (the find bar, a dialog) or another tab is now active.
    fn return_focus_after_menu(&mut self, ctx: &egui::Context) {
        let Some(id) = self.menu_focus else {
            return;
        };
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        self.menu_focus = None;
        let dialog_open =
            self.show_about_modal || self.pending_action.is_some() || self.git_confirm.is_some() || self.pending_delete.is_some();
        let focus_free = ctx.memory(|m| m.focused().is_none());
        if id == self.editor_id() && !dialog_open && focus_free {
            self.editor_request.focus = true;
            self.editor_request.select = self.saved_selection(ctx);
        }
    }

    /// Opens the find bar, seeded with a one-line selection, if any.
    pub fn open_find(&mut self, ctx: &egui::Context, replace: bool) {
        self.find.visible = true;
        self.find.replace_mode = replace || (self.find.replace_mode && self.find.visible);
        self.find.focus_query = true;
        if let Some((a, b)) = self.editor_selection(ctx).filter(|(a, b)| a < b) {
            let picked: String = self.active_tab().text_content.chars().skip(a).take(b - a).collect();
            if !picked.contains('\n') {
                self.find.query = picked;
                self.find.current = None;
            }
        }
    }

    /// Opens the Find in Files panel, seeded with a one-line selection.
    pub fn open_search(&mut self, ctx: &egui::Context) {
        self.search.visible = true;
        self.search.focus_query = true;
        if let Some((a, b)) = self.editor_selection(ctx).filter(|(a, b)| a < b) {
            let picked: String = self.active_tab().text_content.chars().skip(a).take(b - a).collect();
            if !picked.contains('\n') {
                self.search.query = picked;
            }
        }
    }

    pub fn close_search(&mut self) {
        self.search.visible = false;
        self.search.clear();
        self.editor_request.focus = true;
    }

    fn open_search_hit(&mut self, path: PathBuf, line: usize) {
        self.open_file_in_tab(path);
        self.goto_line(line);
    }

    /// Enters or leaves zen mode: only the editor is shown, fullscreen.
    /// Whether the active tab is Markdown, by file extension or by the
    /// language picked from the menu, so the preview also works on an
    /// untitled tab.
    pub fn preview_available(&self) -> bool {
        self.effective_language() == Language::Markdown
    }

    /// Shows or hides the Markdown preview. Does nothing on other files.
    /// View > Split Editor: shows the document in a second view beside the
    /// editor, or closes that view. The two views share the text and scroll
    /// on their own.
    pub fn toggle_split(&mut self) {
        self.split = !self.split;
        self.focused_pane = Pane::Main;
        self.editor_request.focus = true;
    }

    pub fn toggle_markdown_preview(&mut self) {
        if self.preview_available() {
            self.show_markdown_preview = !self.show_markdown_preview;
        }
    }

    /// Whether the Markdown preview is shown; a git diff takes its place.
    fn previewing(&self) -> bool {
        self.show_markdown_preview
            && self.preview_available()
            && !self.zen_mode
            && self.git.diff().is_none()
            && self.git_graph.is_none()
            && self.diff_view.is_none()
            && !self.show_welcome
    }

    /// Moves the preview, and the editor with it, to the next or previous
    /// heading. Does nothing while no preview is shown.
    fn step_heading(&mut self, forward: bool) {
        if self.previewing() {
            self.markdown_preview.step(forward);
        }
    }

    pub fn toggle_zen_mode(&mut self, ctx: &egui::Context) {
        self.zen_mode = !self.zen_mode;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.zen_mode));
        self.editor_request.focus = true;
    }

    /// Writes the font size into egui's monospace text style, so the editor,
    /// the gutter, the diff view and the terminal all use it.
    ///
    /// Both styles are written, so the size survives a switch between the
    /// light and the dark theme. The style is only written when the size
    /// changed: writing it every frame would rebuild egui's font atlas for
    /// nothing.
    fn apply_font_size(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let size = zoom::clamp(self.font_size);
        if ctx.data(|d| d.get_temp::<f32>(font_size_id())) == Some(size) {
            return;
        }
        self.font_size = size;
        ctx.data_mut(|d| d.insert_temp(font_size_id(), size));
        ctx.all_styles_mut(|style| zoom::apply_to_style(style, size));
        // The window was built before the style changed, and keeps the style
        // it was made with. Give it the new size too, so the change shows
        // in this frame and not in the next one.
        zoom::apply_to_style(ui.style_mut(), size);
    }

    /// Makes the editor's font bigger or smaller.
    ///
    /// `steps` is how many steps to move: positive makes the font bigger.
    pub fn zoom(&mut self, steps: i32) {
        self.font_size = zoom::by(self.font_size, steps);
    }

    /// Returns the editor's font to its default size.
    pub fn reset_zoom(&mut self) {
        self.font_size = zoom::reset();
    }

    /// Zooms with the mouse wheel: `Ctrl` + scroll makes the text bigger or
    /// smaller, as in a browser.
    ///
    /// The scroll is taken out of the input, so the same gesture never
    /// scrolls the editor as well. egui leaves the scroll alone while the
    /// command key is down, so nothing else would have used it.
    fn zoom_with_wheel(&mut self, ctx: &egui::Context) {
        let points = ctx.input_mut(|i| {
            let mut points = 0.0;
            i.events.retain(|event| {
                let egui::Event::MouseWheel { unit, delta, modifiers, .. } = event else {
                    return true;
                };
                // Only the wheel with the command key down zooms.
                if !modifiers.command {
                    return true;
                }
                // A positive delta moves the content down, which is the
                // wheel rolling away from the user: that zooms in.
                points += zoom::wheel_points(*unit, delta.y);
                false
            });
            points
        });
        if points == 0.0 {
            return;
        }
        self.font_size = zoom::scrolled(self.font_size, points);
    }

    /// Edit > Go to Symbol: the bar listing what the active document
    /// defines. Does nothing for languages code navigation does not know.
    pub fn open_symbol_picker(&mut self) {
        let language = self.effective_language();
        if symbols::supports(language) {
            self.goto.visible = false;
            let found = symbols::symbols(&self.active_tab().text_content, language);
            self.symbol_picker.open(found);
        }
    }

    /// Opens the Command Palette in `mode`, or closes it when already open
    /// in that mode. Takes over from the other bars that read the keys.
    pub fn open_palette(&mut self, mode: PaletteMode) {
        self.palette.open(mode);
        if self.palette.visible {
            self.symbol_picker.visible = false;
            self.goto.visible = false;
        } else {
            self.editor_request.focus = true;
        }
    }

    /// Draws the palette when open, with this frame's commands or files,
    /// and acts on what the user picked.
    fn show_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.visible {
            return;
        }
        let action = match self.palette.mode {
            PaletteMode::Commands => {
                let editor_hidden = self.git_graph.is_some() || self.diff_view.is_some() || self.show_welcome;
                let items: Vec<Item<ShortcutAction>> = palette::COMMANDS
                    .iter()
                    .filter(|c| !(editor_hidden && c.action.needs_editor()))
                    .map(|c| {
                        let label = match c.action {
                            ShortcutAction::SelectTheme(id) => {
                                let name = id.name().unwrap_or_else(|| self.i18n.t("theme.system"));
                                self.i18n.tf(c.key, &[("name", name)])
                            }
                            _ => self.i18n.t(c.key).to_owned(),
                        };
                        let detail = c
                            .action
                            .shortcut()
                            .map(|s| shortcuts::label(ctx, &s))
                            .unwrap_or_default();
                        Item { search: label.clone(), label, detail, payload: c.action }
                    })
                    .collect();
                palette::show(ctx, &mut self.palette, &items, None, &self.i18n, |a| {
                    PaletteAction::Run(*a)
                })
            }
            PaletteMode::Files => {
                let no_folder = self.i18n.t("palette.no_folder");
                let indexing = self.i18n.t("palette.indexing");
                let (items, status): (Vec<Item<PathBuf>>, Option<&str>) = match self.explorer.as_mut() {
                    None => (Vec::new(), Some(no_folder)),
                    Some(explorer) => {
                        let root = explorer.root().to_path_buf();
                        let (files, done) = explorer.indexed_files(ctx);
                        let items = files
                            .iter()
                            .map(|f| {
                                let relative = f.path.strip_prefix(&root).unwrap_or(&f.path);
                                let folder = relative
                                    .parent()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default();
                                Item {
                                    label: f.name.clone(),
                                    detail: folder,
                                    search: relative.display().to_string(),
                                    payload: f.path.clone(),
                                }
                            })
                            .collect();
                        (items, (!done).then_some(indexing))
                    }
                };
                palette::show(ctx, &mut self.palette, &items, status, &self.i18n, |p| {
                    PaletteAction::Open(p.clone())
                })
            }
        };
        match action {
            Some(PaletteAction::Run(action)) => {
                self.palette.close();
                // Commands that open a bar, a dialog or a panel take the
                // keyboard themselves; the rest hand it back to the editor.
                if !action.takes_keyboard() {
                    self.editor_request.focus = true;
                }
                self.apply_shortcut(ctx, action);
            }
            Some(PaletteAction::Open(path)) => {
                self.palette.close();
                self.open_file_in_tab(path);
                self.editor_request.focus = true;
            }
            Some(PaletteAction::Close) => {
                self.palette.close();
                self.editor_request.focus = true;
            }
            None => {}
        }
    }

    /// The identifier at the `char` index `at`, else at the caret, when code
    /// navigation knows the active document's language.
    fn navigation_word(&self, ctx: &egui::Context, at: Option<usize>) -> Option<(String, usize)> {
        if !symbols::navigates(self.effective_language()) {
            return None;
        }
        let at = at.or_else(|| self.editor_selection(ctx).map(|(a, _)| a))?;
        let word = symbols::word_at(&self.active_tab().text_content, at)?;
        Some((word, at))
    }

    /// Jumps to the definition of the identifier at `at` (else the caret's):
    /// in the document when it has one elsewhere, else in the open folder's
    /// files of the same language, listed when there are several.
    pub fn goto_definition(&mut self, ctx: &egui::Context, at: Option<usize>) {
        let Some((name, at)) = self.navigation_word(ctx, at) else { return };
        let language = self.effective_language();
        let text = &self.active_tab().text_content;
        let here = find::line_col(text, at).0;
        let local = symbols::symbols(text, language)
            .into_iter()
            .find(|symbol| symbol.defines(&name) && symbol.line != here);
        if let Some(symbol) = local {
            self.goto_line(symbol.line);
            return;
        }
        match self.explorer.as_ref().map(|e| e.root().to_path_buf()) {
            Some(root) => {
                self.search.visible = true;
                self.search.start_definitions(ctx, root, name, language);
            }
            None => {
                let message = self.i18n.tf("symbol.no_definition", &[("name", &name)]);
                self.status_message = Some((message, true));
            }
        }
    }

    /// Lists every whole-word use of the identifier at `at` (else the
    /// caret's) in the open folder, in the Find in Files panel.
    pub fn find_references(&mut self, ctx: &egui::Context, at: Option<usize>) {
        let Some((name, _)) = self.navigation_word(ctx, at) else { return };
        self.search.visible = true;
        self.search.query = name;
        self.search.case_sensitive = true;
        self.search.whole_word = true;
        if let Some(root) = self.explorer.as_ref().map(|e| e.root().to_path_buf()) {
            self.search.start(ctx, root);
        }
    }

    pub fn open_goto(&mut self) {
        self.goto.visible = true;
        self.goto.focus = true;
        self.goto.input.clear();
    }

    pub fn goto_line(&mut self, line: usize) {
        self.git_graph = None;
        self.diff_view = None;
        let index = find::line_start(&self.active_tab().text_content, line);
        self.goto.visible = false;
        self.editor_request.focus = true;
        self.editor_request.goto = Some(index);
        self.editor_request.reveal = Some((index, index));
    }

    pub fn close_find(&mut self) {
        self.find.visible = false;
        self.find.current = None;
        self.find.matches.clear();
        self.editor_request.focus = true;
    }

    fn refresh_find_matches(&mut self) {
        self.find.matches = find::find_all(
            &self.active_tab().text_content,
            &self.find.query,
            self.find.case_sensitive,
        );
        let matches = &self.find.matches;
        self.find.current = self.find.current.filter(|c| matches.contains(c));
    }

    /// Jumps to the next or previous match, wrapping around.
    pub fn find_step(&mut self, ctx: &egui::Context, action: FindAction) {
        self.refresh_find_matches();
        let caret = self.editor_selection(ctx).map(|(a, _)| a);
        let (after, before) = match self.find.current {
            Some((start, end)) => (end, start),
            None => {
                let from = self.find.anchor.or(caret).unwrap_or(0);
                (from, from)
            }
        };
        let found = match action {
            FindAction::Prev => find::prev_match(&self.find.matches, before),
            // Typing refines the match under the caret instead of skipping it.
            FindAction::Refresh => find::next_match(&self.find.matches, before),
            _ => find::next_match(&self.find.matches, after),
        };
        self.find.current = found;
        self.find.anchor = None;
        self.editor_request.reveal = found;
    }

    pub fn replace_current(&mut self, ctx: &egui::Context) {
        self.refresh_find_matches();
        if let Some(current) = self.find.current {
            let with = self.find.replacement.clone();
            let index = self.active_tab_index;
            find::replace_range(&mut self.tabs[index].text_content, current, &with);
            self.find.current = None;
            self.find.anchor = Some(current.0 + with.chars().count());
        }
        self.find_step(ctx, FindAction::Next);
    }

    pub fn replace_all(&mut self) {
        let index = self.active_tab_index;
        let (text, count) = find::replace_all(
            &self.tabs[index].text_content,
            &self.find.query,
            &self.find.replacement,
            self.find.case_sensitive,
        );
        if count > 0 {
            self.tabs[index].text_content = text;
        }
        self.find.current = None;
        self.find.matches.clear();
        self.status_message = Some((
            self.i18n.tf("msg.replaced", &[("count", &count.to_string())]),
            false,
        ));
    }

    fn apply_find_action(&mut self, ctx: &egui::Context, action: FindAction) {
        match action {
            FindAction::Close => self.close_find(),
            FindAction::Replace => self.replace_current(ctx),
            FindAction::ReplaceAll => self.replace_all(),
            step => self.find_step(ctx, step),
        }
    }

    /// Runs the action of a keyboard shortcut. Ignored while a dialog waits
    /// for an answer.
    pub fn apply_shortcut(&mut self, ctx: &egui::Context, action: ShortcutAction) {
        if self.pending_action.is_some() || self.show_about_modal || self.git_confirm.is_some() || self.pending_delete.is_some() {
            return;
        }
        if (self.git_graph.is_some() || self.diff_view.is_some() || self.show_welcome) && action.needs_editor() {
            return;
        }
        match action {
            ShortcutAction::NewTab => self.open_new_tab(),
            ShortcutAction::NewWindow => self.open_new_window(),
            ShortcutAction::CloseTab => self.trigger_close_tab(self.active_tab_index),
            ShortcutAction::OpenFile => self.trigger_open_file(),
            ShortcutAction::OpenFolder => self.trigger_open_folder(),
            // Nothing to save on the start page.
            ShortcutAction::Save | ShortcutAction::SaveAs if self.show_welcome => {}
            ShortcutAction::Save => {
                self.save();
            }
            ShortcutAction::SaveAs => {
                self.save_as();
            }
            ShortcutAction::Exit => self.trigger_exit(ctx),
            ShortcutAction::ToggleSidebar => self.toggle_sidebar(),
            ShortcutAction::SourceControl => self.toggle_source_control(),
            ShortcutAction::Blame => self.show_blame = !self.show_blame,
            ShortcutAction::GitGraph => self.toggle_git_graph(ctx),
            ShortcutAction::DiffWithHead => self.toggle_diff_view(ctx, Against::Head),
            ShortcutAction::DiffWithFile => self.pick_and_diff_file(ctx),
            ShortcutAction::NextTab => {
                self.active_tab_index = (self.active_tab_index + 1) % self.tabs.len();
            }
            ShortcutAction::GotoLine => self.open_goto(),
            ShortcutAction::Prettify => self.prettify_document(),
            ShortcutAction::ZenMode => self.toggle_zen_mode(ctx),
            ShortcutAction::MarkdownPreview => self.toggle_markdown_preview(),
            ShortcutAction::ToggleConsole => self.toggle_console(ctx),
            ShortcutAction::FindInFiles => self.open_search(ctx),
            ShortcutAction::Find => self.open_find(ctx, false),
            ShortcutAction::Replace => self.open_find(ctx, true),
            ShortcutAction::FindNext => self.find_step(ctx, FindAction::Next),
            ShortcutAction::FindPrev => self.find_step(ctx, FindAction::Prev),
            ShortcutAction::PrevHeading => self.step_heading(false),
            ShortcutAction::NextHeading => self.step_heading(true),
            ShortcutAction::GotoSymbol => self.open_symbol_picker(),
            ShortcutAction::CommandPalette => self.open_palette(PaletteMode::Commands),
            ShortcutAction::QuickOpen => self.open_palette(PaletteMode::Files),
            ShortcutAction::ZoomIn => self.zoom(1),
            ShortcutAction::ZoomOut => self.zoom(-1),
            ShortcutAction::ZoomReset => self.reset_zoom(),
            ShortcutAction::AutoSave => self.toggle_auto_save(),
            ShortcutAction::SplitEditor => self.toggle_split(),
            ShortcutAction::SelectTheme(id) => self.theme = id,
            ShortcutAction::GotoDefinition => self.goto_definition(ctx, None),
            ShortcutAction::FindReferences => self.find_references(ctx, None),
            ShortcutAction::MoveLineUp => {
                self.edit_lines(ctx, |text, sel| line_ops::move_lines(text, sel, true));
            }
            ShortcutAction::MoveLineDown => {
                self.edit_lines(ctx, |text, sel| line_ops::move_lines(text, sel, false));
            }
            ShortcutAction::CopyLineUp => {
                self.edit_lines(ctx, |text, sel| Some(line_ops::copy_lines(text, sel, true)));
            }
            ShortcutAction::CopyLineDown => {
                self.edit_lines(ctx, |text, sel| Some(line_ops::copy_lines(text, sel, false)));
            }
            ShortcutAction::DeleteLine => {
                self.edit_lines(ctx, |text, sel| Some(line_ops::delete_lines(text, sel)));
            }
            ShortcutAction::ToggleComment => self.toggle_comment(ctx),
            ShortcutAction::SelectNextMatch => self.select_next_match(ctx),
            ShortcutAction::GoToBracket => self.go_to_bracket(ctx),
            ShortcutAction::Fold => self.fold_at_caret(ctx, true),
            ShortcutAction::Unfold => self.fold_at_caret(ctx, false),
            ShortcutAction::FoldAll => {
                let tab = self.active_tab_mut();
                tab.sync_folds();
                for (header, _) in tab.foldable.clone() {
                    tab.folds.fold(header);
                }
                self.keep_caret_visible(ctx);
            }
            ShortcutAction::UnfoldAll => self.active_tab_mut().folds.clear(),
            ShortcutAction::PrevTab => {
                self.active_tab_index =
                    (self.active_tab_index + self.tabs.len() - 1) % self.tabs.len();
            }
        }
    }

    pub fn open_new_tab(&mut self) {
        // The blank tab under the start page becomes the new file.
        if self.show_welcome && self.active_tab().is_pristine() {
            self.leave_welcome();
            return;
        }
        self.leave_welcome();
        self.tabs.push(DocumentTab::default_new());
        self.active_tab_index = self.tabs.len() - 1;
    }

    /// File > New Window: starts another copy of the editor, in its own
    /// process, sharing nothing with this window, which is left as it was.
    pub fn open_new_window(&mut self) {
        self.open_new_window_with(|| new_window_command().and_then(spawn_window));
    }

    /// [`Self::open_new_window`] with `spawn` starting the copy; a failure
    /// is reported in the status bar.
    fn open_new_window_with(&mut self, spawn: impl FnOnce() -> std::io::Result<()>) {
        if let Err(err) = spawn() {
            let message = self
                .i18n
                .tf("msg.new_window_failed", &[("error", &err.to_string())]);
            self.status_message = Some((message, true));
        }
    }

    pub fn open_file_in_tab(&mut self, path: PathBuf) {
        self.git_graph = None;
        self.diff_view = None;
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.current_path.as_ref() == Some(&path))
        {
            self.leave_welcome();
            self.active_tab_index = index;
            self.status_message = Some((self.i18n.t("msg.already_open").to_owned(), false));
            self.history.record(path, EntryKind::File);
            return;
        }

        if self.active_tab().is_pristine() {
            self.load_file(path.clone());
            // Only a file actually read leaves the start page.
            if self.active_tab().current_path.as_ref() == Some(&path) {
                self.leave_welcome();
            }
        } else {
            match DocumentTab::from_file(path.clone()) {
                Ok(tab) => {
                    self.leave_welcome();
                    let exceeds = text_content_exceeds_highlight_limit(&tab.text_content);
                    self.tabs.push(tab);
                    self.active_tab_index = self.tabs.len() - 1;
                    self.history.record(path, EntryKind::File);
                    self.status_message = Some((
                        if exceeds {
                            self.i18n.t("msg.too_large").to_owned()
                        } else {
                            self.i18n.t("msg.opened").to_owned()
                        },
                        false,
                    ));
                }
                Err(err) => {
                    self.status_message = Some((self.open_failed_message(&err), true));
                }
            }
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() <= 1 {
            self.tabs[0] = DocumentTab::default_new();
            self.active_tab_index = 0;
            self.show_welcome_page();
        } else {
            self.tabs.remove(index);
            if self.active_tab_index > index {
                self.active_tab_index -= 1;
            } else if self.active_tab_index >= self.tabs.len() {
                self.active_tab_index = self.tabs.len() - 1;
            }
        }
    }

    /// Closes every tab `close` selects, except those with unsaved changes,
    /// which stay so no edit is lost. If the active tab goes, the nearest
    /// survivor at or after `fallback` becomes active. Closing everything
    /// leaves one blank tab.
    pub fn close_tabs_where(&mut self, fallback: usize, close: impl Fn(usize, &DocumentTab) -> bool) {
        let keep: Vec<bool> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| t.is_dirty() || !close(i, t))
            .collect();
        if !keep.contains(&true) {
            self.tabs = vec![DocumentTab::default_new()];
            self.active_tab_index = 0;
            self.show_welcome_page();
            return;
        }
        let new_index = |old: usize| keep[..old].iter().filter(|k| **k).count();
        let target = if keep.get(self.active_tab_index) == Some(&true) {
            self.active_tab_index
        } else {
            (fallback..keep.len())
                .find(|&i| keep[i])
                .or_else(|| keep.iter().rposition(|k| *k))
                .unwrap_or(0)
        };
        self.active_tab_index = new_index(target);
        let mut flags = keep.into_iter();
        self.tabs.retain(|_| flags.next().unwrap_or(true));
    }

    /// Runs a file operation asked for in the Explorer. A delete only
    /// stores the path: the user confirms it first.
    pub fn apply_explorer_action(&mut self, action: ExplorerAction) {
        match action {
            ExplorerAction::Create { dir, name, folder } => {
                let made = if folder {
                    file_ops::create_dir(&dir, &name)
                } else {
                    file_ops::create_file(&dir, &name)
                };
                match made {
                    Ok(path) => {
                        self.after_file_op();
                        if folder {
                            if let Some(explorer) = self.explorer.as_mut() {
                                explorer.expand(&path);
                            }
                        } else {
                            self.open_file_in_tab(path);
                        }
                    }
                    Err(err) => self.file_op_failed(&dir.join(&name), &err),
                }
            }
            ExplorerAction::Rename { from, name } => match file_ops::rename(&from, &name) {
                Ok(to) => {
                    self.rebase_tabs(&from, &to);
                    if let Some(explorer) = self.explorer.as_mut() {
                        explorer.rebase_expanded(&from, &to);
                    }
                    self.after_file_op();
                }
                Err(err) => self.file_op_failed(&from, &err),
            },
            ExplorerAction::Delete(path) => self.pending_delete = Some(path),
            ExplorerAction::Reveal(path) => {
                if let Err(err) = file_ops::reveal(&path) {
                    self.file_op_failed(&path, &err);
                }
            }
        }
    }

    /// Points the tabs open at or under `from` to the same files under
    /// `to`. Their text, undo and bookmarks stay; the language follows a
    /// new extension unless the user picked one, and git is asked for the
    /// new path's HEAD copy.
    pub fn rebase_tabs(&mut self, from: &Path, to: &Path) {
        for tab in &mut self.tabs {
            let Some(path) = tab
                .current_path
                .as_deref()
                .and_then(|path| file_ops::rebase(path, from, to))
            else {
                continue;
            };
            if tab.language_override.is_none() {
                tab.active_language = Language::from_path(&path);
            }
            tab.disk_stamp = disk_stamp(&path);
            tab.git_base = None;
            tab.marks_text = None;
            tab.current_path = Some(path);
        }
    }

    /// Closes the tabs of files at or under `path` (just moved to the
    /// Trash). Tabs with unsaved changes stay, so saving brings them back.
    pub fn close_tabs_under(&mut self, path: &Path) {
        let fallback = self.active_tab_index;
        self.close_tabs_where(fallback, |_, tab| {
            tab.current_path.as_deref().is_some_and(|p| p.starts_with(path))
        });
    }

    /// Asks before moving `pending_delete` to the Trash.
    fn show_delete_confirm(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.pending_delete else { return };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // A link to a folder is removed as a link, not as a folder.
        let folder = fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        let key = if folder {
            "explorer.delete_folder_message"
        } else {
            "explorer.delete_file_message"
        };
        let message = self.i18n.tf(key, &[("name", &name)]);
        let answer = modals::show_confirm_modal(
            ctx,
            self.i18n.t("explorer.delete_title"),
            &message,
            self.i18n.t("explorer.delete_confirm"),
            self.i18n.t("unsaved.cancel"),
        );
        match answer {
            Some(true) => {
                if let Some(path) = self.pending_delete.take() {
                    match file_ops::move_to_trash(&path) {
                        Ok(()) => {
                            self.close_tabs_under(&path);
                            self.after_file_op();
                        }
                        Err(err) => self.file_op_failed(&path, &err),
                    }
                }
            }
            Some(false) => self.pending_delete = None,
            None => {}
        }
    }

    /// The tree and git status after a file was created, renamed or removed.
    fn after_file_op(&mut self) {
        if let Some(explorer) = self.explorer.as_mut() {
            explorer.refresh();
        }
        self.git.request_refresh();
    }

    fn file_op_failed(&mut self, path: &Path, error: &std::io::Error) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let message = self
            .i18n
            .tf("explorer.op_failed", &[("name", &name), ("error", &error.to_string())]);
        self.status_message = Some((message, true));
    }

    pub fn trigger_close_tab(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index) {
            if tab.is_dirty() {
                self.pending_action = Some(PendingAction::CloseTab(index));
            } else {
                self.close_tab(index);
            }
        }
    }

    pub fn load_file(&mut self, path: PathBuf) {
        match read_document(&path) {
            Ok((content, stamp)) => {
                let exceeds_limit = text_content_exceeds_highlight_limit(&content);
                let tab = self.active_tab_mut();
                tab.text_content = content.clone();
                tab.saved_content = content;
                tab.disk_stamp = stamp;
                tab.disk_conflict = false;
                tab.active_language = Language::from_path(&path);
                tab.current_path = Some(path.clone());
                self.history.record(path, EntryKind::File);
                self.status_message = Some((
                    if exceeds_limit {
                        self.i18n.t("msg.too_large").to_owned()
                    } else {
                        self.i18n.t("msg.opened").to_owned()
                    },
                    false,
                ));
            }
            Err(err) => {
                self.status_message = Some((self.open_failed_message(&err), true));
            }
        }
    }

    /// The status bar's message for a file that could not be read.
    fn open_failed_message(&self, error: &std::io::Error) -> String {
        if error.kind() == std::io::ErrorKind::FileTooLarge {
            let limit = (MAX_OPEN_BYTES / (1024 * 1024)).to_string();
            self.i18n.tf("msg.file_too_big", &[("limit", &limit)])
        } else {
            self.i18n
                .tf("msg.open_failed", &[("error", &error.to_string())])
        }
    }

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

    /// The folder whose repository git features act on: the open folder,
    /// else the active file's.
    fn git_dir(&self) -> Option<PathBuf> {
        match &self.explorer {
            Some(explorer) => Some(explorer.root().to_path_buf()),
            None => self.active_tab().current_path.as_deref()?.parent().map(Path::to_path_buf),
        }
    }

    /// Takes in what git reported and tells the user how commands ended.
    pub fn poll_git(&mut self, ctx: &egui::Context) {
        if let Some(dir) = self.git_dir() {
            self.git.open(ctx, dir);
        }
        self.git.poll(ctx);
        for (op, result) in self.git.take_finished() {
            if op.touches_work_tree() {
                self.check_files_on_disk();
            }
            if matches!(op, Op::Commit(_) | Op::CommitAll(_)) && result.is_ok() {
                self.scm.message.clear();
            }
            if matches!(op, Op::Fetch | Op::Pull | Op::Push) {
                self.scm.auth_failed = result == Err(GitError::AuthRequired);
            }
            self.status_message = self.git_outcome_message(&op, result);
        }
        for base in self.git.take_bases() {
            for tab in &mut self.tabs {
                if tab.current_path.as_ref() == Some(&base.path) {
                    tab.git_base = Some(base.clone());
                    tab.marks_text = None;
                }
            }
        }
        self.update_line_marks(ctx);
    }

    /// Keeps the active tab's gutter markers in step with its text, asking
    /// git for HEAD's copy of the file when HEAD moved.
    fn update_line_marks(&mut self, ctx: &egui::Context) {
        let head = self.git.status.as_ref().map(|s| s.head.clone());
        let tab = &self.tabs[self.active_tab_index];
        let path = match (&tab.current_path, self.git.repo(), &head) {
            (Some(path), Some(repo), Some(_)) if repo.contains(path) => path.clone(),
            _ => {
                let tab = &mut self.tabs[self.active_tab_index];
                tab.git_base = None;
                tab.line_marks = LineMarks::default();
                tab.marks_text = None;
                return;
            }
        };
        if tab.git_base.as_ref().is_none_or(|b| Some(&b.head) != head.as_ref()) {
            self.git.request_base(ctx, &path);
        }
        let Some(base) = &tab.git_base else { return };
        if tab.marks_text.as_ref() == Some(&tab.text_content) {
            return;
        }
        if text_content_exceeds_highlight_limit(&tab.text_content) {
            let tab = &mut self.tabs[self.active_tab_index];
            tab.line_marks = LineMarks::default();
            return;
        }
        // While typing, at most a few diffs a second.
        let since = self.last_marks_at.map(|at| at.elapsed());
        if let Some(since) = since.filter(|s| tab.marks_text.is_some() && *s < MARKS_INTERVAL) {
            ctx.request_repaint_after(MARKS_INTERVAL - since);
            return;
        }
        self.last_marks_at = Some(Instant::now());
        // HEAD lacks a new file: all of it is added. Without a change git
        // ignores the file, and nothing is marked.
        let marks = match (&base.text, self.git.change(&path)) {
            (Some(text), _) => line_diff::marks(text, &tab.text_content),
            (None, Some(_)) => line_diff::marks("", &tab.text_content),
            (None, None) => LineMarks::default(),
        };
        let tab = &mut self.tabs[self.active_tab_index];
        tab.line_marks = marks;
        tab.marks_text = Some(tab.text_content.clone());
    }

    /// Starts a git command, unless one is running.
    pub fn run_git(&mut self, ctx: &egui::Context, op: Op) {
        if !self.git.run(ctx, op) {
            self.status_message = Some((self.i18n.t("git.busy").to_owned(), true));
        }
    }

    /// The status bar message for a finished git command; staging is
    /// silent, the lists show it.
    fn git_outcome_message(&self, op: &Op, result: Result<(), GitError>) -> Option<(String, bool)> {
        let message = match result {
            Err(GitError::AuthRequired) => (self.i18n.t("git.auth_required").to_owned(), true),
            Err(GitError::NotInstalled) => (self.i18n.t("git.not_installed").to_owned(), true),
            // `pull --ff-only` refuses to merge.
            Err(GitError::Failed(e)) if *op == Op::Pull && e.contains("fast-forward") => {
                (self.i18n.t("git.pull_diverged").to_owned(), true)
            }
            Err(e) => (self.i18n.tf("git.failed", &[("error", &e.to_string())]), true),
            Ok(()) => {
                let text = match op {
                    Op::Stage(_) | Op::Unstage(_) => return self.status_message.clone(),
                    Op::StageHunk { .. } => self.i18n.t("scm.hunk_staged").to_owned(),
                    Op::Discard(_) => self.i18n.t("git.discarded").to_owned(),
                    Op::Commit(_) | Op::CommitAll(_) => self.i18n.t("git.committed").to_owned(),
                    Op::Fetch => self.i18n.t("git.fetched").to_owned(),
                    Op::Pull => self.i18n.t("git.pulled").to_owned(),
                    Op::Push => self.i18n.t("git.pushed").to_owned(),
                    Op::Switch(b) => self.i18n.tf("git.switched", &[("branch", b)]),
                    Op::CreateBranch(b) => self.i18n.tf("git.branch_created", &[("branch", b)]),
                    Op::CreateBranchAt { name, .. } => self.i18n.tf("git.branch_created", &[("branch", name)]),
                    Op::DeleteBranch(name) => self.i18n.tf("graph.branch_deleted", &[("branch", name)]),
                    Op::Init(_) => self.i18n.t("git.initialized").to_owned(),
                    Op::RestoreFile { .. } => self.i18n.t("history.restored").to_owned(),
                    Op::Checkout(hash) => {
                        self.i18n.tf("history.checked_out", &[("hash", &hash[..hash.len().min(7)])])
                    }
                };
                (text, false)
            }
        };
        Some(message)
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

    pub fn save_tab(&mut self, index: usize) -> bool {
        self.save_tab_with(index, pick_save_path)
    }

    /// [`Self::save_tab`] with the save dialog replaced by `pick_path`, which
    /// returns `None` when the user cancels.
    fn save_tab_with(
        &mut self,
        index: usize,
        pick_path: impl FnOnce(&DocumentTab) -> Option<PathBuf>,
    ) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        if self.tabs[index].current_path.is_some() {
            // Never overwrite what another program wrote unasked: the
            // conflict dialog decides, then the user saves again.
            if self.tabs[index].changed_on_disk() {
                let tab = &mut self.tabs[index];
                tab.disk_conflict = true;
                let name = tab.file_display_name(self.i18n.t("doc.untitled"));
                self.status_message =
                    Some((self.i18n.tf("msg.changed_on_disk", &[("name", &name)]), true));
                return false;
            }
            let warning = self.prepare_for_save(index);
            match self.tabs[index].save() {
                Ok(_) => {
                    self.status_message = Some(self.saved_message(warning));
                    self.git.request_refresh();
                    true
                }
                Err(err) => {
                    self.status_message = Some((self.i18n.tf("msg.save_failed", &[("error", &err.to_string())]), true));
                    false
                }
            }
        } else {
            let old_idx = self.active_tab_index;
            self.active_tab_index = index;
            let result = self.save_as_with(pick_path);
            self.active_tab_index = old_idx;
            result
        }
    }

    /// What an explicit save does to the text first, by the editor
    /// settings: trim trailing whitespace, then run the formatter. The tab's
    /// text is changed in place so the editor shows what gets written.
    /// Returns a warning for the status bar when the formatter failed; the
    /// file is then saved as it is. Auto-save never calls this: it would
    /// strip a space the user just typed.
    fn prepare_for_save(&mut self, index: usize) -> Option<String> {
        let settings = self.editor_settings;
        let tab = &mut self.tabs[index];
        if settings.trim_trailing_whitespace
            && let Some(text) = line_ops::trim_trailing_whitespace(&tab.text_content)
        {
            tab.text_content = text;
        }
        if !settings.format_on_save {
            return None;
        }
        let language = tab.effective_language();
        if !prettify::supports(language) {
            return None;
        }
        match prettify::prettify(language, &tab.text_content) {
            Ok(text) => {
                if text != tab.text_content {
                    tab.text_content = text;
                }
                None
            }
            Err(error) => Some(self.i18n.tf("msg.prettify_failed", &[("error", &error)])),
        }
    }

    /// The status line after a successful write: the formatter's warning
    /// when there is one, so it is not lost behind "Saved".
    fn saved_message(&self, warning: Option<String>) -> (String, bool) {
        match warning {
            Some(warning) => (warning, true),
            None => (self.i18n.t("msg.saved").to_owned(), false),
        }
    }

    pub fn save(&mut self) -> bool {
        self.save_tab(self.active_tab_index)
    }

    pub fn save_as(&mut self) -> bool {
        self.save_as_with(pick_save_path)
    }

    fn save_as_with(&mut self, pick_path: impl FnOnce(&DocumentTab) -> Option<PathBuf>) -> bool {
        if let Some(path) = pick_path(self.active_tab()) {
            let warning = self.prepare_for_save(self.active_tab_index);
            let text_to_save = self.active_tab().text_content.clone();
            match fs::write(&path, &text_to_save) {
                Ok(_) => {
                    let active = self.active_tab_mut();
                    active.saved_content = active.text_content.clone();
                    active.disk_stamp = disk_stamp(&path);
                    active.disk_conflict = false;
                    active.active_language = Language::from_path(&path);
                    active.current_path = Some(path);
                    self.status_message = Some(self.saved_message(warning));
                    true
                }
                Err(err) => {
                    self.status_message = Some((self.i18n.tf("msg.save_failed", &[("error", &err.to_string())]), true));
                    false
                }
            }
        } else {
            false
        }
    }

    /// File > Auto Save: turns saving after the typing pauses on or off,
    /// and forgets a wait that was already running.
    pub fn toggle_auto_save(&mut self) {
        self.auto_save = !self.auto_save;
        self.autosave_due = None;
        // Turning it on with edits already made starts their pause on the
        // next frame, instead of waiting for the next edit.
        self.autosave_dirty = false;
    }

    /// Notes that the document changed, so auto-save waits a whole pause
    /// again.
    fn note_edit(&mut self, ctx: &egui::Context) {
        if !self.auto_save {
            return;
        }
        let now = ctx.input(|input| input.time);
        self.autosave_due = Some(now + AUTOSAVE_SECS);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(AUTOSAVE_SECS));
    }

    /// Auto Save: once the typing pauses, every dirty file with a path of
    /// its own is written; so is every one of them when the window loses
    /// focus.
    ///
    /// A file another program changed is left alone: overwriting it is the
    /// conflict dialog's job, not auto-save's.
    pub fn autosave(&mut self, ctx: &egui::Context) {
        if !self.auto_save {
            self.autosave_due = None;
            self.autosave_dirty = self.tabs.iter().any(DocumentTab::is_dirty);
            self.was_focused = ctx.input(|input| input.focused);
            return;
        }
        let (now, focused) = ctx.input(|input| (input.time, input.focused));
        let dirty = self.tabs.iter().any(DocumentTab::is_dirty);
        // An edit that did not come through the editor (Find and Replace,
        // Prettify) starts the pause too.
        if dirty && !self.autosave_dirty {
            self.note_edit(ctx);
        }
        self.autosave_dirty = dirty;
        if self.was_focused && !focused {
            self.autosave_now();
        }
        self.was_focused = focused;
        let Some(due) = self.autosave_due else { return };
        if now < due {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(due - now));
            return;
        }
        self.autosave_due = None;
        self.autosave_now();
    }

    /// Writes every tab auto-save handles. Returns whether any was written.
    fn autosave_now(&mut self) -> bool {
        let mut saved = false;
        for index in 0..self.tabs.len() {
            let tab = &self.tabs[index];
            let quiet = tab.current_path.is_some()
                && tab.is_dirty()
                && !tab.disk_conflict
                && !tab.changed_on_disk();
            if !quiet {
                continue;
            }
            match self.tabs[index].save() {
                Ok(()) => saved = true,
                Err(err) => {
                    let message =
                        self.i18n.tf("msg.save_failed", &[("error", &err.to_string())]);
                    self.status_message = Some((message, true));
                }
            }
        }
        if saved {
            self.git.request_refresh();
        }
        saved
    }

    pub fn trigger_open_file(&mut self) {
        self.pick_and_open_file();
    }

    pub fn pick_and_open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.open_file_in_tab(path);
        }
    }

    pub fn trigger_open_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.open_folder(folder);
        }
    }

    /// Shows `folder` in the Explorer sidebar and records it in the history.
    ///
    /// The new folder starts clean: saved tabs of the old one are closed
    /// (unsaved ones stay, so no work is lost), the diff view closes with the
    /// tab it showed, and Search in Files forgets its query, options and
    /// results.
    pub fn open_folder(&mut self, folder: PathBuf) {
        self.close_diff_view();
        self.close_tabs_where(0, |_, _| true);
        self.search.stop();
        self.search = SearchState::default();
        self.leave_welcome();
        self.history.record(folder.clone(), EntryKind::Folder);
        self.explorer = Some(Explorer::new(folder));
        self.show_sidebar = true;
        self.sidebar_view = SidebarView::Explorer;
        self.status_message = Some((self.i18n.t("msg.folder_opened").to_owned(), false));
    }

    /// Opens what was dropped on the window: a folder becomes the workspace
    /// and every file opens in a tab.
    ///
    /// Folders go first, so the files dropped beside one land in its
    /// workspace instead of being closed again by the switch to it.
    pub fn open_dropped(&mut self, paths: Vec<PathBuf>) {
        let dropped = drop::Dropped::split(paths);
        if dropped.is_empty() {
            return;
        }
        for folder in dropped.folders {
            self.open_folder(folder);
        }
        for file in dropped.files {
            self.open_file_in_tab(file);
        }
    }

    /// Reopens an entry of File > Open Recent. One no longer there is
    /// dropped from the history, with an error in the status bar.
    pub fn open_recent(&mut self, entry: Entry) {
        if !entry.exists() {
            self.history.remove(&entry.path);
            let path = entry.path.display().to_string();
            let message = self.i18n.tf("msg.recent_missing", &[("path", &path)]);
            self.status_message = Some((message, true));
            return;
        }
        match entry.kind {
            EntryKind::File => self.open_file_in_tab(entry.path),
            EntryKind::Folder => self.open_folder(entry.path),
        }
    }

    pub fn toggle_explorer(&mut self) {
        if self.explorer.is_none() {
            self.trigger_open_folder();
        } else {
            self.toggle_view(SidebarView::Explorer);
        }
    }

    /// The Activity Bar's Source Control item: shows the view, or hides
    /// the sidebar when the view is already shown.
    pub fn toggle_source_control(&mut self) {
        self.toggle_view(SidebarView::SourceControl);
    }

    fn toggle_view(&mut self, view: SidebarView) {
        if self.show_sidebar && self.sidebar_view == view {
            self.show_sidebar = false;
        } else {
            self.show_sidebar = true;
            self.sidebar_view = view;
        }
    }

    /// Ctrl+B: hides the sidebar, or shows it with the view it had. With no
    /// folder open the Explorer asks for one.
    pub fn toggle_sidebar(&mut self) {
        if self.show_sidebar {
            self.show_sidebar = false;
        } else if self.sidebar_view == SidebarView::Explorer {
            self.toggle_explorer();
        } else {
            self.show_sidebar = true;
        }
    }

    /// Shows the commits touching `path` in the Source Control view.
    pub fn open_file_history(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.git.load_log(ctx, Some(path));
        self.show_sidebar = true;
        self.sidebar_view = SidebarView::SourceControl;
        self.scm.commits_open = true;
    }

    /// Brings the start page back, closing the bars of the editor it hides.
    fn show_welcome_page(&mut self) {
        self.show_welcome = true;
        if self.find.visible {
            self.close_find();
        }
        self.goto.visible = false;
        self.symbol_picker.visible = false;
    }

    /// Leaves the start page for the editor, which gets the keyboard.
    fn leave_welcome(&mut self) {
        if std::mem::take(&mut self.show_welcome) {
            self.editor_request = EditorRequest { focus: true, ..Default::default() };
        }
    }

    /// Shows the active document beside `against` in place of the editor,
    /// or hides the view when it already shows that.
    pub fn toggle_diff_view(&mut self, ctx: &egui::Context, against: Against) {
        if self.diff_view.as_ref().is_some_and(|v| v.tab == self.active_tab_index && v.against == against) {
            self.close_diff_view();
            return;
        }
        if self.show_welcome {
            return;
        }
        if against == Against::Head {
            // HEAD's copy is what the gutter compares with; ask for it.
            let tab = self.active_tab();
            match (&tab.current_path, self.git.repo()) {
                (Some(path), Some(repo)) if repo.contains(path) => {
                    if tab.git_base.is_none() {
                        let path = path.clone();
                        self.git.request_base(ctx, &path);
                    }
                }
                _ => {
                    self.status_message = Some((self.i18n.t("diff.no_head").to_owned(), true));
                    return;
                }
            }
        }
        if self.find.visible {
            self.close_find();
        }
        self.goto.visible = false;
        self.symbol_picker.visible = false;
        self.git_graph = None;
        self.hunk_popup = None;
        self.diff_view = Some(DiffViewUi::new(self.active_tab_index, against));
    }

    /// Asks for a file and shows the active document beside it.
    fn pick_and_diff_file(&mut self, ctx: &egui::Context) {
        if self.show_welcome {
            return;
        }
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.active_tab().current_path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.diff_with_file(ctx, path);
        }
    }

    /// Shows the active document beside the file at `path`.
    pub fn diff_with_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        match read_document(&path) {
            Ok((text, _)) => self.toggle_diff_view(ctx, Against::File { path, text }),
            Err(err) => self.status_message = Some((self.open_failed_message(&err), true)),
        }
    }

    pub fn close_diff_view(&mut self) {
        if self.diff_view.take().is_some() {
            self.editor_request = EditorRequest { focus: true, ..Default::default() };
        }
    }

    /// Draws the diff view for the active tab; it closes when another tab
    /// is shown.
    fn show_diff_view(&mut self, ui: &mut egui::Ui) -> Option<DiffViewAction> {
        let mut view = self.diff_view.take()?;
        if view.tab != self.active_tab_index {
            return None;
        }
        let untitled = self.i18n.t("doc.untitled");
        let tab = &self.tabs[self.active_tab_index];
        let right_title = tab.file_display_name(untitled);
        // The file's text moves out for the frame, so the view can be
        // borrowed mutably beside it.
        let file_text = match &mut view.against {
            Against::File { text, .. } => Some(std::mem::take(text)),
            Against::Head => None,
        };
        let (left, left_title) = match (&view.against, &file_text) {
            (Against::File { path, .. }, Some(text)) => (
                diff_view::Left::Text(text.as_str()),
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            ),
            // HEAD lacks a new file: all of it is added.
            _ => {
                let left = match &tab.git_base {
                    Some(base) => diff_view::Left::Text(base.text.as_deref().unwrap_or("")),
                    None => diff_view::Left::Loading,
                };
                (left, format!("{right_title} (HEAD)"))
            }
        };
        let titles = diff_view::Titles { left: &left_title, right: &right_title };
        let action = diff_view::show(ui, &mut view, left, &tab.text_content, &titles, &self.i18n);
        if let (Against::File { text, .. }, Some(file_text)) = (&mut view.against, file_text) {
            *text = file_text;
        }
        self.diff_view = Some(view);
        action
    }

    /// Shows the Git Graph in place of the editor, or hides it again.
    /// Needs a repository; the first opening loads the graph.
    pub fn toggle_git_graph(&mut self, ctx: &egui::Context) {
        if self.git_graph.is_some() {
            self.git_graph = None;
            self.editor_request = EditorRequest { focus: true, ..Default::default() };
            return;
        }
        if self.git.repo().is_none() {
            return;
        }
        if self.find.visible {
            self.close_find();
        }
        self.goto.visible = false;
        self.symbol_picker.visible = false;
        self.ensure_graph_loaded(ctx, true);
        self.git_graph = Some(GitGraphUi::default());
    }

    /// Loads the graph unless it is loaded or loading; a failed load is
    /// tried again only when `retry` (the graph being opened).
    fn ensure_graph_loaded(&mut self, ctx: &egui::Context, retry: bool) {
        let graph = &self.git.graph;
        let untouched = !graph.complete && graph.error.is_none();
        let failed = retry && graph.error.is_some();
        if graph.commits.is_empty() && !graph.loading && (untouched || failed) {
            self.git.load_graph(ctx);
        }
    }

    /// Does what the Git Graph asked.
    fn apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction) {
        match action {
            GraphAction::Close => self.toggle_git_graph(ctx),
            GraphAction::Select(_) => {}
            GraphAction::LoadMore => self.git.load_more_graph(ctx),
            GraphAction::Checkout(hash) => self.git_confirm = Some(GitConfirm::Checkout(hash)),
            GraphAction::CreateBranch { name, hash } => self.run_git(ctx, Op::CreateBranchAt { name, hash }),
            GraphAction::Switch(name) => self.run_git(ctx, Op::Switch(name)),
            GraphAction::DeleteBranch(name) => self.git_confirm = Some(GitConfirm::DeleteBranch(name)),
            GraphAction::CommitDiff { hash, path } => self.git.show_diff(ctx, path, DiffSource::Commit(hash)),
            GraphAction::Copy(text) => ctx.copy_text(text),
        }
    }

    /// Does what the Source Control view asked.
    fn apply_scm_action(&mut self, ctx: &egui::Context, action: ScmAction) {
        match action {
            ScmAction::Run(op) => self.run_git(ctx, op),
            ScmAction::Discard(changes) => self.git_confirm = Some(GitConfirm::Discard(changes)),
            ScmAction::CommitAll(message) => self.git_confirm = Some(GitConfirm::CommitAll(message)),
            ScmAction::Open(path) => self.trigger_open_specific_file(path),
            ScmAction::Diff { path, source } => {
                if path.is_file() {
                    self.trigger_open_specific_file(path.clone());
                }
                self.git.show_diff(ctx, path, source);
            }
            ScmAction::Refresh => self.git.refresh(ctx),
            ScmAction::LoadMoreLog => self.git.load_more_log(ctx),
            ScmAction::ExpandCommit(hash) => self.git.log.expanded = hash,
            ScmAction::ClearHistoryFilter => self.git.load_log(ctx, None),
            ScmAction::Checkout(hash) => self.git_confirm = Some(GitConfirm::Checkout(hash)),
            ScmAction::OpenGraph => {
                if self.git_graph.is_none() {
                    self.toggle_git_graph(ctx);
                }
            }
            ScmAction::OpenConsole => {
                self.scm.auth_failed = false;
                if !self.show_console {
                    self.toggle_console(ctx);
                }
            }
        }
    }

    /// Asks before a git command that waits for confirmation.
    fn show_git_confirm(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.git_confirm else { return };
        let (title, message, yes) = match confirm {
            GitConfirm::Discard(changes) => {
                let message = match changes.as_slice() {
                    [one] => {
                        let name = one.path.file_name().unwrap_or_default().to_string_lossy();
                        self.i18n.tf("scm.discard_one", &[("name", &name)])
                    }
                    many => self.i18n.tf("scm.discard_many", &[("count", &many.len().to_string())]),
                };
                (self.i18n.t("scm.discard_title"), message, self.i18n.t("scm.discard_confirm"))
            }
            GitConfirm::CommitAll(_) => (
                self.i18n.t("scm.commit_all_title"),
                self.i18n.t("scm.commit_all_message").to_owned(),
                self.i18n.t("scm.commit_all_confirm"),
            ),
            GitConfirm::Checkout(hash) => (
                self.i18n.t("history.checkout"),
                self.i18n.tf("history.checkout_message", &[("hash", &hash[..hash.len().min(7)])]),
                self.i18n.t("history.checkout_confirm"),
            ),
            GitConfirm::RestoreFile { hash, path } => {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                (
                    self.i18n.t("history.restore"),
                    self.i18n.tf(
                        "history.restore_message",
                        &[("name", &name), ("hash", &hash[..hash.len().min(7)])],
                    ),
                    self.i18n.t("history.restore_confirm"),
                )
            }
            GitConfirm::DeleteBranch(branch) => (
                self.i18n.t("graph.delete_branch"),
                self.i18n.tf("graph.delete_branch_message", &[("branch", branch)]),
                self.i18n.t("graph.delete_branch_confirm"),
            ),
        };
        let answer = modals::show_confirm_modal(ctx, title, &message, yes, self.i18n.t("unsaved.cancel"));
        match answer {
            Some(true) => {
                let op = match self.git_confirm.take() {
                    Some(GitConfirm::Discard(changes)) => Op::Discard(changes),
                    Some(GitConfirm::CommitAll(message)) => Op::CommitAll(message),
                    Some(GitConfirm::Checkout(hash)) => Op::Checkout(hash),
                    Some(GitConfirm::RestoreFile { hash, path }) => Op::RestoreFile { hash, path },
                    Some(GitConfirm::DeleteBranch(name)) => Op::DeleteBranch(name),
                    None => return,
                };
                self.run_git(ctx, op);
            }
            Some(false) => self.git_confirm = None,
            None => {}
        }
    }

    pub fn trigger_open_specific_file(&mut self, path: PathBuf) {
        self.open_file_in_tab(path);
    }

    pub fn trigger_exit(&mut self, ctx: &egui::Context) {
        if self.any_dirty() {
            self.pending_action = Some(PendingAction::Exit);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn execute_pending_action(&mut self, ctx: &egui::Context, action: PendingAction) {
        match action {
            PendingAction::CloseTab(index) => {
                self.close_tab(index);
            }
            PendingAction::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub fn handle_modal_response(&mut self, ctx: &egui::Context, response: ModalResponse) {
        if let Some(action) = self.pending_action.take() {
            match response {
                ModalResponse::Save => {
                    let is_exit = action == PendingAction::Exit;
                    let saved = match action {
                        PendingAction::CloseTab(idx) => self.save_tab(idx),
                        PendingAction::Exit => {
                            if let Some(idx) = self.tabs.iter().position(|t| t.is_dirty()) {
                                self.save_tab(idx)
                            } else {
                                true
                            }
                        }
                    };
                    if saved {
                        if is_exit {
                            if self.any_dirty() {
                                self.pending_action = Some(PendingAction::Exit);
                            } else {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else {
                            self.execute_pending_action(ctx, action);
                        }
                    }
                }
                ModalResponse::Discard => {
                    self.execute_pending_action(ctx, action);
                }
                ModalResponse::Cancel => {
                    // Action cancelled, do nothing
                }
            }
        }
    }

    /// Draws the tab strip and applies what the user clicked.
    pub fn render_tab_bar(&mut self, ui: &mut egui::Ui) -> tab_bar::TabBarResponse {
        let untitled = self.i18n.t("doc.untitled");
        let views: Vec<tab_bar::TabView> = self.tabs.iter().map(|t| t.tab_view(untitled)).collect();
        let labels = tab_bar::MenuLabels {
            close: self.i18n.t("file.close_tab"),
            close_others: self.i18n.t("tab.close_others"),
            close_to_right: self.i18n.t("tab.close_to_right"),
            close_saved: self.i18n.t("tab.close_saved"),
            file_history: self.i18n.t("history.file_history"),
            copy_path: self.i18n.t("tab.copy_path"),
        };
        let response = tab_bar::show(ui, &views, self.active_tab_index, &labels);
        match response.action {
            Some(tab_bar::TabBarAction::Select(index)) => {
                self.git_graph = None;
                self.diff_view = None;
                self.active_tab_index = index;
            }
            Some(tab_bar::TabBarAction::Close(index)) => self.trigger_close_tab(index),
            Some(tab_bar::TabBarAction::CloseOthers(index)) => {
                self.close_tabs_where(index, |i, _| i != index)
            }
            Some(tab_bar::TabBarAction::CloseToRight(index)) => {
                self.close_tabs_where(index, |i, _| i > index)
            }
            Some(tab_bar::TabBarAction::CloseSaved) => {
                self.close_tabs_where(self.active_tab_index, |_, _| true)
            }
            Some(tab_bar::TabBarAction::FileHistory(index)) => {
                if let Some(path) = self.tabs.get(index).and_then(|t| t.current_path.clone()) {
                    self.open_file_history(&ui.ctx().clone(), path);
                }
            }
            Some(tab_bar::TabBarAction::CopyPath(index)) => {
                if let Some(path) = self.tabs.get(index).and_then(|t| t.current_path.as_ref()) {
                    ui.ctx().copy_text(path.display().to_string());
                }
            }
            Some(tab_bar::TabBarAction::New) => self.open_new_tab(),
            None => {}
        }
        response
    }
}

/// The command that starts another copy of this editor.
fn new_window_command() -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command.stdin(Stdio::null());
    Ok(command)
}

/// Runs `command` without waiting for it. A thread reaps the process when
/// it exits, so it never lingers as a zombie while this window is open.
fn spawn_window(mut command: Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The history eframe kept from the last run; empty on the first.
fn saved_history(storage: Option<&dyn eframe::Storage>) -> History {
    storage
        .and_then(|storage| storage.get_string(HISTORY_KEY))
        .map(|text| History::from_storage(&text))
        .unwrap_or_default()
}

/// The folder and files open when the editor last quit; empty on the first
/// run.
fn saved_session(storage: Option<&dyn eframe::Storage>) -> Session {
    storage
        .and_then(|storage| storage.get_string(SESSION_KEY))
        .map(|text| Session::from_storage(&text))
        .unwrap_or_default()
}

/// The theme eframe kept from the last run; `System` on the first.
fn saved_theme(storage: Option<&dyn eframe::Storage>) -> ThemeId {
    storage
        .and_then(|storage| storage.get_string(THEME_KEY))
        .map(|slug| ThemeId::from_slug(&slug))
        .unwrap_or_default()
}

/// The font size eframe kept from the last run; the default on the first.
fn saved_font_size(storage: Option<&dyn eframe::Storage>) -> f32 {
    zoom::from_storage(
        storage
            .and_then(|storage| storage.get_string(FONT_SIZE_KEY))
            .as_deref(),
    )
}

/// Whether auto-save was on when eframe last saved; off on the first run.
fn saved_auto_save(storage: Option<&dyn eframe::Storage>) -> bool {
    storage
        .and_then(|storage| storage.get_string(AUTO_SAVE_KEY))
        .is_some_and(|text| text == "true")
}

/// The editor settings eframe last saved; the defaults on the first run.
fn saved_editor_settings(storage: Option<&dyn eframe::Storage>) -> EditorSettings {
    let text = storage.and_then(|storage| storage.get_string(EDITOR_SETTINGS_KEY));
    EditorSettings::from_storage(text.as_deref())
}

/// Where egui's memory keeps the font size the app last wrote, so it is
/// only written when it changed.
fn font_size_id() -> egui::Id {
    egui::Id::unique("rred_font_size")
}

/// The request for the editor view without the keyboard: it draws the same
/// folds and find highlight, but carries none of the user's actions.
fn quiet_request(request: &EditorRequest) -> EditorRequest {
    EditorRequest {
        focus: false,
        key: None,
        goto: None,
        select: None,
        inactive_selection: None,
        reveal: None,
        highlight: request.highlight,
        auto_edit: None,
        editor_settings: request.editor_settings,
        match_brackets: request.match_brackets,
        hidden: request.hidden.clone(),
        foldable: request.foldable.clone(),
    }
}

/// Asks the user where to save `tab`, starting in its current folder.
/// `None` means the dialog was cancelled.
fn pick_save_path(tab: &DocumentTab) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new();
    let dialog = match tab.current_path.as_ref().and_then(|path| path.parent()) {
        Some(parent) => dialog.set_directory(parent),
        None => dialog,
    };
    dialog.save_file()
}

/// What the blame column shows for the active tab.
#[derive(Clone, Copy)]
pub struct BlameColumn<'a> {
    /// `None` while loading, or for a note instead (unsaved edits).
    pub blame: Option<&'a Blame>,
    /// Shown instead of the blame, when there is none to show.
    pub note: &'a str,
    pub now: i64,
}

impl eframe::App for TextEditorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(UI_LANGUAGE_KEY, self.i18n.code().to_owned());
        storage.set_string(HISTORY_KEY, self.history.to_storage());
        storage.set_string(THEME_KEY, self.theme.slug().to_owned());
        storage.set_string(FONT_SIZE_KEY, zoom::to_storage(self.font_size));
        storage.set_string(AUTO_SAVE_KEY, self.auto_save.to_string());
        storage.set_string(EDITOR_SETTINGS_KEY, self.editor_settings.to_storage());
        storage.set_string(SESSION_KEY, Session::from_app(self).to_storage());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let theme = theme::apply(&ctx, self.theme);
        self.highlighter.set_theme(theme.syntax);
        // `Ctrl` + scroll zooms the text. Taken before the editor is drawn,
        // so the new size is used in this very frame.
        self.zoom_with_wheel(&ctx);
        self.apply_font_size(ui);
        // The decorations the editor paints over the text, from View.
        editor_view::set_decorations(
            &ctx,
            editor_view::Decorations {
                indent_guides: self.show_indent_guides,
                whitespace: self.show_whitespace,
            },
        );
        self.autosave(&ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
        self.poll_disk(&ctx);
        // Files dropped on the window open in tabs, a folder as the
        // workspace. Read before git is polled, so the new folder's
        // repository is looked up in this very frame.
        let dropped = drop::paths(&ctx);
        if !dropped.is_empty() {
            self.open_dropped(dropped);
        }
        drop::show_hint(&ctx, &self.i18n);
        self.poll_git(&ctx);
        // A new repository replaces the graph: load it again, or close the
        // graph when there is no repository left.
        if self.git_graph.is_some() {
            if self.git.repo().is_some() {
                self.ensure_graph_loaded(&ctx, false);
            } else if !self.git.is_discovering() {
                self.toggle_git_graph(&ctx);
            }
        }

        // While the Console has focus the shell gets every key, Escape
        // included, but the shortcut that hides it.
        let console_focused =
            self.show_console && ctx.memory(|m| m.has_focus(terminal::widget_id()));
        let shortcut = if console_focused {
            ctx.input_mut(|i| i.consume_shortcut(&shortcuts::TOGGLE_CONSOLE))
                .then_some(ShortcutAction::ToggleConsole)
        } else {
            ctx.input_mut(consume_shortcut)
        };
        if let Some(action) = shortcut {
            self.apply_shortcut(&ctx, action);
        }

        // The palette floats over every bar, so its Escape comes first.
        if self.palette.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.palette.close();
            self.editor_request.focus = true;
        }
        if !console_focused
            && self.goto.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.goto.visible = false;
            self.editor_request.focus = true;
        }
        if !console_focused
            && self.search.visible
            && !self.find.visible
            && !self.goto.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_search();
        }
        if !console_focused
            && self.find.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_find();
        }
        if let Some(graph) = &self.git_graph
            && graph.new_branch.is_none()
            && !console_focused
            && !self.find.visible
            && !self.goto.visible
            && !self.search.visible
            && self.git_confirm.is_none()
            && !self.show_about_modal
            && self.pending_action.is_none()
            && !self.active_tab().disk_conflict
            && !egui::Popup::is_any_open(&ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.toggle_git_graph(&ctx);
        }
        if self.diff_view.is_some()
            && !console_focused
            && !self.palette.visible
            && self.git_confirm.is_none()
            && !self.show_about_modal
            && self.pending_action.is_none()
            && !egui::Popup::is_any_open(&ctx)
            && !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_diff_view();
        }
        if self.zen_mode
            && !self.find.visible
            && !self.goto.visible
            && !self.search.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.toggle_zen_mode(&ctx);
        }
        if self.find.visible {
            self.refresh_find_matches();
        }

        // Read before the menu bar: a click on it takes the editor's focus.
        let editor_had_focus = ctx.memory(|m| m.has_focus(self.editor_id()));

        // 1. Top Panel (Menu Bar & Toolbar)
        if !self.zen_mode {
        let menu_bar = egui::Panel::top("top_panel").show(ui, |ui| {
            ui.horizontal(|ui| {
                let [file, view, help] =
                    ["menu.file", "menu.view", "menu.help"].map(|key| self.i18n.t(key).to_owned());

                // Menu File
                ui.menu_button(file, |ui| {
                    self.show_file_menu(ui, &ctx);
                });

                let edit = self.i18n.t("menu.edit").to_owned();
                ui.menu_button(edit, |ui| {
                    self.show_edit_menu(ui, &ctx);
                });

                // Menu Tampilan
                ui.menu_button(view, |ui| {
                    self.show_view_menu(ui);
                });

                // Menu Help
                ui.menu_button(help, |ui| {
                    self.show_help_menu(ui);
                });


                // Info Karakter di sisi kanan
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = self.active_tab().text_content.len().to_string();
                    ui.label(self.i18n.tf("toolbar.characters", &[("count", &count)]));
                });
            });
        });
        self.note_menu_click(&ctx, editor_had_focus, menu_bar.response.rect);

        // 2. Bottom Status Bar Panel
        }
        if !self.zen_mode {
            egui::Panel::bottom("status_bar").show(ui, |ui| {
                self.show_status_bar(ui);
            });
        }

        if self.search.visible {
            self.search.poll();
            // Go to Definition found one place: go there instead of listing it.
            if let Some(hit) = self.search.take_single() {
                self.close_search();
                self.open_search_hit(hit.path, hit.line);
            }
        }
        if self.search.visible && !self.zen_mode {
            let root = self.explorer.as_ref().map(|e| e.root().to_path_buf());
            let mut action = None;
            egui::Panel::bottom("find_in_files")
                .resizable(true)
                .default_size(180.0)
                .show(ui, |ui| {
                    action = search::show_panel(ui, &mut self.search, root.as_deref(), &self.i18n);
                });
            match (action, root) {
                (Some(SearchAction::Run), Some(root)) => self.search.start(&ctx, root),
                (Some(SearchAction::Open { path, line }), _) => self.open_search_hit(path, line),
                (Some(SearchAction::Close), _) => self.close_search(),
                _ => {}
            }
        }

        // 3. Activity Bar and Explorer sidebar. Both paint their own right
        // border, so egui's separator lines are turned off.
        let side_fill = theme::active(&ctx).sidebar.background;
        let side_frame = egui::Frame::NONE.fill(side_fill);
        let explorer_open =
            self.show_sidebar && self.sidebar_view == SidebarView::Explorer && self.explorer.is_some();
        let source_control_open =
            self.show_sidebar && self.sidebar_view == SidebarView::SourceControl;
        if !self.zen_mode {
            let activity = egui::Panel::left("activity_bar")
                .exact_size(activity_bar::WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(side_frame)
                .show(ui, |ui| {
                    let items = activity_bar::ActivityItems {
                        explorer_open,
                        search_open: self.search.visible,
                        source_control_open,
                        console_open: self.show_console,
                        explorer_tooltip: self.i18n.t("activity.explorer"),
                        search_tooltip: self.i18n.t("edit.find_in_files"),
                        source_control_tooltip: self.i18n.t("activity.source_control"),
                        console_tooltip: self.i18n.t("activity.console"),
                        changes: self.git.change_count(),
                    };
                    activity_bar::show(ui, items)
                })
                .inner;
            if activity.explorer_clicked {
                self.toggle_explorer();
            }
            if activity.search_clicked {
                if self.search.visible {
                    self.close_search();
                } else {
                    self.open_search(&ctx);
                }
            }
            if activity.source_control_clicked {
                self.toggle_source_control();
            }
            if activity.console_clicked {
                self.toggle_console(&ctx);
            }
        }

        let mut file_to_open = None;
        let mut file_history = None;
        let mut explorer_action = None;
        let mut find_in_files = false;
        let active_file = self.active_tab().current_path.clone();
        let mut scm_action = None;
        if self.show_sidebar && !self.zen_mode && self.sidebar_view == SidebarView::SourceControl {
            let folder = self.git_dir();
            egui::Panel::left("source_control_panel")
                .resizable(true)
                .default_size(240.0)
                .min_size(170.0)
                .max_size(500.0)
                .show_separator_line(false)
                .frame(side_frame)
                .show(ui, |ui| {
                    scm_action = source_control::show(
                        ui,
                        &mut self.git,
                        &mut self.scm,
                        folder.as_deref(),
                        &self.i18n,
                    );
                });
        }
        if let Some(action) = scm_action {
            self.apply_scm_action(&ctx, action);
        }
        #[allow(clippy::collapsible_if)]
        if self.show_sidebar && !self.zen_mode && self.sidebar_view == SidebarView::Explorer {
            if let Some(explorer) = self.explorer.as_mut() {
                egui::Panel::left("file_browser_panel")
                    .resizable(true)
                    .default_size(240.0)
                    .min_size(170.0)
                    .max_size(500.0)
                    .show_separator_line(false)
                    .frame(side_frame)
                    .show(ui, |ui| {
                        let tree =
                            file_browser::show(
                            ui,
                            explorer,
                            active_file.as_deref(),
                            Some(&self.git.decorations),
                            &self.i18n,
                        );
                        file_to_open = tree.open_file;
                        file_history = tree.file_history;
                        find_in_files = tree.find_in_files;
                        explorer_action = tree.action;
                    });
            }
        }

        if let Some(path) = file_to_open {
            self.trigger_open_specific_file(path);
        }
        if let Some(path) = file_history {
            self.open_file_history(&ctx, path);
        }
        if find_in_files {
            self.open_search(&ctx);
        }
        if let Some(action) = explorer_action {
            self.apply_explorer_action(action);
        }

        // Console panel, below the editor and the Markdown preview: declared
        // after the side panels so it does not run under them.
        if self.show_console && !self.zen_mode {
            self.show_console_panel(ui);
        }
        // Git Log panel, above the Console when both are shown.
        if self.show_git_log && !self.zen_mode && self.git_log.show(ui, &self.i18n) {
            self.show_git_log = false;
            self.editor_request.focus = true;
        }

        // A file's git diff, right of the editor, in place of the Markdown
        // preview.
        let mut diff_action = None;
        if !self.zen_mode
            && let Some(diff) = self.git.diff()
        {
            let half = ui.available_width() / 2.0;
            egui::Panel::right("git_diff")
                .resizable(true)
                .default_size(half)
                .min_size(200.0)
                .show(ui, |ui| {
                    diff_action = source_control::show_diff(ui, diff, &self.i18n);
                });
        }
        match diff_action {
            Some(DiffAction::Close) => self.git.close_diff(),
            Some(DiffAction::Restore { hash, path }) => {
                self.git_confirm = Some(GitConfirm::RestoreFile { hash, path });
            }
            Some(DiffAction::Goto(line)) => {
                let path = self.git.diff().map(|d| d.path.clone());
                if let Some(path) = path.filter(|p| self.active_tab().current_path.as_ref() != Some(p)) {
                    self.trigger_open_specific_file(path);
                }
                self.goto_line(line);
            }
            None => {}
        }

        // Markdown preview, right of the editor. Declared before the central
        // panel so the editor takes whatever width is left.
        let previewing = self.previewing();
        let mut preview_pos = None;
        let mut preview_jump = None;
        if previewing {
            let half = ui.available_width() / 2.0;
            egui::Panel::right("markdown_preview")
                .resizable(true)
                .default_size(half)
                .min_size(200.0)
                .show(ui, |ui| {
                    let text = &self.tabs[self.active_tab_index].text_content;
                    let scroll_to = self.scroll_sync.take_preview_offset();
                    let shown = self.markdown_preview.show(ui, text, &self.i18n, scroll_to);
                    preview_pos = Some(shown.pos);
                    preview_jump = shown.jumped_to_line;
                });
        }
        // The editor follows a jump to a heading in the preview.
        if let Some(line) = preview_jump {
            self.goto_line(line);
        }

        // 4. Central Panel (Main Text Editor Area)
        // No margin and the active tab's colour, so the active tab and the
        // editor below it read as one surface, as in VS Code.
        let editor_fill = theme::active(&ctx).tab.tab_active;
        let editor_frame = egui::Frame::NONE.fill(editor_fill);
        let mut graph_action = None;
        let mut diff_action = None;
        let mut welcome_action = None;
        egui::CentralPanel::default().frame(editor_frame).show(ui, |ui| {
            // The start page has no tabs to show; the Git Graph wins over it.
            if self.show_welcome && self.git_graph.is_none() {
                welcome_action = welcome::show(ui, &self.history, &self.i18n);
                self.scroll_sync.reset();
                return;
            }
            if !self.zen_mode {
                self.render_tab_bar(ui);
            }
            if let Some(mut graph) = self.git_graph.take() {
                graph_action = git_graph::show(ui, &mut graph, &mut self.git, &self.i18n);
                self.git_graph = Some(graph);
                self.scroll_sync.reset();
                return;
            }
            if self.diff_view.is_some() {
                diff_action = self.show_diff_view(ui);
                self.scroll_sync.reset();
                return;
            }

            if self.find.visible
                && let Some(action) = find::show_bar(ui, &mut self.find, &self.i18n)
            {
                self.apply_find_action(&ctx, action);
            }
            if self.goto.visible {
                let lines = find::line_count(&self.active_tab().text_content);
                match find::show_goto_bar(ui, &mut self.goto, lines, &self.i18n) {
                    Some(GotoAction::Go(line)) => self.goto_line(line),
                    Some(GotoAction::Close) => {
                        self.goto.visible = false;
                        self.editor_request.focus = true;
                    }
                    None => {}
                }
            }
            if self.symbol_picker.visible {
                match symbols::show_picker(ui, &mut self.symbol_picker, &self.i18n) {
                    Some(PickerAction::Go(line)) => {
                        self.symbol_picker.visible = false;
                        self.goto_line(line);
                    }
                    Some(PickerAction::Close) => {
                        self.symbol_picker.visible = false;
                        self.editor_request.focus = true;
                    }
                    None => {}
                }
            }
            let mut request = std::mem::take(&mut self.editor_request);
            request.highlight = self.find.visible.then_some(self.find.current).flatten();
            request.inactive_selection = (self.menu_focus == Some(self.editor_id()))
                .then(|| self.saved_selection(&ctx))
                .flatten();
            let (main_id, split_id) = (
                self.editor_id_of(Pane::Main),
                self.editor_id_of(Pane::Split),
            );
            let keyboard_pane = if self.split { self.focused_pane } else { Pane::Main };

            // `TextEdit::font` resolves this `TextStyle` the same way, so the
            // caret and selection line up with the highlighted glyphs.
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let too_large = text_content_exceeds_highlight_limit(&self.active_tab().text_content);

            let word_wrap = self.word_wrap;
            let line_numbers = self.show_line_numbers;
            let hint = self.i18n.t("editor.placeholder");
            let active_idx = self.active_tab_index;
            let language = self.tabs[active_idx].effective_language();
            request.auto_edit = Some(language);
            request.editor_settings = self.editor_settings;
            request.match_brackets = true;
            if !too_large {
                let tab = &mut self.tabs[active_idx];
                request.hidden = tab.hidden_ranges().to_vec();
                request.foldable = tab.foldable.clone();
            }

            // A tab the session restored opens at the caret and the offset
            // it was left at. Only on its first frame: egui keeps both from
            // then on, and the user's own request for this frame wins.
            let restored_view = if self.tabs[active_idx].restore_view
                && request.select.is_none()
                && request.goto.is_none()
            {
                self.tabs[active_idx].restore_view = false;
                Some((self.tabs[active_idx].caret, self.tabs[active_idx].scroll))
            } else {
                None
            };
            if let Some((caret, _)) = restored_view {
                request.select = Some((caret, caret));
            }

            let scroll_to = if previewing {
                self.scroll_sync.take_editor_offset()
            } else {
                restored_view.map(|(_, scroll)| scroll)
            };
            // Blame only speaks for the committed text: with unsaved edits
            // the lines would not line up, so a note takes its place.
            let blame_now = blame::now();
            let blame_loaded;
            let blame_column = if self.show_blame {
                let tab = &self.tabs[active_idx];
                match &tab.current_path {
                    Some(_) if tab.is_dirty() => Some(BlameColumn {
                        blame: None,
                        note: self.i18n.t("blame.save_first"),
                        now: blame_now,
                    }),
                    Some(path) if self.git.repo().is_some_and(|r| r.contains(path)) => {
                        let path = path.clone();
                        blame_loaded = self.git.blame(&ctx, &path).cloned();
                        Some(BlameColumn {
                            blame: blame_loaded.as_ref(),
                            note: self.i18n.t("blame.loading"),
                            now: blame_now,
                        })
                    }
                    _ => None,
                }
            } else {
                None
            };
            // The document, once; or twice side by side when the split is
            // on. Both views read the same tab, so an edit in either shows
            // in both; the typing goes to the view with the keyboard.
            let mut draw = |ui: &mut egui::Ui,
                            pane: Pane,
                            id: egui::Id,
                            request: EditorRequest,
                            scroll_to: Option<f32>| {
                let tab = &mut self.tabs[active_idx];
                let layout = match pane {
                    Pane::Main => &mut self.editor_layout,
                    Pane::Split => &mut self.editor_layout_split,
                };
                // Blame is one column at the left of the editor; the second
                // view leaves it out.
                let blame = if pane == Pane::Main { blame_column } else { None };
                if too_large {
                    render_editor_with(
                        ui,
                        &mut tab.text_content,
                        None,
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(id),
                        request,
                        scroll_to,
                        &mut tab.bookmarks,
                        &tab.line_marks,
                        blame,
                        layout,
                    )
                } else {
                    let mut layouter = self
                        .highlighter
                        .layouter(language, font_id.clone())
                        .with_document_cache(&mut tab.highlight_cache);
                    render_editor_with(
                        ui,
                        &mut tab.text_content,
                        Some(&mut layouter),
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(id),
                        request,
                        scroll_to,
                        &mut tab.bookmarks,
                        &tab.line_marks,
                        blame,
                        layout,
                    )
                }
            };
            let mut drawn: Vec<(Pane, (ScrollPos, egui::Response, FoldOutput))> =
                Vec::with_capacity(2);
            if self.split {
                let full = ui.available_rect_before_wrap();
                let half = ((full.width() - SPLIT_GAP) * 0.5).max(0.0);
                let left = egui::Rect::from_min_size(full.min, egui::vec2(half, full.height()));
                let right = egui::Rect::from_min_size(
                    egui::pos2(full.min.x + half + SPLIT_GAP, full.min.y),
                    egui::vec2((full.width() - half - SPLIT_GAP).max(0.0), full.height()),
                );
                // The seam between the two views.
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(left.right(), full.top()),
                        egui::vec2(SPLIT_GAP, full.height()),
                    ),
                    0.0,
                    theme::active(&ctx).tab.border,
                );
                for (pane, rect, id) in [
                    (Pane::Main, left, main_id),
                    (Pane::Split, right, split_id),
                ] {
                    let (pane_request, pane_scroll) = if pane == keyboard_pane {
                        (request.clone(), scroll_to)
                    } else {
                        (quiet_request(&request), None)
                    };
                    let painted = ui
                        .scope_builder(
                            egui::UiBuilder::new()
                                .id_salt((pane, active_idx))
                                .max_rect(rect),
                            |ui| draw(ui, pane, id, pane_request, pane_scroll),
                        )
                        .inner;
                    drawn.push((pane, painted));
                }
            } else {
                let full = ui.available_rect_before_wrap();
                let painted = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt((Pane::Main, active_idx))
                            .max_rect(full),
                        |ui| draw(ui, Pane::Main, main_id, request, scroll_to),
                    )
                    .inner;
                drawn.push((Pane::Main, painted));
            }
            // Which view holds the keyboard: the next frame's requests, the
            // status bar's caret and the menus act on it. When neither does
            // (a menu or a dialog has it), the view the app was acting on
            // keeps it, so the focus comes back to the right one.
            if self.split {
                let (main, side) = ctx.memory(|memory| {
                    (memory.has_focus(main_id), memory.has_focus(split_id))
                });
                if side {
                    self.focused_pane = Pane::Split;
                } else if main {
                    self.focused_pane = Pane::Main;
                }
            }
            let focused = if self.split { self.focused_pane } else { Pane::Main };
            // Where the caret and the view are, for the session: egui keeps
            // them in its memory, which does not outlive the process.
            let focused_id = if focused == Pane::Split { split_id } else { main_id };
            let tab = &mut self.tabs[active_idx];
            if let Some(caret) = egui::TextEdit::load_state(&ctx, focused_id)
                .and_then(|state| state.cursor.char_range())
                .map(|range| range.primary.index.0)
            {
                tab.caret = caret;
            }
            if let Some((pos, _, _)) = drawn.iter().find(|(pane, _)| *pane == focused).map(|(_, drawn)| drawn) {
                tab.scroll = pos.offset;
            }
            for (pane, (_, editor, folds)) in &drawn {
                // A caret a fold just hid moves out of it in the view the
                // typing goes to; the other view keeps the document folded.
                if let Some(line) = folds.reveal.filter(|_| *pane == focused) {
                    let tab = &mut self.tabs[active_idx];
                    tab.folds.reveal_line(&tab.text_content, line);
                }
                if let Some(header) = folds.toggle {
                    self.tabs[active_idx].folds.toggle(header);
                    self.keep_caret_visible(&ctx);
                }
                if let Some(line) = folds.clicked_change {
                    let pos = ctx
                        .input(|input| input.pointer.interact_pos())
                        .unwrap_or(editor.rect.left_top());
                    let popup = HunkPopup {
                        tab: active_idx,
                        line,
                        pos: pos + egui::vec2(8.0, 4.0),
                    };
                    // A second click on the same marker closes it.
                    self.hunk_popup = (self.hunk_popup.as_ref().map(|p| (p.tab, p.line))
                        != Some((active_idx, line)))
                    .then_some(popup);
                }
                self.show_editor_context_menu(&ctx, editor);
                if editor.changed() {
                    // The typing pauses from now.
                    self.note_edit(&ctx);
                }
                if editor.clicked() && ctx.input(|input| input.modifiers.command) {
                    self.goto_definition(&ctx, None);
                }
            }
            match preview_pos {
                Some(preview_pos) => {
                    if let Some((pos, _, _)) =
                        drawn.iter().find(|(pane, _)| *pane == Pane::Main).map(|(_, drawn)| drawn)
                    {
                        self.scroll_sync.observe(*pos, preview_pos);
                    }
                }
                None => self.scroll_sync.reset(),
            }
        });
        self.show_palette(&ctx);
        match diff_action {
            Some(DiffViewAction::Close) => self.close_diff_view(),
            Some(DiffViewAction::Goto(line)) => self.goto_line(line),
            None => {}
        }
        if let Some(action) = graph_action {
            self.apply_graph_action(&ctx, action);
        }
        if let Some(action) = welcome_action {
            match action {
                WelcomeAction::NewFile => self.open_new_tab(),
                WelcomeAction::OpenFile => self.trigger_open_file(),
                WelcomeAction::OpenFolder => self.trigger_open_folder(),
                WelcomeAction::Open(entry) => self.open_recent(entry),
            }
        }

        // 5. Modals & Dialogs
        about::show(&ctx, &mut self.show_about_modal, &mut self.about, &self.i18n);

        self.show_bookmarks_window(&ctx);
        self.show_hunk_popup(&ctx);

        #[allow(clippy::collapsible_if)]
        if self.pending_action.is_some() {
            let prompt_name = match self.pending_action {
                Some(PendingAction::CloseTab(idx)) => self
                    .tabs
                    .get(idx)
                    .map(|t| t.file_display_name(self.i18n.t("doc.untitled")))
                    .unwrap_or_else(|| self.file_display_name()),
                Some(PendingAction::Exit) => self
                    .tabs
                    .iter()
                    .find(|t| t.is_dirty())
                    .map(|t| t.file_display_name(self.i18n.t("doc.untitled")))
                    .unwrap_or_else(|| self.file_display_name()),
                _ => self.file_display_name(),
            };
            if let Some(response) = modals::show_unsaved_changes_modal(&ctx, &prompt_name, &self.i18n) {
                self.handle_modal_response(&ctx, response);
            }
        } else if self.active_tab().disk_conflict {
            let name = self.file_display_name();
            if let Some(choice) = modals::show_disk_conflict_modal(&ctx, &name, &self.i18n) {
                self.resolve_disk_conflict(choice);
            }
        } else if self.git_confirm.is_some() {
            self.show_git_confirm(&ctx);
        } else if self.pending_delete.is_some() {
            self.show_delete_confirm(&ctx);
        }

        self.return_focus_after_menu(&ctx);

        // The status bar is drawn before the editor moves the caret and its
        // edits move the bookmarks; draw the frame again when it is stale.
        if !self.zen_mode
            && (self.caret_position(&ctx) != self.status_position
                || self.active_tab().bookmarks.len() != self.status_bookmarks)
        {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// One frame rendering `body` with the given input events, discarding
    /// texture deltas. Returns the painted shapes.
    pub(crate) fn run_frame_with_events(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> Vec<egui::epaint::ClippedShape> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| body(ui));
        output.textures_delta.clear();
        output.shapes
    }

    /// Moves the pointer to `pos` and clicks the primary button there.
    pub(crate) fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    /// Where the editor painted `text`, as `(galley rect, clip rect)` in screen
    /// coordinates, or `None` if it was not painted at all.
    pub(crate) fn painted_text(
        shapes: &[egui::epaint::ClippedShape],
        text: &str,
    ) -> Option<(egui::Rect, egui::Rect)> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => Some((
                egui::Rect::from_min_size(shape.pos, shape.galley.size()),
                clipped.clip_rect,
            )),
            _ => None,
        })
    }

    /// A fresh folder `rred_history_<name>` in the temp directory holding
    /// `a.txt` and `b.txt`.
    pub(crate) fn history_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_history_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        dir
    }

    /// Where the gutter painted the number `n`, and in which colour.
    pub(crate) fn painted_number(
        shapes: &[egui::epaint::ClippedShape],
        n: usize,
    ) -> Option<(egui::Rect, egui::Color32)> {
        let label = n.to_string();
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == label => Some((
                egui::Rect::from_min_size(shape.pos, shape.galley.size()),
                shape.galley.job.sections[0].format.color,
            )),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn test_is_dirty_detection() {
        let mut app = TextEditorApp::default();
        assert!(!app.is_dirty());

        app.active_tab_mut().text_content = "Berubah".to_string();
        assert!(app.is_dirty());

        app.active_tab_mut().saved_content = "Berubah".to_string();
        assert!(!app.is_dirty());
    }

    #[test]
    fn test_window_title_formatting() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        assert_eq!(app.window_title(), "[Dokumen Baru] - Red Text Editor");

        app.active_tab_mut().text_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "[Dokumen Baru] * - Red Text Editor");

        app.active_tab_mut().current_path = Some(PathBuf::from("/home/user/document.txt"));
        app.active_tab_mut().saved_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "document.txt - Red Text Editor");

        app.active_tab_mut().text_content = "Teks diedit".to_string();
        assert_eq!(app.window_title(), "document.txt * - Red Text Editor");
    }

    /// A file over `MAX_OPEN_BYTES` is not read at all: the editor would
    /// freeze laying it out. The status bar says why.
    #[test]
    fn test_files_over_the_size_limit_are_not_opened() {
        let path = std::env::temp_dir().join("red_test_too_big.log");
        File::create(&path)
            .unwrap()
            .set_len(MAX_OPEN_BYTES + 1)
            .unwrap();
        let refused =
            |app: &TextEditorApp| Some((app.i18n.tf("msg.file_too_big", &[("limit", "10")]), true));

        // Into the untouched tab, and into a new tab beside an edited one.
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(path.clone());
        assert_eq!(app.active_tab().current_path, None);
        assert_eq!(app.status_message, refused(&app));

        app.active_tab_mut().text_content = "edited".into();
        app.status_message = None;
        app.open_file_in_tab(path.clone());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.status_message, refused(&app));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_a_file_at_the_size_limit_opens() {
        let path = std::env::temp_dir().join("red_test_at_limit.log");
        File::create(&path)
            .unwrap()
            .set_len(MAX_OPEN_BYTES)
            .unwrap();
        let tab = DocumentTab::from_file(path.clone()).unwrap();
        assert_eq!(tab.text_content.len() as u64, MAX_OPEN_BYTES);
        let _ = fs::remove_file(&path);
    }

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

    #[test]
    fn test_load_file_and_save() {
        let temp_dir = std::env::temp_dir().join("rred_app_test");
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("test_app_doc.txt");

        {
            let mut file = File::create(&test_file).unwrap();
            write!(file, "Hello from file").unwrap();
        }

        let mut app = TextEditorApp::default();
        app.load_file(test_file.clone());

        assert_eq!(app.active_tab().text_content, "Hello from file");
        assert_eq!(app.active_tab().saved_content, "Hello from file");
        assert_eq!(app.active_tab().current_path, Some(test_file.clone()));
        assert!(!app.is_dirty());
        assert_eq!(
            app.status_message,
            Some(("File berhasil dibuka.".to_string(), false))
        );

        app.active_tab_mut().text_content = "Modified content".to_string();
        assert!(app.is_dirty());

        let save_result = app.save();
        assert!(save_result);
        assert!(!app.is_dirty());
        assert_eq!(app.active_tab().saved_content, "Modified content");
        let reloaded = fs::read_to_string(&test_file).unwrap();
        assert_eq!(reloaded, "Modified content");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_trigger_open_specific_file_dirty_vs_clean() {
        let dir = std::env::temp_dir().join("rred_open_dirty_vs_clean_test");
        let _ = fs::create_dir_all(&dir);
        let clean_path = dir.join("clean.txt");
        let dirty_target = dir.join("dirty_target.txt");
        let _ = fs::write(&clean_path, "clean content");
        let _ = fs::write(&dirty_target, "target content");

        let mut app = TextEditorApp::default();

        // Clean: directly attempts to load
        app.trigger_open_specific_file(clean_path.clone());
        assert_eq!(app.pending_action, None);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].text_content, "clean content");

        // Dirty: opening a file opens in a new tab without pending actions, keeping dirty tab intact
        app.active_tab_mut().text_content = "Modified".to_string();
        assert!(app.is_dirty());
        app.trigger_open_specific_file(dirty_target.clone());
        assert_eq!(app.pending_action, None);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);
        assert_eq!(app.tabs[0].text_content, "Modified");
        assert!(app.tabs[0].is_dirty());
        assert_eq!(app.tabs[1].text_content, "target content");
        assert!(!app.tabs[1].is_dirty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_effective_language_prefers_override() {
        let mut app = TextEditorApp::default();
        assert_eq!(
            app.active_tab().active_language,
            crate::highlighter::Language::PlainText
        );
        assert_eq!(app.active_tab().language_override, None);
        assert_eq!(
            app.effective_language(),
            crate::highlighter::Language::PlainText
        );

        app.active_tab_mut().active_language = crate::highlighter::Language::Rust;
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        app.active_tab_mut().language_override = Some(crate::highlighter::Language::Php);
        assert_eq!(app.effective_language(), crate::highlighter::Language::Php);

        app.active_tab_mut().language_override = None;
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);
    }

    #[test]
    fn test_load_file_updates_detected_language() {
        let dir = std::env::temp_dir().join("rred_app_lang_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {}").unwrap();

        let mut app = TextEditorApp::default();
        app.load_file(path);
        assert_eq!(
            app.active_tab().active_language,
            crate::highlighter::Language::Rust
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_file_keeps_explicit_override() {
        let dir = std::env::temp_dir().join("rred_app_lang_override_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("script.php");
        std::fs::write(&path, "<?php echo 1;").unwrap();

        let mut app = TextEditorApp::default();
        app.active_tab_mut().language_override = Some(crate::highlighter::Language::Rust);
        app.load_file(path);
        assert_eq!(
            app.active_tab().active_language,
            crate::highlighter::Language::Php
        );
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_starts_with_one_pristine_tab() {
        let app = TextEditorApp::default();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_index, 0);
        assert!(app.active_tab().is_pristine());
        assert!(!app.is_dirty());
        assert_eq!(app.file_display_name(), "[Dokumen Baru]");
    }

    #[test]
    fn test_open_new_tab() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);
        assert!(app.active_tab().is_pristine());
    }

    #[test]
    fn test_open_file_replaces_pristine_tab() {
        let temp_dir = std::env::temp_dir().join("rred_test_open_pristine");
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("doc.txt");
        fs::write(&test_file, "hello world").unwrap();

        let mut app = TextEditorApp::default();
        assert_eq!(app.tabs.len(), 1);
        assert!(app.active_tab().is_pristine());

        app.open_file_in_tab(test_file.clone());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_index, 0);
        assert_eq!(app.active_tab().text_content, "hello world");
        assert_eq!(app.active_tab().current_path, Some(test_file));
        assert!(!app.active_tab().is_pristine());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_open_file_creates_new_tab_when_active_tab_is_not_pristine() {
        let temp_dir = std::env::temp_dir().join("rred_test_open_not_pristine");
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("second.txt");
        fs::write(&test_file, "second file content").unwrap();

        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "user typed something".to_string();
        assert!(!app.active_tab().is_pristine());

        app.open_file_in_tab(test_file.clone());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);
        assert_eq!(app.active_tab().text_content, "second file content");
        assert_eq!(app.active_tab().current_path, Some(test_file));
        assert_eq!(app.tabs[0].text_content, "user typed something");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_open_existing_file_focuses_tab_without_duplicate() {
        let temp_dir = std::env::temp_dir().join("rred_test_open_existing");
        let _ = fs::create_dir_all(&temp_dir);
        let file1 = temp_dir.join("file1.txt");
        let file2 = temp_dir.join("file2.txt");
        fs::write(&file1, "file 1").unwrap();
        fs::write(&file2, "file 2").unwrap();

        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file1.clone());
        app.open_file_in_tab(file2);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);

        // Re-open file1: should not open a 3rd tab, but switch active_tab_index to 0
        app.open_file_in_tab(file1);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 0);
        assert_eq!(
            app.status_message,
            Some(("File sudah dibuka.".to_string(), false))
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_close_tab_adjusts_active_index() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab(); // tab 1
        app.open_new_tab(); // tab 2
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.active_tab_index, 2);

        // Close the active tab at the end (index 2) -> active index becomes 1
        app.close_tab(2);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);

        // Close tab 0 while active index is 1 -> active index shifts to 0
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_index, 0);
    }

    fn app_with_tabs(dirty: &[bool]) -> TextEditorApp {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        for _ in 1..dirty.len() {
            app.open_new_tab();
        }
        for (i, d) in dirty.iter().enumerate() {
            app.tabs[i].current_path = Some(PathBuf::from(format!("t{i}.txt")));
            if *d {
                app.tabs[i].text_content = "edited".into();
            }
        }
        app
    }

    fn paths(app: &TextEditorApp) -> Vec<String> {
        app.tabs
            .iter()
            .map(|t| t.current_path.as_ref().unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn test_close_others_keeps_clicked_and_dirty_tabs() {
        let mut app = app_with_tabs(&[false, false, true, false]);
        app.active_tab_index = 3;
        app.close_tabs_where(1, |i, _| i != 1);
        assert_eq!(paths(&app), ["t1.txt", "t2.txt"]);
        assert_eq!(app.active_tab_index, 0, "active closed -> clicked tab (t1)");
    }

    #[test]
    fn test_close_to_right_keeps_active_when_it_survives() {
        let mut app = app_with_tabs(&[false, false, false, false]);
        app.active_tab_index = 0;
        app.close_tabs_where(1, |i, _| i > 1);
        assert_eq!(paths(&app), ["t0.txt", "t1.txt"]);
        assert_eq!(app.active_tab_index, 0);
    }

    #[test]
    fn test_close_saved_keeps_only_dirty_tabs() {
        let mut app = app_with_tabs(&[false, true, false, true]);
        app.active_tab_index = 2;
        app.close_tabs_where(2, |_, _| true);
        assert_eq!(paths(&app), ["t1.txt", "t3.txt"]);
        assert_eq!(app.active_tab_index, 1, "next survivor after active");
    }

    #[test]
    fn test_close_saved_with_all_saved_leaves_blank_tab() {
        let mut app = app_with_tabs(&[false, false]);
        app.close_tabs_where(0, |_, _| true);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.active_tab().is_pristine());
    }

    #[test]
    fn test_line_shortcuts_edit_the_document_and_reselect() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "one\ntwo".to_owned();
        // No caret placed yet: the start of the document.
        app.apply_shortcut(&ctx, ShortcutAction::MoveLineDown);
        assert_eq!(app.active_tab().text_content, "two\none");
        assert_eq!(app.editor_request.select, Some((4, 4)));
        assert!(app.editor_request.focus);
        app.apply_shortcut(&ctx, ShortcutAction::CopyLineDown);
        assert_eq!(app.active_tab().text_content, "two\ntwo\none");
        app.apply_shortcut(&ctx, ShortcutAction::DeleteLine);
        assert_eq!(app.active_tab().text_content, "two\none");
        app.apply_shortcut(&ctx, ShortcutAction::SelectNextMatch);
        assert_eq!(app.editor_request.select, Some((0, 3)));
        // Ignored on the start page.
        app.show_welcome = true;
        app.apply_shortcut(&ctx, ShortcutAction::DeleteLine);
        assert_eq!(app.active_tab().text_content, "two\none");
    }

    #[test]
    fn test_fold_commands_change_the_folds() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        // Blocks: (0, 3), (1, 2), (4, 5).
        app.active_tab_mut().text_content = String::from("a\n  b\n    c\n  d\ne\n  f\n");
        let put_caret = |app: &TextEditorApp, at| {
            let mut state = egui::TextEdit::load_state(&ctx, app.editor_id()).unwrap_or_default();
            let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(at));
            state.cursor.set_char_range(Some(caret));
            state.store(&ctx, app.editor_id());
        };
        let folded = |app: &TextEditorApp| -> Vec<usize> {
            (0..6).filter(|&h| app.active_tab().folds.is_folded(h)).collect()
        };
        // On "    c": its innermost block is "  b"'s, which hides the caret,
        // so the caret moves to the end of the header.
        put_caret(&app, 8);
        app.apply_shortcut(&ctx, ShortcutAction::Fold);
        assert_eq!(folded(&app), vec![1]);
        assert_eq!(app.editor_request.select, Some((5, 5)));
        put_caret(&app, 5);
        // Folding again folds the next block out.
        app.apply_shortcut(&ctx, ShortcutAction::Fold);
        assert_eq!(folded(&app), vec![0, 1]);
        put_caret(&app, 1);
        app.apply_shortcut(&ctx, ShortcutAction::Unfold);
        assert_eq!(folded(&app), vec![1]);
        app.apply_shortcut(&ctx, ShortcutAction::UnfoldAll);
        assert!(folded(&app).is_empty());
        app.apply_shortcut(&ctx, ShortcutAction::FoldAll);
        assert_eq!(folded(&app), vec![0, 1, 4]);
    }

    #[test]
    fn test_go_to_line_into_a_folded_block_unfolds_it() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = String::from("a {\n  b\n  c\n}\n");
        app.active_tab_mut().folds.fold(0);
        let frame = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            });
        };
        frame(&mut app);
        assert!(app.active_tab().folds.is_folded(0), "a frame keeps the fold");
        app.goto_line(2);
        frame(&mut app);
        assert!(app.active_tab().folds.is_empty());
    }

    #[test]
    fn test_typing_on_a_folded_header_keeps_the_fold() {
        let mut tab = DocumentTab::default_new();
        tab.text_content = String::from("a {\n  b\n}\n");
        tab.folds.fold(0);
        assert_eq!(tab.hidden_ranges(), &[(3, 7)]);
        tab.text_content.insert(0, 'x');
        assert_eq!(tab.hidden_ranges(), &[(4, 8)]);
        assert!(tab.folds.is_folded(0));
        // Typing inside the hidden lines unfolds them.
        tab.text_content.insert(6, 'z');
        assert!(tab.hidden_ranges().is_empty());
    }

    #[test]
    fn test_go_to_bracket_moves_the_caret_to_the_partner() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = String::from("f(a[1]) + x");
        let put_caret = |app: &TextEditorApp, at| {
            let mut state = egui::TextEdit::load_state(&ctx, app.editor_id()).unwrap_or_default();
            let caret = egui::text::CCursorRange::one(egui::text::CCursor::new(at));
            state.cursor.set_char_range(Some(caret));
            state.store(&ctx, app.editor_id());
        };
        // Before "(": lands after ")".
        put_caret(&app, 1);
        app.apply_shortcut(&ctx, ShortcutAction::GoToBracket);
        assert_eq!(app.editor_request.select, Some((7, 7)));
        assert_eq!(app.editor_request.reveal, Some((7, 7)));
        assert!(app.editor_request.focus);
        // And back.
        app.editor_request = EditorRequest::default();
        put_caret(&app, 7);
        app.apply_shortcut(&ctx, ShortcutAction::GoToBracket);
        assert_eq!(app.editor_request.select, Some((1, 1)));
        // No bracket at the caret: nothing moves.
        app.editor_request = EditorRequest::default();
        put_caret(&app, 9);
        app.apply_shortcut(&ctx, ShortcutAction::GoToBracket);
        assert_eq!(app.editor_request.select, None);
    }

    #[test]
    fn test_toggle_comment_follows_the_language_or_reports_none() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "let x = 1;".to_owned();
        app.active_tab_mut().language_override = Some(Language::Rust);
        app.apply_shortcut(&ctx, ShortcutAction::ToggleComment);
        assert_eq!(app.active_tab().text_content, "// let x = 1;");
        app.apply_shortcut(&ctx, ShortcutAction::ToggleComment);
        assert_eq!(app.active_tab().text_content, "let x = 1;");
        app.active_tab_mut().language_override = Some(Language::PlainText);
        app.apply_shortcut(&ctx, ShortcutAction::ToggleComment);
        assert_eq!(app.active_tab().text_content, "let x = 1;");
        assert!(app.status_message.as_ref().is_some_and(|(_, error)| *error));
    }

    #[test]
    fn test_tab_shortcuts_wrap_around() {
        let ctx = egui::Context::default();
        let mut app = app_with_tabs(&[false, false, false]);
        app.active_tab_index = 2;
        app.apply_shortcut(&ctx, ShortcutAction::NextTab);
        assert_eq!(app.active_tab_index, 0);
        app.apply_shortcut(&ctx, ShortcutAction::PrevTab);
        assert_eq!(app.active_tab_index, 2);
        app.apply_shortcut(&ctx, ShortcutAction::NewTab);
        assert_eq!(app.tabs.len(), 4);
    }

    #[test]
    fn test_zen_mode_toggles_with_shortcut() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        assert!(!app.zen_mode);
        app.apply_shortcut(&ctx, ShortcutAction::ZenMode);
        assert!(app.zen_mode && app.editor_request.focus);
        app.apply_shortcut(&ctx, ShortcutAction::ZenMode);
        assert!(!app.zen_mode);
    }

    #[test]
    fn test_welcome_shows_at_start_and_after_the_last_tab_closes() {
        let mut app = TextEditorApp::default();
        assert!(app.show_welcome, "the start page opens the editor");
        assert_eq!(app.window_title(), "Red Text Editor");
        app.open_file_in_tab(PathBuf::from("/nonexistent/x.txt"));
        assert!(app.show_welcome, "a failed open keeps the page");
        app.open_new_tab();
        assert!(!app.show_welcome, "a new file dismisses it");
        assert_eq!(app.tabs.len(), 1, "New File takes the blank tab underneath");
        app.open_new_tab();
        app.close_tab(app.active_tab_index);
        assert!(!app.show_welcome, "one tab still open");
        app.goto.visible = true;
        app.close_tab(0);
        assert!(app.show_welcome, "the last tab closed");
        assert!(!app.goto.visible, "the page closes the editor's bars");
        app.open_file_in_tab(PathBuf::from("Cargo.toml"));
        assert!(!app.show_welcome, "opening a file dismisses it");
        app.close_tabs_where(app.active_tab_index, |_, _| true);
        assert!(app.show_welcome, "closing every tab brings it back");
    }

    #[test]
    fn test_editor_shortcuts_are_ignored_on_the_welcome_page() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(!app.find.visible);
        app.open_new_tab();
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(app.find.visible);
    }

    #[test]
    fn test_no_markdown_preview_on_the_welcome_page() {
        let mut app = markdown_app();
        app.show_markdown_preview = true;
        app.show_welcome = true;
        assert!(!app.previewing());
    }

    #[test]
    fn test_git_graph_opens_only_with_a_repository() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_none(), "no repository, no graph");

        let Some(repo) = crate::git::temp_repo("app_graph") else { return };
        let root = repo.root().to_path_buf();
        app.git.open(&ctx, root.clone());
        for _ in 0..250 {
            app.git.poll(&ctx);
            if app.git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_some());
        assert!(app.git.graph.loading || app.git.graph.complete, "opening loads the graph");
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_none(), "the shortcut toggles");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_editor_shortcuts_are_ignored_while_the_graph_is_open() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let Some(repo) = crate::git::temp_repo("app_graph_keys") else { return };
        let root = repo.root().to_path_buf();
        app.git.open(&ctx, root.clone());
        for _ in 0..250 {
            app.git.poll(&ctx);
            if app.git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_some());
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(!app.find.visible, "Find waits for the editor");
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(app.find.visible);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_reopening_the_graph_retries_a_failed_load() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let Some(repo) = crate::git::temp_repo("app_graph_retry") else { return };
        let root = repo.root().to_path_buf();
        app.git.open(&ctx, root.clone());
        for _ in 0..250 {
            app.git.poll(&ctx);
            if app.git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.git.graph.error = Some(GitError::Failed("x".into()));
        app.git.graph.loading = false;
        app.git.graph.commits.clear();
        app.ensure_graph_loaded(&ctx, false);
        assert!(app.git.graph.error.is_some(), "no retry every frame");
        app.toggle_git_graph(&ctx);
        assert!(app.git_graph.is_some());
        assert!(app.git.graph.error.is_none() || app.git.graph.loading, "opening retries");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_closing_the_graph_drops_stale_editor_requests() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { git_graph: Some(GitGraphUi::default()), ..Default::default() };
        app.editor_request.goto = Some(5);
        app.toggle_git_graph(&ctx);
        assert!(app.git_graph.is_none());
        assert!(app.editor_request.goto.is_none() && app.editor_request.focus);
    }

    #[test]
    fn test_goto_line_moves_caret_and_scrolls() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "ab\ncde\nf".into();
        app.apply_shortcut(&egui::Context::default(), ShortcutAction::GotoLine);
        assert!(app.goto.visible && app.goto.focus);
        app.goto_line(2);
        assert!(!app.goto.visible);
        assert_eq!(app.editor_request.goto, Some(3));
        assert_eq!(app.editor_request.reveal, Some((3, 3)));
        assert!(app.editor_request.focus);
    }

    #[test]
    fn test_goto_line_places_the_caret() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique(("editor", 0));
        let mut text = "ab\ncde\nf".to_owned();
        let mut frame = |request: EditorRequest| {
            let mut request = Some(request);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                super::render_editor_with(
                    ui,
                    &mut text,
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request.take().unwrap(),
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
            out.textures_delta.clear();
        };
        frame(EditorRequest { focus: true, goto: Some(3), ..Default::default() });
        frame(EditorRequest::default());
        let range = egui::TextEdit::load_state(&ctx, id).unwrap().cursor.char_range().unwrap();
        assert_eq!(range.primary.index.0, 3);
    }

    #[test]
    fn test_find_in_files_shortcut_and_hit_navigation() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("red_fif_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.txt");
        fs::write(&file, "l1\nl2\nl3\n").unwrap();

        let mut app = TextEditorApp::default();
        app.apply_shortcut(&ctx, ShortcutAction::FindInFiles);
        assert!(app.search.visible && app.search.focus_query);
        app.open_search_hit(file.clone(), 3);
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&file));
        assert_eq!(app.editor_request.goto, Some(6), "caret at the start of line 3");
        app.close_search();
        assert!(!app.search.visible && app.search.hits.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_next_walks_matches_and_wraps() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "ab ab ab".into();
        app.find.query = "ab".into();
        app.find_step(&ctx, FindAction::Next);
        assert_eq!(app.find.current, Some((0, 2)));
        app.find_step(&ctx, FindAction::Next);
        assert_eq!(app.find.current, Some((3, 5)));
        app.find_step(&ctx, FindAction::Next);
        app.find_step(&ctx, FindAction::Next);
        assert_eq!(app.find.current, Some((0, 2)), "wraps");
        app.find_step(&ctx, FindAction::Prev);
        assert_eq!(app.find.current, Some((6, 8)), "wraps backwards");
        assert_eq!(app.editor_request.reveal, Some((6, 8)));
    }

    #[test]
    fn test_replace_current_then_replace_all() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "cat cat cat".into();
        app.find.query = "cat".into();
        app.find.replacement = "dog".into();
        app.find_step(&ctx, FindAction::Next);
        app.replace_current(&ctx);
        assert_eq!(app.active_tab().text_content, "dog cat cat");
        assert_eq!(app.find.current, Some((4, 7)), "moves on to the next match");
        app.replace_all();
        assert_eq!(app.active_tab().text_content, "dog dog dog");
        assert!(app.status_message.is_some());
    }

    #[test]
    fn test_find_shortcuts_open_and_close_the_bar() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(app.find.visible && !app.find.replace_mode && app.find.focus_query);
        app.apply_shortcut(&ctx, ShortcutAction::Replace);
        assert!(app.find.replace_mode);
        app.close_find();
        assert!(!app.find.visible && app.find.matches.is_empty());
    }

    /// The menu's Undo presses Ctrl+Z in the editor, which must work even
    /// though the click on the menu took focus away.
    #[test]
    fn test_menu_undo_reverts_typing() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique(("editor", 0));
        let text = std::cell::RefCell::new(String::new());
        let frame = |time: f64, events: Vec<egui::Event>, request: EditorRequest| {
            let mut request = Some(request);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                super::render_editor_with(
                    ui,
                    &mut text.borrow_mut(),
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request.take().unwrap(),
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
            out.textures_delta.clear();
        };
        let focus = || EditorRequest { focus: true, ..Default::default() };
        frame(0.0, Vec::new(), focus());
        frame(0.1, Vec::new(), focus());
        frame(0.2, vec![egui::Event::Text("hello".into())], EditorRequest::default());
        frame(2.0, Vec::new(), EditorRequest::default());
        assert_eq!(*text.borrow(), "hello");
        frame(3.0, Vec::new(), EditorRequest { focus: true, key: Some(egui::Key::Z), ..Default::default() });
        frame(3.1, Vec::new(), EditorRequest::default());
        assert_eq!(*text.borrow(), "", "undo reverts the typed text");
    }

    /// A click on a menu takes the focus, and egui then keeps only the caret
    /// in the editor's state. The editor remembers the selection, and a
    /// `select` request puts it back once the menu closes.
    #[test]
    fn test_selection_survives_focus_lost_to_a_menu() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique(("editor", 0));
        let mut text = "hello world".to_owned();
        let mut frame = |request: EditorRequest| {
            let mut request = Some(request);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                super::render_editor_with(
                    ui,
                    &mut text,
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request.take().unwrap(),
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
            out.textures_delta.clear();
        };
        let selection = |ctx: &egui::Context| {
            let range = egui::TextEdit::load_state(ctx, id)?.cursor.char_range()?;
            let (a, b) = (range.primary.index.0, range.secondary.index.0);
            Some((a.min(b), a.max(b)))
        };

        frame(EditorRequest { focus: true, select: Some((0, 5)), ..Default::default() });
        frame(EditorRequest::default());
        assert_eq!(selection(&ctx), Some((0, 5)));

        ctx.memory_mut(|m| m.surrender_focus(id));
        frame(EditorRequest { inactive_selection: Some((0, 5)), ..Default::default() });
        let (a, b) = selection(&ctx).unwrap();
        assert_eq!(a, b, "egui keeps only the caret once the focus is gone");
        let saved = ctx.data(|d| d.get_temp::<find::Range>(id.with("saved_selection")));
        assert_eq!(saved, Some((0, 5)), "the editor remembers the selection");

        frame(EditorRequest { focus: true, select: saved, ..Default::default() });
        assert_eq!(selection(&ctx), Some((0, 5)), "the selection is back");
    }

    /// Right-clicks `char` index `at` of "hello world", selected `(0, 5)`,
    /// and returns the selection afterwards and whether the menu opened.
    fn right_click_selected_editor(at: f32) -> (find::Range, bool) {
        let ctx = egui::Context::default();
        let id = egui::Id::unique(("editor", 0));
        let mut text = "hello world".to_owned();
        let mut frame = |events: Vec<egui::Event>, request: EditorRequest| {
            let mut request = Some(request);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut editor = None;
            let mut out = ctx.run_ui(input, |ui| {
                let (_, response, _) = super::render_editor_with(
                    ui,
                    &mut text,
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request.take().unwrap(),
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
                response.context_menu(|ui| {
                    ui.label("menu");
                });
                editor = Some(response);
            });
            out.textures_delta.clear();
            editor.unwrap()
        };
        let editor = frame(Vec::new(), EditorRequest { focus: true, select: Some((0, 5)), ..Default::default() });
        frame(Vec::new(), EditorRequest::default());

        let font_id = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        let char_width = ctx.fonts_mut(|f| f.glyph_width(&font_id, 'a'));
        // The editor's inner margin is 8 points left and 6 on top.
        let pos = editor.rect.min + egui::vec2(8.0 + at * char_width, 10.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(pos), button(true)], EditorRequest::default());
        let editor = frame(vec![button(false)], EditorRequest::default());
        assert!(editor.secondary_clicked());

        let range = egui::TextEdit::load_state(&ctx, id).unwrap().cursor.char_range().unwrap();
        let (a, b) = (range.primary.index.0, range.secondary.index.0);
        ((a.min(b), a.max(b)), egui::Popup::is_any_open(&ctx))
    }

    #[test]
    fn test_right_click_inside_the_selection_keeps_it() {
        assert_eq!(right_click_selected_editor(2.5), ((0, 5), true));
    }

    #[test]
    fn test_right_click_outside_the_selection_moves_the_caret() {
        assert_eq!(right_click_selected_editor(8.0), ((8, 8), true));
    }

    /// An app whose editor lost `range` of "hello world" to a menu click.
    fn app_with_menu_selection(ctx: &egui::Context, range: find::Range) -> TextEditorApp {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "hello world".into();
        app.menu_focus = Some(app.editor_id());
        ctx.data_mut(|d| d.insert_temp(app.saved_selection_id(), range));
        app
    }

    #[test]
    fn test_menu_copy_and_cut_use_the_remembered_selection() {
        let ctx = egui::Context::default();
        let mut app = app_with_menu_selection(&ctx, (6, 11));
        assert_eq!(app.selected_text(&ctx).map(|(_, t)| t).as_deref(), Some("world"));

        app.cut_selection(&ctx);
        assert_eq!(app.active_tab().text_content, "hello ");
        assert_eq!(app.editor_request.goto, Some(6));
        assert_eq!(app.selected_text(&ctx), None, "cut drops the selection");
    }

    #[test]
    fn test_remembered_selection_is_ignored_without_a_menu() {
        let ctx = egui::Context::default();
        let mut app = app_with_menu_selection(&ctx, (6, 11));
        app.menu_focus = None;
        assert_eq!(app.selected_text(&ctx), None);
    }

    #[test]
    fn test_closing_a_menu_returns_focus_and_selection() {
        let ctx = egui::Context::default();
        let mut app = app_with_menu_selection(&ctx, (0, 5));
        app.return_focus_after_menu(&ctx);
        assert_eq!(app.menu_focus, None);
        assert!(app.editor_request.focus);
        assert_eq!(app.editor_request.select, Some((0, 5)));
    }

    #[test]
    fn test_menu_focus_is_not_returned_under_a_dialog_or_to_another_tab() {
        let ctx = egui::Context::default();
        let mut app = app_with_menu_selection(&ctx, (0, 5));
        app.show_about_modal = true;
        app.return_focus_after_menu(&ctx);
        assert!(!app.editor_request.focus);
        assert_eq!(app.editor_request.select, None);

        let mut app = app_with_menu_selection(&ctx, (0, 5));
        app.open_new_tab();
        app.return_focus_after_menu(&ctx);
        assert_eq!(app.menu_focus, None);
        assert_eq!(app.editor_request.select, None);
    }

    #[test]
    fn test_close_last_tab_creates_new_blank_tab() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "some edited text".to_string();
        app.active_tab_mut().current_path = Some(PathBuf::from("test.txt"));
        assert!(!app.active_tab().is_pristine());

        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_index, 0);
        assert!(app.active_tab().is_pristine());
        assert!(app.active_tab().text_content.is_empty());
        assert_eq!(app.active_tab().current_path, None);
    }

    #[test]
    fn test_dirty_tab_triggers_close_tab_pending_action() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab();
        app.active_tab_mut().text_content = "dirty content".to_string();
        assert!(app.active_tab().is_dirty());

        app.trigger_close_tab(1);
        assert_eq!(app.pending_action, Some(PendingAction::CloseTab(1)));
        assert_eq!(app.tabs.len(), 2); // Tab was not closed yet

        // If clean, closes immediately
        app.active_tab_mut().saved_content = "dirty content".to_string();
        assert!(!app.active_tab().is_dirty());
        app.pending_action = None;
        app.trigger_close_tab(1);
        assert_eq!(app.pending_action, None);
        assert_eq!(app.tabs.len(), 1);
    }

    /// A tab's own line cache is filled by the editor and survives edits and
    /// other tabs, so only touched lines are re-parsed.
    #[test]
    fn test_tab_maintains_incremental_highlight_cache() {
        let ctx = egui::Context::default();
        let mut highlighter = SyntaxHighlighter::new();
        let mut tab_a = DocumentTab::default_new();
        let mut tab_b = DocumentTab::default_new();
        tab_a.text_content = "fn one() {}\nfn two() {}".to_string();
        tab_b.text_content = "fn other() {}".to_string();

        let mut frame = |tab: &mut DocumentTab| {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter
                        .layouter(Language::Rust, font_id)
                        .with_document_cache(&mut tab.highlight_cache);
                    super::render_editor(ui, &mut tab.text_content, Some(&mut layouter), false, false, "");
                });
            });
        };
        frame(&mut tab_a);
        frame(&mut tab_b);
        assert_eq!(tab_a.highlight_cache.lines.len(), 2);
        assert_eq!(tab_b.highlight_cache.lines.len(), 1);

        tab_a.text_content = "fn one() {}\nfn two_edited() {}".to_string();
        frame(&mut tab_a);
        assert_eq!(tab_a.highlight_cache.lines[1].text, "fn two_edited() {}");
        assert_eq!(tab_b.highlight_cache.lines[0].text, "fn other() {}");
    }

    /// A frame of the app lays the active tab out through the editor's
    /// cache. There is one for all tabs: a galley takes tens of bytes per
    /// character, too much to keep for tabs that are not shown.
    #[test]
    fn test_frame_lays_out_the_active_tab_through_the_editor_cache() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "fn main() {}\n".to_string();
        app.open_new_tab();
        app.active_tab_mut().text_content = "second\n".to_string();
        let frame = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            });
            app.editor_layout.last_galley().map(|g| g.text().to_owned())
        };
        assert_eq!(frame(&mut app).as_deref(), Some("second\n"));
        app.active_tab_index = 0;
        assert_eq!(frame(&mut app).as_deref(), Some("fn main() {}\n"));
    }

    #[test]
    fn test_word_wrap_defaults_to_off() {
        assert!(!TextEditorApp::default().word_wrap);
    }

    /// A fresh folder `rred_app_<name>` in the temp directory holding
    /// `notes.txt`.
    fn folder_with_notes(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_app_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), "notes").unwrap();
        dir
    }

    #[test]
    fn test_create_file_action_opens_the_new_file() {
        let dir = folder_with_notes("explorer_create");
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(dir.clone())),
            ..TextEditorApp::default()
        };
        app.apply_explorer_action(ExplorerAction::Create {
            dir: dir.clone(),
            name: "added.rs".into(),
            folder: false,
        });
        assert!(dir.join("added.rs").is_file());
        assert_eq!(app.active_tab().current_path.as_deref(), Some(dir.join("added.rs").as_path()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename_action_moves_the_tab_and_its_language() {
        let dir = folder_with_notes("explorer_rename");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("notes.txt"));
        app.active_tab_mut().text_content.push_str(" edited");

        app.apply_explorer_action(ExplorerAction::Rename {
            from: dir.join("notes.txt"),
            name: "notes.rs".into(),
        });
        let tab = app.active_tab();
        assert_eq!(tab.current_path.as_deref(), Some(dir.join("notes.rs").as_path()));
        assert_eq!(tab.active_language, Language::Rust);
        assert_eq!(tab.text_content, "notes edited", "unsaved edits stay");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_renaming_a_folder_moves_every_tab_under_it() {
        let dir = folder_with_notes("explorer_rename_dir");
        fs::create_dir(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "a").unwrap();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("src/a.rs"));
        app.open_file_in_tab(dir.join("notes.txt"));

        app.apply_explorer_action(ExplorerAction::Rename { from: dir.join("src"), name: "lib".into() });
        let paths: Vec<_> = app.tabs.iter().filter_map(|t| t.current_path.clone()).collect();
        assert_eq!(paths, vec![dir.join("lib/a.rs"), dir.join("notes.txt")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_close_tabs_under_keeps_unsaved_tabs() {
        let dir = folder_with_notes("explorer_delete_tabs");
        fs::write(dir.join("other.txt"), "other").unwrap();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("notes.txt"));
        app.open_file_in_tab(dir.join("other.txt"));
        app.active_tab_mut().text_content.push_str(" edited");

        app.close_tabs_under(&dir);
        let paths: Vec<_> = app.tabs.iter().filter_map(|t| t.current_path.clone()).collect();
        assert_eq!(paths, vec![dir.join("other.txt")], "the clean tab closes, the dirty one stays");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_action_waits_for_confirmation() {
        let dir = folder_with_notes("explorer_delete_ask");
        let mut app = TextEditorApp::default();
        app.apply_explorer_action(ExplorerAction::Delete(dir.join("notes.txt")));
        assert_eq!(app.pending_delete.as_deref(), Some(dir.join("notes.txt").as_path()));
        assert!(dir.join("notes.txt").exists(), "nothing is deleted before the answer");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_rename_reports_an_error() {
        let dir = folder_with_notes("explorer_rename_fail");
        let mut app = TextEditorApp::default();
        app.apply_explorer_action(ExplorerAction::Rename {
            from: dir.join("missing.txt"),
            name: "x.txt".into(),
        });
        assert!(matches!(app.status_message, Some((_, true))));
        let _ = fs::remove_dir_all(&dir);
    }


    #[test]
    fn test_toggle_explorer_hides_and_shows_the_sidebar() {
        let dir = folder_with_notes("toggle_explorer");
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(dir.clone())),
            show_sidebar: true,
            ..Default::default()
        };

        app.toggle_explorer();
        assert!(!app.show_sidebar);
        app.toggle_explorer();
        assert!(app.show_sidebar);
        assert!(app.explorer.is_some(), "toggling keeps the open folder");
        let _ = fs::remove_dir_all(&dir);
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

    /// Clicking a file in the tree opens it, and clicking it again focuses
    /// its tab instead of opening a second one.
    #[test]
    fn test_tree_click_opens_a_file_once() {
        let dir = folder_with_notes("tree_click");
        let notes = dir.join("notes.txt");
        let mut app = TextEditorApp::default();
        app.open_new_tab();
        app.active_tab_mut().text_content = "unsaved".to_string();
        let mut explorer = Explorer::new(dir.clone());
        let ctx = egui::Context::default();

        let mut tree = |events| {
            let mut response = None;
            run_frame_with_events(&ctx, events, |ui| {
                response = Some(file_browser::show(ui, &mut explorer, None, None, &I18n::builtin()));
            });
            response.expect("show ran")
        };
        let (path, row) = tree(Vec::new()).rows[0].clone();
        assert_eq!(path, notes);
        let opened = tree(click_events(row.center()))
            .open_file
            .expect("the notes row was clicked");
        assert_eq!(opened, notes);

        app.trigger_open_specific_file(opened.clone());
        let tabs = app.tabs.len();
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&notes));
        app.active_tab_index = 0;
        app.trigger_open_specific_file(opened);
        assert_eq!(app.tabs.len(), tabs, "no duplicate tab");
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&notes));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A fresh document is empty and shows the translated placeholder.
    #[test]
    fn test_new_document_is_empty_with_a_placeholder_hint() {
        let app = TextEditorApp::default();
        assert!(app.active_tab().text_content.is_empty());
        assert!(app.active_tab().is_pristine());

        let ctx = egui::Context::default();
        let mut text = String::new();
        let hint = app.i18n.t("editor.placeholder");
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
            super::render_editor(ui, &mut text, None, false, true, hint);
        });
        assert!(painted_text(&shapes, "Ketik sesuatu di sini...").is_some());
        assert!(text.is_empty(), "the hint never enters the buffer");
    }

    #[test]
    fn test_sidebar_switches_between_explorer_and_source_control() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(std::env::temp_dir())),
            show_sidebar: true,
            ..Default::default()
        };

        app.apply_shortcut(&ctx, ShortcutAction::SourceControl);
        assert!(app.show_sidebar);
        assert_eq!(app.sidebar_view, SidebarView::SourceControl);
        // Ctrl+B hides and brings back the same view.
        app.apply_shortcut(&ctx, ShortcutAction::ToggleSidebar);
        assert!(!app.show_sidebar);
        app.apply_shortcut(&ctx, ShortcutAction::ToggleSidebar);
        assert!(app.show_sidebar);
        assert_eq!(app.sidebar_view, SidebarView::SourceControl);

        app.toggle_explorer();
        assert_eq!(app.sidebar_view, SidebarView::Explorer);
        assert!(app.show_sidebar);
        app.toggle_explorer();
        assert!(!app.show_sidebar);
    }

    #[test]
    fn test_ctrl_shift_g_is_source_control_not_goto_line() {
        let ctx = egui::Context::default();
        let event = egui::Event::Key {
            key: egui::Key::G,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        };
        let input = egui::RawInput {
            events: vec![event],
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| action = ui.ctx().input_mut(consume_shortcut));
        output.textures_delta.clear();
        assert_eq!(action, Some(ShortcutAction::SourceControl));
    }

    #[test]
    fn test_blame_column_names_the_author_once_the_file_is_saved() {
        let Some(repo) = crate::git::temp_repo("app_blame") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(repo.root().to_path_buf())),
            show_blame: true,
            ..Default::default()
        };
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.git.status.is_some());
        // Every painted text with the clip rect it is shown through.
        let frame = |app: &mut TextEditorApp| -> Vec<(String, egui::Rect, egui::Rect)> {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some((
                        t.galley.text().to_owned(),
                        egui::Rect::from_min_size(t.pos, t.galley.size()),
                        c.clip_rect,
                    )),
                    _ => None,
                })
                .collect()
        };
        let names = |shapes: &[(String, egui::Rect, egui::Rect)]| -> Vec<String> {
            shapes.iter().map(|(t, _, _)| t.clone()).collect()
        };
        // The first frame asks git for the blame; it is painted once it arrives.
        let mut shapes = frame(&mut app);
        for _ in 0..250 {
            if shapes.iter().any(|(t, _, _)| t.starts_with("Test · ")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            shapes = frame(&mut app);
        }
        let labels: Vec<_> = shapes.iter().filter(|(t, _, _)| t.starts_with("Test · ")).collect();
        assert_eq!(labels.len(), 2, "{:?}", names(&shapes));
        // The strip sits left of the editor's scroll area: the labels must be
        // clipped to the strip, not to the scroll area, or nothing shows.
        for (text, rect, clip) in &labels {
            assert!(
                clip.width() > 0.0 && clip.contains_rect(*rect),
                "{text} at {rect:?} is cut off by clip {clip:?}"
            );
        }
        // Unsaved edits replace the column with a note.
        app.active_tab_mut().text_content.push_str("three\n");
        let texts = names(&frame(&mut app));
        assert!(texts.iter().any(|t| t.contains("Simpan dulu")), "{texts:?}");
        assert!(!texts.iter().any(|t| t.starts_with("Test · ")));
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_file_history_opens_source_control_filtered_to_the_file() {
        let Some(repo) = crate::git::temp_repo("app_history") else { return };
        let a = repo.root().join("a.txt");
        let b = repo.root().join("b.txt");
        fs::write(&a, "a\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("add a").unwrap();
        fs::write(&b, "b\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("add b").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(repo.root().to_path_buf())),
            ..Default::default()
        };
        poll_git_until(&mut app, &ctx, |app| app.git.status.is_some());
        app.open_file_history(&ctx, b.clone());
        assert!(app.show_sidebar);
        assert_eq!(app.sidebar_view, SidebarView::SourceControl);
        assert!(app.scm.commits_open);
        poll_git_until(&mut app, &ctx, |app| app.git.log.complete);
        let subjects: Vec<&str> = app.git.log.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["add b"]);
        assert_eq!(app.git.log.filter.as_ref(), Some(&b));

        app.apply_scm_action(&ctx, ScmAction::ClearHistoryFilter);
        poll_git_until(&mut app, &ctx, |app| app.git.log.complete);
        assert_eq!(app.git.log.commits.len(), 2);
        let _ = fs::remove_dir_all(repo.root());
    }

    /// Runs `poll_git` until `done` holds, for up to five seconds.
    fn poll_git_until(app: &mut TextEditorApp, ctx: &egui::Context, done: impl Fn(&TextEditorApp) -> bool) {
        for _ in 0..250 {
            app.poll_git(ctx);
            if done(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "git did not answer in time: repo {:?} missing {:?} status {:?} base {:?}",
            app.git.repo(),
            app.git.missing,
            app.git.status.as_ref().map(|s| (&s.head, &s.branch)),
            app.active_tab().git_base
        );
    }

    #[test]
    fn test_clicking_a_change_opens_the_file_and_its_diff() {
        let Some(repo) = crate::git::temp_repo("app_diff") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();
        fs::write(&file, "one\ntwo\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(repo.root().to_path_buf())),
            ..Default::default()
        };
        poll_git_until(&mut app, &ctx, |app| app.git.change_count() == 1);
        app.apply_scm_action(
            &ctx,
            ScmAction::Diff { path: file.clone(), source: crate::git_worker::DiffSource::WorkTree },
        );
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&file));
        assert!(!app.previewing());
        poll_git_until(&mut app, &ctx, |app| app.git.diff().is_some_and(|d| d.lines.is_some()));
        let diff = app.git.diff().unwrap();
        let lines = diff.lines.as_ref().unwrap().as_ref().unwrap();
        assert!(lines.iter().any(|l| l.kind == crate::git::DiffKind::Added && l.text == "two"));

        // Staging it from the panel moves it to Staged Changes.
        app.apply_scm_action(&ctx, ScmAction::Run(Op::Stage(vec![file.clone()])));
        poll_git_until(&mut app, &ctx, |app| {
            app.git.busy().is_none() && app.git.change(&file).is_some_and(|c| c.staged.is_some())
        });
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_gutter_marks_lines_changed_since_head() {
        let Some(repo) = crate::git::temp_repo("app_marks") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        assert!(app.active_tab().line_marks.is_empty());

        app.active_tab_mut().text_content = "one\nTWO\nthree\nfour\n".into();
        poll_git_until(&mut app, &ctx, |app| !app.active_tab().line_marks.is_empty());
        let marks = &app.active_tab().line_marks;
        use crate::line_diff::Mark;
        assert_eq!(marks.get(1), None);
        assert_eq!(marks.get(2), Some(Mark::Modified));
        assert_eq!(marks.get(4), Some(Mark::Added));
        assert_eq!(app.git.status.as_ref().unwrap().branch.as_deref(), Some("main"));
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_diff_view_against_head_toggles_and_blocks_editor_shortcuts() {
        let Some(repo) = crate::git::temp_repo("app_diff_view") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        app.active_tab_mut().text_content = "one\nTWO\nthree\n".into();

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_some());
        // The editor is hidden: its shortcuts do nothing.
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(!app.find.visible);
        let mut ui_action = None;
        run_frame_with_events(&ctx, Vec::new(), |ui| ui_action = app.show_diff_view(ui));
        assert_eq!(ui_action, None);
        let kinds: Vec<_> = app.diff_view.as_ref().unwrap().rows().iter().map(|r| r.kind).collect();
        use crate::diff_view::RowKind::*;
        assert_eq!(kinds, [Equal, Changed, Added]);

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_none(), "the shortcut toggles");
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_diff_view_with_a_file_and_outside_a_repository() {
        let dir = std::env::temp_dir().join(format!("red_diff_file_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        fs::write(&a, "x\ny\n").unwrap();
        fs::write(&b, "x\nz\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(a.clone());

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_none(), "no repository, no HEAD");
        assert!(app.status_message.as_ref().is_some_and(|(_, error)| *error));

        app.diff_with_file(&ctx, b.clone());
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_diff_view(ui);
        });
        let view = app.diff_view.as_ref().unwrap();
        assert_eq!(view.rows()[1].left, Some((2, "z".into())));
        assert_eq!(view.rows()[1].right, Some((2, "y".into())));

        // Showing another tab closes it.
        app.open_new_tab();
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_diff_view(ui);
        });
        assert!(app.diff_view.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hunk_popup_reverts_one_change_and_closes() {
        let Some(repo) = crate::git::temp_repo("app_hunk") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        app.active_tab_mut().text_content = "one\nTWO\nthree\nfour\n".into();

        app.hunk_popup = Some(HunkPopup { tab: app.active_tab_index, line: 2, pos: egui::Pos2::ZERO });
        let hunk = app.popup_hunk().expect("a change at line 2");
        assert_eq!(hunk.old_lines, ["two"]);
        assert_eq!(hunk.new_lines, ["TWO"]);
        app.revert_hunk(&hunk);
        assert_eq!(app.active_tab().text_content, "one\ntwo\nthree\nfour\n");

        // Line 1 is unchanged: the popup has nothing to show and closes.
        app.hunk_popup = Some(HunkPopup { tab: app.active_tab_index, line: 1, pos: egui::Pos2::ZERO });
        run_frame_with_events(&ctx, Vec::new(), |ui| app.show_hunk_popup(ui.ctx()));
        assert!(app.hunk_popup.is_none());
        let _ = fs::remove_dir_all(repo.root());
    }

    /// The editor's right-click menu toggles the bookmark of the line under
    /// the pointer: "Tambah Bookmark" adds it, "Hapus Bookmark" removes it.
    #[test]
    fn test_context_menu_toggles_the_bookmark_of_the_clicked_line() {
        let mut app = TextEditorApp::default();
        let doc = "alpha\nbeta\ngamma";
        app.active_tab_mut().text_content = doc.into();
        let ctx = egui::Context::default();
        let id = app.editor_id();
        let frame = |app: &mut TextEditorApp, events| {
            run_frame_with_events(&ctx, events, |ui| {
                let tab = &mut app.tabs[0];
                let (_, editor, _) = super::render_editor_with(
                    ui,
                    &mut tab.text_content,
                    None,
                    false,
                    true,
                    "",
                    Some(id),
                    EditorRequest::default(),
                    None,
                    &mut tab.bookmarks,
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
                app.show_editor_context_menu(ui.ctx(), &editor);
            })
        };
        let (galley, _) = painted_text(&frame(&mut app, Vec::new()), doc).expect("text painted");
        let line2 = egui::pos2(galley.left() + 4.0, galley.center().y);
        // Right-clicks line 2 and clicks the menu item `label`.
        let toggle_via_menu = |app: &mut TextEditorApp, label: &str| {
            let right = |pressed| egui::Event::PointerButton {
                pos: line2,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(line2), right(true)]);
            frame(app, vec![right(false)]);
            let (item, _) = painted_text(&frame(app, Vec::new()), label)
                .unwrap_or_else(|| panic!("menu item {label:?}"));
            frame(app, click_events(item.center()));
            // The closed menu still covers line 2 for one frame.
            frame(app, Vec::new());
        };

        toggle_via_menu(&mut app, "Tambah Bookmark");
        assert_eq!(
            app.active_tab().bookmarks.lines().collect::<Vec<_>>(),
            vec![2]
        );
        toggle_via_menu(&mut app, "Hapus Bookmark");
        assert_eq!(app.active_tab().bookmarks.len(), 0);
    }

    /// A bookmarked line gets a marker in the gutter, left of its number.
    #[test]
    fn test_bookmarked_line_is_marked_in_the_gutter() {
        let ctx = egui::Context::default();
        let mut text = "alpha\nbeta\ngamma".to_string();
        let mut bookmarks = Bookmarks::default();
        bookmarks.toggle(2, &text);
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let request = EditorRequest::default();
                super::render_editor_with(
                    ui,
                    &mut text,
                    None,
                    false,
                    true,
                    "",
                    None,
                    request,
                    None,
                    &mut bookmarks,
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
        });
        let color = crate::theme::active(&ctx).gutter.bookmark;
        let marks: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == color => Some(rect.rect),
                _ => None,
            })
            .collect();
        assert_eq!(marks.len(), 1, "one bookmarked line");
        let (number, _) = painted_number(&shapes, 2).expect("line 2 numbered");
        assert!(
            (marks[0].center().y - number.center().y).abs() < 1.0,
            "level with line 2"
        );
        assert!(marks[0].right() < number.left());
    }

    /// Lines typed above a bookmark push it down with its line.
    #[test]
    fn test_bookmarks_follow_lines_typed_above_them() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique(("editor", 0));
        let mut text = "alpha\nbeta".to_string();
        let mut bookmarks = Bookmarks::default();
        bookmarks.toggle(2, &text);
        let mut frame = |events, request: EditorRequest| {
            let mut request = Some(request);
            run_frame_with_events(&ctx, events, |ui| {
                let request = request.take().unwrap_or_default();
                super::render_editor_with(
                    ui,
                    &mut text,
                    None,
                    false,
                    true,
                    "",
                    Some(id),
                    request,
                    None,
                    &mut bookmarks,
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            });
        };
        frame(
            Vec::new(),
            EditorRequest {
                focus: true,
                select: Some((0, 0)),
                ..Default::default()
            },
        );
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![enter], EditorRequest::default());
        assert_eq!(text, "\nalpha\nbeta");
        assert_eq!(bookmarks.lines().collect::<Vec<_>>(), vec![3]);
    }

    /// The list shows each bookmarked line with its text; picking one puts
    /// the caret there and leaves the list open.
    #[test]
    fn test_bookmark_list_jumps_to_the_picked_line() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "alpha\n  beta\ngamma".into();
        app.toggle_bookmark(3);
        app.toggle_bookmark(2);
        assert_eq!(app.bookmarked_rows(), vec![(2, "  beta"), (3, "gamma")]);

        app.show_bookmarks = true;
        let ctx = egui::Context::default();
        let frame = |app: &mut TextEditorApp, events| {
            run_frame_with_events(&ctx, events, |ui| app.show_bookmarks_window(ui.ctx()))
        };
        // A window is sized, unseen, on its first frame.
        frame(&mut app, Vec::new());
        let (row, _) =
            painted_text(&frame(&mut app, Vec::new()), "Baris 3: gamma").expect("row of line 3");
        assert!(
            painted_text(&frame(&mut app, Vec::new()), "Baris 2: beta").is_some(),
            "text trimmed"
        );

        frame(&mut app, click_events(row.center()));
        assert_eq!(
            app.editor_request.goto,
            Some(13),
            "caret at the start of line 3"
        );
        assert!(app.show_bookmarks);
    }

    #[test]
    fn test_english_translates_title_status_and_menus() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        assert!(app.i18n.select("en"));
        assert_eq!(app.window_title(), "[New Document] - Red Text Editor");

        let ctx = egui::Context::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_view_menu(ui));
        for label in ["Word Wrap", "Line Numbers", "Language", "Interface Language"] {
            assert!(painted_text(&shapes, label).is_some(), "View menu lacks {label}");
        }
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "Document: [New Document]").is_some());
    }

    /// The View menu offers the zoom commands, and the status bar shows the
    /// size when it is not the default one.
    #[test]
    fn test_the_view_menu_offers_the_zoom_commands() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        assert!(app.i18n.select("en"));
        let ctx = egui::Context::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_view_menu(ui));
        assert!(painted_text(&shapes, "Zoom").is_some(), "View menu lacks Zoom");

        // The size is only shown when it is not the default one.
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "Font size: 100%").is_none());

        app.font_size = 20.0;
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "Font size: 154%").is_some());
    }

    #[test]
    fn test_plain_text_label_is_translated() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.language_label(Language::PlainText), "Teks Biasa");
        app.i18n.select("en");
        assert_eq!(app.language_label(Language::PlainText), "Plain Text");
        assert_eq!(app.language_label(Language::Rust), "Rust");
    }

    /// An in-memory stand-in for eframe's on-disk storage.
    #[derive(Default)]
    struct MemoryStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn test_save_remembers_the_interface_language() {
        let mut app = TextEditorApp::default();
        app.i18n.select("en");
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert_eq!(
            eframe::Storage::get_string(&storage, super::UI_LANGUAGE_KEY).as_deref(),
            Some("en")
        );
    }

    #[test]
    fn test_save_remembers_the_editor_settings() {
        let mut app = TextEditorApp::default();
        app.editor_settings.tab_size = 2;
        app.editor_settings.format_on_save = true;
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        let text = eframe::Storage::get_string(&storage, super::EDITOR_SETTINGS_KEY);
        assert_eq!(EditorSettings::from_storage(text.as_deref()), app.editor_settings);
    }

    fn history_of(app: &TextEditorApp) -> Vec<(PathBuf, EntryKind)> {
        app.history
            .entries()
            .iter()
            .map(|entry| (entry.path.clone(), entry.kind))
            .collect()
    }

    #[test]
    fn test_opened_files_and_folders_are_recorded_newest_first() {
        let dir = history_folder("record");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(a.clone());
        app.open_file_in_tab(b.clone());
        app.open_folder(dir.clone());
        // Opened again: it moves to the front.
        app.open_file_in_tab(a.clone());
        assert_eq!(
            history_of(&app),
            vec![
                (a, EntryKind::File),
                (dir.clone(), EntryKind::Folder),
                (b, EntryKind::File),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_opening_a_folder_resets_tabs_and_search() {
        let dir = history_folder("reset");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(a.clone());
        app.open_file_in_tab(b.clone());
        app.active_tab_mut().text_content = "edited".into();
        app.search.visible = true;
        app.search.query = "needle".into();
        app.search.include = "*.rs".into();
        app.search.searched = true;

        app.open_folder(dir.clone());

        // Saved tabs go; the unsaved one stays so no work is lost.
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&b));
        assert_eq!(app.active_tab_index, 0);
        assert!(!app.search.visible);
        assert!(app.search.query.is_empty());
        assert!(app.search.include.is_empty());
        assert!(!app.search.searched);
        assert!(app.search.hits.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_opening_a_folder_with_only_saved_tabs_leaves_a_blank_editor() {
        let dir = history_folder("reset_clean");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("a.txt"));
        app.open_folder(dir.clone());
        assert_eq!(app.tabs.len(), 1);
        assert!(app.active_tab().is_pristine());
        assert!(!app.show_welcome);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_files_that_fail_to_open_are_not_recorded() {
        let missing = std::env::temp_dir().join("rred_history_no_such_file.txt");
        let _ = fs::remove_file(&missing);
        let mut app = TextEditorApp::default();
        // Once into the blank tab, once into a new tab beside an edited one.
        app.open_file_in_tab(missing.clone());
        app.active_tab_mut().text_content = "edited".into();
        app.open_file_in_tab(missing);
        assert!(app.history.is_empty());
        assert!(matches!(app.status_message, Some((_, true))));
    }

    #[test]
    fn test_open_recent_reopens_files_and_folders() {
        let dir = history_folder("reopen");
        let a = dir.join("a.txt");
        let mut app = TextEditorApp::default();
        app.open_recent(Entry {
            path: dir.clone(),
            kind: EntryKind::Folder,
        });
        assert_eq!(
            app.explorer.as_ref().map(Explorer::root),
            Some(dir.as_path())
        );
        assert!(app.show_sidebar);

        app.open_recent(Entry {
            path: a.clone(),
            kind: EntryKind::File,
        });
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&a));
        assert_eq!(app.active_tab().text_content, "a");
        assert_eq!(
            history_of(&app),
            vec![(a, EntryKind::File), (dir.clone(), EntryKind::Folder)]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_recent_of_a_missing_path_reports_it_and_drops_it() {
        let dir = history_folder("missing");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("a.txt"));
        app.open_folder(dir.clone());
        app.close_tab(0);
        app.explorer = None;
        fs::remove_dir_all(&dir).unwrap();

        for entry in app.history.entries().to_vec() {
            let path = entry.path.display().to_string();
            app.open_recent(entry);
            let message = app.i18n.tf("msg.recent_missing", &[("path", &path)]);
            assert_eq!(app.status_message, Some((message, true)));
        }
        assert!(app.history.is_empty(), "missing entries are dropped");
        assert!(app.active_tab().is_pristine(), "no file was opened");
        assert!(app.explorer.is_none(), "no folder was opened");
    }

    /// Runs one frame of auto-save at egui time `time`, with the window
    /// focused or not.
    fn autosave_frame(ctx: &egui::Context, app: &mut TextEditorApp, time: f64, focused: bool) {
        let input = egui::RawInput {
            time: Some(time),
            focused,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.autosave(ui.ctx()));
        output.textures_delta.clear();
    }

    /// Auto-save writes once the typing pauses, not before.
    #[test]
    fn test_auto_save_waits_for_the_pause_then_writes() {
        let dir = history_folder("autosave_pause");
        let path = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        assert!(!app.auto_save, "off until the user asks for it");
        app.toggle_auto_save();
        app.active_tab_mut().text_content = "edited".to_owned();

        // The edit starts the pause at t=10; nothing is written before it
        // runs out.
        autosave_frame(&ctx, &mut app, 10.0, true);
        autosave_frame(&ctx, &mut app, 10.5, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a", "saved too early");
        assert!(app.active_tab().is_dirty());

        autosave_frame(&ctx, &mut app, 11.1, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited");
        assert!(!app.active_tab().is_dirty(), "the tab is clean again");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_explicit_save_trims_and_formats_when_on() {
        let dir = history_folder("save_trim_format");
        let path = dir.join("a.json");
        fs::write(&path, "{}").unwrap();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.editor_settings.trim_trailing_whitespace = true;
        app.editor_settings.format_on_save = true;
        app.active_tab_mut().text_content = "{\"a\":1}   ".to_owned();
        assert!(app.save());
        let expected = prettify::prettify(Language::Json, "{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(app.active_tab().text_content, expected, "the editor shows what was saved");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_explicit_save_leaves_the_text_alone_when_off() {
        let dir = history_folder("save_plain");
        let path = dir.join("a.json");
        fs::write(&path, "{}").unwrap();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.active_tab_mut().text_content = "{\"a\":1}   ".to_owned();
        assert!(app.save());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}   ");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_a_failing_formatter_still_saves_and_reports() {
        let dir = history_folder("save_bad_json");
        let path = dir.join("a.json");
        fs::write(&path, "{}").unwrap();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.editor_settings.format_on_save = true;
        app.active_tab_mut().text_content = "{ not json".to_owned();
        assert!(app.save());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        let (_, is_error) = app.status_message.clone().unwrap();
        assert!(is_error, "the formatter's failure is reported, not 'Saved'");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_auto_save_neither_trims_nor_formats() {
        let dir = history_folder("autosave_no_trim");
        let path = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.editor_settings.trim_trailing_whitespace = true;
        app.toggle_auto_save();
        app.active_tab_mut().text_content = "edited  ".to_owned();
        autosave_frame(&ctx, &mut app, 10.0, true);
        autosave_frame(&ctx, &mut app, 11.1, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited  ");
        let _ = fs::remove_dir_all(dir);
    }

    /// A typing pause that never comes still saves when the window loses
    /// focus.
    #[test]
    fn test_auto_save_writes_when_the_window_loses_focus() {
        let dir = history_folder("autosave_focus");
        let path = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.toggle_auto_save();
        app.active_tab_mut().text_content = "edited".to_owned();

        autosave_frame(&ctx, &mut app, 10.0, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a", "still typing");
        autosave_frame(&ctx, &mut app, 10.2, false);
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited");
        let _ = fs::remove_dir_all(dir);
    }

    /// A file another program changed is the conflict dialog's job: auto-save
    /// leaves it alone.
    #[test]
    fn test_auto_save_leaves_a_file_changed_on_disk_alone() {
        let dir = history_folder("autosave_conflict");
        let path = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.toggle_auto_save();
        app.active_tab_mut().text_content = "mine".to_owned();
        // Another program writes the file after the tab read it.
        fs::write(&path, "theirs").unwrap();

        autosave_frame(&ctx, &mut app, 10.0, true);
        autosave_frame(&ctx, &mut app, 11.5, true);

        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert!(app.active_tab().is_dirty(), "the edit waits for the user");
        let _ = fs::remove_dir_all(dir);
    }

    /// Turning auto-save on with edits already made saves them once the
    /// typing pauses too, not only after the next edit.
    #[test]
    fn test_enabling_auto_save_saves_what_was_already_unsaved() {
        let dir = history_folder("autosave_late");
        let path = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(path.clone());
        app.active_tab_mut().text_content = "edited".to_owned();

        // Off: nothing is scheduled, and the file is left as it was.
        autosave_frame(&ctx, &mut app, 5.0, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a");

        app.toggle_auto_save();
        autosave_frame(&ctx, &mut app, 6.0, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a", "saved at once");
        autosave_frame(&ctx, &mut app, 7.1, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited");
        let _ = fs::remove_dir_all(dir);
    }

    /// An untitled tab has nowhere to save to; auto-save leaves it dirty.
    #[test]
    fn test_auto_save_leaves_an_untitled_tab_alone() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.toggle_auto_save();
        app.active_tab_mut().text_content = "draft".to_owned();

        autosave_frame(&ctx, &mut app, 10.0, true);
        autosave_frame(&ctx, &mut app, 11.5, true);

        assert!(app.active_tab().is_dirty());
        assert!(app.active_tab().current_path.is_none());
    }

    #[test]
    fn test_auto_save_is_saved_and_restored() {
        let mut app = TextEditorApp::default();
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert!(!saved_auto_save(Some(&storage)), "off by default");

        app.auto_save = true;
        eframe::App::save(&mut app, &mut storage);
        assert!(saved_auto_save(Some(&storage)));
        assert!(!saved_auto_save(None));
    }

    /// Go to Symbol lists a Markdown document's headings, and a JavaScript
    /// file's functions; Go to Definition stays a code-only command.
    #[test]
    fn test_go_to_symbol_covers_markdown_and_javascript() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "# Guide\n\n## Install\n".to_owned();
        app.active_tab_mut().active_language = Language::Markdown;
        app.open_symbol_picker();
        assert!(app.symbol_picker.visible, "no outline for Markdown");
        let names: Vec<&str> =
            app.symbol_picker.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Guide", "Install"]);

        // A heading is no identifier: Go to Definition does nothing.
        app.symbol_picker.visible = false;
        app.goto_definition(&ctx, Some(3));
        assert!(!app.search.visible, "a heading was looked up as a definition");

        app.active_tab_mut().text_content = "function load() {}\nload();\n".to_owned();
        app.active_tab_mut().active_language = Language::JavaScript;
        app.open_symbol_picker();
        let names: Vec<&str> =
            app.symbol_picker.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["load"]);
        // Go to Definition on the call jumps to the function's line.
        app.symbol_picker.visible = false;
        app.goto_definition(&ctx, Some(20));
        assert_eq!(app.editor_request.goto, Some(0), "did not jump to line 1");
    }

    /// Dropping a file on the window opens it in a tab, from the start page
    /// too.
    #[test]
    fn test_a_dropped_file_opens_in_a_tab() {
        let dir = history_folder("dropped_file");
        let path = dir.join("a.txt");
        let mut app = TextEditorApp::default();
        assert!(app.show_welcome, "the start page is the default");

        app.open_dropped(vec![path.clone()]);

        assert!(!app.show_welcome, "the drop left the start page");
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&path));
        assert_eq!(app.active_tab().text_content, "a");
        assert_eq!(app.history.entries().len(), 1, "the file is recent");
        let _ = fs::remove_dir_all(dir);
    }

    /// A dropped folder becomes the workspace, and the files dropped beside
    /// it are still open afterwards: folders are handled first, so the
    /// switch to the new workspace does not close them again.
    #[test]
    fn test_a_dropped_folder_becomes_the_workspace_beside_its_files() {
        let dir = history_folder("dropped_folder");
        let (old, file) = (dir.join("b.txt"), dir.join("a.txt"));
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(old.clone());

        app.open_dropped(vec![file.clone(), dir.clone()]);

        let root = app.explorer.as_ref().map(|e| e.root().to_path_buf());
        assert_eq!(root, Some(dir.clone()), "the folder did not become the workspace");
        assert!(
            app.tabs.iter().any(|t| t.current_path.as_ref() == Some(&file)),
            "the dropped file is not open"
        );
        assert!(
            !app.tabs.iter().any(|t| t.current_path.as_ref() == Some(&old)),
            "the old workspace's saved tab stayed open"
        );
        assert_eq!(app.sidebar_view, SidebarView::Explorer);
        let _ = fs::remove_dir_all(dir);
    }

    /// Closing the diff view with the folder it belongs to matters: the view
    /// is indexed by tab, and the new folder replaces the tabs.
    #[test]
    fn test_opening_a_folder_closes_the_diff_view() {
        let dir = history_folder("dropped_diff");
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_file_in_tab(dir.join("a.txt"));
        let ctx = egui::Context::default();
        app.toggle_diff_view(&ctx, Against::File { path: dir.join("b.txt"), text: "b".to_owned() });
        assert!(app.diff_view.is_some(), "the diff view did not open");

        app.open_folder(dir.clone());

        assert!(app.diff_view.is_none(), "the diff view outlived its tab");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_theme_is_saved_and_restored() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.theme, ThemeId::System);
        app.theme = ThemeId::SolarizedDark;
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert_eq!(
            eframe::Storage::get_string(&storage, super::THEME_KEY).as_deref(),
            Some("solarized-dark")
        );
        assert_eq!(saved_theme(Some(&storage)), ThemeId::SolarizedDark);
        assert_eq!(saved_theme(None), ThemeId::System);
    }

    #[test]
    fn test_the_font_size_is_saved_and_restored() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE);
        app.font_size = 18.0;
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert_eq!(
            eframe::Storage::get_string(&storage, super::FONT_SIZE_KEY).as_deref(),
            Some("18.0")
        );
        assert_eq!(saved_font_size(Some(&storage)), 18.0);
        assert_eq!(saved_font_size(None), zoom::DEFAULT_SIZE);
    }

    /// A size that is out of range in the setting is brought back in, so a
    /// corrupt setting cannot make the text unreadable.
    #[test]
    fn test_a_saved_font_size_out_of_range_is_clamped() {
        let mut storage = MemoryStorage::default();
        eframe::Storage::set_string(&mut storage, super::FONT_SIZE_KEY, "1000".into());
        assert_eq!(saved_font_size(Some(&storage)), zoom::MAX_SIZE);
        eframe::Storage::set_string(&mut storage, super::FONT_SIZE_KEY, "not a number".into());
        assert_eq!(saved_font_size(Some(&storage)), zoom::DEFAULT_SIZE);
    }

    #[test]
    fn test_zooming_makes_the_font_bigger_and_smaller() {
        let mut app = TextEditorApp::default();
        app.zoom(1);
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE + zoom::STEP as f32);
        app.zoom(-1);
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE);
        app.zoom(5);
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE + 5.0 * zoom::STEP as f32);
    }

    #[test]
    fn test_zooming_stops_at_the_ends() {
        let mut app = TextEditorApp { font_size: zoom::MAX_SIZE, ..Default::default() };
        app.zoom(1);
        assert_eq!(app.font_size, zoom::MAX_SIZE);
        app.font_size = zoom::MIN_SIZE;
        app.zoom(-1);
        assert_eq!(app.font_size, zoom::MIN_SIZE);
    }

    #[test]
    fn test_resetting_the_zoom_returns_to_the_default_size() {
        let mut app = TextEditorApp::default();
        app.zoom(10);
        app.reset_zoom();
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE);
    }

    /// The font size reaches egui's monospace text style, so every part of
    /// the editor that draws code uses it.
    #[test]
    fn test_a_frame_writes_the_font_size_into_the_style() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { font_size: 20.0, ..Default::default() };
        run_frame_with_events(&ctx, Vec::new(), |ui| app.apply_font_size(ui));
        let font = ctx.global_style().text_styles[&egui::TextStyle::Monospace].clone();
        assert_eq!(font.size, 20.0);
    }

    /// `Ctrl` + scroll makes the text bigger, and the scroll is taken out of
    /// the input so the editor does not scroll as well.
    #[test]
    fn test_ctrl_scroll_makes_the_text_bigger() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::COMMAND,
        };
        run_frame_with_events(&ctx, vec![wheel], |ui| app.zoom_with_wheel(ui.ctx()));
        assert!(app.font_size > zoom::DEFAULT_SIZE, "{}", app.font_size);
    }

    /// A scroll without the command key is left alone: it scrolls.
    #[test]
    fn test_scroll_without_the_command_key_does_not_zoom() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame_with_events(&ctx, vec![wheel], |ui| app.zoom_with_wheel(ui.ctx()));
        assert_eq!(app.font_size, zoom::DEFAULT_SIZE);
    }

    /// The zoom shortcuts reach the app and change the font size.
    #[test]
    fn test_the_zoom_shortcuts_change_the_font_size() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        for (key, want) in [
            (egui::Key::Plus, zoom::DEFAULT_SIZE + zoom::STEP as f32),
            (egui::Key::Minus, zoom::DEFAULT_SIZE),
            (egui::Key::Num0, zoom::DEFAULT_SIZE),
        ] {
            let mut action = None;
            run_frame_with_events(&ctx, vec![press(key)], |ui| {
                action = ui.ctx().input_mut(crate::shortcuts::consume_shortcut);
            });
            assert_eq!(action, Some(match key {
                egui::Key::Plus => ShortcutAction::ZoomIn,
                egui::Key::Minus => ShortcutAction::ZoomOut,
                _ => ShortcutAction::ZoomReset,
            }));
            app.apply_shortcut(&ctx, action.expect("a shortcut"));
            assert_eq!(app.font_size, want, "{key:?}");
        }
    }

    /// The whole point of the font size: the text the editor paints grows
    /// and shrinks with it.
    #[test]
    fn test_the_editor_text_grows_with_the_font_size() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "hello".into();
        let frame = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            })
        };
        let width = |app: &mut TextEditorApp| {
            let shapes = frame(app);
            painted_text(&shapes, "hello")
                .expect("the editor painted its text")
                .0
                .width()
        };

        let small = width(&mut app);
        app.zoom(6);
        let big = width(&mut app);
        assert!(big > small, "the text did not grow: {small} vs {big}");
    }

    /// The gutter, the diff view and the terminal draw their text in the
    /// monospace font, so one size moves all of them.
    #[test]
    fn test_the_font_size_reaches_every_part_that_draws_code() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { font_size: 20.0, ..Default::default() };
        run_frame_with_events(&ctx, Vec::new(), |ui| app.apply_font_size(ui));
        let font = ctx.global_style().text_styles[&egui::TextStyle::Monospace].clone();
        assert_eq!(font.size, 20.0);
        // The style every one of them resolves its monospace font from.
        let resolved = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        assert_eq!(resolved.size, 20.0);
        assert_eq!(resolved.family, font.family);
    }

    #[test]
    fn test_a_frame_applies_the_chosen_theme() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let frame = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            });
        };
        app.theme = ThemeId::EightiesDark;
        frame(&mut app);
        assert_eq!(theme::active(&ctx), ThemeId::EightiesDark.theme());
        assert_eq!(app.highlighter.theme_name(), "base16-eighties.dark");

        app.theme = ThemeId::LightModern;
        frame(&mut app);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(app.highlighter.theme_name(), "InspiredGitHub");
    }

    #[test]
    fn test_history_is_saved_and_restored() {
        let mut app = TextEditorApp::default();
        app.history
            .record(PathBuf::from("/project"), EntryKind::Folder);
        app.history
            .record(PathBuf::from("/project/a.rs"), EntryKind::File);
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert!(eframe::Storage::get_string(&storage, super::HISTORY_KEY).is_some());
        assert_eq!(saved_history(Some(&storage)), app.history);
        assert_eq!(saved_history(None), History::default());
    }

    #[test]
    fn test_session_is_saved_and_restored() {
        let dir = std::env::temp_dir().join("rred_test_app_session");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();
        let mut app = TextEditorApp::default();
        app.open_folder(dir.clone());
        app.open_file_in_tab(file.clone());
        app.active_tab_mut().caret = 7;
        app.active_tab_mut().scroll = 24.0;
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert!(eframe::Storage::get_string(&storage, SESSION_KEY).is_some());

        let session = saved_session(Some(&storage));
        assert_eq!(session.folder.as_ref(), Some(&dir));
        let paths: Vec<&PathBuf> = session.tabs.iter().map(|tab| &tab.path).collect();
        assert_eq!(paths, [&file]);
        assert_eq!((session.tabs[0].caret, session.tabs[0].scroll), (7, 24.0));
        assert_eq!(saved_session(None), Session::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Three hundred lines, tall enough to scroll.
    fn long_document() -> String {
        (0..300).map(|i| format!("line {i}\n")).collect()
    }

    /// How many editor galleys carry `text`: one per view of the document.
    fn views_of(shapes: &[egui::epaint::ClippedShape], text: &str) -> usize {
        fn walk(shape: &egui::Shape, text: &str) -> usize {
            match shape {
                egui::Shape::Vec(items) => items.iter().map(|item| walk(item, text)).sum(),
                egui::Shape::Text(shape) if shape.galley.text() == text => 1,
                _ => 0,
            }
        }
        shapes.iter().map(|clipped| walk(&clipped.shape, text)).sum()
    }

    /// Split Editor shows the document twice, side by side, each view with
    /// its own editor.
    #[test]
    fn test_split_editor_shows_the_document_twice() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "unique-line".to_owned();
        app.active_tab_mut().saved_content = "unique-line".to_owned();
        let shapes = |ctx: &egui::Context, app: &mut TextEditorApp| {
            run_frame_with_events(ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            })
        };

        let single = shapes(&ctx, &mut app);
        assert_eq!(views_of(&single, "unique-line"), 1, "one view while whole");

        app.toggle_split();
        let split = shapes(&ctx, &mut app);
        assert_eq!(views_of(&split, "unique-line"), 2, "the document is not twice");
        // Both editors keep their own state.
        let main = egui::TextEdit::load_state(&ctx, app.editor_id_of(Pane::Main));
        let side = egui::TextEdit::load_state(&ctx, app.editor_id_of(Pane::Split));
        assert!(main.is_some() && side.is_some(), "a view has no state");

        app.toggle_split();
        let closed = shapes(&ctx, &mut app);
        assert_eq!(views_of(&closed, "unique-line"), 1, "the split did not close");
    }

    /// Clicking the second view moves the keyboard to it, so the status bar
    /// and the menus act there.
    #[test]
    fn test_the_second_view_can_take_the_keyboard() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "one\ntwo\nthree".to_owned();
        app.active_tab_mut().saved_content = "one\ntwo\nthree".to_owned();
        app.toggle_split();
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
        });
        assert_eq!(app.focused_pane, Pane::Main);

        // A click in the right-hand view.
        run_frame_with_events(&ctx, click_events(egui::pos2(600.0, 300.0)), |ui| {
            eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
        });
        assert_eq!(app.focused_pane, Pane::Split, "the click did not move the keyboard");
    }

    /// The two views scroll on their own: the wheel over one moves only
    /// that one.
    #[test]
    fn test_each_view_scrolls_on_its_own() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let text = long_document();
        app.active_tab_mut().text_content = text.clone();
        app.active_tab_mut().saved_content = text;
        app.toggle_split();
        let frame = |ctx: &egui::Context, app: &mut TextEditorApp, events: Vec<egui::Event>| {
            run_frame_with_events(ctx, events, |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            })
        };
        let wheel = |at: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -1400.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };

        frame(&ctx, &mut app, Vec::new());
        // The right-hand view takes the keyboard, then the wheel scrolls it.
        // The pointer stays over it while egui's smooth scrolling runs out,
        // so no of it is left for the other view.
        frame(&ctx, &mut app, click_events(egui::pos2(600.0, 300.0)));
        assert_eq!(app.focused_pane, Pane::Split);
        frame(&ctx, &mut app, wheel(egui::pos2(600.0, 300.0)));
        // egui spreads a wheel notch over the frames that follow: let it run
        // out with the pointer still over the view it began in, or the rest
        // of it lands in the other view.
        for _ in 0..30 {
            frame(&ctx, &mut app, Vec::new());
        }
        let side = app.active_tab().scroll;
        assert!(side > 100.0, "the second view did not scroll: {side}");

        // The left view kept its own place, at the top.
        frame(&ctx, &mut app, click_events(egui::pos2(200.0, 120.0)));
        for _ in 0..3 {
            frame(&ctx, &mut app, Vec::new());
        }
        assert_eq!(app.focused_pane, Pane::Main);
        let main = app.active_tab().scroll;
        assert!(main < 1.0, "the first view moved too: {main}");
    }

    /// The frame records where the caret and the view are, so the session
    /// saves the position the tab was left at.
    #[test]
    fn test_the_frame_records_the_caret_and_scroll_for_the_session() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let text = long_document();
        app.active_tab_mut().text_content = text.clone();
        app.active_tab_mut().saved_content = text;
        app.editor_request = EditorRequest { focus: true, select: Some((42, 42)), ..Default::default() };

        run_frame_with_events(&ctx, Vec::new(), |ui| {
            eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
        });

        assert_eq!(app.active_tab().caret, 42, "the caret is not recorded");
    }

    /// A tab restored from the session puts its caret back and opens at the
    /// offset it was left at, on its first frame only.
    #[test]
    fn test_a_restored_tab_opens_at_its_caret_and_scroll() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let text = long_document();
        app.active_tab_mut().text_content = text.clone();
        app.active_tab_mut().saved_content = text;
        app.active_tab_mut().caret = 42;
        app.active_tab_mut().scroll = 600.0;
        app.active_tab_mut().restore_view = true;

        run_frame_with_events(&ctx, Vec::new(), |ui| {
            eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
        });

        let caret = egui::TextEdit::load_state(&ctx, app.editor_id())
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.0);
        assert_eq!(caret, Some(42), "the caret did not come back");
        assert!(!app.active_tab().restore_view, "the restore is applied once");
        assert!(
            (app.active_tab().scroll - 600.0).abs() < 1.0,
            "the view did not come back: {}",
            app.active_tab().scroll
        );
    }

    /// Scrolling the editor is part of the position: what the frame shows is
    /// what the session saves.
    #[test]
    fn test_scrolling_the_editor_is_recorded_for_the_session() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let text = long_document();
        app.active_tab_mut().text_content = text.clone();
        app.active_tab_mut().saved_content = text;

        // The pointer over the editor, then the wheel rolling down.
        let events = vec![
            egui::Event::PointerMoved(egui::pos2(400.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -300.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, events, |ui| {
            eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
        });

        assert!(
            app.active_tab().scroll > 0.0,
            "the scroll was not recorded: {}",
            app.active_tab().scroll
        );
    }

    /// View > Indent Guides and Render Whitespace reach the editor: the
    /// guides are thin lines, the whitespace marks are dots.
    #[test]
    fn test_the_view_toggles_paint_the_decorations() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        let text = "    a b\n".to_owned();
        app.active_tab_mut().text_content = text.clone();
        app.active_tab_mut().saved_content = text;
        let shapes = |ctx: &egui::Context, app: &mut TextEditorApp| {
            run_frame_with_events(ctx, Vec::new(), |ui| {
                eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
            })
        };
        // Guides are painted together in one `Shape::Vec`.
        fn count(shapes: &[egui::epaint::ClippedShape], what: fn(&egui::Shape) -> bool) -> usize {
            fn walk(shape: &egui::Shape, what: fn(&egui::Shape) -> bool) -> usize {
                match shape {
                    egui::Shape::Vec(items) => items.iter().map(|item| walk(item, what)).sum(),
                    _ => usize::from(what(shape)),
                }
            }
            shapes.iter().map(|clipped| walk(&clipped.shape, what)).sum()
        }
        let thin = |shapes: &[egui::epaint::ClippedShape]| {
            count(shapes, |shape| {
                matches!(shape, egui::Shape::Rect(rect) if rect.rect.width() <= 1.5 && rect.rect.height() > 2.0)
            })
        };
        let dots = |shapes: &[egui::epaint::ClippedShape]| {
            count(shapes, |shape| matches!(shape, egui::Shape::Circle(_)))
        };

        app.show_indent_guides = false;
        app.show_whitespace = false;
        let plain = shapes(&ctx, &mut app);

        app.show_indent_guides = true;
        let guides = shapes(&ctx, &mut app);
        assert!(thin(&guides) > thin(&plain), "no guide was painted");

        app.show_whitespace = true;
        let both = shapes(&ctx, &mut app);
        // The line holds five spaces: four of indentation, one between the
        // words.
        assert!(
            dots(&both) >= dots(&guides) + 5,
            "whitespace marks were not painted: {} to {}",
            dots(&guides),
            dots(&both)
        );
    }

    #[test]
    fn test_line_numbers_default_to_on() {
        assert!(TextEditorApp::default().show_line_numbers);
    }

    #[test]
    fn test_tab_bar_ui_rendering_and_interaction() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab(); // Tab 0 and Tab 1, both blank
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);

        let mut bar = None;
        run_frame_with_events(&ctx, Vec::new(), |ui| bar = Some(app.render_tab_bar(ui)));
        let tab_0 = bar.take().unwrap().tab_rects[0].center();

        run_frame_with_events(&ctx, click_events(tab_0), |ui| bar = Some(app.render_tab_bar(ui)));
        assert_eq!(app.active_tab_index, 0, "clicking tab 0 must activate it");
        let close_1 = bar.take().unwrap().close_rects[1].center();

        run_frame_with_events(&ctx, click_events(close_1), |ui| bar = Some(app.render_tab_bar(ui)));
        assert_eq!(app.tabs.len(), 1, "clicking tab 1's close slot must close it");
        assert_eq!(app.active_tab_index, 0);

        run_frame_with_events(&ctx, Vec::new(), |ui| bar = Some(app.render_tab_bar(ui)));
        let plus = bar.take().unwrap().new_tab_rect.center();

        run_frame_with_events(&ctx, click_events(plus), |ui| {
            app.render_tab_bar(ui);
        });
        assert_eq!(app.tabs.len(), 2, "clicking + must open a new tab");
        assert_eq!(app.active_tab_index, 1);
    }

    #[test]
    fn test_tab_view_carries_dirty_state_instead_of_a_title_marker() {
        let mut tab = DocumentTab::default_new();
        tab.text_content.push('!');
        tab.active_language = Language::Php;
        let view = tab.tab_view("[Dokumen Baru]");
        assert_eq!(view.title, "[Dokumen Baru]");
        assert!(view.dirty);
        assert_eq!(view.language, Language::Php);
        assert_eq!(view.tooltip, None);

        tab.current_path = Some(PathBuf::from("/tmp/x/index.php"));
        assert_eq!(tab.tab_view("[Dokumen Baru]").tooltip.as_deref(), Some("/tmp/x/index.php"));
    }

    #[test]
    fn test_tab_bar_close_dirty_tab_triggers_pending_action() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);
        app.tabs[1].text_content = "modified dirty content".to_string();
        assert!(app.tabs[1].is_dirty());

        // Frame 1: render tab bar
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.render_tab_bar(ui);
        });

        // Click close button on dirty tab 1
        let click_close_tab_1 = vec![
            egui::Event::PointerMoved(egui::pos2(299.0, 17.0)),
            egui::Event::PointerButton {
                pos: egui::pos2(299.0, 17.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: egui::pos2(299.0, 17.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, click_close_tab_1, |ui| {
            app.render_tab_bar(ui);
        });

        // Tab 1 is dirty, so it should NOT be closed immediately; pending_action is set
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.pending_action, Some(PendingAction::CloseTab(1)));
    }

    #[test]
    fn test_new_window_command_starts_this_executable() {
        let command = new_window_command().expect("the executable is known");
        assert_eq!(
            Path::new(command.get_program()),
            std::env::current_exe().unwrap()
        );
        assert_eq!(command.get_args().count(), 0, "a plain second copy");
    }

    #[test]
    fn test_spawn_window_starts_a_process_or_reports_why_not() {
        // `--list` makes the test binary print its tests and exit at once.
        let mut quick = std::process::Command::new(std::env::current_exe().unwrap());
        quick.arg("--list").stdout(std::process::Stdio::null());
        assert!(spawn_window(quick).is_ok());
        let missing = std::process::Command::new("/no/such/rred_editor_binary");
        assert!(spawn_window(missing).is_err());
    }

    #[test]
    fn test_new_window_leaves_this_window_unchanged() {
        let dir = history_folder("new_window");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("a.txt"));
        app.open_file_in_tab(dir.join("b.txt"));
        app.active_tab_mut().text_content.push_str(" edited");
        app.open_folder(dir.clone());
        app.status_message = None;
        let (tabs, active, history) = (app.tabs.clone(), app.active_tab_index, app.history.clone());

        let mut spawned = 0;
        app.open_new_window_with(|| {
            spawned += 1;
            Ok(())
        });
        assert_eq!(spawned, 1);
        assert_eq!(app.tabs, tabs, "tabs, texts and unsaved edits stay");
        assert_eq!(app.active_tab_index, active);
        assert_eq!(app.history, history);
        assert_eq!(
            app.explorer.as_ref().map(Explorer::root),
            Some(dir.as_path())
        );
        assert_eq!(app.status_message, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_new_window_failure_is_reported() {
        let mut app = TextEditorApp::default();
        app.open_new_window_with(|| Err(std::io::Error::other("boom")));
        let message = app.i18n.tf("msg.new_window_failed", &[("error", "boom")]);
        assert_eq!(app.status_message, Some((message, true)));
        assert_eq!(app.tabs.len(), 1, "this window is unchanged");
    }

    #[test]
    fn test_tab_bar_dirty_indicator_in_title() {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.open_new_tab();
        assert_eq!(app.tabs.len(), 2);
        assert!(!app.tabs[0].is_dirty());
        assert!(!app.tabs[1].is_dirty());

        app.tabs[0].text_content = "dirty buffer".to_string();
        assert!(app.tabs[0].is_dirty());

        let title_0 = format!(
            "{}{}",
            app.tabs[0].file_display_name("[Dokumen Baru]"),
            if app.tabs[0].is_dirty() { " *" } else { "" }
        );
        let title_1 = format!(
            "{}{}",
            app.tabs[1].file_display_name("[Dokumen Baru]"),
            if app.tabs[1].is_dirty() { " *" } else { "" }
        );

        assert_eq!(title_0, "[Dokumen Baru] *");
        assert_eq!(title_1, "[Dokumen Baru]");
    }

    #[test]
    fn test_multi_dirty_tab_exit_loop() {
        let dir = std::env::temp_dir().join("rred_multi_dirty_exit");
        let _ = fs::create_dir_all(&dir);
        let path1 = dir.join("tab1.txt");
        let path2 = dir.join("tab2.txt");
        let _ = fs::write(&path1, "initial 1");
        let _ = fs::write(&path2, "initial 2");

        let mut app = TextEditorApp::default();
        app.load_file(path1.clone());
        app.active_tab_mut().text_content = "modified 1".to_string();

        app.open_new_tab();
        app.load_file(path2.clone());
        app.active_tab_mut().text_content = "modified 2".to_string();

        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[0].is_dirty());
        assert!(app.tabs[1].is_dirty());
        assert!(app.any_dirty());

        let ctx = egui::Context::default();

        // Trigger exit -> sets pending_action to Exit because any_dirty() is true
        app.trigger_exit(&ctx);
        assert_eq!(app.pending_action, Some(PendingAction::Exit));

        // First prompt handles the first dirty tab (tab 0)
        app.handle_modal_response(&ctx, ModalResponse::Save);

        // Tab 0 should now be saved and clean
        assert!(!app.tabs[0].is_dirty());
        assert_eq!(fs::read_to_string(&path1).unwrap(), "modified 1");

        // Tab 1 is still dirty, so pending_action is re-set to Exit (prompting next dirty tab)
        assert!(app.tabs[1].is_dirty());
        assert!(app.any_dirty());
        assert_eq!(app.pending_action, Some(PendingAction::Exit));

        // Second prompt handles tab 1
        app.handle_modal_response(&ctx, ModalResponse::Save);

        // Now tab 1 is saved and clean
        assert!(!app.tabs[1].is_dirty());
        assert_eq!(fs::read_to_string(&path2).unwrap(), "modified 2");
        assert!(!app.any_dirty());

        // No more dirty tabs -> pending_action is cleared and exit finishes
        assert_eq!(app.pending_action, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_markdown_preview_beside_the_graph() {
        let mut app = markdown_app();
        app.show_markdown_preview = true;
        assert!(app.previewing());
        app.git_graph = Some(GitGraphUi::default());
        assert!(!app.previewing());
    }

    fn markdown_app() -> TextEditorApp {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().current_path = Some(PathBuf::from("notes.md"));
        app.active_tab_mut().active_language = Language::Markdown;
        app.active_tab_mut().text_content = "# Title\n\nsome *text*".into();
        app
    }

    #[test]
    fn test_markdown_preview_toggles_only_on_markdown_tabs() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.active_tab_mut().current_path = Some(PathBuf::from("main.rs"));
        assert!(!app.preview_available());
        app.apply_shortcut(&ctx, ShortcutAction::MarkdownPreview);
        assert!(!app.show_markdown_preview, "no preview for a non-Markdown tab");

        let mut app = markdown_app();
        assert!(app.preview_available());
        app.apply_shortcut(&ctx, ShortcutAction::MarkdownPreview);
        assert!(app.show_markdown_preview);
        app.apply_shortcut(&ctx, ShortcutAction::MarkdownPreview);
        assert!(!app.show_markdown_preview);
    }

    #[test]
    fn test_markdown_preview_follows_the_selected_language() {
        let mut app = TextEditorApp::default();
        assert!(!app.preview_available());
        app.active_tab_mut().language_override = Some(Language::Markdown);
        assert!(app.preview_available(), "picking Markdown enables the preview");
        app.active_tab_mut().active_language = Language::Markdown;
        app.active_tab_mut().language_override = Some(Language::PlainText);
        assert!(!app.preview_available(), "an override away from Markdown turns it off");
    }

    #[test]
    fn test_markdown_preview_choice_survives_switching_tabs() {
        let ctx = egui::Context::default();
        let mut app = markdown_app();
        app.apply_shortcut(&ctx, ShortcutAction::MarkdownPreview);
        app.open_new_tab();
        assert!(!app.preview_available(), "an untitled tab has no preview");
        assert!(app.show_markdown_preview, "the choice is kept for the next .md tab");
        app.active_tab_index = 0;
        assert!(app.preview_available() && app.show_markdown_preview);
    }

    /// A whole frame with the preview open must run and leave the buffer alone.
    #[test]
    fn test_frame_with_markdown_preview_open_keeps_the_buffer() {
        let ctx = egui::Context::default();
        let mut app = markdown_app();
        app.active_tab_mut().text_content = "# Heading\n\ntext *here*\n\n".repeat(300);
        let original = app.active_tab().text_content.clone();
        app.show_markdown_preview = true;
        // Several frames: the scroll sync needs one to observe and one to apply.
        for _ in 0..5 {
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
            });
        }
        assert_eq!(app.active_tab().text_content, original);
    }

    #[test]
    fn test_save_tab_restores_active_tab_index() {
        let mut app = TextEditorApp::default();
        app.open_new_tab();
        app.tabs[0].text_content = "saved text".into();
        app.active_tab_index = 1;

        // Cancelled dialog: nothing saved, active tab untouched.
        assert!(!app.save_tab_with(0, |_| None));
        assert_eq!(app.active_tab_index, 1);
        assert!(app.tabs[0].current_path.is_none());

        // Confirmed dialog: the file is written to tab 0, active tab untouched.
        let path = std::env::temp_dir().join(format!("red_save_tab_{}.rs", std::process::id()));
        assert!(app.save_tab_with(0, |_| Some(path.clone())));
        assert_eq!(app.active_tab_index, 1);
        assert_eq!(app.tabs[0].current_path.as_deref(), Some(path.as_path()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved text");
        assert!(!app.tabs[0].is_dirty());
        let _ = fs::remove_file(&path);
    }

    fn rust_app(text: &str) -> TextEditorApp {
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = text.into();
        app.active_tab_mut().language_override = Some(Language::Rust);
        app
    }

    #[test]
    fn test_code_navigation_only_for_its_languages() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        app.active_tab_mut().text_content = "fn main() {}\nmain();\n".into();
        app.apply_shortcut(&ctx, ShortcutAction::GotoSymbol);
        assert!(!app.symbol_picker.visible, "plain text has no symbols");
        app.goto_definition(&ctx, Some(14));
        assert_eq!(app.editor_request.goto, None);
        app.find_references(&ctx, Some(14));
        assert!(!app.search.visible);
    }

    #[test]
    fn test_goto_symbol_lists_the_document() {
        let ctx = egui::Context::default();
        let mut app = rust_app("struct A;\n\nfn run() {}\n");
        app.goto.visible = true;
        app.apply_shortcut(&ctx, ShortcutAction::GotoSymbol);
        assert!(app.symbol_picker.visible && app.symbol_picker.focus);
        assert!(!app.goto.visible, "one bar at a time");
        let names: Vec<_> = app.symbol_picker.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A", "run"]);
    }

    #[test]
    fn test_goto_definition_in_the_document() {
        let ctx = egui::Context::default();
        let mut app = rust_app("fn main() {\n    helper();\n}\n\nfn helper() {}\n");
        // The caret inside `helper` on line 2.
        app.goto_definition(&ctx, Some(18));
        assert_eq!(app.editor_request.goto, Some(find::line_start(&app.active_tab().text_content, 5)));
        assert!(!app.search.visible, "found without searching the folder");
    }

    #[test]
    fn test_goto_definition_without_a_folder_reports_it() {
        let ctx = egui::Context::default();
        let mut app = rust_app("fn main() {\n    missing();\n}\n");
        app.goto_definition(&ctx, Some(18));
        assert_eq!(app.editor_request.goto, None);
        let (message, is_error) = app.status_message.clone().unwrap();
        assert!(message.contains("missing") && is_error, "{message}");
    }

    #[test]
    fn test_goto_definition_opens_a_single_match_in_the_folder() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("red_goto_def_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.rs");
        fs::write(&other, "\npub fn helper() {}\n").unwrap();

        let mut app = rust_app("fn main() {\n    helper();\n}\n");
        app.explorer = Some(Explorer::new(dir.clone()));
        app.goto_definition(&ctx, Some(18));
        assert!(app.search.visible && app.search.query == "helper");
        for _ in 0..200 {
            if app.active_tab().current_path.as_ref() == Some(&other) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                eframe::App::ui(&mut app, ui, &mut eframe::Frame::_new_kittest());
            });
            output.textures_delta.clear();
        }
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&other));
        assert!(!app.search.visible, "a lone definition is opened, not listed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_references_searches_whole_words() {
        let ctx = egui::Context::default();
        let mut app = rust_app("fn main() {\n    helper();\n}\n");
        app.find_references(&ctx, Some(18));
        assert!(app.search.visible);
        assert_eq!(app.search.query, "helper");
        assert!(app.search.case_sensitive && app.search.whole_word);
    }
}
