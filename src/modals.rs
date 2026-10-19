//! Modal dialog state and UI rendering.

use eframe::egui;

use crate::i18n::I18n;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    CloseTab(usize),
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalResponse {
    Save,
    Discard,
    Cancel,
}

pub fn show_about_modal(ctx: &egui::Context, open: &mut bool, i18n: &I18n) {
    if !*open {
        return;
    }

    egui::Window::new(i18n.t("about.title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Red Text Editor");
                ui.label(i18n.tf("about.version", &[("version", env!("CARGO_PKG_VERSION"))]));
                ui.add_space(8.0);
                ui.label(i18n.t("about.description"));
                ui.add_space(12.0);
                if ui.button(i18n.t("about.close")).clicked() {
                    *open = false;
                }
            });
        });
}

pub fn show_unsaved_changes_modal(
    ctx: &egui::Context,
    file_name: &str,
    i18n: &I18n,
) -> Option<ModalResponse> {
    let mut response = None;

    egui::Window::new(i18n.t("unsaved.title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(i18n.tf("unsaved.message", &[("name", file_name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(i18n.t("unsaved.save")).clicked() {
                    response = Some(ModalResponse::Save);
                }
                if ui.button(i18n.t("unsaved.discard")).clicked() {
                    response = Some(ModalResponse::Discard);
                }
                if ui.button(i18n.t("unsaved.cancel")).clicked() {
                    response = Some(ModalResponse::Cancel);
                }
            });
        });

    response
}

/// What to keep when a file with unsaved edits changed on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChoice {
    /// Read the file again, dropping the unsaved edits.
    Reload,
    /// Keep the edits; the next save overwrites the file.
    Keep,
}

/// Asks what to keep of `file_name`, changed on disk while it had unsaved
/// edits.
pub fn show_disk_conflict_modal(
    ctx: &egui::Context,
    file_name: &str,
    i18n: &I18n,
) -> Option<DiskChoice> {
    let mut choice = None;
    egui::Window::new(i18n.t("disk.title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(i18n.tf("disk.message", &[("name", file_name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(i18n.t("disk.reload")).clicked() {
                    choice = Some(DiskChoice::Reload);
                }
                if ui.button(i18n.t("disk.keep")).clicked() {
                    choice = Some(DiskChoice::Keep);
                }
            });
        });
    choice
}

/// Lists the bookmarked `(line, text)` rows of the active document above
/// the status bar. It stays open while the user jumps between them; the
/// caret's line is shown selected. Returns the line picked.
pub fn show_bookmarks_panel(
    ctx: &egui::Context,
    open: &mut bool,
    rows: &[(usize, &str)],
    caret_line: Option<usize>,
    i18n: &I18n,
) -> Option<usize> {
    let mut picked = None;
    egui::Window::new(i18n.t("bookmark.title"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(320.0)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -36.0])
        .show(ctx, |ui| {
            if rows.is_empty() {
                ui.weak(i18n.t("bookmark.empty"));
                return;
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for &(line, text) in rows {
                    let number = i18n.tf("bookmark.line", &[("line", &line.to_string())]);
                    let row = egui::Button::selectable(caret_line == Some(line), format!("{number}: {}", text.trim()))
                        .truncate()
                        .min_size(egui::vec2(ui.available_width(), 0.0));
                    if ui.add(row).clicked() {
                        picked = Some(line);
                    }
                }
            });
        });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_action_equality() {
        let action1 = PendingAction::Exit;
        let action2 = PendingAction::Exit;
        let action3 = PendingAction::CloseTab(1);
        let action4 = PendingAction::CloseTab(1);
        let action5 = PendingAction::CloseTab(2);

        assert_eq!(action1, action2);
        assert_ne!(action1, action3);
        assert_eq!(action3, action4);
        assert_ne!(action3, action5);
        assert_ne!(action5, action1);
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
        show_about_modal(&ctx, &mut open, &I18n::builtin());
        assert!(!open);

        let mut open = true;
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            show_about_modal(ctx, &mut open, &I18n::builtin());
        });
        output.textures_delta.clear();
        assert!(open);
    }

    #[test]
    fn test_show_disk_conflict_modal_headless() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let choice = show_disk_conflict_modal(ui.ctx(), "test.txt", &I18n::builtin());
            assert_eq!(choice, None, "nothing clicked");
        });
        output.textures_delta.clear();
    }

    #[test]
    fn test_show_bookmarks_panel_headless() {
        let ctx = egui::Context::default();
        let mut open = true;
        let rows = [(2, "  let x = 1;"), (9, "x")];
        for rows in [&rows[..], &[]] {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let picked = show_bookmarks_panel(ui.ctx(), &mut open, rows, Some(9), &I18n::builtin());
                assert_eq!(picked, None, "nothing clicked");
            });
            output.textures_delta.clear();
        }
        assert!(open);
    }

    #[test]
    fn test_show_unsaved_changes_modal_headless() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            let response = show_unsaved_changes_modal(ctx, "test.txt", &I18n::builtin());
            assert_eq!(response, None);
        });
        output.textures_delta.clear();
    }
}


