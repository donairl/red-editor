//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::about::{self, AboutState, HOME_URL};
use crate::activity_bar;
use crate::blame::{self, Blame};
use crate::bookmarks::Bookmarks;
use crate::file_browser::{self, Explorer};
use crate::git::{FileChange, GitError};
use crate::git_graph::{self, GitGraphUi, GraphAction};
use crate::git_worker::{Base, DiffSource, GitState, Op};
use crate::gutter;
use crate::highlighter::{DocumentHighlightCache, HighlighterLayouter, Language, SyntaxHighlighter};
use crate::history::{Entry, EntryKind, History};
use crate::line_ops;
use crate::i18n::{self, I18n};
use crate::layout_cache::{self, LayoutCache};
use crate::line_diff::{self, LineMarks};
use crate::markdown_preview::{MarkdownPreview, ScrollPos, ScrollSync};
use crate::modals::{self, DiskChoice, ModalResponse, PendingAction};
use crate::prettify;
use crate::find::{self, FindAction, FindState, GotoAction, GotoState};
use crate::search::{self, SearchAction, SearchState};
use crate::source_control::{self, DiffAction, ScmAction, ScmUi};
use crate::symbols::{self, PickerAction, SymbolPicker};
use crate::tab_bar;
use crate::palette::{self, Item, Palette, PaletteAction, PaletteMode};
use crate::git_log::GitLogPanel;
use crate::terminal::{self, Terminal};
use crate::theme::{self, ThemeId};
use crate::shortcuts::{self, ShortcutAction, consume_shortcut};
use crate::welcome::{self, WelcomeAction};

/// Storage key under which eframe remembers the interface language.
const UI_LANGUAGE_KEY: &str = "ui_language";

/// Storage key under which eframe remembers the recently opened files and
/// folders.
const HISTORY_KEY: &str = "recent_paths";

/// Storage key under which eframe remembers the chosen color theme.
const THEME_KEY: &str = "theme";

/// Seconds between checks for open files changed by other programs.
const DISK_CHECK_SECS: f64 = 2.0;

/// Least time between two updates of the gutter's git change markers.
const MARKS_INTERVAL: Duration = Duration::from_millis(250);

/// Largest file the editor opens. egui lays out the whole document, on every
/// edit: past this size each keystroke takes well over a frame.
const MAX_OPEN_BYTES: u64 = 10 * 1024 * 1024;

/// Under the editor's id, the `char` index it was last right-clicked at.
const CONTEXT_CHAR: &str = "context_char";

/// Least room worth giving an Open Recent entry's folder; with less, only
/// the row's hover text shows it.
const MIN_RECENT_FOLDER_WIDTH: f32 = 48.0;

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
    status_position: Option<(usize, usize)>,
    /// The bookmark count the status bar last showed.
    status_bookmarks: usize,
    /// Show the window listing the active document's bookmarks.
    pub show_bookmarks: bool,
    /// egui's clock when the open files were last compared with the disk.
    last_disk_check: f64,
    pub highlighter: SyntaxHighlighter,
    /// The editor's last layout, reused while the text does not change.
    /// Kept for the active tab only: a galley takes tens of bytes per
    /// character.
    editor_layout: LayoutCache,
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
    /// Show the line-number gutter left of the text. On by default, as in
    /// VS Code.
    pub show_line_numbers: bool,
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
    /// Git status of the open folder, or of the active file's folder.
    pub git: GitState,
    /// The name typed in the status bar's branch menu.
    pub new_branch_name: String,
    /// The Source Control view's commit message and open lists.
    pub scm: ScmUi,
    /// A git command asking for confirmation.
    pub git_confirm: Option<GitConfirm>,
    /// The Git Graph in place of the editor while `Some`.
    pub git_graph: Option<GitGraphUi>,
    /// The start page in place of the editor; back whenever no tab is left.
    pub show_welcome: bool,
    /// When the gutter's change markers were last computed.
    last_marks_at: Option<Instant>,
}

/// What the app wants done to the editor widget, which only `render_editor`
/// can reach.
#[derive(Default)]
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
            word_wrap: false,
            show_line_numbers: true,
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
            git: GitState::default(),
            new_branch_name: String::new(),
            scm: ScmUi::default(),
            git_confirm: None,
            git_graph: None,
            show_welcome: true,
            last_marks_at: None,
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

/// A menu entry with its shortcut on the right; closes the menu when clicked.
fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui
        .add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked
}

impl TextEditorApp {
    /// The app as eframe starts it: language files from disk, and the saved
    /// or the system interface language.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_fallback_font(&cc.egui_ctx);
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
        Self {
            i18n,
            status_message,
            history: saved_history(cc.storage),
            theme: saved_theme(cc.storage),
            ..Default::default()
        }
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

    /// Draws the "Bahasa" dropdown used by both the File and Tampilan menus.
    ///
    /// Re-selecting the already active language clears the override and returns
    /// to automatic detection.
    pub fn show_language_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.syntax").to_owned();
        ui.menu_button(label, |ui| {
            let current = self.effective_language();
            for lang in Language::all() {
                let selected = *lang == current;
                if ui
                    .selectable_label(selected, self.language_label(*lang))
                    .clicked()
                {
                    self.active_tab_mut().language_override =
                        if selected { None } else { Some(*lang) };
                    ui.close();
                }
            }
        });
    }

    /// The File > Open Recent submenu.
    pub fn show_recent_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("file.open_recent").to_owned();
        ui.menu_button(label, |ui| self.show_recent_items(ui));
    }

    /// The history, newest first, each entry's folder on its right; a click
    /// reopens the entry. Ends with Clear Recently Opened.
    fn show_recent_items(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() {
            ui.weak(self.i18n.t("file.no_recent"));
            return;
        }
        let mut picked = None;
        for entry in self.history.entries() {
            let row = ui
                .add(recent_row(ui, entry))
                .on_hover_text(entry.path.display().to_string());
            if row.clicked() {
                picked = Some(entry.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui.button(self.i18n.t("file.clear_recent")).clicked() {
            self.history.clear();
            ui.close();
        }
        if let Some(entry) = picked {
            self.open_recent(entry);
        }
    }

    pub fn show_view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.word_wrap, self.i18n.t("view.word_wrap"));
        ui.checkbox(&mut self.show_line_numbers, self.i18n.t("view.line_numbers"));
        let ctx = ui.ctx().clone();
        let blame = egui::Button::new(self.i18n.t("view.blame"))
            .selected(self.show_blame)
            .shortcut_text(shortcuts::label(&ctx, &shortcuts::BLAME));
        if ui.add(blame).clicked() {
            self.show_blame = !self.show_blame;
            ui.close();
        }
        let graph = egui::Button::new(self.i18n.t("view.git_graph"))
            .selected(self.git_graph.is_some())
            .shortcut_text(shortcuts::label(&ctx, &shortcuts::GIT_GRAPH));
        if ui.add_enabled(self.git.repo().is_some(), graph).clicked() {
            self.toggle_git_graph(&ctx);
            ui.close();
        }
        if menu_item(ui, self.i18n.t("view.zen_mode"), &shortcuts::label(&ctx, &shortcuts::ZEN_MODE)) {
            self.toggle_zen_mode(&ctx);
        }
        let preview_label = self.i18n.t("view.markdown_preview");
        let preview_shortcut = shortcuts::label(&ctx, &shortcuts::MARKDOWN_PREVIEW);
        if ui
            .add_enabled(
                self.preview_available(),
                egui::Button::new(preview_label)
                    .selected(self.show_markdown_preview)
                    .shortcut_text(preview_shortcut),
            )
            .clicked()
        {
            self.toggle_markdown_preview();
            ui.close();
        }
        let scm_label = self.i18n.t("view.source_control");
        let scm_shortcut = shortcuts::label(&ctx, &shortcuts::SOURCE_CONTROL);
        let scm_open = self.show_sidebar && self.sidebar_view == SidebarView::SourceControl;
        if ui
            .add(egui::Button::new(scm_label).selected(scm_open).shortcut_text(scm_shortcut))
            .clicked()
        {
            self.toggle_source_control();
            ui.close();
        }
        let console_label = self.i18n.t("view.console");
        let console_shortcut = shortcuts::label(&ctx, &shortcuts::TOGGLE_CONSOLE);
        if ui
            .add(
                egui::Button::new(console_label)
                    .selected(self.show_console)
                    .shortcut_text(console_shortcut),
            )
            .clicked()
        {
            self.toggle_console(&ctx);
            ui.close();
        }
        if ui
            .add(egui::Button::new(self.i18n.t("view.git_log")).selected(self.show_git_log))
            .clicked()
        {
            self.show_git_log = !self.show_git_log;
            ui.close();
        }
        if menu_item(
            ui,
            self.i18n.t("view.command_palette"),
            &shortcuts::label(&ctx, &shortcuts::COMMAND_PALETTE),
        ) {
            self.open_palette(PaletteMode::Commands);
        }
        self.show_theme_menu(ui);
        ui.separator();
        self.show_language_menu(ui);
        self.show_ui_language_menu(ui);
    }

    /// Lists every color theme; picking one switches to it at once.
    pub fn show_theme_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.theme").to_owned();
        ui.menu_button(label, |ui| {
            for id in ThemeId::ALL {
                let name = id.name().unwrap_or_else(|| self.i18n.t("theme.system"));
                if ui.selectable_label(self.theme == id, name).clicked() {
                    self.theme = id;
                    ui.close();
                }
            }
        });
    }

    /// Lists every interface language; picking one switches to it.
    pub fn show_ui_language_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.ui_language").to_owned();
        ui.menu_button(label, |ui| {
            let current = self.i18n.code().to_owned();
            let locales: Vec<(String, String)> = self
                .i18n
                .locales()
                .map(|(code, name)| (code.to_owned(), name.to_owned()))
                .collect();
            for (code, name) in locales {
                if ui.selectable_label(code == current, name).clicked() {
                    self.i18n.select(&code);
                    ui.close();
                }
            }
        });
    }

    pub fn show_help_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(self.i18n.t("help.home")).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(HOME_URL));
            ui.close();
        }
        if ui.button(self.i18n.t("help.about")).clicked() {
            self.show_about_modal = true;
            ui.close();
        }
    }

    /// The document name and the latest status message, and the caret's
    /// line and column at the right end, after a badge counting the
    /// bookmarks that opens their list.
    pub fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        self.status_position = self.caret_position(ui.ctx());
        self.active_tab_mut().sync_bookmarks();
        self.status_bookmarks = self.active_tab().bookmarks.len();
        ui.horizontal(|ui| {
            if self.git.status.is_some() {
                self.show_branch_menu(ui);
                ui.separator();
            }
            let modified = if self.is_dirty() {
                self.i18n.t("status.modified")
            } else {
                ""
            };
            let name = self.file_display_name();
            ui.label(
                self.i18n
                    .tf("status.document", &[("name", &name), ("modified", modified)]),
            );

            if let Some((msg, is_error)) = &self.status_message {
                ui.separator();
                let color = if *is_error {
                    egui::Color32::from_rgb(220, 60, 60)
                } else {
                    egui::Color32::from_rgb(60, 180, 60)
                };
                ui.colored_label(color, msg);
            }

            if self.status_position.is_some() || self.status_bookmarks > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((line, col)) = self.status_position {
                        let (line, col) = (line.to_string(), col.to_string());
                        ui.label(
                            self.i18n
                                .tf("status.position", &[("line", &line), ("col", &col)]),
                        );
                    }
                    if self.status_bookmarks > 0 {
                        let count = self.status_bookmarks.to_string();
                        let color = theme::active(ui.ctx()).gutter.bookmark;
                        let label = egui::RichText::new(
                            self.i18n.tf("status.bookmarks", &[("count", &count)]),
                        )
                        .small()
                        .color(color);
                        let badge = egui::Button::new(label)
                            .stroke(egui::Stroke::new(1.0, color))
                            .corner_radius(8)
                            .selected(self.show_bookmarks);
                        if ui
                            .add(badge)
                            .on_hover_text(self.i18n.t("bookmark.show_list"))
                            .clicked()
                        {
                            self.show_bookmarks = !self.show_bookmarks;
                        }
                    }
                });
            }
        });
    }

    /// The status bar's branch button, `main* ↓1 ↑2`, and the menu it opens:
    /// switch to or create a branch, fetch, pull and push.
    fn show_branch_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(status) = &self.git.status else { return };
        let mut label = status.head_label();
        if !status.files.is_empty() {
            label.push('*');
        }
        if status.behind > 0 {
            label.push_str(&format!(" ↓{}", status.behind));
        }
        if status.ahead > 0 {
            label.push_str(&format!(" ↑{}", status.ahead));
        }
        let busy = self.git.busy().is_some();
        if busy {
            ui.spinner();
        }
        let mut op = None;
        ui.menu_button(label, |ui| {
            ui.set_min_width(200.0);
            ui.label(egui::RichText::new(self.i18n.t("git.branches")).small().weak());
            for branch in &self.git.branches {
                if ui.add_enabled(!branch.current, egui::Button::selectable(branch.current, &branch.name)).clicked() {
                    op = Some(Op::Switch(branch.name.clone()));
                    ui.close();
                }
            }
            ui.separator();
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.new_branch_name)
                    .hint_text(self.i18n.t("git.new_branch_hint")),
            );
            let name = self.new_branch_name.trim().to_owned();
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.is_empty() {
                op = Some(Op::CreateBranch(name));
                self.new_branch_name.clear();
                ui.close();
            }
            ui.separator();
            for (key, action) in [("git.fetch", Op::Fetch), ("git.pull", Op::Pull), ("git.push", Op::Push)] {
                if ui.add_enabled(!busy, egui::Button::new(self.i18n.t(key))).clicked() {
                    op = Some(action);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(self.i18n.t("git.branch_tooltip"));
        if let Some(op) = op {
            self.run_git(&ctx, op);
        }
    }

    /// 1-based line and column of the editor's caret, once it has one.
    fn caret_position(&self, ctx: &egui::Context) -> Option<(usize, usize)> {
        let state = egui::TextEdit::load_state(ctx, self.editor_id())?;
        let caret = state.cursor.char_range()?.primary.index.0;
        Some(find::line_col(&self.active_tab().text_content, caret))
    }

    pub fn show_edit_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("edit.undo"), &shortcuts::UNDO) {
            self.send_editor_key(egui::Key::Z);
        }
        if item(ui, self.i18n.t("edit.redo"), &shortcuts::REDO) {
            self.send_editor_key(egui::Key::Y);
        }
        ui.separator();
        self.show_clipboard_items(ui, ctx);
        ui.separator();
        let editing = !self.show_welcome;
        if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t("edit.find"), &shortcuts::FIND)).inner {
            self.open_find(ctx, false);
        }
        if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t("edit.replace"), &shortcuts::REPLACE)).inner {
            self.open_find(ctx, true);
        }
        if item(ui, self.i18n.t("edit.find_in_files"), &shortcuts::FIND_IN_FILES) {
            self.open_search(ctx);
        }
        if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t("edit.goto_line"), &shortcuts::GOTO_LINE)).inner {
            self.open_goto();
        }
        ui.separator();
        let line_items = [
            ("edit.move_line_up", &shortcuts::MOVE_LINE_UP, ShortcutAction::MoveLineUp),
            ("edit.move_line_down", &shortcuts::MOVE_LINE_DOWN, ShortcutAction::MoveLineDown),
            ("edit.copy_line_up", &shortcuts::COPY_LINE_UP, ShortcutAction::CopyLineUp),
            ("edit.copy_line_down", &shortcuts::COPY_LINE_DOWN, ShortcutAction::CopyLineDown),
            ("edit.delete_line", &shortcuts::DELETE_LINE, ShortcutAction::DeleteLine),
            ("edit.toggle_comment", &shortcuts::TOGGLE_COMMENT, ShortcutAction::ToggleComment),
            ("edit.select_next_match", &shortcuts::SELECT_NEXT_MATCH, ShortcutAction::SelectNextMatch),
        ];
        for (key, shortcut, action) in line_items {
            if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t(key), shortcut)).inner {
                self.apply_shortcut(ctx, action);
            }
        }
        ui.separator();
        let navigable = symbols::supports(self.effective_language());
        let nav_item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            let button = egui::Button::new(label).shortcut_text(shortcuts::label(ctx, shortcut));
            let clicked = ui.add_enabled(navigable, button).clicked();
            if clicked {
                ui.close();
            }
            clicked
        };
        if nav_item(ui, self.i18n.t("edit.goto_symbol"), &shortcuts::GOTO_SYMBOL) {
            self.open_symbol_picker();
        }
        if nav_item(ui, self.i18n.t("edit.goto_definition"), &shortcuts::GOTO_DEFINITION) {
            self.goto_definition(ctx, None);
        }
        if nav_item(ui, self.i18n.t("edit.find_references"), &shortcuts::FIND_REFERENCES) {
            self.find_references(ctx, None);
        }
        ui.separator();
        let prettify_label = self.i18n.t("edit.prettify");
        let prettify_shortcut = shortcuts::label(ctx, &shortcuts::PRETTIFY);
        if ui
            .add_enabled(
                prettify::supports(self.effective_language()),
                egui::Button::new(prettify_label).shortcut_text(prettify_shortcut),
            )
            .clicked()
        {
            self.prettify_document();
            ui.close();
        }
    }

    /// Cut, Copy and Paste, shared by the Edit menu and the editor's
    /// right-click menu.
    fn show_clipboard_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("edit.cut"), &shortcuts::CUT) {
            self.cut_selection(ctx);
        }
        if item(ui, self.i18n.t("edit.copy"), &shortcuts::COPY) {
            self.copy_selection(ctx);
        }
        if item(ui, self.i18n.t("edit.paste"), &shortcuts::PASTE) {
            self.paste_clipboard(ctx);
        }
    }

    /// The editor's right-click menu. Opening it takes the focus like a click
    /// on the menu bar: the selection stays painted, and the editor gets both
    /// back when the menu closes.
    fn show_editor_context_menu(&mut self, ctx: &egui::Context, editor: &egui::Response) {
        if editor.secondary_clicked() && ctx.memory(|m| m.has_focus(editor.id)) {
            self.menu_focus = Some(editor.id);
            ctx.memory_mut(|m| m.surrender_focus(editor.id));
        }
        editor.context_menu(|ui| {
            self.show_clipboard_items(ui, ctx);
            if symbols::supports(self.effective_language()) {
                ui.separator();
                let at = ctx.data(|d| d.get_temp::<usize>(self.editor_id().with(CONTEXT_CHAR)));
                let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
                    menu_item(ui, label, &shortcuts::label(ctx, shortcut))
                };
                if item(ui, self.i18n.t("edit.goto_definition"), &shortcuts::GOTO_DEFINITION) {
                    self.goto_definition(ctx, at);
                }
                if item(ui, self.i18n.t("edit.find_references"), &shortcuts::FIND_REFERENCES) {
                    self.find_references(ctx, at);
                }
            }
            if let Some(line) = self.context_line(ctx) {
                ui.separator();
                let key = if self.active_tab().bookmarks.contains(line) {
                    "bookmark.remove"
                } else {
                    "bookmark.add"
                };
                if ui.button(self.i18n.t(key)).clicked() {
                    self.toggle_bookmark(line);
                    ui.close();
                }
            }
        });
    }

    /// The line the editor was last right-clicked on, else the caret's.
    fn context_line(&self, ctx: &egui::Context) -> Option<usize> {
        ctx.data(|d| d.get_temp::<usize>(self.editor_id().with(CONTEXT_CHAR)))
            .map(|at| find::line_col(&self.active_tab().text_content, at).0)
            .or_else(|| self.caret_position(ctx).map(|(line, _)| line))
    }

    /// Adds or removes the bookmark on `line` of the active document.
    pub fn toggle_bookmark(&mut self, line: usize) {
        let tab = self.active_tab_mut();
        tab.bookmarks.toggle(line, &tab.text_content);
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

    fn send_editor_key(&mut self, key: egui::Key) {
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

    fn saved_selection_id(&self) -> egui::Id {
        self.editor_id().with("saved_selection")
    }

    fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("editor", self.active_tab_index))
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
    fn copy_selection(&mut self, ctx: &egui::Context) {
        if let Some((_, text)) = self.selected_text(ctx) {
            ctx.copy_text(text);
        }
        self.editor_request.focus = true;
    }

    /// Edit > Cut: copies the selection, then deletes it.
    fn cut_selection(&mut self, ctx: &egui::Context) {
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
    fn paste_clipboard(&mut self, ctx: &egui::Context) {
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
            self.show_about_modal || self.pending_action.is_some() || self.git_confirm.is_some();
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
                let editor_hidden = self.git_graph.is_some() || self.show_welcome;
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
        if !symbols::supports(self.effective_language()) {
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

    pub fn show_file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &shortcuts::label(ctx, shortcut))
        };
        if item(ui, self.i18n.t("file.new_tab"), &shortcuts::NEW_TAB) {
            self.open_new_tab();
        }
        if item(ui, self.i18n.t("file.close_tab"), &shortcuts::CLOSE_TAB) {
            self.trigger_close_tab(self.active_tab_index);
        }
        if item(ui, self.i18n.t("file.new_window"), &shortcuts::NEW_WINDOW) {
            self.open_new_window();
        }
        ui.separator();
        if item(ui, self.i18n.t("file.open_file"), &shortcuts::OPEN_FILE) {
            self.trigger_open_file();
        }
        if item(ui, self.i18n.t("file.open_folder"), &shortcuts::OPEN_FOLDER) {
            self.trigger_open_folder();
        }
        if item(ui, self.i18n.t("file.quick_open"), &shortcuts::QUICK_OPEN) {
            self.open_palette(PaletteMode::Files);
        }
        self.show_recent_menu(ui);
        ui.separator();
        let editing = !self.show_welcome;
        if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t("file.save"), &shortcuts::SAVE)).inner {
            self.save();
        }
        if ui.add_enabled_ui(editing, |ui| item(ui, self.i18n.t("file.save_as"), &shortcuts::SAVE_AS)).inner {
            self.save_as();
        }
        ui.separator();
        self.show_language_menu(ui);
        ui.separator();
        if item(ui, self.i18n.t("file.exit"), &shortcuts::EXIT) {
            self.trigger_exit(ctx);
        }
    }

    /// Runs the action of a keyboard shortcut. Ignored while a dialog waits
    /// for an answer.
    pub fn apply_shortcut(&mut self, ctx: &egui::Context, action: ShortcutAction) {
        if self.pending_action.is_some() || self.show_about_modal || self.git_confirm.is_some() {
            return;
        }
        if (self.git_graph.is_some() || self.show_welcome) && action.needs_editor() {
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
            match self.tabs[index].save() {
                Ok(_) => {
                    self.status_message = Some((self.i18n.t("msg.saved").to_owned(), false));
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

    pub fn save(&mut self) -> bool {
        self.save_tab(self.active_tab_index)
    }

    pub fn save_as(&mut self) -> bool {
        self.save_as_with(pick_save_path)
    }

    fn save_as_with(&mut self, pick_path: impl FnOnce(&DocumentTab) -> Option<PathBuf>) -> bool {
        if let Some(path) = pick_path(self.active_tab()) {
            let text_to_save = self.active_tab().text_content.clone();
            match fs::write(&path, &text_to_save) {
                Ok(_) => {
                    let active = self.active_tab_mut();
                    active.saved_content = active.text_content.clone();
                    active.disk_stamp = disk_stamp(&path);
                    active.disk_conflict = false;
                    active.active_language = Language::from_path(&path);
                    active.current_path = Some(path);
                    self.status_message = Some((self.i18n.t("msg.saved").to_owned(), false));
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
    /// (unsaved ones stay, so no work is lost) and Search in Files forgets
    /// its query, options and results.
    pub fn open_folder(&mut self, folder: PathBuf) {
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

    /// The Activity Bar's Explorer item: asks for a folder when none is
    /// open, otherwise shows or hides the sidebar.
    /// Shows or hides the Console panel. Showing it starts the shell, in the
    /// open folder, if none is running, and gives it the keyboard.
    pub fn toggle_console(&mut self, ctx: &egui::Context) {
        if self.show_console {
            self.show_console = false;
            self.editor_request.focus = true;
            return;
        }
        if self.console.is_none() {
            match Terminal::spawn(ctx, &self.console_dir()) {
                Ok(console) => self.console = Some(console),
                Err(error) => {
                    let message = self.i18n.tf("msg.console_failed", &[("error", &error)]);
                    self.status_message = Some((message, true));
                    return;
                }
            }
        }
        self.show_console = true;
        ctx.memory_mut(|m| m.request_focus(terminal::widget_id()));
    }

    /// The Console panel: a title row and the terminal under it. Drops the
    /// terminal and hides the panel once the shell exits.
    fn show_console_panel(&mut self, ui: &mut egui::Ui) {
        let Some(console) = self.console.as_mut() else {
            self.show_console = false;
            return;
        };
        if console.has_exited() {
            self.console = None;
            self.show_console = false;
            self.editor_request.focus = true;
            return;
        }
        let (mut hide, mut kill) = (false, false);
        egui::Panel::bottom("console_panel")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(self.i18n.t("console.title"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        hide = console_icon_button(ui, "console_hide", self.i18n.t("console.hide"), paint_hide_icon);
                        kill = console_icon_button(ui, "console_kill", self.i18n.t("console.kill"), paint_kill_icon);
                    });
                });
                console.show(ui);
            });
        if kill {
            self.console = None;
        }
        if hide || kill {
            self.show_console = false;
            self.editor_request.focus = true;
        }
    }

    /// Where the shell starts: the open folder, else the active file's
    /// folder, else the editor's working directory.
    fn console_dir(&self) -> PathBuf {
        self.explorer
            .as_ref()
            .map(|e| e.root().to_path_buf())
            .or_else(|| {
                let path = self.active_tab().current_path.as_ref()?;
                path.parent().map(|dir| dir.to_path_buf())
            })
            .filter(|dir| dir.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"))
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

/// Documents above this size skip syntax highlighting and render plain.
///
/// Highlighting of anything past a few KB runs on a background thread (see
/// `HighlighterLayouter::frame_job`), so this only bounds how long that pass can
/// take, not how long the UI can freeze.
const HIGHLIGHT_SIZE_LIMIT: usize = 1024 * 1024;

/// A painted icon button in the Console title row, with `tooltip` on hover.
/// Returns whether it was clicked.
fn console_icon_button(
    ui: &mut egui::Ui,
    id: &str,
    tooltip: &str,
    paint: fn(&egui::Painter, egui::Pos2, egui::Color32),
) -> bool {
    let palette = theme::active(ui.ctx()).sidebar;
    let size = egui::Vec2::splat(file_browser::ACTION_SIZE);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui
        .interact(rect, egui::Id::unique(id), egui::Sense::click())
        .on_hover_text(tooltip);
    let color = if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink(2.0), egui::CornerRadius::same(4), palette.row_hover);
        palette.icon_active
    } else {
        palette.icon_inactive
    };
    paint(ui.painter(), rect.center(), color);
    response.clicked()
}

/// A chevron pointing down: tuck the panel away.
fn paint_hide_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    painter.line(
        vec![
            center + egui::vec2(-4.5, -2.0),
            center + egui::vec2(0.0, 2.5),
            center + egui::vec2(4.5, -2.0),
        ],
        stroke,
    );
}

/// A trash can: end the shell.
fn paint_kill_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.2, color);
    let lid_y = center.y - 3.5;
    // Lid and its handle.
    painter.line_segment([egui::pos2(center.x - 5.0, lid_y), egui::pos2(center.x + 5.0, lid_y)], stroke);
    painter.line(
        vec![
            egui::pos2(center.x - 1.5, lid_y),
            egui::pos2(center.x - 1.5, lid_y - 1.5),
            egui::pos2(center.x + 1.5, lid_y - 1.5),
            egui::pos2(center.x + 1.5, lid_y),
        ],
        stroke,
    );
    // Body, narrowing slightly to the bottom.
    let bottom = center.y + 5.0;
    painter.line(
        vec![
            egui::pos2(center.x - 3.5, lid_y),
            egui::pos2(center.x - 3.0, bottom),
            egui::pos2(center.x + 3.0, bottom),
            egui::pos2(center.x + 3.5, lid_y),
        ],
        stroke,
    );
    // Two ribs.
    for dx in [-1.0, 1.0] {
        painter.line_segment(
            [egui::pos2(center.x + dx, lid_y + 2.0), egui::pos2(center.x + dx, bottom - 1.5)],
            stroke,
        );
    }
}

/// One File > Open Recent row: the entry's name, then its folder dimmed,
/// fitted into egui's menu width. A name too long for it is cut with `…`,
/// as is the folder in the room the name leaves. Cuts fall between whole
/// characters, and only change what is shown, never the entry.
fn recent_row(ui: &egui::Ui, entry: &Entry) -> egui::Button<'static> {
    let fit = |text: egui::RichText, width: f32| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            width,
            egui::TextStyle::Button,
        )
    };
    let (name, folder) = entry.label();
    let width = ui.spacing().menu_width;
    let name = fit(egui::RichText::new(name), width);
    let room = width - name.size().x - ui.spacing().item_spacing.x;
    let row = egui::Button::new(name);
    if room < MIN_RECENT_FOLDER_WIDTH {
        return row;
    }
    row.shortcut_text(fit(egui::RichText::new(folder).weak(), room))
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

/// The theme eframe kept from the last run; `System` on the first.
fn saved_theme(storage: Option<&dyn eframe::Storage>) -> ThemeId {
    storage
        .and_then(|storage| storage.get_string(THEME_KEY))
        .map(|slug| ThemeId::from_slug(&slug))
        .unwrap_or_default()
}

/// True when a document is too large to highlight cheaply.
fn text_content_exceeds_highlight_limit(content: &str) -> bool {
    content.len() > HIGHLIGHT_SIZE_LIMIT
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

/// Renders the multiline editor, optionally through a syntax highlighter.
///
/// When `layouter` is `None` the text renders plain, which is what oversized
/// documents get. Lines are joined back together with `\n` separators so the
/// buffer round-trips exactly, keeping the caret and selection aligned.
///
/// With `word_wrap` on, long lines wrap to the window width and only vertical
/// scrolling is needed. With it off, lines never wrap and the editor scrolls
/// sideways as well. Wrap off needs a layouter on the plain path too: a stock
/// multiline `TextEdit` always shrinks to the available width, so it wraps
/// whatever `desired_width` says.
///
/// The editor is wrapped in a `ScrollArea`: `TextEdit` has no scrolling
/// of its own for multiline input (`clip_text` is `false`, so its `text_offset`
/// handling never runs) and its only `ui.scroll_to_rect` call writes to
/// `PassState::scroll_target`, which only `ScrollArea::show` drains. Without this
/// wrapper a document taller than the viewport cannot be scrolled and, with the
/// default `cursor_at_end`, the caret lands off-screen and stays unreachable.
///
/// With `line_numbers` on, the editor's left margin widens to hold the
/// gutter, which is painted over it once the text is laid out. `hint` shows
/// in grey while the document is empty.
#[cfg(test)]
fn render_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
) {
    render_editor_with(
        ui,
        text,
        layouter,
        word_wrap,
        line_numbers,
        hint,
        None,
        EditorRequest::default(),
        None,
        &mut Bookmarks::default(),
        &LineMarks::default(),
        None,
        &mut LayoutCache::default(),
    );
}

/// Rectangles covering the `char` range `(start, end)` of `galley`, one per
/// row, relative to the galley. Drawn the way egui draws a focused editor's
/// selection, including the half-height nub for a selected newline.
fn selection_rects(galley: &egui::Galley, (start, end): find::Range) -> Vec<egui::Rect> {
    if start >= end {
        return Vec::new();
    }
    let min = galley.layout_from_cursor(egui::text::CCursor::new(start));
    let max = galley.layout_from_cursor(egui::text::CCursor::new(end));
    (min.row..=max.row)
        .map(|index| {
            let row = &galley.rows[index];
            let left = if index == min.row { row.x_offset(min.column) } else { 0.0 };
            let right = if index == max.row {
                row.x_offset(max.column)
            } else if row.ends_with_newline {
                row.size.x + row.height() * 0.5
            } else {
                row.size.x
            };
            egui::Rect::from_min_max(egui::pos2(left, 0.0), egui::pos2(right, row.size.y))
                .translate(row.pos.to_vec2())
        })
        .collect()
}

/// [`render_editor`] with a fixed widget `id` (so the app can find the
/// editor's state) and a one-shot `request` to act on. `bookmarks` follow
/// the edits and are marked in the gutter. The text is laid out through
/// `layout_cache`, which keeps the layout while nothing changes. Also
/// returns the editor's response, for its right-click menu.
#[allow(clippy::too_many_arguments)]
fn render_editor_with(
    ui: &mut egui::Ui,
    text: &mut String,
    layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
    id: Option<egui::Id>,
    request: EditorRequest,
    scroll_to: Option<f32>,
    bookmarks: &mut Bookmarks,
    changes: &LineMarks,
    blame: Option<BlameColumn<'_>>,
    layout_cache: &mut LayoutCache,
) -> (ScrollPos, egui::Response) {
    // The blame column is a strip left of the scroll area, painted after
    // it, once the text's rows are known.
    let Some(column) = blame else {
        return render_editor_inner(
            ui, text, layouter, word_wrap, line_numbers, hint, id, request, scroll_to, bookmarks,
            changes, layout_cache, None,
        );
    };
    let small = egui::FontId::monospace(11.0);
    let width = column.blame.map_or(BLAME_NOTE_WIDTH, |b| b.width(ui, &small));
    // The strip lies outside the scroll area, so it is painted through this
    // clip rect, not the scroll area's, which stops at the strip's edge.
    let clip = ui.clip_rect();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (strip, _) =
            ui.allocate_exact_size(egui::vec2(width, ui.available_height()), egui::Sense::hover());
        render_editor_inner(
            ui, text, layouter, word_wrap, line_numbers, hint, id, request, scroll_to, bookmarks,
            changes, layout_cache, Some((strip, clip, &column, small)),
        )
    })
    .inner
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

/// Width of the column while it only carries a note.
const BLAME_NOTE_WIDTH: f32 = 140.0;

#[allow(clippy::too_many_arguments)]
fn render_editor_inner(
    ui: &mut egui::Ui,
    text: &mut String,
    mut layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
    id: Option<egui::Id>,
    request: EditorRequest,
    scroll_to: Option<f32>,
    bookmarks: &mut Bookmarks,
    changes: &LineMarks,
    layout_cache: &mut LayoutCache,
    blame: Option<(egui::Rect, egui::Rect, &BlameColumn<'_>, egui::FontId)>,
) -> (ScrollPos, egui::Response) {
    let scroll_area = if word_wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    let scroll_area = match scroll_to {
        Some(offset) => scroll_area.vertical_scroll_offset(offset),
        None => scroll_area,
    };
    let output = scroll_area.show(ui, |ui| {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let gutter_width =
            line_numbers.then(|| gutter::width(ui, &font_id, gutter::line_count(text)));
        let margin = egui::Margin {
            // `gutter::width` is whole points and fits an `i8`.
            left: gutter_width.map_or(8, |w| w as i8),
            ..egui::Margin::symmetric(8, 6)
        };

        // Fill the viewport through `min_size`, not `ui.add_sized`: `add_sized`
        // centres a widget larger than the space it is given, so a line wider
        // than the window pushed the start of every line off the left edge,
        // beyond the reach of the scroll bar. A plain `show` grows the
        // editor right and down from the top-left corner instead.
        // Clamped here: the text is lent to the editor below, and may have
        // shrunk (Prettify) since the selection was taken.
        let select = request.select.map(|(a, b)| {
            let n = text.chars().count();
            (a.min(n), b.min(n))
        });
        let text_edit = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace)
            .lock_focus(true)
            .desired_width(f32::INFINITY)
            .min_size(ui.available_size())
            .hint_text(hint.to_owned())
            // No outline or rounded fill: the editor sits on the central
            // panel's background, the same colour as the active tab.
            .frame(egui::Frame::NONE.inner_margin(margin));

        let text_edit = match id {
            Some(id) => text_edit.id(id),
            None => text_edit,
        };
        if let Some(id) = id {
            if request.focus {
                ui.memory_mut(|m| m.request_focus(id));
            }
            // Before `show`, so a paste or undo of this frame acts on it.
            if let Some((a, b)) = select {
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(a),
                    egui::text::CCursor::new(b),
                )));
                state.store(ui.ctx(), id);
            }
            if let Some(key) = request.key {
                ui.ctx().input_mut(|i| {
                    i.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::COMMAND,
                    });
                });
            }
        }

        // egui moves the caret to any press, a right one included; a
        // right-click inside the selection must keep it for the menu.
        let right_press = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        let before_press = id
            .filter(|_| right_press)
            .and_then(|id| egui::TextEdit::load_state(ui.ctx(), id))
            .and_then(|state| state.cursor.char_range())
            .filter(|range| !range.is_empty());

        // Reserved before `show` so the selection is painted under the text.
        let inactive_shape = request
            .inactive_selection
            .map(|_| ui.painter().add(egui::Shape::Noop));

        let output = match layouter.as_mut() {
            Some(highlighter) => {
                // `TextEdit` calls this every frame. `frame_job` reuses the
                // cached result, and highlights large documents on a background
                // thread (plain text meanwhile) so the window never freezes.
                // Wrapping is a layout concern, so it is applied by the layout
                // cache and never reaches the highlighter's cache.
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let job = highlighter.frame_job(buf.as_str(), ui.ctx());
                    let wrap_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    layout_cache.galley(
                        ui,
                        buf.as_str(),
                        layout_cache::Style::Highlighted(job),
                        wrap_width,
                    )
                };
                text_edit.layouter(&mut lay).show(ui)
            }
            None => {
                // Mirror egui's stock `TextEdit` layouter, so plain text looks
                // the same with or without wrapping: the colour is the
                // `inactive` widget colour (a text edit does not brighten on
                // hover), unless the theme overrides it, and every section gets
                // the same line height. Wrap off needs a layouter of its own: a
                // stock multiline `TextEdit` always wraps.
                let color = ui
                    .visuals()
                    .override_text_color
                    .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
                let line_height =
                    ui.fonts_mut(|f| f.row_height(&font_id)) + ui.spacing().extra_text_line_spacing;
                let style = layout_cache::Style::Plain {
                    font_id: font_id.clone(),
                    color,
                    line_height,
                };
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let wrap_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    layout_cache.galley(ui, buf.as_str(), style.clone(), wrap_width)
                };
                text_edit.layouter(&mut lay).show(ui)
            }
        };

        bookmarks.sync(text);
        if let Some(width) = gutter_width {
            gutter::paint(ui, &output, &font_id, width, bookmarks, changes);
        }
        if let Some((strip, clip, column, small)) = &blame {
            match column.blame {
                Some(blame) => blame::paint_column(
                    ui,
                    *strip,
                    *clip,
                    blame,
                    &output.galley,
                    output.galley_pos,
                    small,
                    column.now,
                ),
                None => blame::paint_note(ui, *strip, *clip, column.note, small),
            }
        }

        // Where the right-click menu was opened, for its bookmark item: the
        // click that opens it, a long touch included, records the spot.
        if let (true, Some(id), Some(pointer)) = (
            output.response.secondary_clicked(),
            id,
            ui.input(|i| i.pointer.interact_pos()),
        ) {
            let at = output
                .galley
                .cursor_from_pos(pointer - output.galley_pos)
                .index
                .0;
            ui.ctx()
                .data_mut(|d| d.insert_temp(id.with(CONTEXT_CHAR), at));
        }

        let mut cursor_range = output.cursor_range;
        if let (Some(before), Some(id), Some(pointer)) =
            (before_press, id, ui.input(|i| i.pointer.interact_pos()))
        {
            let at = output.galley.cursor_from_pos(pointer - output.galley_pos).index.0;
            let (a, b) = (before.primary.index.0, before.secondary.index.0);
            if output.response.contains_pointer() && (a.min(b)..=a.max(b)).contains(&at) {
                let mut state = output.state.clone();
                state.cursor.set_char_range(Some(before));
                state.store(ui.ctx(), id);
                cursor_range = Some(before);
                ui.ctx().request_repaint();
            }
        }

        // Remembered while focused: a click on a menu takes the focus, and
        // from the next frame egui keeps only the caret.
        if let (Some(id), true) = (id, output.response.has_focus()) {
            let saved: Option<find::Range> = cursor_range
                .map(|r| {
                    let (a, b) = (r.primary.index.0, r.secondary.index.0);
                    (a.min(b), a.max(b))
                })
                .filter(|(a, b)| a < b);
            ui.ctx().data_mut(|d| {
                let key = id.with("saved_selection");
                match saved {
                    Some(range) => {
                        d.insert_temp(key, range);
                    }
                    None => d.remove::<find::Range>(key),
                }
            });
        }

        // egui paints a selection only while it is in the editor's state.
        let egui_selection = output.cursor_range.is_some_and(|r| !r.is_empty());
        if let (Some(shape), Some(range), false) =
            (inactive_shape, request.inactive_selection, egui_selection)
        {
            let fill = ui.visuals().selection.bg_fill;
            let offset = output.galley_pos.to_vec2();
            let rects = selection_rects(&output.galley, range)
                .into_iter()
                .map(|rect| egui::Shape::rect_filled(rect.translate(offset), 0.0, fill));
            ui.painter().set(shape, egui::Shape::Vec(rects.collect()));
        }

        if let (Some(index), Some(id)) = (request.goto, id) {
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(index))));
            state.store(ui.ctx(), id);
            ui.ctx().request_repaint();
        }

        // Rectangle of a `char` range, when it sits on one row.
        let span = |(start, end): find::Range| {
            let a = output.galley.pos_from_cursor(egui::text::CCursor::new(start));
            let b = output.galley.pos_from_cursor(egui::text::CCursor::new(end));
            (a.center().y == b.center().y)
                .then(|| egui::Rect::from_min_max(a.min, b.max).translate(output.galley_pos.to_vec2()))
        };
        if let Some(rect) = request.highlight.and_then(span) {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(2),
                egui::Color32::from_rgba_unmultiplied(255, 200, 0, 90),
            );
        }
        if let Some(rect) = request.reveal.and_then(span) {
            ui.scroll_to_rect(rect, None);
        }
        output.response.response.clone()
    });
    (ScrollPos::of(&output), output.inner)
}

impl eframe::App for TextEditorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(UI_LANGUAGE_KEY, self.i18n.code().to_owned());
        storage.set_string(HISTORY_KEY, self.history.to_storage());
        storage.set_string(THEME_KEY, self.theme.slug().to_owned());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let theme = theme::apply(&ctx, self.theme);
        self.highlighter.set_theme(theme.syntax);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
        self.poll_disk(&ctx);
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
                        source_control_open,
                        console_open: self.show_console,
                        explorer_tooltip: self.i18n.t("activity.explorer"),
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
            if activity.source_control_clicked {
                self.toggle_source_control();
            }
            if activity.console_clicked {
                self.toggle_console(&ctx);
            }
        }

        let mut file_to_open = None;
        let mut file_history = None;
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
            let editor_id = self.editor_id();

            // `TextEdit::font` resolves this `TextStyle` the same way, so the
            // caret and selection line up with the highlighted glyphs.
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let too_large = text_content_exceeds_highlight_limit(&self.active_tab().text_content);

            let word_wrap = self.word_wrap;
            let line_numbers = self.show_line_numbers;
            let hint = self.i18n.t("editor.placeholder");
            let active_idx = self.active_tab_index;
            let language = self.tabs[active_idx].effective_language();

            let scroll_to = if previewing { self.scroll_sync.take_editor_offset() } else { None };
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
            let (editor_pos, editor) = ui.push_id(self.active_tab_index, |ui| {
                let tab = &mut self.tabs[active_idx];
                if too_large {
                    render_editor_with(
                        ui,
                        &mut tab.text_content,
                        None,
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(editor_id),
                        request,
                        scroll_to,
                        &mut tab.bookmarks,
                        &tab.line_marks,
                        blame_column,
                        &mut self.editor_layout,
                    )
                } else {
                    let mut layouter = self
                        .highlighter
                        .layouter(language, font_id)
                        .with_document_cache(&mut tab.highlight_cache);
                    render_editor_with(
                        ui,
                        &mut tab.text_content,
                        Some(&mut layouter),
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(editor_id),
                        request,
                        scroll_to,
                        &mut tab.bookmarks,
                        &tab.line_marks,
                        blame_column,
                        &mut self.editor_layout,
                    )
                }
            })
            .inner;
            self.show_editor_context_menu(&ctx, &editor);
            if editor.clicked() && ctx.input(|i| i.modifiers.command) {
                self.goto_definition(&ctx, None);
            }
            match preview_pos {
                Some(preview_pos) => self.scroll_sync.observe(editor_pos, preview_pos),
                None => self.scroll_sync.reset(),
            }
        });
        self.show_palette(&ctx);
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
mod tests {
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
                let (_, response) = super::render_editor_with(
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
    fn test_selection_rects_cover_each_selected_row() {
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            galley = Some(ui.fonts_mut(|f| {
                f.layout_no_wrap("ab\ncd".to_owned(), font_id, egui::Color32::WHITE)
            }));
        });
        out.textures_delta.clear();
        let galley = galley.unwrap();

        assert!(super::selection_rects(&galley, (2, 2)).is_empty());
        // "b\nc": the end of the first row, its newline, and the start of the second.
        let rects = super::selection_rects(&galley, (1, 4));
        assert_eq!(rects.len(), 2);
        assert!(rects[0].min.x > 0.0, "starts after 'a'");
        assert!(rects[0].max.x > galley.rows[0].size.x, "covers the newline");
        assert_eq!(rects[1].min.x, 0.0);
        assert!(rects[1].max.x < galley.rows[1].size.x, "stops before 'd'");
        assert!(rects[1].min.y >= rects[0].center().y, "on the second row");
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

    #[test]
    fn test_highlight_size_limit() {
        assert!(!super::text_content_exceeds_highlight_limit("small"));
        assert!(!super::text_content_exceeds_highlight_limit(&"a".repeat(super::HIGHLIGHT_SIZE_LIMIT)));
        assert!(super::text_content_exceeds_highlight_limit(&"a".repeat(super::HIGHLIGHT_SIZE_LIMIT + 1)));
    }

    /// A document with more lines than fit on one screen.
    fn long_document(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("let x{i} = {i};"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The single `ScrollArea` the editor created, if any. `render_editor` must
    /// create exactly one, on the plain and on the highlighted path alike:
    /// `TextEdit` does not scroll multiline input by itself, and its
    /// `ui.scroll_to_rect` call is drained only by `ScrollArea::show`.
    fn editor_scroll_area(ctx: &egui::Context) -> Option<egui::containers::scroll_area::State> {
        ctx.data(|data| {
            data.iter_temp_by_type::<egui::containers::scroll_area::State>()
                .next()
                .cloned()
        })
    }

    /// One frame rendering `body` with the given input events, discarding
    /// texture deltas. Returns the painted shapes.
    fn run_frame_with_events(
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
    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
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
    fn painted_text(
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

    /// Scrolls the editor with a real wheel event: the pointer hovers over the
    /// editor and the wheel moves the content by `delta` points (negative moves
    /// the content up or left). Runs enough frames for the animated scroll to
    /// settle.
    fn scroll_editor_with_wheel_delta(
        ctx: &egui::Context,
        delta: egui::Vec2,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        let pointer = egui::Event::PointerMoved(egui::pos2(100.0, 100.0));
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame_with_events(ctx, vec![pointer, wheel], &mut body);
        for _ in 0..30 {
            run_frame_with_events(ctx, Vec::new(), &mut body);
        }
        editor_scroll_area(ctx).expect("ScrollArea must survive")
    }

    /// Scrolls the editor down with the mouse wheel.
    fn scroll_editor_with_wheel(
        ctx: &egui::Context,
        body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        scroll_editor_with_wheel_delta(ctx, egui::vec2(0.0, -200.0), body)
    }

    /// Regression: the editor must live inside a `ScrollArea` on the plain path.
    /// Without it a document taller than the viewport cannot be scrolled, and
    /// because `TextEdit` starts with the cursor at the end, the caret opens
    /// off-screen and stays unreachable.
    #[test]
    fn test_render_editor_plain_path_is_scrollable() {
        let ctx = egui::Context::default();
        let original = long_document(400);
        let mut text = original.clone();

        run_frame_with_events(&ctx, Vec::new(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, false, "");
            });
        });
        let area = editor_scroll_area(&ctx).expect("editor must render inside a ScrollArea");
        assert_eq!(area.clamped_offset().y, 0.0, "a fresh editor starts at the top");

        let area = scroll_editor_with_wheel(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, false, "");
            });
        });
        assert!(
            area.clamped_offset().y > 0.0,
            "the wheel must scroll the editor, offset {:?}",
            area.clamped_offset()
        );
        assert_eq!(text, original, "scrolling must not alter the buffer");
    }

    /// Regression: the same for the highlighted path, which additionally proves
    /// the multi-line layouter keeps the text byte-identical through layout.
    #[test]
    fn test_render_editor_highlighted_path_is_scrollable() {
        let ctx = egui::Context::default();
        let original = long_document(400);
        let mut text = original.clone();

        let area = scroll_editor_with_wheel(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut highlighter = SyntaxHighlighter::new();
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = highlighter.layouter(Language::Rust, font_id);
                super::render_editor(ui, &mut text, Some(&mut layouter), false, false, "");
            });
        });
        assert!(
            area.clamped_offset().y > 0.0,
            "the highlighted editor must scroll, offset {:?}",
            area.clamped_offset()
        );
        assert_eq!(text, original, "layouter must not alter the buffer");
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

    /// The editor lays its text out through the cache it is given, on the
    /// highlighted and the plain path alike, so a frame where nothing
    /// changed reuses the last layout.
    #[test]
    fn test_render_editor_lays_out_through_the_cache() {
        let mut highlighter = SyntaxHighlighter::new();
        for highlighted in [true, false] {
            for word_wrap in [false, true] {
                let ctx = egui::Context::default();
                let original = "fn main() {}\nlet x = 1;".to_string();
                let mut text = original.clone();
                let mut cache = LayoutCache::default();
                let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                        let mut layouter = highlighter.layouter(Language::Rust, font_id);
                        super::render_editor_with(
                            ui,
                            &mut text,
                            highlighted.then_some(&mut layouter),
                            word_wrap,
                            false,
                            "",
                            None,
                            EditorRequest::default(),
                            None,
                            &mut Bookmarks::default(),
                            &LineMarks::default(),
                            None,
                            &mut cache,
                        );
                    });
                });
                let painted = shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(shape) if shape.galley.text() == original => {
                            Some(shape.galley.clone())
                        }
                        _ => None,
                    })
                    .expect("the editor painted the text");
                let cached = cache.last_galley().expect("the editor filled the cache");
                assert!(
                    std::sync::Arc::ptr_eq(&painted, cached),
                    "highlighted {highlighted}, wrap {word_wrap}"
                );
            }
        }
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

    /// Both branches of `render_editor` keep the buffer byte-identical.
    #[test]
    fn test_render_editor_round_trips_text_on_both_paths() {
        let original = "fn main() {\n    let x = 1;\n}\n// tail\nlast".to_string();
        let ctx = egui::Context::default();

        for word_wrap in [false, true] {
            let mut plain = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                super::render_editor(ui, &mut plain, None, word_wrap, false, "");
            });
            assert_eq!(plain, original);

            let mut highlighted = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                let mut highlighter = SyntaxHighlighter::new();
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = highlighter.layouter(Language::C, font_id);
                super::render_editor(ui, &mut highlighted, Some(&mut layouter), word_wrap, false, "");
            });
            assert_eq!(highlighted, original);
        }
    }

    /// One long line, far wider than the 800 point test viewport.
    fn wide_document() -> String {
        format!("let a = \"{}\";\nshort\nline3", "x".repeat(3000))
    }

    /// Renders `text` through `render_editor` on the plain or highlighted path
    /// and returns the scroll state after a horizontal wheel scroll of 300
    /// points. The buffer is asserted unchanged.
    fn scroll_sideways(highlighted: bool, word_wrap: bool) -> egui::containers::scroll_area::State {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let mut highlighter = SyntaxHighlighter::new();

        let area = scroll_editor_with_wheel_delta(&ctx, egui::vec2(-300.0, 0.0), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, &mut text, Some(&mut layouter), word_wrap, false, "");
                } else {
                    super::render_editor(ui, &mut text, None, word_wrap, false, "");
                }
            });
        });
        assert_eq!(text, original, "scrolling must not alter the buffer");
        area
    }

    /// Renders `text` with wrap off on the plain or highlighted path, scrolled
    /// sideways by `scroll_x` points, and returns where it was painted.
    fn paint_wide_line(highlighted: bool, scroll_x: f32) -> Option<(egui::Rect, egui::Rect)> {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let mut highlighter = SyntaxHighlighter::new();
        let mut body = |ui: &mut egui::Ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, &mut text, Some(&mut layouter), false, false, "");
                } else {
                    super::render_editor(ui, &mut text, None, false, false, "");
                }
            });
        };
        if scroll_x != 0.0 {
            scroll_editor_with_wheel_delta(&ctx, egui::vec2(-scroll_x, 0.0), &mut body);
        }
        let shapes = run_frame_with_events(&ctx, Vec::new(), &mut body);
        painted_text(&shapes, &original)
    }

    /// Regression: with wrap off, one line wider than the window pushed the
    /// start of every line off the left edge, where no scroll offset reaches.
    /// `Ui::add_sized` centres a widget that is larger than the space it is
    /// given, so the editor overflowed to the left by half its excess width.
    #[test]
    fn test_wide_line_keeps_line_starts_visible() {
        for highlighted in [false, true] {
            let (galley, clip) =
                paint_wide_line(highlighted, 0.0).expect("the editor must paint the buffer");
            assert!(
                galley.left() >= clip.left(),
                "line starts painted left of the viewport (highlighted: {highlighted}): \
                 galley {galley:?}, clip {clip:?}"
            );
        }
    }

    /// Regression: scrolled fully right, the end of the long line must fill the
    /// viewport. With the centred overflow the scroll range ran past the text
    /// and the viewport showed empty space.
    #[test]
    fn test_wide_line_end_is_reachable_by_scrolling() {
        for highlighted in [false, true] {
            let painted = paint_wide_line(highlighted, 1_000_000.0);
            assert!(
                painted.is_some_and(|(galley, clip)| galley.right() > clip.center().x),
                "line end not visible when scrolled fully right (highlighted: {highlighted}): \
                 painted (galley, clip) {painted:?}"
            );
        }
    }

    #[test]
    fn test_word_wrap_defaults_to_off() {
        assert!(!TextEditorApp::default().word_wrap);
    }

    /// Wrap off: a line wider than the window scrolls sideways on the plain path.
    #[test]
    fn test_plain_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(false, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap off: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(true, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the text wraps to the window, so there is nothing to scroll
    /// sideways, on the plain path.
    #[test]
    fn test_plain_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(false, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(true, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }

    /// The language picker sits in the Tampilan menu, not in the status bar.
    #[test]
    fn test_language_menu_lives_in_view_menu_not_status_bar() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(
            painted_text(&shapes, "Bahasa").is_none(),
            "status bar must not offer Bahasa"
        );
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_view_menu(ui));
        assert!(
            painted_text(&shapes, "Bahasa").is_some(),
            "Tampilan menu must offer Bahasa"
        );
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

    #[test]
    fn test_status_bar_shows_the_git_branch() {
        let mut app = TextEditorApp::default();
        let ctx = egui::Context::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "main").is_none(), "no repository, no branch");

        app.git.status = Some(crate::git::RepoStatus {
            branch: Some("main".into()),
            ahead: 2,
            behind: 1,
            files: vec![crate::git::FileChange {
                path: PathBuf::from("/r/a"),
                orig_path: None,
                staged: None,
                unstaged: Some(crate::git::Kind::Modified),
            }],
            ..Default::default()
        });
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui));
        assert!(painted_text(&shapes, "main* ↓1 ↑2").is_some());
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

    /// The caret's line and column sit at the right end of the status bar,
    /// once the editor has a caret; the document name stays on the left.
    #[test]
    fn test_status_bar_shows_the_caret_position_on_the_right() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "ab\ncde\n".to_string();
        let ctx = egui::Context::default();
        let status = |app: &mut TextEditorApp| {
            run_frame_with_events(&ctx, Vec::new(), |ui| app.show_status_bar(ui))
        };
        // Where the first text starting with `prefix` was painted.
        let starting = |shapes: &[egui::epaint::ClippedShape], prefix: &str| {
            shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(shape) if shape.galley.text().starts_with(prefix) => {
                    Some(egui::Rect::from_min_size(shape.pos, shape.galley.size()))
                }
                _ => None,
            })
        };

        let before = status(&mut app);
        assert_eq!(starting(&before, "Baris"), None, "no caret yet, no position");
        let document = starting(&before, "Dokumen:").expect("document label");

        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(5))));
        state.store(&ctx, app.editor_id());
        let after = status(&mut app);
        let (position, _) = painted_text(&after, "Baris 2, Kolom 3").expect("caret position");
        assert_eq!(
            starting(&after, "Dokumen:"),
            Some(document),
            "the left side does not move"
        );
        assert!(position.left() > document.right());
        assert!(position.right() > 780.0, "right-aligned in the 800 pt window");

        assert!(app.i18n.select("en"));
        assert!(painted_text(&status(&mut app), "Ln 2, Col 3").is_some());
    }

    /// Typing and the arrow keys move the caret; the status bar follows on
    /// the next frame, as it is drawn before the editor.
    #[test]
    fn test_status_bar_position_follows_typing_and_arrows() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "ab\ncde".to_string();
        let ctx = egui::Context::default();
        let id = app.editor_id();
        // The status bar, then the editor: every frame shows the editor, or
        // egui drops its focus.
        let frame = |app: &mut TextEditorApp, events, request: EditorRequest| {
            let mut request = Some(request);
            run_frame_with_events(&ctx, events, |ui| {
                app.show_status_bar(ui);
                let text = &mut app.active_tab_mut().text_content;
                let request = request.take().unwrap_or_default();
                super::render_editor_with(
                    ui,
                    text,
                    None,
                    false,
                    false,
                    "",
                    Some(id),
                    request,
                    None,
                    &mut Bookmarks::default(),
                    &LineMarks::default(),
                    None,
                    &mut LayoutCache::default(),
                );
            })
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let step = |app: &mut TextEditorApp, events, request| {
            frame(app, events, request);
            frame(app, Vec::new(), EditorRequest::default())
        };

        let shapes = step(&mut app, Vec::new(), EditorRequest { focus: true, select: Some((0, 0)), ..Default::default() });
        assert!(painted_text(&shapes, "Baris 1, Kolom 1").is_some());
        let shapes = step(&mut app, vec![egui::Event::Text("xy".into())], EditorRequest::default());
        assert_eq!(app.active_tab().text_content, "xyab\ncde");
        assert!(painted_text(&shapes, "Baris 1, Kolom 3").is_some(), "typing moves the caret");
        let shapes = step(&mut app, vec![key(egui::Key::ArrowDown)], EditorRequest::default());
        assert!(painted_text(&shapes, "Baris 2, Kolom 3").is_some(), "down arrow");
        let shapes = step(&mut app, vec![key(egui::Key::ArrowRight)], EditorRequest::default());
        assert!(painted_text(&shapes, "Baris 2, Kolom 4").is_some(), "right arrow");
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
                let (_, editor) = super::render_editor_with(
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

    /// With bookmarks, the status bar shows their count left of the caret
    /// position; a click on that badge opens their list, a second closes it.
    #[test]
    fn test_status_bar_badge_counts_bookmarks_and_opens_the_list() {
        let mut app = TextEditorApp::default();
        app.active_tab_mut().text_content = "a\nb\nc".into();
        let ctx = egui::Context::default();
        let status = |app: &mut TextEditorApp, events| {
            run_frame_with_events(&ctx, events, |ui| app.show_status_bar(ui))
        };
        let badge_shown = |shapes: &[egui::epaint::ClippedShape]| {
            shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().starts_with("Di-bookmark"))
            })
        };
        assert!(
            !badge_shown(&status(&mut app, Vec::new())),
            "no badge without bookmarks"
        );

        app.toggle_bookmark(1);
        app.toggle_bookmark(3);
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(2),
            )));
        state.store(&ctx, app.editor_id());
        let shapes = status(&mut app, Vec::new());
        let (badge, _) = painted_text(&shapes, "Di-bookmark: 2").expect("badge");
        let (position, _) = painted_text(&shapes, "Baris 2, Kolom 1").expect("caret position");
        assert!(
            badge.right() < position.left(),
            "the caret position stays rightmost"
        );

        assert!(!app.show_bookmarks);
        status(&mut app, click_events(badge.center()));
        assert!(app.show_bookmarks, "the badge opens the list");
        status(&mut app, click_events(badge.center()));
        assert!(!app.show_bookmarks, "and closes it");

        assert!(app.i18n.select("en"));
        assert!(painted_text(&status(&mut app, Vec::new()), "Bookmarked: 2").is_some());
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

    /// Help > Home opens the project page in the browser.
    #[test]
    fn test_help_home_opens_the_project_page() {
        let mut app = TextEditorApp::default();
        let ctx = egui::Context::default();
        let frame = |app: &mut TextEditorApp, events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.show_help_menu(ui));
            output.textures_delta.clear();
            output
        };
        let home = frame(&mut app, Vec::new())
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Beranda" => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("Home item");
        let output = frame(&mut app, click_events(home.center()));
        let opened: Vec<&str> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, ["https://github.com/donairl/red-editor/"]);
        assert!(!app.show_about_modal, "Home is not About");
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

    /// A fresh folder `rred_history_<name>` in the temp directory holding
    /// `a.txt` and `b.txt`.
    fn history_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_history_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        dir
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
    fn test_open_recent_menu_lists_entries_and_reopens_the_clicked_one() {
        let dir = history_folder("menu");
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(a.clone(), EntryKind::File);
        app.history.record(b, EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let (b_rect, _) = painted_text(&shapes, "b.txt").expect("b.txt is listed");
        let (a_rect, _) = painted_text(&shapes, "a.txt").expect("a.txt is listed");
        assert!(b_rect.top() < a_rect.top(), "the newest is listed first");
        assert!(
            painted_text(&shapes, &dir.display().to_string()).is_some(),
            "each entry shows its folder"
        );

        run_frame_with_events(&ctx, click_events(a_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert_eq!(
            app.active_tab().current_path.as_ref(),
            Some(&a),
            "one click reopens the entry"
        );
        assert_eq!(app.history.entries()[0].path, a);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The painted text shape whose full text is `text`.
    fn painted_galley(
        shapes: &[egui::epaint::ClippedShape],
        text: &str,
    ) -> Option<std::sync::Arc<egui::Galley>> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => Some(shape.galley.clone()),
            _ => None,
        })
    }

    /// The characters a galley actually shows, the `…` of a cut included.
    fn shown_chars(galley: &egui::Galley) -> String {
        galley
            .rows
            .iter()
            .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
            .collect()
    }

    #[test]
    fn test_open_recent_menu_cuts_long_names_with_an_ellipsis() {
        let dir = history_folder("long_name");
        // Two-, three- and one-byte characters, 220 bytes in all.
        let name = format!("{}.txt", "café_ñandú_€_".repeat(12));
        let long = dir.join(&name);
        fs::write(&long, "long").unwrap();
        let short = dir.join("a.txt");
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(short.clone(), EntryKind::File);
        app.history.record(long.clone(), EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let menu_width = ctx.global_style().spacing.menu_width;
        let cut = painted_galley(&shapes, &name).expect("the long name is painted");
        assert!(cut.elided, "the long name is cut");
        assert!(cut.size().x <= menu_width, "and fits the menu width");
        let shown = shown_chars(&cut);
        let kept = shown.strip_suffix('…').expect("the cut ends in …");
        assert!(
            !kept.is_empty() && name.starts_with(kept),
            "{shown:?} is whole characters of {name:?}"
        );

        let whole = painted_galley(&shapes, "a.txt").expect("the short name is painted");
        assert!(!whole.elided, "a short name is shown as is");
        assert_eq!(shown_chars(&whole), "a.txt");

        let (cut_rect, _) = painted_text(&shapes, &name).unwrap();
        run_frame_with_events(&ctx, click_events(cut_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert_eq!(
            app.active_tab().current_path.as_ref(),
            Some(&long),
            "the full path is opened"
        );
        assert_eq!(app.active_tab().text_content, "long");
        assert_eq!(
            app.history.entries()[0].path,
            long,
            "the history keeps the full path"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_recent_menu_cuts_a_long_folder_to_the_room_left() {
        let folder: PathBuf = std::iter::once(std::path::MAIN_SEPARATOR.to_string())
            .chain((0..30).map(|i| format!("folder_{i}")))
            .collect();
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.history.record(folder.join("x.txt"), EntryKind::File);

        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let menu_width = ctx.global_style().spacing.menu_width;
        let name = painted_galley(&shapes, "x.txt").expect("the name is painted");
        assert!(!name.elided, "the name keeps its room");
        let folder =
            painted_galley(&shapes, &folder.display().to_string()).expect("the folder is painted");
        assert!(folder.elided, "the long folder is cut");
        assert!(name.size().x + folder.size().x <= menu_width);
    }

    #[test]
    fn test_open_recent_menu_when_empty_and_after_clearing() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let empty = app.i18n.t("file.no_recent").to_owned();
        let clear = app.i18n.t("file.clear_recent").to_owned();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        assert!(painted_text(&shapes, &empty).is_some());
        assert!(painted_text(&shapes, &clear).is_none(), "nothing to clear");

        app.history.record(PathBuf::from("/x.txt"), EntryKind::File);
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| app.show_recent_items(ui));
        let (clear_rect, _) = painted_text(&shapes, &clear).expect("Clear is offered");
        run_frame_with_events(&ctx, click_events(clear_rect.center()), |ui| {
            app.show_recent_items(ui)
        });
        assert!(app.history.is_empty());
    }

    #[test]
    fn test_line_numbers_default_to_on() {
        assert!(TextEditorApp::default().show_line_numbers);
    }

    /// Where the gutter painted the number `n`, and in which colour.
    fn painted_number(
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

    /// Renders `text` through `render_editor` with the gutter on or off.
    fn render_with_gutter(
        ctx: &egui::Context,
        text: &mut String,
        highlighted: bool,
        word_wrap: bool,
        line_numbers: bool,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut highlighter = SyntaxHighlighter::new();
        run_frame_with_events(ctx, events, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, text, Some(&mut layouter), word_wrap, line_numbers, "");
                } else {
                    super::render_editor(ui, text, None, word_wrap, line_numbers, "");
                }
            });
        })
    }

    /// Each line gets its number, right of the window edge, left of the
    /// text and level with the line, on every path.
    #[test]
    fn test_line_numbers_paint_beside_each_line() {
        let doc = "alpha\nbeta\ngamma".to_string();
        for highlighted in [false, true] {
            for word_wrap in [false, true] {
                let ctx = egui::Context::default();
                let mut text = doc.clone();
                let shapes = render_with_gutter(&ctx, &mut text, highlighted, word_wrap, true, Vec::new());
                let (galley, _) = painted_text(&shapes, &doc).expect("text painted");
                let row_h = galley.height() / 3.0;
                for n in 1..=3 {
                    let (rect, _) = painted_number(&shapes, n).unwrap_or_else(|| {
                        panic!("line {n} not numbered (highlighted {highlighted}, wrap {word_wrap})")
                    });
                    assert!(rect.left() > 0.0, "number {n} off the left edge: {rect:?}");
                    assert!(rect.right() < galley.left(), "number {n} overlaps the text");
                    let line_center = galley.top() + row_h * (n as f32 - 0.5);
                    assert!(
                        (rect.center().y - line_center).abs() < 1.0,
                        "number {n} at y {} but its line is at {line_center}",
                        rect.center().y
                    );
                }
                assert!(painted_number(&shapes, 4).is_none(), "only 3 lines");
                assert_eq!(text, doc, "the gutter must not alter the buffer");
            }
        }
    }

    #[test]
    fn test_hidden_line_numbers_paint_nothing() {
        let doc = "alpha\nbeta\ngamma".to_string();
        for highlighted in [false, true] {
            let ctx = egui::Context::default();
            let mut text = doc.clone();
            let shapes = render_with_gutter(&ctx, &mut text, highlighted, false, true, Vec::new());
            let (with_gutter, _) = painted_text(&shapes, &doc).expect("text painted");

            let shapes = render_with_gutter(&ctx, &mut text, highlighted, false, false, Vec::new());
            assert!(painted_number(&shapes, 1).is_none());
            let (without, _) = painted_text(&shapes, &doc).expect("text painted");
            assert!(
                without.left() < with_gutter.left(),
                "hiding the gutter must give its room back to the text: \
                 {without:?} vs {with_gutter:?}"
            );
        }
    }

    /// Scrolled sideways, the numbers stay put and the text slides under them.
    #[test]
    fn test_line_numbers_stick_when_scrolled_sideways() {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (before, _) = painted_number(&shapes, 1).expect("line 1 numbered");

        scroll_editor_with_wheel_delta(&ctx, egui::vec2(-300.0, 0.0), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false, true, "");
            });
        });
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (after, _) = painted_number(&shapes, 1).expect("line 1 still numbered");
        let (galley, clip) = painted_text(&shapes, &original).expect("text painted");
        assert!(galley.left() < clip.left(), "the editor must have scrolled sideways");
        assert_eq!(after, before, "the gutter must not scroll sideways");
    }

    /// The caret's line number is brighter than the rest.
    #[test]
    fn test_caret_line_number_is_highlighted() {
        let ctx = egui::Context::default();
        let doc = "alpha\nbeta\ngamma".to_string();
        let mut text = doc.clone();
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let (galley, _) = painted_text(&shapes, &doc).expect("text painted");
        let line2 = egui::pos2(galley.left() + 4.0, galley.top() + galley.height() * 0.5);

        render_with_gutter(&ctx, &mut text, false, false, true, click_events(line2));
        let shapes = render_with_gutter(&ctx, &mut text, false, false, true, Vec::new());
        let palette = crate::theme::active(&ctx).gutter;
        assert_eq!(painted_number(&shapes, 2).unwrap().1, palette.number_active);
        assert_eq!(painted_number(&shapes, 1).unwrap().1, palette.number);
        assert_eq!(painted_number(&shapes, 3).unwrap().1, palette.number);
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
    fn test_file_menu_tab_actions() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_welcome: false, ..Default::default() };
        assert_eq!(app.tabs.len(), 1);

        // Warm up frame so egui lays out the buttons
        let mut tab_baru_center = egui::pos2(0.0, 0.0);
        let mut tutup_tab_center = egui::pos2(0.0, 0.0);
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            let r1 = ui.button("Tab Baru").rect;
            let r2 = ui.button("Tutup Tab").rect;
            tab_baru_center = r1.center();
            tutup_tab_center = r2.center();
        });

        // Click "Tab Baru" button in show_file_menu
        let click_tab_baru = vec![
            egui::Event::PointerMoved(tab_baru_center),
            egui::Event::PointerButton {
                pos: tab_baru_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: tab_baru_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, click_tab_baru, |ui| {
            app.show_file_menu(ui, &ctx);
        });
        assert_eq!(app.tabs.len(), 2, "Menu 'Tab Baru' should create a new tab");
        assert_eq!(app.active_tab_index, 1);

        // Warm up frame before next click
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_file_menu(ui, &ctx);
        });

        // Click "Tutup Tab" button in show_file_menu
        let click_tutup_tab = vec![
            egui::Event::PointerMoved(tutup_tab_center),
            egui::Event::PointerButton {
                pos: tutup_tab_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: tutup_tab_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        run_frame_with_events(&ctx, click_tutup_tab, |ui| {
            app.show_file_menu(ui, &ctx);
        });
        assert_eq!(app.tabs.len(), 1, "Menu 'Tutup Tab' should close active tab");
        assert_eq!(app.active_tab_index, 0);
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
    fn test_file_menu_offers_new_window_after_close_tab() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_file_menu(ui, &ctx);
        });
        let (close_tab, _) = painted_text(&shapes, app.i18n.t("file.close_tab")).unwrap();
        let (new_window, _) =
            painted_text(&shapes, app.i18n.t("file.new_window")).expect("New Window is listed");
        assert!(new_window.top() > close_tab.top(), "below Close Tab");
        let shortcut = shortcuts::label(&ctx, &shortcuts::NEW_WINDOW);
        let (shortcut, _) = painted_text(&shapes, &shortcut).expect("its shortcut is shown");
        assert!((shortcut.center().y - new_window.center().y).abs() < 1.0);
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
