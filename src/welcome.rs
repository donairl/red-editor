//! The start page: what the central area shows when there is nothing to
//! edit yet. It only reports what was clicked; `app.rs` opens things.

use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense, Vec2, pos2, vec2};

use crate::file_browser::{self, ROW_HEIGHT, SidebarPalette};
use crate::history::{Entry, EntryKind, History};
use crate::i18n::I18n;
use crate::theme;

/// Widest the centred column gets.
const COLUMN_WIDTH: f32 = 520.0;
/// Below this column width the two recent lists stack.
const SIDE_BY_SIDE_MIN: f32 = 440.0;
const BUTTON_SIZE: Vec2 = vec2(150.0, 32.0);
/// Entries shown per recent list.
const MAX_RECENT: usize = 8;
const PADDING: f32 = 48.0;

/// What the user asked of the start page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WelcomeAction {
    NewFile,
    OpenFile,
    OpenFolder,
    Open(Entry),
}

/// Draws the start page over the whole available area.
pub fn show(ui: &mut egui::Ui, history: &History, i18n: &I18n) -> Option<WelcomeAction> {
    let theme = theme::active(ui.ctx());
    let palette = theme.sidebar;
    let area = ui.max_rect();
    ui.painter().rect_filled(area, CornerRadius::ZERO, theme.tab.tab_active);
    let mut action = None;

    let column_width = (area.width() - 2.0 * PADDING).clamp(240.0, COLUMN_WIDTH).min(area.width());
    let side_by_side = column_width >= SIDE_BY_SIDE_MIN;
    let folders: Vec<&Entry> = history.entries().iter().filter(|e| e.kind == EntryKind::Folder).take(MAX_RECENT).collect();
    let files: Vec<&Entry> = history.entries().iter().filter(|e| e.kind == EntryKind::File).take(MAX_RECENT).collect();
    let list_rows = |n: usize| n.max(1) as f32 * ROW_HEIGHT + ROW_HEIGHT;
    let lists_height = if side_by_side {
        list_rows(folders.len()).max(list_rows(files.len()))
    } else {
        list_rows(folders.len()) + list_rows(files.len()) + 16.0
    };
    // Title, gap, buttons, gap, lists.
    let content_height = 40.0 + 16.0 + BUTTON_SIZE.y + 24.0 + lists_height;
    let top = if content_height < area.height() - 2.0 * PADDING {
        area.center().y - content_height / 2.0
    } else {
        area.top() + PADDING
    };
    let left = area.center().x - column_width / 2.0;

    egui::ScrollArea::vertical()
        .id_salt("welcome_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.allocate_space(vec2(ui.available_width(), (top - area.top()).max(0.0)));
            let column = Rect::from_min_size(pos2(left, ui.cursor().top()), vec2(column_width, content_height));
            ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Red Text Editor").size(22.0).color(palette.foreground));
                    ui.label(RichText::new(env!("CARGO_PKG_VERSION")).size(13.0).color(palette.icon_inactive));
                });
                ui.add_space(16.0);
                ui.horizontal_wrapped(|ui| {
                    let ctx = ui.ctx().clone();
                    for (key, shortcut, result) in [
                        ("welcome.new_file", crate::app::shortcuts::NEW_TAB, WelcomeAction::NewFile),
                        ("welcome.open_file", crate::app::shortcuts::OPEN_FILE, WelcomeAction::OpenFile),
                        ("welcome.open_folder", crate::app::shortcuts::OPEN_FOLDER, WelcomeAction::OpenFolder),
                    ] {
                        let button = egui::Button::new(RichText::new(i18n.t(key)).color(palette.foreground))
                            .shortcut_text(RichText::new(crate::app::shortcuts::label(&ctx, &shortcut)).color(palette.icon_inactive))
                            .min_size(BUTTON_SIZE);
                        if ui.add(button).clicked() {
                            action = Some(result);
                        }
                    }
                });
                ui.add_space(24.0);
                let list_width = if side_by_side { (column_width - 16.0) / 2.0 } else { column_width };
                let folders_heading = i18n.t("welcome.recent_folders");
                let files_heading = i18n.t("welcome.recent_files");
                if side_by_side {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(vec2(list_width, lists_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                            if let Some(a) = show_list(ui, &palette, folders_heading, &folders, list_width, i18n) {
                                action = Some(a);
                            }
                        });
                        ui.add_space(16.0);
                        ui.allocate_ui_with_layout(vec2(list_width, lists_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                            if let Some(a) = show_list(ui, &palette, files_heading, &files, list_width, i18n) {
                                action = Some(a);
                            }
                        });
                    });
                } else {
                    if let Some(a) = show_list(ui, &palette, folders_heading, &folders, list_width, i18n) {
                        action = Some(a);
                    }
                    ui.add_space(16.0);
                    if let Some(a) = show_list(ui, &palette, files_heading, &files, list_width, i18n) {
                        action = Some(a);
                    }
                }
            });
        });
    action
}

/// A heading over the entries, or `file.no_recent` when there are none.
fn show_list(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    heading: &str,
    entries: &[&Entry],
    width: f32,
    i18n: &I18n,
) -> Option<WelcomeAction> {
    ui.scope(|ui| {
        let mut action = None;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            heading.to_uppercase(),
            FontId::proportional(11.0),
            palette.icon_inactive,
        );
        if entries.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            ui.painter().text(
                pos2(rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                i18n.t("file.no_recent"),
                FontId::proportional(13.0),
                palette.icon_inactive,
            );
            return None;
        }
        for entry in entries {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            let row = ui
                .interact(rect, egui::Id::unique(("welcome_recent", &entry.path)), Sense::click())
                .on_hover_text(entry.path.display().to_string())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if row.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(4), palette.row_hover);
            }
            let (name, folder) = entry.label();
            let name = file_browser::elided(ui, name, FontId::proportional(13.0), palette.accent, width * 0.6);
            let name_width = name.size().x;
            ui.painter().galley(pos2(rect.left() + 4.0, rect.center().y - name.size().y / 2.0), name, palette.accent);
            let room = width - name_width - 16.0;
            if room > 40.0 {
                let folder = file_browser::elided(ui, folder, FontId::proportional(11.0), palette.icon_inactive, room);
                ui.painter().galley(pos2(rect.left() + 4.0 + name_width + 8.0, rect.center().y - folder.size().y / 2.0), folder, palette.icon_inactive);
            }
            if row.clicked() {
                action = Some(WelcomeAction::Open((*entry).clone()));
            }
        }
        action
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn history() -> History {
        let mut h = History::default();
        h.record(PathBuf::from("/tmp/proj-a"), EntryKind::Folder);
        h.record(PathBuf::from("/tmp/proj-b"), EntryKind::Folder);
        h.record(PathBuf::from("/tmp/notes.md"), EntryKind::File);
        h.record(PathBuf::from("/tmp/main.rs"), EntryKind::File);
        h
    }

    fn frame(ctx: &egui::Context, history: &History, events: Vec<egui::Event>) -> (Option<WelcomeAction>, Vec<(String, Rect)>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            action = show(ui, history, &I18n::builtin());
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect();
        (action, texts)
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn test_paints_buttons_headings_and_recent_names() {
        let ctx = egui::Context::default();
        let (_, texts) = frame(&ctx, &history(), Vec::new());
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for expected in ["File Baru", "Buka File…", "Buka Folder…", "FOLDER TERBARU", "FILE TERBARU", "proj-a/", "proj-b/", "notes.md", "main.rs"] {
            assert!(names.contains(&expected), "{expected} missing from {names:?}");
        }
        let rect_of = |name: &str| texts.iter().find(|(t, _)| t == name).map(|(_, r)| *r).unwrap();
        let (proj_b, proj_a) = (rect_of("proj-b/"), rect_of("proj-a/"));
        assert!(proj_b.top() < proj_a.top(), "folder rows must stack: {proj_b:?} vs {proj_a:?}");
        assert!((proj_a.left() - proj_b.left()).abs() < 1.0, "folder rows must share a left edge: {proj_b:?} vs {proj_a:?}");
    }

    #[test]
    fn test_buttons_and_recent_rows_report_actions() {
        let ctx = egui::Context::default();
        let history = history();
        let (_, texts) = frame(&ctx, &history, Vec::new());
        let (_, new_file) = texts.iter().find(|(t, _)| t == "File Baru").unwrap();
        let (action, _) = frame(&ctx, &history, click(new_file.center()));
        assert_eq!(action, Some(WelcomeAction::NewFile));

        let (_, texts) = frame(&ctx, &history, Vec::new());
        let (_, notes) = texts.iter().find(|(t, _)| t == "notes.md").unwrap();
        let (action, _) = frame(&ctx, &history, click(notes.center()));
        assert_eq!(action, Some(WelcomeAction::Open(Entry { path: PathBuf::from("/tmp/notes.md"), kind: EntryKind::File })));
    }

    #[test]
    fn test_empty_history_says_so_twice() {
        let ctx = egui::Context::default();
        let (_, texts) = frame(&ctx, &History::default(), Vec::new());
        let count = texts.iter().filter(|(t, _)| t == I18n::builtin().t("file.no_recent")).count();
        assert_eq!(count, 2, "one note per list");
    }
}
