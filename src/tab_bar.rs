//! VS Code-style tab strip drawn above the editor.
//!
//! Knows nothing about documents: `app.rs` describes each tab as a
//! [`TabView`] and applies the returned [`TabBarAction`].

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::highlighter::Language;

/// What the strip needs to know about one open document.
#[derive(Clone, Debug, PartialEq)]
pub struct TabView {
    /// File name, or the placeholder for a new document. No dirty marker:
    /// the close slot shows that.
    pub title: String,
    /// Full path shown on hover; `None` for a document never saved.
    pub tooltip: Option<String>,
    pub language: Language,
    pub dirty: bool,
}

/// Colours of the strip, after VS Code's "Dark Modern" and "Light Modern".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabPalette {
    pub strip: Color32,
    pub tab_inactive: Color32,
    /// Also the editor background, so the active tab joins the editor.
    pub tab_active: Color32,
    pub tab_hover: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub fg_active: Color32,
    pub fg_inactive: Color32,
    /// Rounded box behind the `×` under the pointer.
    pub close_hover: Color32,
}

impl TabPalette {
    pub const DARK: TabPalette = TabPalette {
        strip: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_inactive: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_active: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        tab_hover: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        border: Color32::from_rgb(0x2B, 0x2B, 0x2B),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        fg_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        fg_inactive: Color32::from_rgb(0x9D, 0x9D, 0x9D),
        close_hover: Color32::from_rgb(0x31, 0x31, 0x31),
    };

    pub const LIGHT: TabPalette = TabPalette {
        strip: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_inactive: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        tab_hover: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
        fg_active: Color32::from_rgb(0x3B, 0x3B, 0x3B),
        fg_inactive: Color32::from_rgb(0x86, 0x86, 0x86),
        close_hover: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    };

    /// The palette matching egui's current light or dark mode.
    pub fn for_visuals(visuals: &egui::Visuals) -> TabPalette {
        if visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Short coloured label shown before a tab's name. A `None` colour means
/// "use the inactive foreground".
pub fn badge(language: Language) -> (&'static str, Option<Color32>) {
    match language {
        Language::PlainText => ("TXT", None),
        Language::Rust => ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84))),
        Language::C => ("C", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Cpp => ("C++", Some(Color32::from_rgb(0xF3, 0x4B, 0x7D))),
        Language::Php => ("PHP", Some(Color32::from_rgb(0xA0, 0x74, 0xC4))),
        Language::JavaScript => ("JS", Some(Color32::from_rgb(0xCB, 0xCB, 0x41))),
        Language::TypeScript => ("TS", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Html => ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26))),
        Language::Json => ("{}", Some(Color32::from_rgb(0xCB, 0xB0, 0x41))),
        Language::Python => ("PY", Some(Color32::from_rgb(0x4B, 0x8B, 0xBE))),
        Language::Go => ("GO", Some(Color32::from_rgb(0x51, 0xA8, 0xDA))),
        Language::Blade => ("BL", Some(Color32::from_rgb(0xF0, 0x53, 0x40))),
        Language::Twig => ("TW", Some(Color32::from_rgb(0x8D, 0xB3, 0x3A))),
        Language::Smarty => ("SM", Some(Color32::from_rgb(0xE8, 0xB3, 0x39))),
        Language::Markdown => ("MD", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
    }
}

/// What the close slot at the right of a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseSlot {
    Empty,
    Dot,
    Cross,
    /// `×` on a rounded hover box.
    CrossHovered,
}

/// VS Code's rule for the close slot.
fn close_slot(slot_hovered: bool, dirty: bool, active: bool, tab_hovered: bool) -> CloseSlot {
    if slot_hovered {
        CloseSlot::CrossHovered
    } else if dirty {
        CloseSlot::Dot
    } else if active || tab_hovered {
        CloseSlot::Cross
    } else {
        CloseSlot::Empty
    }
}

/// Height of the strip and of every tab in it.
pub const TAB_HEIGHT: f32 = 35.0;
const PADDING_X: f32 = 10.0;
const BADGE_GAP: f32 = 6.0;
const CLOSE_GAP: f32 = 8.0;
const CLOSE_SIZE: f32 = 16.0;

/// What the user asked the strip to do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    /// Context menu: close every tab but this one.
    CloseOthers(usize),
    /// Context menu: close every tab after this one.
    CloseToRight(usize),
    /// Context menu: close every tab without unsaved changes.
    CloseSaved,
    New,
}

/// Texts of the tab context menu; the strip does no translation itself.
#[derive(Clone, Copy, Debug)]
pub struct MenuLabels<'a> {
    pub close: &'a str,
    pub close_others: &'a str,
    pub close_to_right: &'a str,
    pub close_saved: &'a str,
}

/// One frame of the strip: what was clicked, and where each part was drawn.
#[derive(Clone, Debug)]
pub struct TabBarResponse {
    pub action: Option<TabBarAction>,
    // Only the tests read where things were drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub tab_rects: Vec<Rect>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub close_rects: Vec<Rect>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub new_tab_rect: Rect,
}

/// Draws the strip across the available width and reports what was clicked.
pub fn show(
    ui: &mut egui::Ui,
    tabs: &[TabView],
    active: usize,
    labels: &MenuLabels,
) -> TabBarResponse {
    let palette = TabPalette::for_visuals(ui.visuals());

    // Background and bottom border for the whole width, including the empty
    // part after the last tab. The active tab paints over the border.
    let top = ui.available_rect_before_wrap();
    let strip = Rect::from_min_size(top.min, vec2(top.width(), TAB_HEIGHT));
    ui.painter()
        .rect_filled(strip, CornerRadius::ZERO, palette.strip);
    ui.painter().hline(
        strip.x_range(),
        strip.bottom() - 0.5,
        Stroke::new(1.0, palette.border),
    );

    egui::ScrollArea::horizontal()
        .id_salt("tab_bar_scroll")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let mut response = TabBarResponse {
                    action: None,
                    tab_rects: Vec::with_capacity(tabs.len()),
                    close_rects: Vec::with_capacity(tabs.len()),
                    new_tab_rect: Rect::NOTHING,
                };
                for (index, tab) in tabs.iter().enumerate() {
                    let (rect, close_rect, action) =
                        show_tab(
                            ui,
                            &palette,
                            index,
                            tab,
                            index == active,
                            tabs.len(),
                            labels,
                        );
                    response.tab_rects.push(rect);
                    response.close_rects.push(close_rect);
                    response.action = response.action.or(action);
                }

                let (rect, new_tab) =
                    ui.allocate_exact_size(Vec2::splat(TAB_HEIGHT), Sense::click());
                if new_tab.hovered() {
                    // Leave the strip's bottom border visible.
                    let fill = rect.with_max_y(rect.bottom() - 1.0);
                    ui.painter()
                        .rect_filled(fill, CornerRadius::ZERO, palette.tab_hover);
                }
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(16.0),
                    palette.fg_inactive,
                );
                if new_tab.clicked() {
                    response.action = Some(TabBarAction::New);
                }
                response.new_tab_rect = rect;
                response
            })
            .inner
        })
        .inner
}

/// Draws one tab; returns its rect, its close slot's rect and any click.
fn show_tab(
    ui: &mut egui::Ui,
    palette: &TabPalette,
    index: usize,
    tab: &TabView,
    active: bool,
    tab_count: usize,
    labels: &MenuLabels,
) -> (Rect, Rect, Option<TabBarAction>) {
    let fg = if active {
        palette.fg_active
    } else {
        palette.fg_inactive
    };
    let (badge_text, badge_color) = badge(tab.language);
    let badge_galley = ui.painter().layout_no_wrap(
        badge_text.to_owned(),
        FontId::monospace(10.0),
        badge_color.unwrap_or(palette.fg_inactive),
    );
    let name_galley =
        ui.painter()
            .layout_no_wrap(tab.title.clone(), FontId::proportional(13.0), fg);

    let width = PADDING_X
        + badge_galley.size().x
        + BADGE_GAP
        + name_galley.size().x
        + CLOSE_GAP
        + CLOSE_SIZE
        + PADDING_X;
    let (rect, body) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
    let close_rect = Rect::from_center_size(
        pos2(rect.right() - PADDING_X - CLOSE_SIZE / 2.0, rect.center().y),
        Vec2::splat(CLOSE_SIZE),
    );
    // Registered after the body, so it wins clicks inside the slot.
    let close = ui.interact(close_rect, body.id.with("close"), Sense::click());
    let tab_hovered = body.hovered() || close.hovered();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if active {
            palette.tab_active
        } else if tab_hovered {
            palette.tab_hover
        } else {
            palette.tab_inactive
        };
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        let border = Stroke::new(1.0, palette.border);
        painter.vline(rect.right() - 0.5, rect.y_range(), border);
        if active {
            painter.hline(
                rect.x_range(),
                rect.top() + 0.5,
                Stroke::new(1.0, palette.accent),
            );
        } else {
            painter.hline(rect.x_range(), rect.bottom() - 0.5, border);
        }

        let badge_pos = pos2(
            rect.left() + PADDING_X,
            rect.center().y - badge_galley.size().y / 2.0,
        );
        let name_pos = pos2(
            badge_pos.x + badge_galley.size().x + BADGE_GAP,
            rect.center().y - name_galley.size().y / 2.0,
        );
        painter.galley(badge_pos, badge_galley, fg);
        painter.galley(name_pos, name_galley, fg);

        let slot = close_slot(close.hovered(), tab.dirty, active, tab_hovered);
        paint_close_slot(painter, palette, close_rect, slot, fg);
    }

    let body = match &tab.tooltip {
        Some(path) => body.on_hover_text(path),
        None => body,
    };
    let mut menu_action = None;
    body.context_menu(|ui| {
        if ui.button(labels.close).clicked() {
            menu_action = Some(TabBarAction::Close(index));
        }
        if ui
            .add_enabled(tab_count > 1, egui::Button::new(labels.close_others))
            .clicked()
        {
            menu_action = Some(TabBarAction::CloseOthers(index));
        }
        if ui
            .add_enabled(index + 1 < tab_count, egui::Button::new(labels.close_to_right))
            .clicked()
        {
            menu_action = Some(TabBarAction::CloseToRight(index));
        }
        if ui.button(labels.close_saved).clicked() {
            menu_action = Some(TabBarAction::CloseSaved);
        }
        if menu_action.is_some() {
            ui.close();
        }
    });
    let action = if menu_action.is_some() {
        menu_action
    } else if close.clicked() {
        Some(TabBarAction::Close(index))
    } else if body.clicked() {
        Some(TabBarAction::Select(index))
    } else {
        None
    };
    (rect, close_rect, action)
}

/// The dot and cross are drawn as shapes, not glyphs, so they never depend on
/// what the bundled fonts cover.
fn paint_close_slot(
    painter: &egui::Painter,
    palette: &TabPalette,
    rect: Rect,
    slot: CloseSlot,
    color: Color32,
) {
    let center = rect.center();
    match slot {
        CloseSlot::Empty => {}
        CloseSlot::Dot => {
            painter.circle_filled(center, 4.0, color);
        }
        CloseSlot::Cross | CloseSlot::CrossHovered => {
            if slot == CloseSlot::CrossHovered {
                painter.rect_filled(rect, CornerRadius::same(4), palette.close_hover);
            }
            let d = 3.5;
            let stroke = Stroke::new(1.3, color);
            painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], stroke);
            painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dark_palette_matches_vscode_dark_modern() {
        let p = TabPalette::DARK;
        assert_eq!(p.strip, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_active, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.tab_hover, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.border, Color32::from_rgb(0x2B, 0x2B, 0x2B));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x78, 0xD4));
        assert_eq!(p.fg_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x9D, 0x9D, 0x9D));
    }

    #[test]
    fn test_light_palette_matches_vscode_light_modern() {
        let p = TabPalette::LIGHT;
        assert_eq!(p.strip, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.tab_hover, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.border, Color32::from_rgb(0xE5, 0xE5, 0xE5));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x5F, 0xB8));
        assert_eq!(p.fg_active, Color32::from_rgb(0x3B, 0x3B, 0x3B));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x86, 0x86, 0x86));
    }

    #[test]
    fn test_palette_follows_egui_theme() {
        assert_eq!(
            TabPalette::for_visuals(&egui::Visuals::dark()),
            TabPalette::DARK
        );
        assert_eq!(
            TabPalette::for_visuals(&egui::Visuals::light()),
            TabPalette::LIGHT
        );
    }

    #[test]
    fn test_every_language_has_a_badge() {
        for lang in Language::all() {
            assert!(!badge(*lang).0.is_empty(), "no badge for {lang:?}");
        }
        assert_eq!(
            badge(Language::Rust),
            ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84)))
        );
        assert_eq!(
            badge(Language::Html),
            ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26)))
        );
        assert_eq!(
            badge(Language::Json),
            ("{}", Some(Color32::from_rgb(0xCB, 0xB0, 0x41)))
        );
        assert_eq!(badge(Language::PlainText), ("TXT", None));
    }

    /// The slot under the pointer always offers `×`; otherwise unsaved
    /// changes win over "active or hovered", so an active dirty tab keeps
    /// its dot.
    #[test]
    fn test_close_slot_rule() {
        use CloseSlot::*;
        // (slot hovered, dirty, active, tab hovered) -> slot
        assert_eq!(close_slot(true, true, false, true), CrossHovered);
        assert_eq!(close_slot(true, false, false, true), CrossHovered);
        assert_eq!(close_slot(false, true, true, false), Dot);
        assert_eq!(close_slot(false, true, false, true), Dot);
        assert_eq!(close_slot(false, false, true, false), Cross);
        assert_eq!(close_slot(false, false, false, true), Cross);
        assert_eq!(close_slot(false, false, false, false), Empty);
    }

    const LABELS: MenuLabels = MenuLabels {
        close: "Close",
        close_others: "Close Others",
        close_to_right: "Close to the Right",
        close_saved: "Close Saved",
    };

    fn view(title: &str, dirty: bool) -> TabView {
        TabView {
            title: title.to_owned(),
            tooltip: None,
            language: Language::Rust,
            dirty,
        }
    }

    /// One headless frame of the strip; returns its response and the shapes.
    fn run(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        tabs: &[TabView],
        active: usize,
    ) -> (TabBarResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| response = Some(show(ui, tabs, active, &LABELS)));
        output.textures_delta.clear();
        (response.expect("show ran"), output.shapes)
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

    /// A filled circle drawn inside `rect`: the dirty dot.
    fn dot_in(shapes: &[egui::epaint::ClippedShape], rect: egui::Rect) -> bool {
        shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Circle(circle)
                if rect.contains(circle.center) && circle.fill != Color32::TRANSPARENT)
        })
    }

    #[test]
    fn test_tabs_are_flush_and_full_height() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        assert_eq!(bar.tab_rects.len(), 2);
        assert_eq!(bar.close_rects.len(), 2);
        assert_eq!(bar.tab_rects[0].height(), TAB_HEIGHT);
        assert_eq!(
            bar.tab_rects[0].right(),
            bar.tab_rects[1].left(),
            "tabs must touch"
        );
        assert_eq!(bar.tab_rects[1].right(), bar.new_tab_rect.left());
        assert!(bar.tab_rects[1].contains_rect(bar.close_rects[1]));
    }

    #[test]
    fn test_clicking_a_tab_selects_it() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 1);
        let (bar, _) = run(&ctx, click(bar.tab_rects[0].center()), &tabs, 1);
        assert_eq!(bar.action, Some(TabBarAction::Select(0)));
    }

    #[test]
    fn test_clicking_a_close_slot_closes_that_tab() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        let (bar, _) = run(&ctx, click(bar.close_rects[1].center()), &tabs, 0);
        assert_eq!(bar.action, Some(TabBarAction::Close(1)));
    }

    #[test]
    fn test_clicking_plus_asks_for_a_new_tab() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        let (bar, _) = run(&ctx, click(bar.new_tab_rect.center()), &tabs, 0);
        assert_eq!(bar.action, Some(TabBarAction::New));
    }

    #[test]
    fn test_dirty_tab_shows_a_dot_until_its_slot_is_hovered() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", true)];
        let (bar, shapes) = run(&ctx, Vec::new(), &tabs, 0);
        assert!(
            dot_in(&shapes, bar.close_rects[1]),
            "dirty tab must show a dot"
        );
        assert!(!dot_in(&shapes, bar.close_rects[0]), "clean tab has no dot");

        let hover = vec![egui::Event::PointerMoved(bar.close_rects[1].center())];
        let (bar, shapes) = run(&ctx, hover, &tabs, 0);
        assert!(
            !dot_in(&shapes, bar.close_rects[1]),
            "hovered slot shows × instead"
        );
        assert_eq!(bar.action, None, "hovering alone does nothing");
    }
}
