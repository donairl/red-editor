//! The Console panel: an embedded shell under the editor.

use eframe::egui;
use std::path::PathBuf;

use crate::app::TextEditorApp;
use crate::file_browser;
use crate::terminal::{self, Terminal};
use crate::theme;

impl TextEditorApp {
    /// Shows or hides the Console panel. Showing it starts the first shell,
    /// in the open folder, when there is none, and gives it the keyboard.
    pub fn toggle_console(&mut self, ctx: &egui::Context) {
        if self.show_console {
            self.show_console = false;
            self.editor_request.focus = true;
            return;
        }
        // A shell that ended while the panel was hidden leaves no tab.
        self.drop_ended_consoles();
        if self.consoles.is_empty() && !self.spawn_console(ctx) {
            return;
        }
        self.show_console = true;
        ctx.memory_mut(|m| m.request_focus(terminal::widget_id()));
    }

    /// Opens another terminal tab, its shell started in the open folder, and
    /// shows the Console panel with it.
    pub fn new_console(&mut self, ctx: &egui::Context) {
        if !self.spawn_console(ctx) {
            return;
        }
        self.show_console = true;
        ctx.memory_mut(|m| m.request_focus(terminal::widget_id()));
    }

    /// Starts a shell in the Console's folder, adds its tab and selects it.
    /// A shell that cannot start is reported in the status bar instead.
    fn spawn_console(&mut self, ctx: &egui::Context) -> bool {
        match Terminal::spawn(ctx, &self.console_dir()) {
            Ok(console) => {
                self.consoles.push(console);
                self.active_console_index = self.consoles.len() - 1;
                ctx.memory_mut(|m| m.request_focus(terminal::widget_id()));
                true
            }
            Err(error) => {
                let message = self.i18n.tf("msg.console_failed", &[("error", &error)]);
                self.status_message = Some((message, true));
                false
            }
        }
    }

    /// Drops the tabs whose shell has ended, keeping the same terminal
    /// active when it is still running. Returns whether a tab is left.
    fn drop_ended_consoles(&mut self) -> bool {
        let active = self
            .active_console_index
            .min(self.consoles.len().saturating_sub(1));
        let (mut index, mut ended_before) = (0, 0);
        self.consoles.retain_mut(|console| {
            let ended = console.has_exited();
            if ended && index < active {
                ended_before += 1;
            }
            index += 1;
            !ended
        });
        if self.consoles.is_empty() {
            self.active_console_index = 0;
            return false;
        }
        self.active_console_index = (active - ended_before).min(self.consoles.len() - 1);
        true
    }

    /// The Console panel: a strip of terminal tabs, and the shell of the
    /// active one under it. A shell that has ended loses its tab, and the
    /// panel closes with the last one.
    pub(crate) fn show_console_panel(&mut self, ui: &mut egui::Ui) {
        if !self.drop_ended_consoles() {
            self.show_console = false;
            self.editor_request.focus = true;
            return;
        }

        let (mut hide, mut kill, mut add) = (false, false, false);
        egui::Panel::bottom("console_panel")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(self.i18n.t("console.title"));
                    for index in 0..self.consoles.len() {
                        let number = (index + 1).to_string();
                        let label = self.i18n.tf("console.tab", &[("n", &number)]);
                        let selected = index == self.active_console_index;
                        if ui.selectable_label(selected, label).clicked() {
                            self.active_console_index = index;
                            ui.ctx().memory_mut(|m| m.request_focus(terminal::widget_id()));
                        }
                    }
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
                        add = ui
                            .button("+")
                            .on_hover_text(self.i18n.t("view.new_console"))
                            .clicked();
                    });
                });
                self.consoles[self.active_console_index].show(ui);
            });

        if add {
            self.spawn_console(ui.ctx());
        }
        if kill {
            self.consoles.remove(self.active_console_index);
            if self.consoles.is_empty() {
                hide = true;
            } else {
                self.active_console_index =
                    clamp_index(self.active_console_index, self.consoles.len());
            }
        }
        if hide {
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

/// The active tab after the tab list changed: the last one when the old
/// position no longer exists.
fn clamp_index(index: usize, count: usize) -> usize {
    if count == 0 { 0 } else { index.min(count - 1) }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The tab that takes the keyboard after the list shrinks or empties.
    #[test]
    fn test_the_active_tab_survives_a_close() {
        assert_eq!(clamp_index(0, 3), 0);
        assert_eq!(clamp_index(2, 2), 1, "the last tab takes over");
        assert_eq!(clamp_index(1, 0), 0, "no tabs, no index");
    }

    /// A panel with no shell left closes and hands the keyboard back.
    #[test]
    fn test_the_panel_closes_without_a_terminal() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_console: true, ..TextEditorApp::default() };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| app.show_console_panel(ui),
        );
        output.textures_delta.clear();
        assert!(!app.show_console);
        assert!(app.consoles.is_empty());
    }

    /// Every terminal gets its own tab in the strip, and the active one is
    /// the one drawn under it.
    #[cfg(unix)]
    #[test]
    fn test_every_terminal_gets_a_tab() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        for _ in 0..2 {
            let console = Terminal::spawn(&ctx, &std::env::temp_dir()).expect("pty opens");
            app.consoles.push(console);
        }
        app.show_console = true;
        app.active_console_index = 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| app.show_console_panel(ui),
        );
        output.textures_delta.clear();
        let painted: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(painted.iter().any(|t| t == "Terminal 1"), "{painted:?}");
        assert!(painted.iter().any(|t| t == "Terminal 2"), "{painted:?}");
        assert!(app.show_console);
        assert_eq!(app.consoles.len(), 2);
        assert_eq!(app.active_console_index, 1);
    }

    /// A tab whose shell ended goes away, and the terminal that was active
    /// stays active wherever its tab moves to.
    #[cfg(unix)]
    #[test]
    fn test_an_ended_shell_loses_only_its_tab() {
        use portable_pty::CommandBuilder;

        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let ended = || {
            let mut command = CommandBuilder::new("sh");
            command.args(["-c", "exit 0"]);
            let mut console =
                Terminal::spawn_test_command(&ctx, command, &dir).expect("pty opens");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !console.has_exited() {
                assert!(std::time::Instant::now() < deadline, "the shell did not exit");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            console
        };

        // (where the dead tab sits, which tab was active).
        for (dead_at, active) in [(0, 1), (1, 0), (1, 1)] {
            let mut app = TextEditorApp::default();
            let alive = Terminal::spawn(&ctx, &dir).expect("pty opens");
            let dead = ended();
            if dead_at == 0 {
                app.consoles.extend([dead, alive]);
            } else {
                app.consoles.extend([alive, dead]);
            }
            app.active_console_index = active;
            assert!(app.drop_ended_consoles(), "{dead_at} {active}");
            assert_eq!(app.consoles.len(), 1, "{dead_at} {active}");
            assert_eq!(app.active_console_index, 0, "{dead_at} {active}");
        }
    }
}
