//! Drawing the spaces and tabs of the text: a faint dot for every space and
//! an arrow for every tab, so trailing whitespace and mixed indentation show
//! without counting characters.
//!
//! The marks are read off the laid-out text, not the source, and only the
//! rows the viewport shows are painted.

use eframe::egui::{self, Color32, Galley, Rect, Shape, Stroke, pos2};

/// The radius of a space's dot, in points.
const DOT_RADIUS: f32 = 0.8;

/// The width of a tab's arrow, in points.
const ARROW_STROKE: f32 = 1.0;

/// How long the arrow's head is, in points.
const ARROW_HEAD: f32 = 3.0;

/// How far the arrow's head stands from its shaft, in points.
const ARROW_SPREAD: f32 = 2.5;

/// How faint the marks are against the text colour, out of one.
pub const FAINT: f32 = 0.4;

/// The marks of `galley`, placed `origin` away (the galley's position on
/// screen), for the part of it the `clip` shows.
///
/// A folded row has no height and is skipped, as are the rows above and
/// below the viewport.
pub fn shapes(galley: &Galley, origin: egui::Pos2, clip: Rect, color: Color32) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for placed in &galley.rows {
        let row = placed.rect().translate(origin.to_vec2());
        if row.top() > clip.bottom() {
            break;
        }
        if row.bottom() < clip.top() || row.height() <= 0.0 {
            continue;
        }
        for glyph in &placed.glyphs {
            if glyph.chr != ' ' && glyph.chr != '\t' {
                continue;
            }
            let rect = glyph
                .logical_rect()
                .translate(origin.to_vec2() + placed.pos.to_vec2());
            shapes.extend(marks(glyph.chr, rect, color));
        }
    }
    shapes
}

/// The marks for one whitespace `chr` filling `rect`: a dot for a space, an
/// arrow for a tab.
fn marks(chr: char, rect: Rect, color: Color32) -> Vec<Shape> {
    let center = rect.center();
    match chr {
        '\t' => {
            let stroke = Stroke::new(ARROW_STROKE, color);
            let (left, right) = (rect.left() + 1.0, rect.right() - 1.0);
            let (y, head) = (center.y, right - ARROW_HEAD);
            vec![
                Shape::line_segment([pos2(left, y), pos2(right, y)], stroke),
                Shape::line_segment([pos2(head, y - ARROW_SPREAD), pos2(right, y)], stroke),
                Shape::line_segment([pos2(head, y + ARROW_SPREAD), pos2(right, y)], stroke),
            ]
        }
        _ => vec![Shape::circle_filled(center, DOT_RADIUS, color)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame, discarding the font texture a real window would
    /// upload.
    fn frame(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) {
        let mut output = ctx.run_ui(egui::RawInput::default(), body);
        output.textures_delta.clear();
    }

    fn layout(ui: &egui::Ui, text: &str) -> std::sync::Arc<Galley> {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font_id, egui::Color32::WHITE))
    }

    fn whole_viewport() -> Rect {
        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))
    }

    #[test]
    fn test_every_space_gets_a_dot() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        frame(&ctx, |ui| {
            let galley = layout(ui, "a  b ");
            painted = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                egui::Color32::WHITE,
            );
        });
        // "a", two spaces, "b", one space: three dots.
        assert_eq!(painted.len(), 3, "{painted:?}");
        assert!(
            painted
                .iter()
                .all(|shape| matches!(shape, Shape::Circle(_)))
        );
    }

    #[test]
    fn test_a_tab_gets_an_arrow() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        frame(&ctx, |ui| {
            let galley = layout(ui, "a\tb");
            painted = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                egui::Color32::WHITE,
            );
        });
        // A shaft and two head strokes.
        assert_eq!(painted.len(), 3, "{painted:?}");
        assert!(
            painted
                .iter()
                .all(|shape| matches!(shape, Shape::LineSegment { .. }))
        );
    }

    #[test]
    fn test_text_without_whitespace_gets_no_marks() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        frame(&ctx, |ui| {
            let galley = layout(ui, "abcdef\nlet_x=1;");
            painted = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                egui::Color32::WHITE,
            );
        });
        assert!(painted.is_empty(), "{painted:?}");
    }

    /// A space scrolled out of view is not painted.
    #[test]
    fn test_rows_outside_the_clip_are_skipped() {
        let ctx = egui::Context::default();
        let mut count = 0;
        frame(&ctx, |ui| {
            let galley = layout(ui, "      \n      ");
            count = shapes(
                &galley,
                egui::pos2(0.0, 100.0),
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 40.0)),
                egui::Color32::WHITE,
            )
            .len();
        });
        assert_eq!(count, 0, "the rows sit below the viewport");
    }

    /// The marks sit on the glyphs they stand for: a space's dot is at the
    /// centre of the cell the glyph fills.
    #[test]
    fn test_a_dots_centre_sits_on_its_space() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        let mut space = None;
        frame(&ctx, |ui| {
            let galley = layout(ui, "a b");
            space = galley.rows[0]
                .glyphs
                .iter()
                .find(|glyph| glyph.chr == ' ')
                .map(|glyph| {
                    glyph
                        .logical_rect()
                        .translate(galley.rows[0].pos.to_vec2())
                        .center()
                });
            painted = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                egui::Color32::WHITE,
            );
        });
        let dot = match &painted[0] {
            Shape::Circle(circle) => circle.center,
            other => panic!("not a dot: {other:?}"),
        };
        let space = space.expect("the line has a space");
        assert!(
            (dot.x - space.x).abs() < 0.01 && (dot.y - space.y).abs() < 0.01,
            "{dot:?} {space:?}"
        );
    }
}
