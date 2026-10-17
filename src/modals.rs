//! Modal dialog state and UI rendering.

use eframe::egui;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    OpenFile(PathBuf),
    PromptOpenFile,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalResponse {
    Save,
    Discard,
    Cancel,
}

pub fn show_about_modal(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }

    egui::Window::new("Tentang Red Text Editor")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Red Text Editor");
                ui.label("Versi 0.1.0");
                ui.add_space(8.0);
                ui.label("Editor teks sederhana dan cepat berbasis Rust & egui.");
                ui.add_space(12.0);
                if ui.button("Tutup").clicked() {
                    *open = false;
                }
            });
        });
}

pub fn show_unsaved_changes_modal(ctx: &egui::Context, file_name: &str) -> Option<ModalResponse> {
    let mut response = None;

    egui::Window::new("Perubahan Belum Disimpan")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Terdapat perubahan belum disimpan pada \"{}\". Simpan sekarang?",
                file_name
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Simpan").clicked() {
                    response = Some(ModalResponse::Save);
                }
                if ui.button("Abaikan").clicked() {
                    response = Some(ModalResponse::Discard);
                }
                if ui.button("Batal").clicked() {
                    response = Some(ModalResponse::Cancel);
                }
            });
        });

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_pending_action_equality() {
        let action1 = PendingAction::OpenFile(PathBuf::from("test.txt"));
        let action2 = PendingAction::OpenFile(PathBuf::from("test.txt"));
        let action3 = PendingAction::Exit;
        let action4 = PendingAction::PromptOpenFile;

        assert_eq!(action1, action2);
        assert_ne!(action1, action3);
        assert_ne!(action1, action4);
        assert_ne!(action3, action4);
    }

    #[test]
    fn test_modal_response_equality() {
        let resp1 = ModalResponse::Save;
        let resp2 = ModalResponse::Save;
        let resp3 = ModalResponse::Discard;
        let resp4 = ModalResponse::Cancel;

        assert_eq!(resp1, resp2);
        assert_ne!(resp1, resp3);
        assert_ne!(resp3, resp4);
    }

    #[test]
    fn test_show_about_modal() {
        let ctx = egui::Context::default();
        let mut open = false;
        show_about_modal(&ctx, &mut open);
        assert!(!open);

        let mut open = true;
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            show_about_modal(ctx, &mut open);
        });
        output.textures_delta.clear();
        assert!(open);
    }

    #[test]
    fn test_show_unsaved_changes_modal_headless() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            let response = show_unsaved_changes_modal(ctx, "test.txt");
            assert_eq!(response, None);
        });
        output.textures_delta.clear();
    }
}


