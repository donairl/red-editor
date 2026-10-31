//! The About dialog: the editor's mark, name, version and links on one
//! page, the keyboard shortcuts on another.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, RichText, Stroke, pos2, vec2};

use crate::app::shortcuts;
use crate::i18n::I18n;
use crate::theme;

/// The project page, opened by Help > Home and the dialog's links.
pub const HOME_URL: &str = "https://github.com/donairl/red-editor/";
/// The license named in the dialog and in `LICENSE`.
pub const LICENSE: &str = "MIT";
const COPYRIGHT: &str = "© 2026 Donny Airlangga";
/// The red of the mark: the editor's name.
const BRAND: Color32 = Color32::from_rgb(0xD3, 0x2F, 0x2F);
const WIDTH: f32 = 440.0;
/// How long "Copied" stays after the info was copied.
const COPIED_SECS: f64 = 2.0;

/// Which page the dialog shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AboutPage {
    #[default]
    About,
    Shortcuts,
}

/// What the dialog keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct AboutState {
    pub page: AboutPage,
    /// egui's clock when the info was last copied.
    copied_at: Option<f64>,
}

/// Version, platform and interface, for bug reports.
pub fn info_text() -> String {
    format!(
        "Red Text Editor {}\nPlatform: {} {}\nUI: egui/eframe",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// The keyboard shortcuts the Shortcuts page lists: translation key and
/// shortcut, in menu order.
fn shortcut_rows() -> [(&'static str, egui::KeyboardShortcut); 16] {
    [
        ("file.new_tab", shortcuts::NEW_TAB),
        ("file.open_file", shortcuts::OPEN_FILE),
        ("file.open_folder", shortcuts::OPEN_FOLDER),
        ("file.save", shortcuts::SAVE),
        ("edit.find", shortcuts::FIND),
        ("edit.replace", shortcuts::REPLACE),
        ("edit.find_in_files", shortcuts::FIND_IN_FILES),
        ("edit.goto_line", shortcuts::GOTO_LINE),
        ("edit.goto_symbol", shortcuts::GOTO_SYMBOL),
        ("edit.goto_definition", shortcuts::GOTO_DEFINITION),
        ("edit.prettify", shortcuts::PRETTIFY),
        ("view.markdown_preview", shortcuts::MARKDOWN_PREVIEW),
        ("view.source_control", shortcuts::SOURCE_CONTROL),
        ("view.blame", shortcuts::BLAME),
        ("view.console", shortcuts::TOGGLE_CONSOLE),
        ("view.zen_mode", shortcuts::ZEN_MODE),
    ]
}

/// Draws the dialog while `open`. Escape and the Close button close it.
pub fn show(ctx: &egui::Context, open: &mut bool, state: &mut AboutState, i18n: &I18n) {
    if !*open {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        *open = false;
        return;
    }
    let theme = theme::active(ctx);
    let palette = theme.sidebar;
    let frame = egui::Frame::NONE
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(10))
        .shadow(ctx.global_style().visuals.window_shadow)
        .inner_margin(egui::Margin::same(0));
    egui::Window::new("about_dialog")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .min_width(WIDTH)
        .max_width(WIDTH)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(frame)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            header(ui, &palette, i18n);
            tabs(ui, &palette, &mut state.page, i18n);
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(24, 16))
                .show(ui, |ui| match state.page {
                    AboutPage::About => about_page(ui, &palette, i18n),
                    AboutPage::Shortcuts => shortcuts_page(ui, &palette, i18n),
                });
            footer(ui, &palette, open, state, i18n);
        });
}

/// The mark, name and version on a band of the chrome colour.
fn header(ui: &mut egui::Ui, palette: &crate::file_browser::SidebarPalette, i18n: &I18n) {
    egui::Frame::NONE
        .fill(palette.row_hover)
        .corner_radius(CornerRadius {
            nw: 10,
            ne: 10,
            sw: 0,
            se: 0,
        })
        .inner_margin(egui::Margin::symmetric(24, 20))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), egui::Sense::hover());
                paint_mark(ui.painter(), rect);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(
                        RichText::new("Red Text Editor")
                            .size(22.0)
                            .strong()
                            .color(palette.icon_active),
                    );
                    ui.label(
                        RichText::new(i18n.tf("about.version", &[("version", env!("CARGO_PKG_VERSION"))]))
                            .size(13.0)
                            .color(palette.icon_inactive),
                    );
                    ui.label(RichText::new(i18n.t("about.description")).size(13.0).color(palette.foreground));
                });
            });
        });
}

/// A red rounded square with a white `R`.
fn paint_mark(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::same(12), BRAND);
    // A lighter sheen on the upper half gives it some depth.
    let sheen = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.center().y));
    painter.rect_filled(
        sheen,
        CornerRadius {
            nw: 12,
            ne: 12,
            sw: 0,
            se: 0,
        },
        Color32::from_white_alpha(12),
    );
    painter.text(
        rect.center() + vec2(0.0, -1.0),
        Align2::CENTER_CENTER,
        "R",
        FontId::proportional(30.0),
        Color32::WHITE,
    );
}

/// The two page tabs, the open one underlined in the accent colour.
fn tabs(ui: &mut egui::Ui, palette: &crate::file_browser::SidebarPalette, page: &mut AboutPage, i18n: &I18n) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 24,
            right: 24,
            top: 8,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                for (candidate, key) in [
                    (AboutPage::About, "about.tab_about"),
                    (AboutPage::Shortcuts, "about.tab_shortcuts"),
                ] {
                    let selected = *page == candidate;
                    let color = if selected { palette.icon_active } else { palette.icon_inactive };
                    let response = ui.add(
                        egui::Label::new(RichText::new(i18n.t(key)).size(13.0).color(color))
                            .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        *page = candidate;
                    }
                    if selected {
                        let rect = response.rect;
                        ui.painter().hline(
                            rect.x_range(),
                            rect.bottom() + 6.0,
                            Stroke::new(2.0, palette.accent),
                        );
                    }
                }
            });
        });
    let rect = ui.max_rect();
    ui.painter().hline(
        rect.x_range(),
        ui.cursor().top() + 6.5,
        Stroke::new(1.0, palette.border),
    );
    ui.add_space(6.0);
}

/// Facts about this build, then the links.
fn about_page(ui: &mut egui::Ui, palette: &crate::file_browser::SidebarPalette, i18n: &I18n) {
    let dim = |text: &str| RichText::new(text).size(12.0).color(palette.icon_inactive);
    let value = |text: &str| RichText::new(text).size(13.0).color(palette.foreground);
    egui::Grid::new("about_facts")
        .num_columns(2)
        .spacing([24.0, 6.0])
        .show(ui, |ui| {
            ui.label(dim(i18n.t("about.fact_version")));
            ui.label(value(env!("CARGO_PKG_VERSION")));
            ui.end_row();
            ui.label(dim(i18n.t("about.fact_built_with")));
            ui.label(value("Rust · egui / eframe"));
            ui.end_row();
            ui.label(dim(i18n.t("about.fact_platform")));
            ui.label(value(&format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)));
            ui.end_row();
            ui.label(dim(i18n.t("about.fact_license")));
            ui.label(value(LICENSE));
            ui.end_row();
        });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let version = env!("CARGO_PKG_VERSION");
        let links = [
            (i18n.t("about.link_home"), HOME_URL.to_owned()),
            (i18n.t("about.link_release_notes"), format!("{HOME_URL}releases/tag/v{version}")),
            (i18n.t("about.link_report_issue"), format!("{HOME_URL}issues/new")),
        ];
        for (i, (label, url)) in links.iter().enumerate() {
            if i > 0 {
                ui.label(dim("·"));
            }
            ui.hyperlink_to(RichText::new(*label).size(13.0), url);
        }
    });
}

/// The main keyboard shortcuts, as the menus show them.
fn shortcuts_page(ui: &mut egui::Ui, palette: &crate::file_browser::SidebarPalette, i18n: &I18n) {
    let ctx = ui.ctx().clone();
    // What the window leaves for the list, so the footer stays in view.
    let max_height = (ctx.content_rect().height() - 260.0).clamp(160.0, 400.0);
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("about_shortcuts")
                .num_columns(2)
                .spacing([24.0, 5.0])
                .striped(true)
                .show(ui, |ui| {
                    for (key, shortcut) in shortcut_rows() {
                        ui.label(RichText::new(i18n.t(key)).size(13.0).color(palette.foreground));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(ctx.format_shortcut(&shortcut))
                                    .monospace()
                                    .size(12.0)
                                    .color(palette.icon_inactive),
                            );
                        });
                        ui.end_row();
                    }
                });
        });
}

/// Copyright on the left; Copy Info and Close on the right.
fn footer(
    ui: &mut egui::Ui,
    palette: &crate::file_browser::SidebarPalette,
    open: &mut bool,
    state: &mut AboutState,
    i18n: &I18n,
) {
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), ui.cursor().top() - 0.5, Stroke::new(1.0, palette.border));
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 24,
            right: 16,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{COPYRIGHT} · {LICENSE}"))
                        .size(11.0)
                        .color(palette.icon_inactive),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(i18n.t("about.close")).clicked() {
                        *open = false;
                    }
                    let now = ui.input(|i| i.time);
                    let copied = state.copied_at.is_some_and(|at| now - at < COPIED_SECS);
                    let label = if copied { i18n.t("about.copied") } else { i18n.t("about.copy_info") };
                    if ui.button(label).on_hover_text(info_text()).clicked() {
                        ui.ctx().copy_text(info_text());
                        state.copied_at = Some(now);
                    }
                    if copied {
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(COPIED_SECS));
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Several frames: a window measures itself before it settles.
    fn texts(ctx: &egui::Context, open: &mut bool, state: &mut AboutState) -> Vec<String> {
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 800.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input(), |ui| show(ui.ctx(), open, state, &I18n::builtin()));
        for _ in 0..8 {
            output.textures_delta.clear();
            output = ctx.run_ui(input(), |ui| show(ui.ctx(), open, state, &I18n::builtin()));
        }
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_about_page_shows_name_version_and_links() {
        let ctx = egui::Context::default();
        let mut state = AboutState::default();
        let mut open = false;
        assert!(texts(&ctx, &mut open, &mut state).is_empty(), "closed, nothing drawn");

        let mut open = true;
        let painted = texts(&ctx, &mut open, &mut state);
        assert!(open);
        assert!(painted.iter().any(|t| t == "Red Text Editor"), "{painted:?}");
        assert!(painted.iter().any(|t| t == env!("CARGO_PKG_VERSION")));
        assert!(painted.iter().any(|t| t == "MIT"));
        assert!(painted.iter().any(|t| t == "Beranda"), "home link, in Indonesian");
        assert!(!painted.iter().any(|t| t == "Ctrl+S"), "shortcuts are on the other page");
    }

    #[test]
    fn test_shortcuts_page_lists_the_menu_shortcuts() {
        let ctx = egui::Context::default();
        let mut state = AboutState {
            page: AboutPage::Shortcuts,
            copied_at: None,
        };
        let mut open = true;
        let painted = texts(&ctx, &mut open, &mut state);
        assert!(painted.iter().any(|t| t == "Ctrl+S"), "{painted:?}");
        assert!(painted.iter().any(|t| t == "Simpan"));
        assert!(painted.iter().any(|t| t == "F11"), "the last row is in view too: {painted:?}");
        assert!(painted.iter().any(|t| t == "Tutup"), "and so is the footer");
        // Every row names a real translation key.
        let i18n = I18n::builtin();
        for (key, _) in shortcut_rows() {
            assert_ne!(i18n.t(key), key, "{key} is not translated");
        }
    }

    #[test]
    fn test_escape_closes_the_dialog() {
        let ctx = egui::Context::default();
        let mut state = AboutState::default();
        let mut open = true;
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| show(ui.ctx(), &mut open, &mut state, &I18n::builtin()));
        output.textures_delta.clear();
        assert!(!open);
    }

    #[test]
    fn test_info_text_names_version_and_platform() {
        let info = info_text();
        assert!(info.starts_with(&format!("Red Text Editor {}\n", env!("CARGO_PKG_VERSION"))));
        assert!(info.contains(std::env::consts::ARCH));
    }
}
