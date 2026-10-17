//! Main application state and orchestration.

use eframe::egui;
use std::fs;
use std::path::PathBuf;

use crate::file_browser;
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

    pub fn load_file(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(content) => {
                self.text_content = content.clone();
                self.saved_content = content;
                self.current_path = Some(path);
                self.status_message = Some(("File berhasil dibuka.".to_string(), false));
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
                    if ui.button("Keluar").clicked() {
                        self.trigger_exit(&ctx);
                        ui.close();
                    }
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
            let text_style = egui::TextStyle::Monospace;
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.text_content)
                        .font(text_style)
                        .lock_focus(true)
                        .desired_width(f32::INFINITY),
                );
            });
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
}
