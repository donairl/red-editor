//! VS Code-style Activity Bar: the icon column at the far left.
//!
//! Holds two items, the Explorer and the Console, and only reports clicks:
//! `app.rs` decides what a click does.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::file_browser::SidebarPalette;
use crate::theme;

/// Width of the bar, and side of each square item in it.
pub const WIDTH: f32 = 48.0;
/// The bar along the left edge of the item whose view is open.
const ACCENT_WIDTH: f32 = 2.0;

/// One frame of the bar.
#[derive(Clone, Copy, Debug)]
pub struct ActivityBarResponse {
    pub explorer_clicked: bool,
    pub console_clicked: bool,
    // Only the tests read where the items were drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub explorer_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub console_rect: Rect,
}

/// Whether each item's view is open, and the tooltips naming the items.
#[derive(Clone, Copy, Debug)]
pub struct ActivityItems<'a> {
    pub explorer_open: bool,
    pub console_open: bool,
    pub explorer_tooltip: &'a str,
    pub console_tooltip: &'a str,
}

/// Draws the bar down the available height. An item whose view is open is
/// lit and marked with the accent bar.
pub fn show(ui: &mut egui::Ui, items: ActivityItems) -> ActivityBarResponse {
    let palette = theme::active(ui.ctx()).sidebar;

    // Right border, over the full height of the panel.
    let panel = ui.max_rect();
    ui.painter().vline(
        panel.right() - 0.5,
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );

    // Items sit flush, one below the other.
    ui.spacing_mut().item_spacing.y = 0.0;
    let (explorer_rect, explorer_clicked) = item(
        ui,
        &palette,
        items.explorer_open,
        items.explorer_tooltip,
        paint_explorer_icon,
    );
    let (console_rect, console_clicked) = item(
        ui,
        &palette,
        items.console_open,
        items.console_tooltip,
        paint_console_icon,
    );
    ActivityBarResponse {
        explorer_clicked,
        console_clicked,
        explorer_rect,
        console_rect,
    }
}

/// One square item below the previous one; returns where it was drawn and
/// whether it was clicked.
fn item(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    open: bool,
    tooltip: &str,
    paint_icon: fn(&egui::Painter, egui::Pos2, Color32),
) -> (Rect, bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(WIDTH), Sense::click());
    let response = response.on_hover_text(tooltip);
    let painter = ui.painter();
    if open {
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(ACCENT_WIDTH, WIDTH)),
            CornerRadius::ZERO,
            palette.accent,
        );
    }
    let color = if open || response.hovered() {
        palette.icon_active
    } else {
        palette.icon_inactive
    };
    paint_icon(painter, rect.center(), color);
    (rect, response.clicked())
}

/// A window with a `>` prompt and an underscore caret.
fn paint_console_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let frame = Rect::from_center_size(center, vec2(18.0, 15.0));
    painter.rect_stroke(frame, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
    let prompt = frame.left_center() + vec2(4.0, 0.0);
    painter.line(
        vec![
            prompt + vec2(0.0, -3.0),
            prompt + vec2(3.0, 0.0),
            prompt + vec2(0.0, 3.0),
        ],
        stroke,
    );
    let caret_y = frame.center().y + 3.0;
    painter.line_segment([pos2(prompt.x + 5.0, caret_y), pos2(prompt.x + 10.0, caret_y)], stroke);
}

/// Two overlapping pages, the front one with a folded top-right corner.
/// Drawn as lines, so it never depends on the bundled fonts.
fn paint_explorer_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let fold = 4.0;
    let front = Rect::from_center_size(center + vec2(2.0, 2.0), vec2(13.0, 16.0));
    painter.line(
        vec![
            front.left_top(),
            pos2(front.right() - fold, front.top()),
            pos2(front.right(), front.top() + fold),
            front.right_bottom(),
            front.left_bottom(),
            front.left_top(),
        ],
        stroke,
    );
    // Top and left edges of the page behind, up to where the front page
    // covers them.
    let back = front.translate(vec2(-4.0, -4.0));
    painter.line(
        vec![
            pos2(back.left(), front.top() + 8.0),
            back.left_top(),
            pos2(back.right() - fold, back.top()),
            pos2(back.right() - fold, front.top()),
        ],
        stroke,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One headless frame of the bar; returns its response and the shapes.
    fn run(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        explorer_open: bool,
        console_open: bool,
    ) -> (ActivityBarResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(WIDTH, 600.0))),
            events,
            ..Default::default()
        };
        let mut response = None;
        let items = ActivityItems {
            explorer_open,
            console_open,
            explorer_tooltip: "Explorer",
            console_tooltip: "Console",
        };
        let mut output = ctx.run_ui(input, |ui| response = Some(show(ui, items)));
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

    /// Whether a rect filled with `fill` was painted inside `within`.
    fn painted_fill(shapes: &[egui::epaint::ClippedShape], fill: Color32, within: Rect) -> bool {
        shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Rect(rect)
                if rect.fill == fill && within.contains_rect(rect.rect))
        })
    }

    #[test]
    fn test_items_are_squares_explorer_first() {
        let ctx = egui::Context::default();
        let (bar, _) = run(&ctx, Vec::new(), false, false);
        assert_eq!(bar.explorer_rect.size(), Vec2::splat(WIDTH));
        assert_eq!(bar.console_rect.size(), Vec2::splat(WIDTH));
        assert_eq!(bar.console_rect.top(), bar.explorer_rect.bottom());
        assert!(!bar.explorer_clicked && !bar.console_clicked);
    }

    #[test]
    fn test_clicking_an_item_reports_that_item_only() {
        let ctx = egui::Context::default();
        let (bar, _) = run(&ctx, Vec::new(), false, false);
        let (clicked, _) = run(&ctx, click(bar.explorer_rect.center()), false, false);
        assert!(clicked.explorer_clicked && !clicked.console_clicked);
        let (clicked, _) = run(&ctx, click(bar.console_rect.center()), false, false);
        assert!(clicked.console_clicked && !clicked.explorer_clicked);
    }

    #[test]
    fn test_accent_bar_marks_open_items_only() {
        let ctx = egui::Context::default();
        let accent = theme::active(&ctx).sidebar.accent;
        let (bar, shapes) = run(&ctx, Vec::new(), true, false);
        assert!(painted_fill(&shapes, accent, bar.explorer_rect));
        assert!(!painted_fill(&shapes, accent, bar.console_rect));
        let (bar, shapes) = run(&ctx, Vec::new(), false, true);
        assert!(!painted_fill(&shapes, accent, bar.explorer_rect));
        assert!(painted_fill(&shapes, accent, bar.console_rect));
    }
}
