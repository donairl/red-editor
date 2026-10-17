//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::PathBuf;

use crate::file_browser;
use crate::highlighter::{HighlighterLayouter, Language, SyntaxHighlighter};
use crate::modals::{self, ModalResponse, PendingAction};
use crate::tab_bar;

pub const INITIAL_TEXT: &str = "Ketik sesuatu di sini...";

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentTab {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub active_language: Language,
    pub language_override: Option<Language>,
}

impl Default for DocumentTab {
    fn default() -> Self {
        Self::default_new()
    }
}

impl DocumentTab {
    pub fn default_new() -> Self {
        let initial = String::from(INITIAL_TEXT);
        Self {
            text_content: initial.clone(),
            saved_content: initial,
            current_path: None,
            active_language: Language::PlainText,
            language_override: None,
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
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
    }

    pub fn is_pristine(&self) -> bool {
        self.current_path.is_none() && !self.is_dirty() && self.text_content == INITIAL_TEXT
    }

    pub fn file_display_name(&self) -> String {
        self.current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[Dokumen Baru]".to_string())
    }

    pub fn effective_language(&self) -> Language {
        self.language_override.unwrap_or(self.active_language)
    }

    /// How this document appears in the tab strip.
    pub fn tab_view(&self) -> tab_bar::TabView {
        tab_bar::TabView {
            title: self.file_display_name(),
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
    pub opened_folder: Option<PathBuf>,
    pub show_file_browser: bool,
    pub pending_action: Option<PendingAction>,
    pub show_about_modal: bool,
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub highlighter: SyntaxHighlighter,
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
}

impl Default for TextEditorApp {
    fn default() -> Self {
        Self {
            tabs: vec![DocumentTab::default_new()],
            active_tab_index: 0,
            opened_folder: None,
            show_file_browser: false,
            pending_action: None,
            show_about_modal: false,
            status_message: None,
            highlighter: SyntaxHighlighter::new(),
            word_wrap: false,
        }
    }
}

impl TextEditorApp {
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
        self.active_tab().file_display_name()
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

    /// Draws the "Bahasa" dropdown used by both the File menu and the status bar.
    ///
    /// Re-selecting the already active language clears the override and returns
    /// to automatic detection.
    pub fn show_language_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Bahasa", |ui| {
            let current = self.effective_language();
            for lang in Language::all() {
                let selected = *lang == current;
                if ui.selectable_label(selected, lang.display_name()).clicked() {
                    self.active_tab_mut().language_override =
                        if selected { None } else { Some(*lang) };
                    ui.close();
                }
            }
        });
    }

    pub fn show_file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui.button("Tab Baru").clicked() {
            self.open_new_tab();
            ui.close();
        }
        if ui.button("Tutup Tab").clicked() {
            self.trigger_close_tab(self.active_tab_index);
            ui.close();
        }
        ui.separator();
        if ui.button("Buka File...").clicked() {
            self.trigger_open_file();
            ui.close();
        }
        if ui.button("Buka Folder...").clicked() {
            self.trigger_open_folder();
            ui.close();
        }
        ui.separator();
        if ui.button("Simpan").clicked() {
            self.save();
            ui.close();
        }
        if ui.button("Simpan Sebagai...").clicked() {
            self.save_as();
            ui.close();
        }
        ui.separator();
        self.show_language_menu(ui);
        ui.separator();
        if ui.button("Keluar").clicked() {
            self.trigger_exit(ctx);
            ui.close();
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
            self.status_message = Some(("File sudah dibuka.".to_string(), false));
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
                            "File terlalu besar untuk syntax highlighting.".to_string()
                        } else {
                            "File berhasil dibuka.".to_string()
                        },
                        false,
                    ));
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal membuka file: {err}"), true));
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
                        "File terlalu besar untuk syntax highlighting.".to_string()
                    } else {
                        "File berhasil dibuka.".to_string()
                    },
                    false,
                ));
            }
            Err(err) => {
                self.status_message = Some((format!("Gagal membuka file: {err}"), true));
            }
        }
    }

    pub fn save_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        if self.tabs[index].current_path.is_some() {
            match self.tabs[index].save() {
                Ok(_) => {
                    self.status_message = Some(("File berhasil disimpan.".to_string(), false));
                    true
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal menyimpan file: {err}"), true));
                    false
                }
            }
        } else {
            let old_idx = self.active_tab_index;
            self.active_tab_index = index;
            let result = self.save_as();
            self.active_tab_index = old_idx;
            result
        }
    }

    pub fn save(&mut self) -> bool {
        self.save_tab(self.active_tab_index)
    }

    pub fn save_as(&mut self) -> bool {
        let active = self.active_tab();
        let dialog = rfd::FileDialog::new();
        let dialog = if let Some(ref path) = active.current_path {
            if let Some(parent) = path.parent() {
                dialog.set_directory(parent)
            } else {
                dialog
            }
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            let text_to_save = self.active_tab().text_content.clone();
            match fs::write(&path, &text_to_save) {
                Ok(_) => {
                    let active = self.active_tab_mut();
                    active.saved_content = active.text_content.clone();
                    active.active_language = Language::from_path(&path);
                    active.current_path = Some(path);
                    self.status_message = Some(("File berhasil disimpan.".to_string(), false));
                    true
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal menyimpan file: {err}"), true));
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
            self.opened_folder = Some(folder);
            self.show_file_browser = true;
            self.status_message = Some(("Folder dibuka.".to_string(), false));
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
        let views: Vec<tab_bar::TabView> = self.tabs.iter().map(DocumentTab::tab_view).collect();
        let response = tab_bar::show(ui, &views, self.active_tab_index);
        match response.action {
            Some(tab_bar::TabBarAction::Select(index)) => self.active_tab_index = index,
            Some(tab_bar::TabBarAction::Close(index)) => self.trigger_close_tab(index),
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
fn render_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    mut layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
) {
    let scroll_area = if word_wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    scroll_area.show(ui, |ui| {
        // Fill the viewport through `min_size`, not `ui.add_sized`: `add_sized`
        // centres a widget larger than the space it is given, so a line wider
        // than the window pushed the start of every line off the left edge,
        // beyond the reach of the scroll bar. A plain `ui.add` grows the
        // editor right and down from the top-left corner instead.
        let text_edit = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace)
            .lock_focus(true)
            .desired_width(f32::INFINITY)
            .min_size(ui.available_size())
            // No outline or rounded fill: the editor sits on the central
            // panel's background, the same colour as the active tab.
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 6)));

        match layouter.as_mut() {
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
                ui.add(text_edit.layouter(&mut lay));
            }
            None if word_wrap => {
                ui.add(text_edit);
            }
            None => {
                // Mirror egui's stock `TextEdit` layouter so wrap-off plain text
                // looks identical to the wrapped text: the colour is the
                // `inactive` widget colour (a text edit does not brighten on
                // hover), unless the theme overrides it, and every section gets
                // the same line height.
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
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
                ui.add(text_edit.layouter(&mut lay));
            }
        }
    });
}

impl eframe::App for TextEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        // 1. Top Panel (Menu Bar & Toolbar)
        egui::Panel::top("top_panel").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Menu File
                ui.menu_button("File", |ui| {
                    self.show_file_menu(ui, &ctx);
                });

                // Menu Tampilan
                ui.menu_button("Tampilan", |ui| {
                    ui.checkbox(&mut self.word_wrap, "Bungkus Baris");
                });

                // Menu Help
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.show_about_modal = true;
                        ui.close();
                    }
                });

                ui.separator();

                // Toggle Sidebar File Browser button
                if self.opened_folder.is_some() {
                    let toggle_text = if self.show_file_browser {
                        "📂 Sembunyikan Sidebar"
                    } else {
                        "📁 Tampilkan Sidebar"
                    };
                    if ui.button(toggle_text).clicked() {
                        self.show_file_browser = !self.show_file_browser;
                    }
                }

                // Info Karakter di sisi kanan
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Karakter: {}", self.active_tab().text_content.len()));
                });
            });
        });

        // 2. Bottom Status Bar Panel
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let doc_label = self.file_display_name();
                let dirty_str = if self.is_dirty() { " [diubah]" } else { "" };
                ui.label(format!("Dokumen: {doc_label}{dirty_str}"));
                ui.separator();
                self.show_language_menu(ui);

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
        });

        // 3. Left Panel (File Browser Sidebar)
        let mut file_to_open = None;
        let mut close_sidebar = false;

        #[allow(clippy::collapsible_if)]
        if self.show_file_browser {
            if let Some(ref folder) = self.opened_folder {
                egui::Panel::left("file_browser_panel")
                    .resizable(true)
                    .default_size(200.0)
                    .min_size(120.0)
                    .max_size(400.0)
                    .show(ui, |ui| {
                        file_browser::show_file_browser(
                            ui,
                            folder,
                            &mut file_to_open,
                            &mut close_sidebar,
                        );
                    });
            }
        }

        if close_sidebar {
            self.show_file_browser = false;
        }

        if let Some(path) = file_to_open {
            self.trigger_open_specific_file(path);
        }

        // 4. Central Panel (Main Text Editor Area)
        // No margin and the active tab's colour, so the active tab and the
        // editor below it read as one surface, as in VS Code.
        let editor_fill = tab_bar::TabPalette::for_visuals(ui.visuals()).tab_active;
        let editor_frame = egui::Frame::NONE.fill(editor_fill);
        egui::CentralPanel::default().frame(editor_frame).show(ui, |ui| {
            self.render_tab_bar(ui);

            // `TextEdit::font` resolves this `TextStyle` the same way, so the
            // caret and selection line up with the highlighted glyphs.
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let too_large = text_content_exceeds_highlight_limit(&self.active_tab().text_content);

            let word_wrap = self.word_wrap;
            let active_idx = self.active_tab_index;
            let language = self.tabs[active_idx].effective_language();

            ui.push_id(self.active_tab_index, |ui| {
                if too_large {
                    render_editor(ui, &mut self.tabs[active_idx].text_content, None, word_wrap);
                } else {
                    let mut layouter = self.highlighter.layouter(language, font_id);
                    render_editor(
                        ui,
                        &mut self.tabs[active_idx].text_content,
                        Some(&mut layouter),
                        word_wrap,
                    );
                }
            });
        });

        // 5. Modals & Dialogs
        modals::show_about_modal(&ctx, &mut self.show_about_modal);

        #[allow(clippy::collapsible_if)]
        if self.pending_action.is_some() {
            let prompt_name = match self.pending_action {
                Some(PendingAction::CloseTab(idx)) => self
                    .tabs
                    .get(idx)
                    .map(|t| t.file_display_name())
                    .unwrap_or_else(|| self.file_display_name()),
                Some(PendingAction::Exit) => self
                    .tabs
                    .iter()
                    .find(|t| t.is_dirty())
                    .map(|t| t.file_display_name())
                    .unwrap_or_else(|| self.file_display_name()),
                _ => self.file_display_name(),
            };
            if let Some(response) = modals::show_unsaved_changes_modal(&ctx, &prompt_name) {
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
        assert_eq!(app.active_tab().text_content, INITIAL_TEXT);
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
                super::render_editor(ui, &mut text, None, false);
            });
        });
        let area = editor_scroll_area(&ctx).expect("editor must render inside a ScrollArea");
        assert_eq!(area.clamped_offset().y, 0.0, "a fresh editor starts at the top");

        let area = scroll_editor_with_wheel(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                super::render_editor(ui, &mut text, None, false);
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
                super::render_editor(ui, &mut text, Some(&mut layouter), false);
            });
        });
        assert!(
            area.clamped_offset().y > 0.0,
            "the highlighted editor must scroll, offset {:?}",
            area.clamped_offset()
        );
        assert_eq!(text, original, "layouter must not alter the buffer");
    }

    /// Both branches of `render_editor` keep the buffer byte-identical.
    #[test]
    fn test_render_editor_round_trips_text_on_both_paths() {
        let original = "fn main() {\n    let x = 1;\n}\n// tail\nlast".to_string();
        let ctx = egui::Context::default();

        for word_wrap in [false, true] {
            let mut plain = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                super::render_editor(ui, &mut plain, None, word_wrap);
            });
            assert_eq!(plain, original);

            let mut highlighted = original.clone();
            run_frame_with_events(&ctx, Vec::new(), |ui| {
                let mut highlighter = SyntaxHighlighter::new();
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = highlighter.layouter(Language::C, font_id);
                super::render_editor(ui, &mut highlighted, Some(&mut layouter), word_wrap);
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
                    super::render_editor(ui, &mut text, Some(&mut layouter), word_wrap);
                } else {
                    super::render_editor(ui, &mut text, None, word_wrap);
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
                    super::render_editor(ui, &mut text, Some(&mut layouter), false);
                } else {
                    super::render_editor(ui, &mut text, None, false);
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
        let view = tab.tab_view();
        assert_eq!(view.title, "[Dokumen Baru]");
        assert!(view.dirty);
        assert_eq!(view.language, Language::Php);
        assert_eq!(view.tooltip, None);

        tab.current_path = Some(PathBuf::from("/tmp/x/index.php"));
        assert_eq!(tab.tab_view().tooltip.as_deref(), Some("/tmp/x/index.php"));
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
            app.tabs[0].file_display_name(),
            if app.tabs[0].is_dirty() { " *" } else { "" }
        );
        let title_1 = format!(
            "{}{}",
            app.tabs[1].file_display_name(),
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
    fn test_save_tab_restores_active_tab_index() {
        let mut app = TextEditorApp::default();
        app.open_new_tab();
        app.active_tab_index = 1;

        let _saved = app.save_tab(0);
        // Regardless of whether save_as succeeded or failed, active_tab_index must be restored!
        assert_eq!(app.active_tab_index, 1);

        // Clean up if a file was created by the dialog
        if let Some(ref path) = app.tabs[0].current_path {
            let _ = fs::remove_file(path);
        }
    }
}
