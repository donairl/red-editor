//! The editor's laid-out text, kept from one frame to the next.
//!
//! `TextEdit` asks for the layout every frame. egui remembers layouts, but
//! looking one up clones and hashes the whole job: for a highlighted 1 MB
//! file that alone took over 10 ms a frame, spent while nothing changed.
//! [`LayoutCache`] hands back the galley it made last time instead, as long
//! as nothing it was made from changed.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Galley, text::LayoutJob};

/// What the text is laid out with, besides the text itself.
#[derive(Clone, Debug)]
pub enum Style {
    /// The highlighter's job for the text. Compared by identity: the
    /// highlighter hands out the same job until the text, language, font or
    /// theme changes.
    Highlighted(Arc<LayoutJob>),
    /// One font and colour throughout, as egui's stock `TextEdit` lays out
    /// multiline text.
    Plain {
        font_id: FontId,
        color: Color32,
        line_height: f32,
    },
}

impl Style {
    /// The job laying out `text` in this style, wrapped at `wrap_width`.
    fn job(&self, text: &str, wrap_width: f32) -> LayoutJob {
        match self {
            Style::Highlighted(job) => {
                let mut job = (**job).clone();
                job.wrap.max_width = wrap_width;
                job
            }
            Style::Plain {
                font_id,
                color,
                line_height,
            } => {
                let mut job =
                    LayoutJob::simple(text.to_owned(), font_id.clone(), *color, wrap_width);
                // Typing a trailing space must not make the caret jump.
                job.keep_trailing_whitespace = true;
                for section in &mut job.sections {
                    section.format.line_height = Some(*line_height);
                }
                job
            }
        }
    }
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Style::Highlighted(a), Style::Highlighted(b)) => Arc::ptr_eq(a, b),
            (
                Style::Plain {
                    font_id,
                    color,
                    line_height,
                },
                Style::Plain {
                    font_id: other_font,
                    color: other_color,
                    line_height: other_height,
                },
            ) => font_id == other_font && color == other_color && line_height == other_height,
            _ => false,
        }
    }
}

/// The last galley one editor laid out, and what it was made from.
#[derive(Default, Clone)]
pub struct LayoutCache {
    last: Option<Cached>,
    /// How many times a job went to egui to be laid out.
    #[cfg(test)]
    layouts: usize,
}

/// A cache never takes part in comparing the tab that owns it.
impl PartialEq for LayoutCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("filled", &self.last.is_some())
            .finish()
    }
}

#[derive(Clone)]
struct Cached {
    galley: Arc<Galley>,
    style: Style,
    wrap_width: f32,
    pixels_per_point: f32,
    /// How full egui's font atlas was when last looked at. egui empties the
    /// atlas when it fills up, leaving the galley pointing at glyphs that
    /// are gone; in between, it only fills up further.
    atlas_fill: f32,
    /// The pass the galley was last handed out in. In a pass without it
    /// the atlas could have been emptied and filled up again unseen.
    pass: u64,
}

impl LayoutCache {
    /// The galley of `text` in `style`, wrapped at `wrap_width` (infinite
    /// for no wrapping).
    pub fn galley(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        style: Style,
        wrap_width: f32,
    ) -> Arc<Galley> {
        let ctx = ui.ctx();
        let pass = ctx.cumulative_pass_nr();
        let pixels_per_point = ctx.pixels_per_point();
        let atlas_fill = ctx.fonts(|f| f.font_atlas_fill_ratio());
        // egui rounds the wrap width the same way before laying out.
        let wrap_width = if wrap_width.is_finite() {
            wrap_width.round()
        } else {
            wrap_width
        };

        if let Some(cached) = &mut self.last
            && cached.pass + 1 >= pass
            && cached.atlas_fill <= atlas_fill
            && cached.pixels_per_point == pixels_per_point
            && cached.wrap_width == wrap_width
            && cached.style == style
            && cached.galley.text() == text
        {
            cached.pass = pass;
            cached.atlas_fill = atlas_fill;
            return Arc::clone(&cached.galley);
        }

        let galley = ui.fonts_mut(|f| f.layout_job(style.job(text, wrap_width)));
        #[cfg(test)]
        {
            self.layouts += 1;
        }
        self.last = Some(Cached {
            galley: Arc::clone(&galley),
            style,
            wrap_width,
            pixels_per_point,
            // Laying out may have added glyphs.
            atlas_fill: ctx.fonts(|f| f.font_atlas_fill_ratio()),
            pass,
        });
        galley
    }

    /// The galley handed out last.
    #[cfg(test)]
    pub fn last_galley(&self) -> Option<&Arc<Galley>> {
        self.last.as_ref().map(|cached| &cached.galley)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_WRAP: f32 = f32::INFINITY;

    fn pass(ctx: &egui::Context, mut body: impl FnMut(&egui::Ui)) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| body(ui));
        output.textures_delta.clear();
    }

    fn plain() -> Style {
        Style::Plain {
            font_id: FontId::monospace(14.0),
            color: Color32::WHITE,
            line_height: 18.0,
        }
    }

    /// The galley `cache` hands out in each of `passes` passes, laying out
    /// `text` in `style` at `wrap_width` every time.
    fn galleys(
        ctx: &egui::Context,
        cache: &mut LayoutCache,
        text: &str,
        style: &Style,
        wrap_width: f32,
        passes: usize,
    ) -> Vec<Arc<Galley>> {
        let mut out = Vec::new();
        for _ in 0..passes {
            pass(ctx, |ui| {
                out.push(cache.galley(ui, text, style.clone(), wrap_width))
            });
        }
        out
    }

    #[test]
    fn test_reuses_the_galley_while_nothing_changes() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let galleys = galleys(&ctx, &mut cache, "fn main() {}\n", &plain(), NO_WRAP, 3);
        assert!(Arc::ptr_eq(&galleys[0], &galleys[2]));
        assert_eq!(cache.layouts, 1);
    }

    #[test]
    fn test_lays_out_again_when_the_text_changes() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let before = galleys(&ctx, &mut cache, "one", &plain(), NO_WRAP, 1);
        let after = galleys(&ctx, &mut cache, "two", &plain(), NO_WRAP, 1);
        assert_eq!(after[0].text(), "two");
        assert!(!Arc::ptr_eq(&before[0], &after[0]));
        assert_eq!(cache.layouts, 2);
    }

    #[test]
    fn test_lays_out_again_when_the_style_changes() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let text = "let x = 1;";
        galleys(&ctx, &mut cache, text, &plain(), NO_WRAP, 1);
        let red = Style::Plain {
            font_id: FontId::monospace(14.0),
            color: Color32::RED,
            line_height: 18.0,
        };
        galleys(&ctx, &mut cache, text, &red, NO_WRAP, 1);
        assert_eq!(cache.layouts, 2);

        // A highlighted job is told apart by identity, not by contents.
        let job = |color| {
            Arc::new(LayoutJob::simple(
                text.to_owned(),
                FontId::monospace(14.0),
                color,
                NO_WRAP,
            ))
        };
        let highlighted = Style::Highlighted(job(Color32::WHITE));
        galleys(&ctx, &mut cache, text, &highlighted, NO_WRAP, 2);
        assert_eq!(cache.layouts, 3);
        galleys(
            &ctx,
            &mut cache,
            text,
            &Style::Highlighted(job(Color32::WHITE)),
            NO_WRAP,
            1,
        );
        assert_eq!(cache.layouts, 4);
    }

    #[test]
    fn test_lays_out_again_when_the_wrap_width_changes() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let text = "a line long enough to wrap somewhere";
        galleys(&ctx, &mut cache, text, &plain(), 300.0, 1);
        galleys(&ctx, &mut cache, text, &plain(), 400.0, 1);
        assert_eq!(cache.layouts, 2);
        // egui rounds wrap widths to whole points, and so does the cache.
        galleys(&ctx, &mut cache, text, &plain(), 400.2, 1);
        assert_eq!(cache.layouts, 2);
    }

    #[test]
    fn test_lays_out_again_when_the_display_scale_changes() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        galleys(&ctx, &mut cache, "text", &plain(), NO_WRAP, 1);
        ctx.set_pixels_per_point(2.0);
        let galleys = galleys(&ctx, &mut cache, "text", &plain(), NO_WRAP, 1);
        assert_eq!(cache.layouts, 2);
        assert_eq!(galleys[0].pixels_per_point, 2.0);
    }

    /// While the editor is not shown, egui may empty its font atlas and fill
    /// it again unseen: the galley would then point at the wrong glyphs.
    #[test]
    fn test_lays_out_again_after_a_pass_without_it() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        galleys(&ctx, &mut cache, "text", &plain(), NO_WRAP, 1);
        pass(&ctx, |_| {});
        galleys(&ctx, &mut cache, "text", &plain(), NO_WRAP, 1);
        assert_eq!(cache.layouts, 2);
    }

    /// egui empties a nearly full font atlas at the start of a pass, and
    /// with it the glyphs a kept galley points at.
    #[test]
    fn test_lays_out_again_after_the_font_atlas_is_emptied() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let fill = || ctx.fonts(|f| f.font_atlas_fill_ratio());
        let mut last_fill = 0.0;
        let mut size = 40.0;
        for _ in 0..500 {
            let mut emptied = false;
            let layouts = cache.layouts;
            pass(&ctx, |ui| {
                emptied = fill() < last_fill;
                cache.galley(ui, "text", plain(), NO_WRAP);
                // Big glyphs in ever new sizes fill the atlas quickly.
                ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        "ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_owned(),
                        FontId::proportional(size),
                        Color32::WHITE,
                    )
                });
                last_fill = fill();
            });
            size += 7.0;
            if emptied {
                assert_eq!(cache.layouts, layouts + 1, "laid out again once emptied");
                return;
            }
            if layouts > 0 {
                assert_eq!(cache.layouts, layouts, "kept while the atlas only grows");
            }
        }
        panic!("the atlas never filled up");
    }
}
