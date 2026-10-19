//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::PathBuf;

use crate::activity_bar;
use crate::file_browser::{self, Explorer};
use crate::gutter;
use crate::highlighter::{DocumentHighlightCache, HighlighterLayouter, Language, SyntaxHighlighter};
use crate::i18n::{self, I18n};
use crate::markdown_preview::{MarkdownPreview, ScrollPos, ScrollSync};
use crate::modals::{self, ModalResponse, PendingAction};
use crate::prettify;
use crate::find::{self, FindAction, FindState, GotoAction, GotoState};
use crate::search::{self, SearchAction, SearchState};
use crate::tab_bar;

/// Storage key under which eframe remembers the interface language.
const UI_LANGUAGE_KEY: &str = "ui_language";

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentTab {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub active_language: Language,
    pub language_override: Option<Language>,
    /// Line-by-line highlight state, so an edit only re-parses the lines it touched.
    pub highlight_cache: DocumentHighlightCache,
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
        }
    }

    pub fn from_file(path: PathBuf) -> Result<Self, std::io::Error> {
        let content = fs::read_to_string(&path)?;
        let active_language = Language::from_path(&path);
        Ok(Self {
            text_content: content.clone(),
            saved_content: content,
            current_path: Some(path),
            active_language,
            language_override: None,
            highlight_cache: DocumentHighlightCache::default(),
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
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
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No file path specified",
            ))
        }
    }
}

pub struct TextEditorApp {
    pub tabs: Vec<DocumentTab>,
    pub active_tab_index: usize,
    /// The open folder's tree; `None` until a folder is opened.
    pub explorer: Option<Explorer>,
    /// Whether the Explorer sidebar is shown, once a folder is open.
    pub show_sidebar: bool,
    pub pending_action: Option<PendingAction>,
    pub show_about_modal: bool,
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub highlighter: SyntaxHighlighter,
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
    /// Show the line-number gutter left of the text. On by default, as in
    /// VS Code.
    pub show_line_numbers: bool,
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
    pub search: SearchState,
    /// One-shot asks for the editor widget, applied on its next frame.
    pub editor_request: EditorRequest,
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
            pending_action: None,
            show_about_modal: false,
            status_message: None,
            highlighter: SyntaxHighlighter::new(),
            word_wrap: false,
            show_line_numbers: true,
            zen_mode: false,
            show_markdown_preview: false,
            markdown_preview: MarkdownPreview::default(),
            scroll_sync: ScrollSync::default(),
            i18n: I18n::builtin(),
            find: FindState::default(),
            goto: GotoState::default(),
            search: SearchState::default(),
            editor_request: EditorRequest::default(),
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

/// Keyboard shortcuts of the editor. `COMMAND` is Ctrl, or Cmd on macOS.
pub mod shortcuts {
    use eframe::egui::{Key, KeyboardShortcut, Modifiers};

    const fn cmd(key: Key) -> KeyboardShortcut {
        KeyboardShortcut::new(Modifiers::COMMAND, key)
    }
    const fn cmd_shift(key: Key) -> KeyboardShortcut {
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), key)
    }

    pub const NEW_TAB: KeyboardShortcut = cmd(Key::N);
    pub const CLOSE_TAB: KeyboardShortcut = cmd(Key::W);
    pub const OPEN_FILE: KeyboardShortcut = cmd(Key::O);
    pub const OPEN_FOLDER: KeyboardShortcut = cmd_shift(Key::O);
    pub const SAVE: KeyboardShortcut = cmd(Key::S);
    pub const SAVE_AS: KeyboardShortcut = cmd_shift(Key::S);
    pub const EXIT: KeyboardShortcut = cmd(Key::Q);
    pub const TOGGLE_SIDEBAR: KeyboardShortcut = cmd(Key::B);
    pub const NEXT_TAB: KeyboardShortcut = cmd(Key::Tab);
    pub const PREV_TAB: KeyboardShortcut = cmd_shift(Key::Tab);
    pub const FIND_IN_FILES: KeyboardShortcut = cmd_shift(Key::F);
    pub const GOTO_LINE: KeyboardShortcut = cmd(Key::G);
    pub const PRETTIFY: KeyboardShortcut = cmd_shift(Key::I);
    pub const ZEN_MODE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F11);
    pub const MARKDOWN_PREVIEW: KeyboardShortcut = cmd_shift(Key::V);
    pub const FIND: KeyboardShortcut = cmd(Key::F);
    pub const REPLACE: KeyboardShortcut = cmd(Key::H);
    pub const FIND_NEXT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F3);
    pub const FIND_PREV: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);
    // Shown in the Edit menu only: the focused text editor handles these
    // keys itself.
    pub const UNDO: KeyboardShortcut = cmd(Key::Z);
    pub const REDO: KeyboardShortcut = cmd(Key::Y);
    pub const CUT: KeyboardShortcut = cmd(Key::X);
    pub const COPY: KeyboardShortcut = cmd(Key::C);
    pub const PASTE: KeyboardShortcut = cmd(Key::V);
}

/// What a keyboard shortcut asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutAction {
    NewTab,
    CloseTab,
    OpenFile,
    OpenFolder,
    Save,
    SaveAs,
    Exit,
    ToggleSidebar,
    NextTab,
    PrevTab,
    GotoLine,
    Prettify,
    ZenMode,
    MarkdownPreview,
    FindInFiles,
    Find,
    Replace,
    FindNext,
    FindPrev,
}

/// Takes at most one shortcut out of this frame's input. Shift variants are
/// tried first: egui lets `Ctrl+S` match while Shift is also held.
pub fn consume_shortcut(input: &mut egui::InputState) -> Option<ShortcutAction> {
    use ShortcutAction::*;
    [
        (shortcuts::FIND_PREV, FindPrev),
        (shortcuts::FIND_NEXT, FindNext),
        (shortcuts::FIND_IN_FILES, FindInFiles),
        (shortcuts::MARKDOWN_PREVIEW, MarkdownPreview),
        (shortcuts::PRETTIFY, Prettify),
        (shortcuts::SAVE_AS, SaveAs),
        (shortcuts::OPEN_FOLDER, OpenFolder),
        (shortcuts::PREV_TAB, PrevTab),
        (shortcuts::SAVE, Save),
        (shortcuts::OPEN_FILE, OpenFile),
        (shortcuts::NEW_TAB, NewTab),
        (shortcuts::CLOSE_TAB, CloseTab),
        (shortcuts::EXIT, Exit),
        (shortcuts::TOGGLE_SIDEBAR, ToggleSidebar),
        (shortcuts::NEXT_TAB, NextTab),
        (shortcuts::GOTO_LINE, GotoLine),
        (shortcuts::ZEN_MODE, ZenMode),
        (shortcuts::FIND, Find),
        (shortcuts::REPLACE, Replace),
    ]
    .into_iter()
    .find(|(shortcut, _)| input.consume_shortcut(shortcut))
    .map(|(_, action)| action)
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

    pub fn show_view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.word_wrap, self.i18n.t("view.word_wrap"));
        ui.checkbox(&mut self.show_line_numbers, self.i18n.t("view.line_numbers"));
        let ctx = ui.ctx().clone();
        if menu_item(ui, self.i18n.t("view.zen_mode"), &ctx.format_shortcut(&shortcuts::ZEN_MODE)) {
            self.toggle_zen_mode(&ctx);
        }
        let preview_label = self.i18n.t("view.markdown_preview");
        let preview_shortcut = ctx.format_shortcut(&shortcuts::MARKDOWN_PREVIEW);
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
        ui.separator();
        self.show_language_menu(ui);
        self.show_ui_language_menu(ui);
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

    /// The document name and the latest status message.
    pub fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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
        });
    }

    pub fn show_edit_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &egui::KeyboardShortcut| {
            menu_item(ui, label, &ctx.format_shortcut(shortcut))
        };
        if item(ui, self.i18n.t("edit.undo"), &shortcuts::UNDO) {
            self.send_editor_key(egui::Key::Z);
        }
        if item(ui, self.i18n.t("edit.redo"), &shortcuts::REDO) {
            self.send_editor_key(egui::Key::Y);
        }
        ui.separator();
        if item(ui, self.i18n.t("edit.cut"), &shortcuts::CUT) {
            self.editor_request.focus = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut);
        }
        if item(ui, self.i18n.t("edit.copy"), &shortcuts::COPY) {
            self.editor_request.focus = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy);
        }
        if item(ui, self.i18n.t("edit.paste"), &shortcuts::PASTE) {
            self.editor_request.focus = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        ui.separator();
        if item(ui, self.i18n.t("edit.find"), &shortcuts::FIND) {
            self.open_find(ctx, false);
        }
        if item(ui, self.i18n.t("edit.replace"), &shortcuts::REPLACE) {
            self.open_find(ctx, true);
        }
        if item(ui, self.i18n.t("edit.find_in_files"), &shortcuts::FIND_IN_FILES) {
            self.open_search(ctx);
        }
        if item(ui, self.i18n.t("edit.goto_line"), &shortcuts::GOTO_LINE) {
            self.open_goto();
        }
        ui.separator();
        let prettify_label = self.i18n.t("edit.prettify");
        let prettify_shortcut = ctx.format_shortcut(&shortcuts::PRETTIFY);
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

    fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("editor", self.active_tab_index))
    }

    /// The editor's selection as sorted `char` indices, or just the caret.
    fn editor_selection(&self, ctx: &egui::Context) -> Option<find::Range> {
        let state = egui::TextEdit::load_state(ctx, self.editor_id())?;
        let range = state.cursor.char_range()?;
        let (a, b) = (range.primary.index.0, range.secondary.index.0);
        Some((a.min(b), a.max(b)))
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

    pub fn toggle_zen_mode(&mut self, ctx: &egui::Context) {
        self.zen_mode = !self.zen_mode;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.zen_mode));
        self.editor_request.focus = true;
    }

    pub fn open_goto(&mut self) {
        self.goto.visible = true;
        self.goto.focus = true;
        self.goto.input.clear();
    }

    pub fn goto_line(&mut self, line: usize) {
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
            menu_item(ui, label, &ctx.format_shortcut(shortcut))
        };
        if item(ui, self.i18n.t("file.new_tab"), &shortcuts::NEW_TAB) {
            self.open_new_tab();
        }
        if item(ui, self.i18n.t("file.close_tab"), &shortcuts::CLOSE_TAB) {
            self.trigger_close_tab(self.active_tab_index);
        }
        ui.separator();
        if item(ui, self.i18n.t("file.open_file"), &shortcuts::OPEN_FILE) {
            self.trigger_open_file();
        }
        if item(ui, self.i18n.t("file.open_folder"), &shortcuts::OPEN_FOLDER) {
            self.trigger_open_folder();
        }
        ui.separator();
        if item(ui, self.i18n.t("file.save"), &shortcuts::SAVE) {
            self.save();
        }
        if item(ui, self.i18n.t("file.save_as"), &shortcuts::SAVE_AS) {
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
        if self.pending_action.is_some() || self.show_about_modal {
            return;
        }
        match action {
            ShortcutAction::NewTab => self.open_new_tab(),
            ShortcutAction::CloseTab => self.trigger_close_tab(self.active_tab_index),
            ShortcutAction::OpenFile => self.trigger_open_file(),
            ShortcutAction::OpenFolder => self.trigger_open_folder(),
            ShortcutAction::Save => {
                self.save();
            }
            ShortcutAction::SaveAs => {
                self.save_as();
            }
            ShortcutAction::Exit => self.trigger_exit(ctx),
            ShortcutAction::ToggleSidebar => self.toggle_explorer(),
            ShortcutAction::NextTab => {
                self.active_tab_index = (self.active_tab_index + 1) % self.tabs.len();
            }
            ShortcutAction::GotoLine => self.open_goto(),
            ShortcutAction::Prettify => self.prettify_document(),
            ShortcutAction::ZenMode => self.toggle_zen_mode(ctx),
            ShortcutAction::MarkdownPreview => self.toggle_markdown_preview(),
            ShortcutAction::FindInFiles => self.open_search(ctx),
            ShortcutAction::Find => self.open_find(ctx, false),
            ShortcutAction::Replace => self.open_find(ctx, true),
            ShortcutAction::FindNext => self.find_step(ctx, FindAction::Next),
            ShortcutAction::FindPrev => self.find_step(ctx, FindAction::Prev),
            ShortcutAction::PrevTab => {
                self.active_tab_index =
                    (self.active_tab_index + self.tabs.len() - 1) % self.tabs.len();
            }
        }
    }

    pub fn open_new_tab(&mut self) {
        self.tabs.push(DocumentTab::default_new());
        self.active_tab_index = self.tabs.len() - 1;
    }

    pub fn open_file_in_tab(&mut self, path: PathBuf) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.current_path.as_ref() == Some(&path))
        {
            self.active_tab_index = index;
            self.status_message = Some((self.i18n.t("msg.already_open").to_owned(), false));
            return;
        }

        if self.active_tab().is_pristine() {
            self.load_file(path);
        } else {
            match DocumentTab::from_file(path) {
                Ok(tab) => {
                    let exceeds = text_content_exceeds_highlight_limit(&tab.text_content);
                    self.tabs.push(tab);
                    self.active_tab_index = self.tabs.len() - 1;
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
                    self.status_message = Some((self.i18n.tf("msg.open_failed", &[("error", &err.to_string())]), true));
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
        match fs::read_to_string(&path) {
            Ok(content) => {
                let exceeds_limit = text_content_exceeds_highlight_limit(&content);
                let tab = self.active_tab_mut();
                tab.text_content = content.clone();
                tab.saved_content = content;
                tab.active_language = Language::from_path(&path);
                tab.current_path = Some(path);
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
                self.status_message = Some((self.i18n.tf("msg.open_failed", &[("error", &err.to_string())]), true));
            }
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
            match self.tabs[index].save() {
                Ok(_) => {
                    self.status_message = Some((self.i18n.t("msg.saved").to_owned(), false));
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
            self.explorer = Some(Explorer::new(folder));
            self.show_sidebar = true;
            self.status_message = Some((self.i18n.t("msg.folder_opened").to_owned(), false));
        }
    }

    /// The Activity Bar's Explorer item: asks for a folder when none is
    /// open, otherwise shows or hides the sidebar.
    pub fn toggle_explorer(&mut self) {
        if self.explorer.is_none() {
            self.trigger_open_folder();
        } else {
            self.show_sidebar = !self.show_sidebar;
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
        };
        let response = tab_bar::show(ui, &views, self.active_tab_index, &labels);
        match response.action {
            Some(tab_bar::TabBarAction::Select(index)) => self.active_tab_index = index,
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
    render_editor_with(ui, text, layouter, word_wrap, line_numbers, hint, None, EditorRequest::default(), None);
}

/// [`render_editor`] with a fixed widget `id` (so the app can find the
/// editor's state) and a one-shot `request` to act on.
#[allow(clippy::too_many_arguments)]
fn render_editor_with(
    ui: &mut egui::Ui,
    text: &mut String,
    mut layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
    line_numbers: bool,
    hint: &str,
    id: Option<egui::Id>,
    request: EditorRequest,
    scroll_to: Option<f32>,
) -> ScrollPos {
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

        let output = match layouter.as_mut() {
            Some(highlighter) => {
                // `TextEdit` calls this every frame. `frame_job` reuses the
                // cached result, and highlights large documents on a background
                // thread (plain text meanwhile) so the window never freezes.
                // Wrapping is a layout concern, so it is applied to the clone and
                // never reaches the cache.
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = (*highlighter.frame_job(buf.as_str(), ui.ctx())).clone();
                    job.wrap.max_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                text_edit.layouter(&mut lay).show(ui)
            }
            None if word_wrap => text_edit.show(ui),
            None => {
                // Mirror egui's stock `TextEdit` layouter so wrap-off plain text
                // looks identical to the wrapped text: the colour is the
                // `inactive` widget colour (a text edit does not brighten on
                // hover), unless the theme overrides it, and every section gets
                // the same line height.
                let font_id = font_id.clone();
                let color = ui
                    .visuals()
                    .override_text_color
                    .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
                let line_height = ui.fonts_mut(|f| f.row_height(&font_id))
                    + ui.spacing().extra_text_line_spacing;
                let mut lay = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
                    let mut job = egui::text::LayoutJob::simple(
                        buf.as_str().to_owned(),
                        font_id.clone(),
                        color,
                        f32::INFINITY,
                    );
                    // Typing a trailing space must not make the caret jump.
                    job.keep_trailing_whitespace = true;
                    for section in &mut job.sections {
                        section.format.line_height = Some(line_height);
                    }
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                text_edit.layouter(&mut lay).show(ui)
            }
        };

        if let Some(width) = gutter_width {
            gutter::paint(ui, &output, &font_id, width);
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
    });
    ScrollPos::of(&output)
}

impl eframe::App for TextEditorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(UI_LANGUAGE_KEY, self.i18n.code().to_owned());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        if let Some(action) = ctx.input_mut(consume_shortcut) {
            self.apply_shortcut(&ctx, action);
        }

        if self.goto.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.goto.visible = false;
            self.editor_request.focus = true;
        }
        if self.search.visible
            && !self.find.visible
            && !self.goto.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_search();
        }
        if self.find.visible
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_find();
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

        // 1. Top Panel (Menu Bar & Toolbar)
        if !self.zen_mode {
        egui::Panel::top("top_panel").show(ui, |ui| {
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
                    if ui.button(self.i18n.t("help.about")).clicked() {
                        self.show_about_modal = true;
                        ui.close();
                    }
                });


                // Info Karakter di sisi kanan
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = self.active_tab().text_content.len().to_string();
                    ui.label(self.i18n.tf("toolbar.characters", &[("count", &count)]));
                });
            });
        });

        // 2. Bottom Status Bar Panel
        }
        if !self.zen_mode {
            egui::Panel::bottom("status_bar").show(ui, |ui| {
                self.show_status_bar(ui);
            });
        }

        if self.search.visible && !self.zen_mode {
            self.search.poll();
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
        let side_fill = file_browser::SidebarPalette::for_visuals(ui.visuals()).background;
        let side_frame = egui::Frame::NONE.fill(side_fill);
        let explorer_open = self.show_sidebar && self.explorer.is_some();
        if !self.zen_mode {
            let activity = egui::Panel::left("activity_bar")
                .exact_size(activity_bar::WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(side_frame)
                .show(ui, |ui| {
                    activity_bar::show(ui, explorer_open, self.i18n.t("activity.explorer"))
                })
                .inner;
            if activity.clicked {
                self.toggle_explorer();
            }
        }

        let mut file_to_open = None;
        let active_file = self.active_tab().current_path.clone();
        #[allow(clippy::collapsible_if)]
        if self.show_sidebar && !self.zen_mode {
            if let Some(explorer) = self.explorer.as_mut() {
                egui::Panel::left("file_browser_panel")
                    .resizable(true)
                    .default_size(240.0)
                    .min_size(170.0)
                    .max_size(500.0)
                    .show_separator_line(false)
                    .frame(side_frame)
                    .show(ui, |ui| {
                        file_to_open =
                            file_browser::show(ui, explorer, active_file.as_deref(), &self.i18n)
                                .open_file;
                    });
            }
        }

        if let Some(path) = file_to_open {
            self.trigger_open_specific_file(path);
        }

        // Markdown preview, right of the editor. Declared before the central
        // panel so the editor takes whatever width is left.
        let previewing = self.show_markdown_preview && self.preview_available() && !self.zen_mode;
        let mut preview_pos = None;
        if previewing {
            let half = ui.available_width() / 2.0;
            egui::Panel::right("markdown_preview")
                .resizable(true)
                .default_size(half)
                .min_size(200.0)
                .show(ui, |ui| {
                    let too_large = self.i18n.t("preview.too_large");
                    let text = &self.tabs[self.active_tab_index].text_content;
                    let scroll_to = self.scroll_sync.take_preview_offset();
                    preview_pos = Some(self.markdown_preview.show(ui, text, too_large, scroll_to));
                });
        }

        // 4. Central Panel (Main Text Editor Area)
        // No margin and the active tab's colour, so the active tab and the
        // editor below it read as one surface, as in VS Code.
        let editor_fill = tab_bar::TabPalette::for_visuals(ui.visuals()).tab_active;
        let editor_frame = egui::Frame::NONE.fill(editor_fill);
        egui::CentralPanel::default().frame(editor_frame).show(ui, |ui| {
            if !self.zen_mode {
                self.render_tab_bar(ui);
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
            let mut request = std::mem::take(&mut self.editor_request);
            request.highlight = self.find.visible.then_some(self.find.current).flatten();
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
            let mut editor_pos = ScrollPos::default();
            ui.push_id(self.active_tab_index, |ui| {
                if too_large {
                    editor_pos = render_editor_with(
                        ui,
                        &mut self.tabs[active_idx].text_content,
                        None,
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(editor_id),
                        request,
                        scroll_to,
                    );
                } else {
                    let tab = &mut self.tabs[active_idx];
                    let mut layouter = self
                        .highlighter
                        .layouter(language, font_id)
                        .with_document_cache(&mut tab.highlight_cache);
                    editor_pos = render_editor_with(
                        ui,
                        &mut tab.text_content,
                        Some(&mut layouter),
                        word_wrap,
                        line_numbers,
                        hint,
                        Some(editor_id),
                        request,
                        scroll_to,
                    );
                }
            });
            match preview_pos {
                Some(preview_pos) => self.scroll_sync.observe(editor_pos, preview_pos),
                None => self.scroll_sync.reset(),
            }
        });

        // 5. Modals & Dialogs
        modals::show_about_modal(&ctx, &mut self.show_about_modal, &self.i18n);

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
        let mut app = TextEditorApp::default();
        assert_eq!(app.window_title(), "[Dokumen Baru] - Red Text Editor");

        app.active_tab_mut().text_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "[Dokumen Baru] * - Red Text Editor");

        app.active_tab_mut().current_path = Some(PathBuf::from("/home/user/document.txt"));
        app.active_tab_mut().saved_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "document.txt - Red Text Editor");

        app.active_tab_mut().text_content = "Teks diedit".to_string();
        assert_eq!(app.window_title(), "document.txt * - Red Text Editor");
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
        let mut app = TextEditorApp::default();
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
        let mut app = TextEditorApp::default();
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
        let mut app = TextEditorApp::default();
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
    fn test_consume_shortcut_prefers_shift_variant() {
        let ctx = egui::Context::default();
        let press = |key, modifiers| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let ctrl = egui::Modifiers::COMMAND;
        let ctrl_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        for (key, mods, want) in [
            (egui::Key::S, ctrl, ShortcutAction::Save),
            (egui::Key::S, ctrl_shift, ShortcutAction::SaveAs),
            (egui::Key::O, ctrl, ShortcutAction::OpenFile),
            (egui::Key::O, ctrl_shift, ShortcutAction::OpenFolder),
            (egui::Key::Tab, ctrl, ShortcutAction::NextTab),
            (egui::Key::Tab, ctrl_shift, ShortcutAction::PrevTab),
            (egui::Key::Q, ctrl, ShortcutAction::Exit),
            (egui::Key::V, ctrl_shift, ShortcutAction::MarkdownPreview),
        ] {
            let mut got = None;
            let mut output = ctx.run_ui(press(key, mods), |ui| {
                got = ui.ctx().input_mut(consume_shortcut);
            });
            output.textures_delta.clear();
            assert_eq!(got, Some(want), "{key:?} {mods:?}");
        }
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
    fn test_goto_line_moves_caret_and_scrolls() {
        let mut app = TextEditorApp::default();
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
                super::render_editor_with(ui, &mut text, None, false, false, "", Some(id), request.take().unwrap(), None);
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
        let mut app = TextEditorApp::default();
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
                super::render_editor_with(ui, &mut text.borrow_mut(), None, false, false, "", Some(id), request.take().unwrap(), None);
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
        let mut app = TextEditorApp::default();
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
                response = Some(file_browser::show(ui, &mut explorer, None, &I18n::builtin()));
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
    fn test_english_translates_title_status_and_menus() {
        let mut app = TextEditorApp::default();
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
        let palette = crate::gutter::GutterPalette::for_visuals(&ctx.global_style().visuals);
        assert_eq!(painted_number(&shapes, 2).unwrap().1, palette.number_active);
        assert_eq!(painted_number(&shapes, 1).unwrap().1, palette.number);
        assert_eq!(painted_number(&shapes, 3).unwrap().1, palette.number);
    }

    #[test]
    fn test_tab_bar_ui_rendering_and_interaction() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
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
        let mut app = TextEditorApp::default();
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
        let mut app = TextEditorApp::default();
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
    fn test_tab_bar_dirty_indicator_in_title() {
        let mut app = TextEditorApp::default();
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

    fn markdown_app() -> TextEditorApp {
        let mut app = TextEditorApp::default();
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
}
