//! Thin vertical lines at each indentation level of the editor, as VS Code
//! draws them.
//!
//! The guides are read off the laid-out text rather than the source: a row
//! that starts a source line carries as many guides as its indentation is
//! worth. A wrapped continuation row starts mid-line and has no indentation
//! of its own, and a folded row has no height, so both are skipped.

use eframe::egui::{self, Color32, FontId, Galley, Rect, Shape};

use crate::theme;

/// How many columns one indentation step is: the width egui gives a tab, so
/// spaces and tabs line their guides up.
pub const TAB_COLUMNS: usize = 4;

/// The width of one space in `font_id`, in points.
pub fn space_width(ui: &egui::Ui, font_id: &FontId) -> f32 {
    ui.fonts_mut(|fonts| fonts.glyph_width(font_id, ' '))
}

/// The width of one indentation step: [`TAB_COLUMNS`] spaces.
pub fn step(ui: &egui::Ui, font_id: &FontId) -> f32 {
    space_width(ui, font_id) * TAB_COLUMNS as f32
}

/// How many guides a line gets, from the glyphs of its indentation: a space
/// is one column, a tab [`TAB_COLUMNS`].
///
/// Whole steps only: three spaces with a four-column step have not reached a
/// level yet, and get no guide. A line that starts with text gets none
/// either.
pub fn levels(chars: impl IntoIterator<Item = char>) -> usize {
    let mut columns = 0;
    for chr in chars {
        match chr {
            ' ' => columns += 1,
            '\t' => columns += TAB_COLUMNS,
            _ => break,
        }
    }
    columns / TAB_COLUMNS
}

/// The colour of the guides: the one the Explorer's tree indents use, so the
/// two agree in every theme.
pub fn color(ctx: &egui::Context) -> Color32 {
    theme::active(ctx).sidebar.indent_guide
}

/// The guide lines of `galley`, placed `origin` away (the galley's position
/// on screen), for the part of it the `clip` shows.
///
/// One line per level, at the left edge of each level's step; a whole point
/// wide, rounded to the pixel so it stays crisp.
pub fn shapes(
    galley: &Galley,
    origin: egui::Pos2,
    clip: Rect,
    step: f32,
    color: Color32,
) -> Vec<Shape> {
    let mut shapes = Vec::new();
    if !step.is_finite() || step <= 0.0 {
        return shapes;
    }
    for placed in &galley.rows {
        let rect = placed.rect().translate(origin.to_vec2());
        if rect.top() > clip.bottom() {
            break;
        }
        if rect.bottom() < clip.top() || rect.height() <= 0.0 {
            continue;
        }
        let levels = levels(placed.glyphs.iter().map(|glyph| glyph.chr));
        for level in 1..=levels {
            let x = (origin.x + level as f32 * step).round();
            shapes.push(Shape::rect_filled(
                Rect::from_x_y_ranges(x..=(x + 1.0), rect.y_range()),
                0.0,
                color,
            ));
        }
    }
    shapes
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
    fn test_only_whole_steps_are_levels() {
        assert_eq!(levels("abc".chars()), 0);
        assert_eq!(levels("   x".chars()), 0);
        assert_eq!(levels("    x".chars()), 1);
        assert_eq!(levels("        x".chars()), 2);
        assert_eq!(levels("\tx".chars()), 1);
        assert_eq!(levels("\t\tx".chars()), 2);
        // A tab is a whole step wherever it stands.
        assert_eq!(levels(" \tx".chars()), 1);
        assert_eq!(levels("".chars()), 0);
    }

    #[test]
    fn test_each_level_of_a_line_gets_a_guide() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        let mut space = 0.0;
        frame(&ctx, |ui| {
            let galley = layout(ui, "    a\n        b\nc");
            space = space_width(ui, &egui::TextStyle::Monospace.resolve(ui.style()));
            painted = shapes(
                &galley,
                egui::pos2(10.0, 20.0),
                whole_viewport(),
                space * TAB_COLUMNS as f32,
                egui::Color32::WHITE,
            );
        });
        // One guide for the four-space line, two for the eight-space one,
        // none for the line at the margin.
        assert_eq!(painted.len(), 3, "{painted:?}");
        let x = |shape: &Shape| match shape {
            Shape::Rect(rect) => rect.rect.min.x,
            _ => panic!("a guide is a thin rectangle: {shape:?}"),
        };
        let step = space * TAB_COLUMNS as f32;
        assert_eq!(x(&painted[0]), (10.0 + step).round());
        assert_eq!(x(&painted[1]), (10.0 + step).round());
        assert_eq!(x(&painted[2]), (10.0 + 2.0 * step).round());
    }

    #[test]
    fn test_a_line_starting_with_a_tab_has_a_guide() {
        let ctx = egui::Context::default();
        let mut count = 0;
        frame(&ctx, |ui| {
            let galley = layout(ui, "\ta\n\t\tb");
            let step = step(ui, &egui::TextStyle::Monospace.resolve(ui.style()));
            count = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                step,
                egui::Color32::WHITE,
            )
            .len();
        });
        assert_eq!(count, 3, "one guide, then two");
    }

    /// A row scrolled out of view is not painted: the clip decides.
    #[test]
    fn test_rows_outside_the_clip_are_skipped() {
        let ctx = egui::Context::default();
        let mut above = 0;
        let mut below = 0;
        frame(&ctx, |ui| {
            let galley = layout(ui, "    a\n        b");
            let step = step(ui, &egui::TextStyle::Monospace.resolve(ui.style()));
            let origin = egui::pos2(0.0, 100.0);
            above = shapes(
                &galley,
                origin,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 40.0)),
                step,
                egui::Color32::WHITE,
            )
            .len();
            below = shapes(
                &galley,
                origin,
                egui::Rect::from_min_max(egui::pos2(0.0, 400.0), egui::pos2(800.0, 500.0)),
                step,
                egui::Color32::WHITE,
            )
            .len();
        });
        assert_eq!(above, 0, "the rows sit below the viewport");
        assert_eq!(below, 0, "the rows sit above the viewport");
    }

    /// A guide stands at the left edge of its level's step, within the pixel
    /// rounding of the text it sits beside.
    #[test]
    fn test_a_guide_stands_at_its_step() {
        let ctx = egui::Context::default();
        let mut painted = Vec::new();
        let mut step_px = 0.0;
        frame(&ctx, |ui| {
            let galley = layout(ui, "    x");
            step_px = step(ui, &egui::TextStyle::Monospace.resolve(ui.style()));
            painted = shapes(
                &galley,
                egui::Pos2::ZERO,
                whole_viewport(),
                step_px,
                egui::Color32::WHITE,
            );
        });
        assert_eq!(painted.len(), 1, "{painted:?}");
        match &painted[0] {
            Shape::Rect(rect) => {
                assert!(
                    (rect.rect.min.x - step_px.round()).abs() <= 1.0,
                    "the guide is off its step: {} vs {step_px}",
                    rect.rect.min.x
                );
            }
            other => panic!("not a guide: {other:?}"),
        }
    }
}
