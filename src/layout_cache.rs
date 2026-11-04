//! The editor's laid-out text, kept from one frame to the next.
//!
//! `TextEdit` asks for the layout every frame. egui remembers layouts, but
//! looking one up clones and hashes the whole job: for a highlighted 1 MB
//! file that alone took over 10 ms a frame, spent while nothing changed.
//! [`LayoutCache`] hands back the galley it made last time instead, as long
//! as nothing it was made from changed.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontId, Galley,
    text::{ByteIndex, LayoutJob, LayoutSection, TextFormat},
};

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

/// Font size of folded-away text. Small enough that its rows add no
/// width, and egui still shapes it.
const COLLAPSED_FONT_SIZE: f32 = 0.1;

/// The format of text hidden by a fold: laid out as rows of zero height,
/// invisible, so the row after it sits right below the fold's header.
fn collapsed(format: &TextFormat) -> TextFormat {
    TextFormat {
        font_id: FontId::new(COLLAPSED_FONT_SIZE, format.font_id.family.clone()),
        line_height: Some(0.0),
        color: Color32::TRANSPARENT,
        background: Color32::TRANSPARENT,
        ..Default::default()
    }
}

/// Gives the `char` ranges `hidden` of `job` (sorted, apart) the
/// [`collapsed`] format, splitting sections at their edges.
fn collapse(job: &mut LayoutJob, hidden: &[(usize, usize)]) {
    if hidden.is_empty() {
        return;
    }
    // The ranges' edges as byte offsets, in order.
    let mut edges = Vec::with_capacity(hidden.len() * 2);
    let mut wanted = hidden.iter().flat_map(|&(a, b)| [a, b]).peekable();
    for (chars, (byte, _)) in job.text.char_indices().enumerate() {
        while wanted.next_if(|&c| c <= chars).is_some() {
            edges.push(byte);
        }
        if wanted.peek().is_none() {
            break;
        }
    }
    edges.extend(wanted.map(|_| job.text.len()));
    let ranges: Vec<(usize, usize)> = edges.chunks(2).map(|e| (e[0], e[1])).collect();

    let mut sections = Vec::with_capacity(job.sections.len() + ranges.len() * 2);
    let mut push = |start: usize, end: usize, format: &TextFormat, leading_space: f32| {
        if start < end {
            sections.push(LayoutSection {
                leading_space,
                byte_range: ByteIndex(start)..ByteIndex(end),
                format: format.clone(),
            });
        }
    };
    let mut next = 0;
    for section in &job.sections {
        let (mut at, end) = (section.byte_range.start.0, section.byte_range.end.0);
        let mut leading_space = section.leading_space;
        while at < end {
            while next < ranges.len() && ranges[next].1 <= at {
                next += 1;
            }
            match ranges.get(next) {
                Some(&(a, b)) if a <= at => {
                    let stop = b.min(end);
                    push(at, stop, &collapsed(&section.format), 0.0);
                    at = stop;
                }
                Some(&(a, _)) if a < end => {
                    push(at, a, &section.format, leading_space);
                    at = a;
                }
                _ => {
                    push(at, end, &section.format, leading_space);
                    at = end;
                }
            }
            leading_space = 0.0;
        }
    }
    job.sections = sections;
}

impl Style {
    /// The job laying out `text` in this style, wrapped at `wrap_width`,
    /// with the `char` ranges `hidden` folded away.
    fn job(&self, text: &str, wrap_width: f32, hidden: &[(usize, usize)]) -> LayoutJob {
        let mut job = self.unfolded_job(text, wrap_width);
        collapse(&mut job, hidden);
        job
    }

    fn unfolded_job(&self, text: &str, wrap_width: f32) -> LayoutJob {
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
    /// The `char` ranges folded away.
    hidden: Vec<(usize, usize)>,
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
    /// for no wrapping), with the sorted `char` ranges `hidden` laid out at
    /// zero height (folded away).
    pub fn galley(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        style: Style,
        wrap_width: f32,
        hidden: &[(usize, usize)],
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
            && cached.hidden == hidden
            && cached.galley.text() == text
        {
            cached.pass = pass;
            cached.atlas_fill = atlas_fill;
            return Arc::clone(&cached.galley);
        }

        let galley = ui.fonts_mut(|f| f.layout_job(style.job(text, wrap_width, hidden)));
        #[cfg(test)]
        {
            self.layouts += 1;
        }
        self.last = Some(Cached {
            galley: Arc::clone(&galley),
            style,
            wrap_width,
            hidden: hidden.to_vec(),
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
                out.push(cache.galley(ui, text, style.clone(), wrap_width, &[]))
            });
        }
        out
    }

    /// The folding spike, kept as a guard against egui upgrades: text in
    /// the collapsed format lays out as rows of zero height that add no
    /// width, so the line after it sits right below the header's rows.
    #[test]
    fn test_collapsed_text_lays_out_at_zero_height() {
        use egui::text::CCursor;
        let ctx = egui::Context::default();
        let visible = TextFormat {
            font_id: FontId::monospace(14.0),
            color: Color32::WHITE,
            line_height: Some(18.0),
            ..Default::default()
        };
        let header = "fn main() {";
        let body: String = (0..10)
            .map(|i| format!("\n    let x{i} = {i} + some + long + words;"))
            .collect();
        let tail = "\n}\nafter";
        let (header_len, body_len) = (header.chars().count(), body.chars().count());
        for wrap in [NO_WRAP, 60.0] {
            let mut job = LayoutJob::default();
            job.wrap.max_width = wrap;
            for part in [header, body.as_str(), tail] {
                job.append(part, 0.0, visible.clone());
            }
            collapse(&mut job, &[(header_len, header_len + body_len)]);
            let mut reference = LayoutJob::default();
            reference.wrap.max_width = wrap;
            reference.append(header, 0.0, visible.clone());
            reference.append(tail, 0.0, visible.clone());
            let mut galleys = None;
            pass(&ctx, |ui| {
                galleys = Some(ui.fonts_mut(|f| {
                    (f.layout_job(job.clone()), f.layout_job(reference.clone()))
                }))
            });
            let (galley, reference) = galleys.unwrap();
            let close = galley.pos_from_cursor(CCursor::new(header_len + body_len + 1));
            let expected = reference.pos_from_cursor(CCursor::new(header_len + 1));
            assert!((close.top() - expected.top()).abs() < 1.0, "wrap {wrap}: {close:?} vs {expected:?}");
            assert!((galley.rect.height() - reference.rect.height()).abs() < 1.0);
            assert!((galley.rect.width() - reference.rect.width()).abs() < 1.0);
            // Every cursor in the hidden run still maps somewhere.
            for i in 0..=galley.text().chars().count() {
                galley.pos_from_cursor(CCursor::new(i));
            }
        }
    }

    #[test]
    fn test_collapse_splits_sections_at_the_range_edges() {
        let mut job = LayoutJob::default();
        let format = TextFormat::simple(FontId::monospace(14.0), Color32::RED);
        let blue = TextFormat::simple(FontId::monospace(14.0), Color32::BLUE);
        job.append("ab", 3.0, blue);
        job.append("écd", 0.0, format.clone());
        // Chars 1..3 are "bé": the end falls inside the second section.
        collapse(&mut job, &[(1, 3)]);
        let ranges: Vec<_> = job.sections.iter().map(|s| s.byte_range.start.0..s.byte_range.end.0).collect();
        assert_eq!(ranges, vec![0..1, 1..2, 2..4, 4..6]);
        let hidden: Vec<_> = job.sections.iter().map(|s| s.format.line_height == Some(0.0)).collect();
        assert_eq!(hidden, vec![false, true, true, false]);
        assert_eq!(job.sections[0].leading_space, 3.0);
        assert_eq!(job.sections[3].format, format);
    }

    #[test]
    fn test_a_folded_document_is_shorter_by_the_hidden_rows() {
        let ctx = egui::Context::default();
        let mut cache = LayoutCache::default();
        let text = "a {\n  b\n  c\n  d\n}\n";
        let open = galleys(&ctx, &mut cache, text, &plain(), NO_WRAP, 1);
        let mut folded = None;
        // "a {" ends at char 3, "  d" at char 15.
        pass(&ctx, |ui| folded = Some(cache.galley(ui, text, plain(), NO_WRAP, &[(3, 15)])));
        let folded = folded.unwrap();
        assert_eq!(cache.layouts, 2, "folding lays out again");
        let shorter = open[0].rect.height() - folded.rect.height();
        assert!((shorter - 3.0 * 18.0).abs() < 1.0, "shorter by {shorter}");
        // Kept while the folds stay the same.
        pass(&ctx, |ui| {
            cache.galley(ui, text, plain(), NO_WRAP, &[(3, 15)]);
        });
        assert_eq!(cache.layouts, 2);
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
                cache.galley(ui, "text", plain(), NO_WRAP, &[]);
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
