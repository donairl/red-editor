//! VS Code-style Activity Bar: the icon column at the far left.
//!
//! Holds one item, the Explorer, and only reports clicks: `app.rs` decides
//! what a click does.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::file_browser::SidebarPalette;

/// Width of the bar, and side of each square item in it.
pub const WIDTH: f32 = 48.0;
/// The bar along the left edge of the item whose view is open.
const ACCENT_WIDTH: f32 = 2.0;

/// One frame of the bar.
#[derive(Clone, Copy, Debug)]
pub struct ActivityBarResponse {
    pub clicked: bool,
    // Only the tests read where the item was drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub explorer_rect: Rect,
}

/// Draws the bar down the available height. `explorer_open` lights the
/// Explorer item and marks it with the accent bar.
pub fn show(ui: &mut egui::Ui, explorer_open: bool) -> ActivityBarResponse {
    let palette = SidebarPalette::for_visuals(ui.visuals());

    // Right border, over the full height of the panel.
    let panel = ui.max_rect();
    ui.painter().vline(
        panel.right() - 0.5,
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(WIDTH), Sense::click());
    let response = response.on_hover_text("Explorer");
    let painter = ui.painter();
    if explorer_open {
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(ACCENT_WIDTH, WIDTH)),
            CornerRadius::ZERO,
            palette.accent,
        );
    }
    let color = if explorer_open || response.hovered() {
        palette.icon_active
    } else {
        palette.icon_inactive
    };
    paint_explorer_icon(painter, rect.center(), color);

    ActivityBarResponse {
        clicked: response.clicked(),
        explorer_rect: rect,
    }
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
    ) -> (ActivityBarResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(WIDTH, 600.0))),
            events,
            ..Default::default()
        };
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| response = Some(show(ui, explorer_open)));
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
    fn test_explorer_item_is_a_square_at_the_top() {
        let ctx = egui::Context::default();
        let (bar, _) = run(&ctx, Vec::new(), false);
        assert_eq!(bar.explorer_rect.size(), Vec2::splat(WIDTH));
        assert!(!bar.clicked);
    }

    #[test]
    fn test_clicking_the_explorer_item_reports_it() {
        let ctx = egui::Context::default();
        let (bar, _) = run(&ctx, Vec::new(), false);
        let (bar, _) = run(&ctx, click(bar.explorer_rect.center()), false);
        assert!(bar.clicked);
    }

    #[test]
    fn test_accent_bar_marks_an_open_explorer_only() {
        let ctx = egui::Context::default();
        let accent = SidebarPalette::for_visuals(&ctx.global_style().visuals).accent;
        let (bar, shapes) = run(&ctx, Vec::new(), true);
        assert!(painted_fill(&shapes, accent, bar.explorer_rect));
        let (bar, shapes) = run(&ctx, Vec::new(), false);
        assert!(!painted_fill(&shapes, accent, bar.explorer_rect));
    }
}
