//! The Console panel: an embedded shell under the editor.

use eframe::egui;
use std::path::PathBuf;

use crate::app::TextEditorApp;
use crate::file_browser;
use crate::terminal::{self, Terminal};
use crate::theme;

impl TextEditorApp {
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
    pub(crate) fn show_console_panel(&mut self, ui: &mut egui::Ui) {
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
                        hide = console_icon_button(
                            ui,
                            "console_hide",
                            self.i18n.t("console.hide"),
                            paint_hide_icon,
                        );
                        kill = console_icon_button(
                            ui,
                            "console_kill",
                            self.i18n.t("console.kill"),
                            paint_kill_icon,
                        );
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
}

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
        ui.painter().rect_filled(
            rect.shrink(2.0),
            egui::CornerRadius::same(4),
            palette.row_hover,
        );
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
    painter.line_segment(
        [
            egui::pos2(center.x - 5.0, lid_y),
            egui::pos2(center.x + 5.0, lid_y),
        ],
        stroke,
    );
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
            [
                egui::pos2(center.x + dx, lid_y + 2.0),
                egui::pos2(center.x + dx, bottom - 1.5),
            ],
            stroke,
        );
    }
}
