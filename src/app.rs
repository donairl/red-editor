//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::PathBuf;

use crate::file_browser;
use crate::highlighter::{HighlighterLayouter, Language, SyntaxHighlighter};
use crate::modals::{self, ModalResponse, PendingAction};

pub struct TextEditorApp {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub opened_folder: Option<PathBuf>,
    pub show_file_browser: bool,
    pub pending_action: Option<PendingAction>,
    pub show_about_modal: bool,
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub active_language: Language,
    pub language_override: Option<Language>,
    pub highlighter: SyntaxHighlighter,
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
}

impl Default for TextEditorApp {
    fn default() -> Self {
        let initial_text = String::from("Ketik sesuatu di sini...");
        Self {
            text_content: initial_text.clone(),
            saved_content: initial_text,
            current_path: None,
            opened_folder: None,
            show_file_browser: false,
            pending_action: None,
            show_about_modal: false,
            status_message: None,
            active_language: Language::PlainText,
            language_override: None,
            highlighter: SyntaxHighlighter::new(),
            word_wrap: false,
        }
    }
}

impl TextEditorApp {
    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
    }

    pub fn file_display_name(&self) -> String {
        self.current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[Dokumen Baru]".to_string())
    }

    pub fn window_title(&self) -> String {
        let dirty_flag = if self.is_dirty() { " *" } else { "" };
        format!("{}{dirty_flag} - Red Text Editor", self.file_display_name())
    }

    /// The language actually used for highlighting: the manual override if set,
    /// otherwise the language auto-detected from the current file.
    pub fn effective_language(&self) -> Language {
        self.language_override.unwrap_or(self.active_language)
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
                    self.language_override = if selected { None } else { Some(*lang) };
                    ui.close();
                }
            }
        });
    }

    pub fn load_file(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(content) => {
                self.text_content = content.clone();
                self.saved_content = content;
                self.active_language = Language::from_path(&path);
                self.current_path = Some(path);
                self.status_message = Some((
                    if text_content_exceeds_highlight_limit(&self.text_content) {
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

    pub fn save(&mut self) -> bool {
        if let Some(path) = self.current_path.clone() {
            match fs::write(&path, &self.text_content) {
                Ok(_) => {
                    self.saved_content = self.text_content.clone();
                    self.status_message = Some(("File berhasil disimpan.".to_string(), false));
                    true
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal menyimpan file: {err}"), true));
                    false
                }
            }
        } else {
            self.save_as()
        }
    }

    pub fn save_as(&mut self) -> bool {
        let dialog = rfd::FileDialog::new();
        let dialog = if let Some(ref path) = self.current_path {
            if let Some(parent) = path.parent() {
                dialog.set_directory(parent)
            } else {
                dialog
            }
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            match fs::write(&path, &self.text_content) {
                Ok(_) => {
                    self.saved_content = self.text_content.clone();
                    self.active_language = Language::from_path(&path);
                    self.current_path = Some(path);
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
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::PromptOpenFile);
        } else {
            self.pick_and_open_file();
        }
    }

    pub fn pick_and_open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.load_file(path);
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
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::OpenFile(path));
        } else {
            self.load_file(path);
        }
    }

    pub fn trigger_exit(&mut self, ctx: &egui::Context) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::Exit);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn execute_pending_action(&mut self, ctx: &egui::Context, action: PendingAction) {
        match action {
            PendingAction::OpenFile(path) => {
                self.load_file(path);
            }
            PendingAction::PromptOpenFile => {
                self.pick_and_open_file();
            }
            PendingAction::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
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
        let text_edit = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace)
            .lock_focus(true)
            .desired_width(f32::INFINITY);

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
                ui.add_sized(ui.available_size(), text_edit.layouter(&mut lay));
            }
            None if word_wrap => {
                ui.add_sized(ui.available_size(), text_edit);
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
                ui.add_sized(ui.available_size(), text_edit.layouter(&mut lay));
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
                        self.trigger_exit(&ctx);
                        ui.close();
                    }
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
                    ui.label(format!("Karakter: {}", self.text_content.len()));
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
        egui::CentralPanel::default().show(ui, |ui| {
            // `TextEdit::font` resolves this `TextStyle` the same way, so the
            // caret and selection line up with the highlighted glyphs.
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            let too_large = text_content_exceeds_highlight_limit(&self.text_content);

            // Split the borrows so the text and the highlighter can be handed to
            // `render_editor` together.
            let Self {
                text_content,
                highlighter,
                active_language,
                language_override,
                word_wrap,
                ..
            } = self;
            let language = language_override.unwrap_or(*active_language);

            if too_large {
                render_editor(ui, text_content, None, *word_wrap);
            } else {
                let mut layouter = highlighter.layouter(language, font_id);
                render_editor(ui, text_content, Some(&mut layouter), *word_wrap);
            }
        });

        // 5. Modals & Dialogs
        modals::show_about_modal(&ctx, &mut self.show_about_modal);

        #[allow(clippy::collapsible_if)]
        if self.pending_action.is_some() {
            if let Some(response) =
                modals::show_unsaved_changes_modal(&ctx, &self.file_display_name())
            {
                let action = self.pending_action.take().unwrap();
                match response {
                    ModalResponse::Save => {
                        if self.save() {
                            self.execute_pending_action(&ctx, action);
                        }
                    }
                    ModalResponse::Discard => {
                        self.execute_pending_action(&ctx, action);
                    }
                    ModalResponse::Cancel => {
                        // Action cancelled, do nothing
                    }
                }
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

        app.text_content = "Berubah".to_string();
        assert!(app.is_dirty());

        app.saved_content = "Berubah".to_string();
        assert!(!app.is_dirty());
    }

    #[test]
    fn test_window_title_formatting() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.window_title(), "[Dokumen Baru] - Red Text Editor");

        app.text_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "[Dokumen Baru] * - Red Text Editor");

        app.current_path = Some(PathBuf::from("/home/user/document.txt"));
        app.saved_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "document.txt - Red Text Editor");

        app.text_content = "Teks diedit".to_string();
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

        assert_eq!(app.text_content, "Hello from file");
        assert_eq!(app.saved_content, "Hello from file");
        assert_eq!(app.current_path, Some(test_file.clone()));
        assert!(!app.is_dirty());
        assert_eq!(
            app.status_message,
            Some(("File berhasil dibuka.".to_string(), false))
        );

        app.text_content = "Modified content".to_string();
        assert!(app.is_dirty());

        let save_result = app.save();
        assert!(save_result);
        assert!(!app.is_dirty());
        assert_eq!(app.saved_content, "Modified content");
        let reloaded = fs::read_to_string(&test_file).unwrap();
        assert_eq!(reloaded, "Modified content");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_trigger_open_specific_file_dirty_vs_clean() {
        let mut app = TextEditorApp::default();
        let target_path = PathBuf::from("/dummy/path.txt");

        // Clean: directly attempts to load
        app.trigger_open_specific_file(target_path.clone());
        assert_eq!(app.pending_action, None);
        // Because /dummy/path.txt doesn't exist, status_message has an error
        assert!(app.status_message.as_ref().unwrap().1);

        // Dirty: should prompt via pending_action
        app.text_content = "Modified".to_string();
        app.trigger_open_specific_file(target_path.clone());
        assert_eq!(
            app.pending_action,
            Some(PendingAction::OpenFile(target_path))
        );
    }

    #[test]
    fn test_effective_language_prefers_override() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.active_language, crate::highlighter::Language::PlainText);
        assert_eq!(app.language_override, None);
        assert_eq!(
            app.effective_language(),
            crate::highlighter::Language::PlainText
        );

        app.active_language = crate::highlighter::Language::Rust;
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        app.language_override = Some(crate::highlighter::Language::Php);
        assert_eq!(app.effective_language(), crate::highlighter::Language::Php);

        app.language_override = None;
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
        assert_eq!(app.active_language, crate::highlighter::Language::Rust);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_file_keeps_explicit_override() {
        let dir = std::env::temp_dir().join("rred_app_lang_override_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("script.php");
        std::fs::write(&path, "<?php echo 1;").unwrap();

        let mut app = TextEditorApp {
            language_override: Some(crate::highlighter::Language::Rust),
            ..Default::default()
        };
        app.load_file(path);
        assert_eq!(app.active_language, crate::highlighter::Language::Php);
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        let _ = std::fs::remove_dir_all(&dir);
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
    /// texture deltas.
    fn run_frame_with_events(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
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
}
