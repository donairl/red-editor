//! The Settings window: the editor settings and the toggles that also live
//! in the menus, in one place. Every control edits the same app field its
//! menu edits, so the two always agree; a change applies at once and is
//! kept with the rest of eframe's storage.

use eframe::egui;

use crate::agent_settings;
use crate::app::TextEditorApp;
use crate::settings::{MAX_TAB_SIZE, MIN_TAB_SIZE};
use crate::zoom;

impl TextEditorApp {
    /// Draws the window while `show_settings` is on; its close button turns
    /// it off. Not modal: the editor stays usable, so a change can be seen
    /// at work.
    pub fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new(self.i18n.t("settings.title").to_owned())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| self.settings_contents(ui));
        self.show_settings = open;
    }

    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.i18n.t("settings.editor"));
        egui::Grid::new("settings_editor").num_columns(2).show(ui, |ui| {
            ui.label(self.i18n.t("settings.tab_size"));
            ui.add(
                egui::DragValue::new(&mut self.editor_settings.tab_size)
                    .range(MIN_TAB_SIZE..=MAX_TAB_SIZE),
            );
            ui.end_row();
        });
        let s = &mut self.editor_settings;
        ui.checkbox(&mut s.insert_spaces, self.i18n.t("settings.insert_spaces"));
        ui.checkbox(
            &mut s.trim_trailing_whitespace,
            self.i18n.t("settings.trim_trailing_whitespace"),
        );
        ui.checkbox(&mut s.format_on_save, self.i18n.t("settings.format_on_save"));

        ui.separator();
        ui.heading(self.i18n.t("settings.appearance"));
        egui::Grid::new("settings_appearance").num_columns(2).show(ui, |ui| {
            ui.label(self.i18n.t("settings.font_size"));
            let mut size = self.font_size;
            // `apply_font_size` picks the field up on the next frame.
            if ui
                .add(egui::DragValue::new(&mut size).range(zoom::MIN_SIZE..=zoom::MAX_SIZE).speed(0.5))
                .changed()
            {
                self.font_size = zoom::clamp(size);
            }
            ui.end_row();

            ui.label(self.i18n.t("settings.theme"));
            let system = self.i18n.t("theme.system").to_owned();
            let name_of = |id: crate::theme::ThemeId| {
                id.name().map(str::to_owned).unwrap_or_else(|| system.clone())
            };
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(name_of(self.theme))
                .show_ui(ui, |ui| {
                    for id in crate::theme::ThemeId::ALL {
                        ui.selectable_value(&mut self.theme, id, name_of(id));
                    }
                });
            ui.end_row();
        });
        ui.checkbox(&mut self.show_indent_guides, self.i18n.t("view.indent_guides"));
        ui.checkbox(&mut self.show_whitespace, self.i18n.t("view.whitespace"));

        ui.separator();
        ui.heading(self.i18n.t("settings.general"));
        let mut auto_save = self.auto_save;
        if ui.checkbox(&mut auto_save, self.i18n.t("file.auto_save")).changed() {
            // Through the toggle, so its timers reset as the menu's do.
            self.toggle_auto_save();
        }
        egui::Grid::new("settings_general").num_columns(2).show(ui, |ui| {
            ui.label(self.i18n.t("settings.language"));
            let current = self.i18n.code().to_owned();
            let locales: Vec<(String, String)> = self
                .i18n
                .locales()
                .map(|(code, name)| (code.to_owned(), name.to_owned()))
                .collect();
            let current_name = locales
                .iter()
                .find(|(code, _)| *code == current)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| current.clone());
            egui::ComboBox::from_id_salt("settings_language")
                .selected_text(current_name)
                .show_ui(ui, |ui| {
                    for (code, name) in locales {
                        if ui.selectable_label(code == current, name).clicked() {
                            self.i18n.select(&code);
                        }
                    }
                });
            ui.end_row();
        });

        ui.separator();
        ui.heading(self.i18n.t("settings.agent"));
        let field_id = egui::Id::unique("settings_agent_command");
        egui::Grid::new("settings_agent").num_columns(2).show(ui, |ui| {
            ui.label(self.i18n.t("settings.agent_preset"));
            // A preset button fills in its command; "Others" is lit for any
            // other command and empties a preset's one so you can type yours.
            let current = agent_settings::preset_of(&self.agent_settings.command);
            ui.horizontal(|ui| {
                for (index, preset) in agent_settings::PRESETS.iter().enumerate() {
                    if ui.selectable_label(current == Some(index), preset.name).clicked() {
                        self.agent_settings.command = preset.command.to_owned();
                    }
                }
                if ui
                    .selectable_label(current.is_none(), self.i18n.t("settings.agent_other"))
                    .clicked()
                {
                    if current.is_some() {
                        self.agent_settings.command.clear();
                    }
                    ui.memory_mut(|memory| memory.request_focus(field_id));
                }
            });
            ui.end_row();

            ui.label(self.i18n.t("settings.agent_command"));
            ui.add(
                egui::TextEdit::singleline(&mut self.agent_settings.command)
                    .id(field_id)
                    .hint_text(self.i18n.t("settings.agent_command_hint"))
                    .desired_width(320.0),
            );
            ui.end_row();
        });
        ui.label(egui::RichText::new(self.i18n.t("settings.agent_restart_note")).weak().small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;

    /// A click on the "Insert spaces" checkbox flips the setting.
    #[test]
    fn test_the_window_toggles_insert_spaces() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_settings: true, ..Default::default() };
        assert!(app.i18n.select("en"));
        assert!(app.editor_settings.insert_spaces);
        // A new window is laid out invisibly on its first frame.
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = run_frame_with_events(&ctx, Vec::new(), |ui| {
                app.show_settings_window(ui.ctx())
            });
        }
        let (rect, _) =
            painted_text(&shapes, "Insert spaces").expect("window lacks Insert spaces");

        run_frame_with_events(&ctx, click_events(rect.center()), |ui| {
            app.show_settings_window(ui.ctx())
        });
        assert!(!app.editor_settings.insert_spaces, "the click did not flip it");
    }

    /// Lays the window out until `label` is painted and returns its rect.
    fn find_label(ctx: &egui::Context, app: &mut TextEditorApp, label: &str) -> egui::Rect {
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = run_frame_with_events(ctx, Vec::new(), |ui| app.show_settings_window(ui.ctx()));
        }
        painted_text(&shapes, label).unwrap_or_else(|| panic!("window lacks {label}")).0
    }

    /// A preset button fills in its command; "Others" empties it for typing,
    /// and leaves a hand-typed command alone.
    #[test]
    fn test_agent_preset_buttons_set_the_command() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { show_settings: true, ..Default::default() };
        assert!(app.i18n.select("en"));

        let rect = find_label(&ctx, &mut app, "Codex");
        run_frame_with_events(&ctx, click_events(rect.center()), |ui| {
            app.show_settings_window(ui.ctx())
        });
        assert_eq!(app.agent_settings.command, "npx -y @zed-industries/codex-acp");

        let rect = find_label(&ctx, &mut app, "Others");
        run_frame_with_events(&ctx, click_events(rect.center()), |ui| {
            app.show_settings_window(ui.ctx())
        });
        assert_eq!(app.agent_settings.command, "", "Others keeps the preset command");

        app.agent_settings.command = "my-agent --acp".to_owned();
        let rect = find_label(&ctx, &mut app, "Others");
        run_frame_with_events(&ctx, click_events(rect.center()), |ui| {
            app.show_settings_window(ui.ctx())
        });
        assert_eq!(app.agent_settings.command, "my-agent --acp");
    }
}
