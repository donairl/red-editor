//! Rendered preview of Markdown documents, shown beside the editor.
//!
//! `app.rs` decides when the preview is visible (a tab whose language is
//! `Language::Markdown`); this module only knows how to draw one, and how to
//! move around in it: an outline of the headings, `#anchor` links, and
//! stepping from one heading to the next.

use std::collections::HashMap;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::app::shortcuts;
use crate::i18n::I18n;

/// Documents past this size are not rendered. The viewer is immediate-mode, so
/// its cost grows with the document and is paid every frame.
pub const MAX_PREVIEW_BYTES: usize = 512 * 1024;

/// True when `text` is too large to render every frame.
pub fn exceeds_limit(text: &str) -> bool {
    text.len() > MAX_PREVIEW_BYTES
}

/// Where a vertical scroll area sits: how far down, and how far it can go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPos {
    pub offset: f32,
    pub max: f32,
}

impl ScrollPos {
    /// Builds the position from a finished scroll area.
    pub fn of<R>(output: &egui::scroll_area::ScrollAreaOutput<R>) -> Self {
        Self {
            offset: output.state.clamped_offset().y,
            max: (output.content_size.y - output.inner_rect.height()).max(0.0),
        }
    }

    /// How far down as `0.0..=1.0`; `0.0` when there is nothing to scroll.
    pub fn fraction(self) -> f32 {
        if self.max <= 0.0 {
            0.0
        } else {
            (self.offset / self.max).clamp(0.0, 1.0)
        }
    }
}

/// Keeps the editor and the preview scrolled to the same relative place.
///
/// Rendered Markdown and its source differ in height, so the two are matched
/// by how far down each is, not by pixels. Whichever pane the user moved this
/// frame drives the other one on the next frame. A pane that was just moved by
/// this sync is not treated as the user scrolling it, so the two never echo.
#[derive(Default)]
pub struct ScrollSync {
    /// Offsets of (editor, preview) seen after the previous frame.
    last: Option<(f32, f32)>,
    force_editor: Option<f32>,
    force_preview: Option<f32>,
    applied_editor: bool,
    applied_preview: bool,
}

impl ScrollSync {
    /// Offset to force on the editor this frame, if the preview was scrolled.
    pub fn take_editor_offset(&mut self) -> Option<f32> {
        let offset = self.force_editor.take();
        self.applied_editor = offset.is_some();
        offset
    }

    /// Offset to force on the preview this frame, if the editor was scrolled.
    pub fn take_preview_offset(&mut self) -> Option<f32> {
        let offset = self.force_preview.take();
        self.applied_preview = offset.is_some();
        offset
    }

    /// Records where both panes ended up and decides who drives next frame.
    pub fn observe(&mut self, editor: ScrollPos, preview: ScrollPos) {
        const MOVED: f32 = 0.5;
        match self.last {
            // The preview has just opened: line it up with the editor.
            None => self.force_preview = Some(editor.fraction() * preview.max),
            Some((last_editor, last_preview)) => {
                let editor_moved =
                    !self.applied_editor && (editor.offset - last_editor).abs() > MOVED;
                let preview_moved =
                    !self.applied_preview && (preview.offset - last_preview).abs() > MOVED;
                if editor_moved && !preview_moved {
                    self.force_preview = Some(editor.fraction() * preview.max);
                } else if preview_moved && !editor_moved {
                    self.force_editor = Some(preview.fraction() * editor.max);
                }
            }
        }
        self.last = Some((editor.offset, preview.offset));
        self.applied_editor = false;
        self.applied_preview = false;
    }

    /// Forgets everything; call while the preview is hidden.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A heading of the document, as listed in the preview's outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// 1 for `#`, up to 6.
    pub level: u8,
    pub title: String,
    /// The link that leads to it, derived from the title as GitHub does:
    /// `## Getting Started` is `#getting-started`.
    pub anchor: String,
    /// 1-based source line the heading is on.
    pub line: usize,
    /// Byte offset where the heading starts.
    start: usize,
}

/// The parser options `egui_commonmark` renders with, so both see the same
/// headings.
fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

/// GitHub's anchor text for a heading title: lowercase, spaces as hyphens,
/// punctuation other than `-` and `_` dropped.
fn slug(title: &str) -> String {
    title
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// The document cut into sections at its top-level headings. Each section
/// is rendered on its own, which tells the preview where every heading lands.
#[derive(Default)]
struct Outline {
    /// The text this outline was built from.
    source: String,
    headings: Vec<Heading>,
    /// What comes before the first heading (empty if only blank), then one
    /// section per heading, each running up to the next heading.
    sections: Vec<String>,
}

impl Outline {
    fn build(text: &str) -> Self {
        let mut parser = Parser::new_ext(text, parser_options()).into_offset_iter();
        let mut headings = Vec::new();
        let mut taken: HashMap<String, usize> = HashMap::new();
        let mut open: Option<Heading> = None;
        let mut depth = 0usize;
        let (mut line, mut counted) = (1, 0);
        for (event, range) in &mut parser {
            match event {
                // Only top-level headings: one inside a list or a quote cannot
                // start a section of its own.
                Event::Start(Tag::Heading { level, .. }) if depth == 0 => {
                    line += text[counted..range.start].matches('\n').count();
                    counted = range.start;
                    open = Some(Heading {
                        level: level as u8,
                        title: String::new(),
                        anchor: String::new(),
                        line,
                        start: range.start,
                    });
                    depth += 1;
                }
                Event::Start(_) => depth += 1,
                Event::End(end) => {
                    depth -= 1;
                    if depth == 0
                        && matches!(end, TagEnd::Heading(_))
                        && let Some(mut heading) = open.take()
                    {
                        let base = slug(&heading.title);
                        let seen = taken.entry(base.clone()).or_insert(0);
                        heading.anchor = match *seen {
                            0 => format!("#{base}"),
                            n => format!("#{base}-{n}"),
                        };
                        *seen += 1;
                        headings.push(heading);
                    }
                }
                Event::Text(part) | Event::Code(part) => {
                    if let Some(heading) = &mut open {
                        heading.title.push_str(&part);
                    }
                }
                _ => {}
            }
        }

        // `[text][ref]` may sit in another section than its `[ref]: url`, so
        // every section gets all the definitions; they render as nothing.
        let references = parser
            .reference_definitions()
            .iter()
            .map(|(_, def)| &text[def.span.clone()])
            .collect::<Vec<_>>()
            .join("\n\n");
        let starts: Vec<usize> = std::iter::once(0)
            .chain(headings.iter().map(|heading| heading.start))
            .collect();
        let sections = starts
            .iter()
            .enumerate()
            .map(|(i, &start)| {
                let end = starts.get(i + 1).copied().unwrap_or(text.len());
                let body = &text[start..end];
                if body.trim().is_empty() {
                    return String::new();
                }
                let mut section = body.to_owned();
                if !references.is_empty() {
                    section.push_str("\n\n");
                    section.push_str(&references);
                }
                section
            })
            .collect();
        Self {
            source: text.to_owned(),
            headings,
            sections,
        }
    }
}

/// Offsets this close count as the same place.
const SAME_PLACE: f32 = 1.0;

/// The heading being read at `offset`: the last one at or above the top.
fn heading_at(ys: &[f32], offset: f32) -> Option<usize> {
    ys.iter().rposition(|&y| y <= offset + SAME_PLACE)
}

/// The heading after `pos` (`forward`) or before it. Nothing comes after the
/// end of the document, even a heading too near the end to reach the top.
fn neighbour(ys: &[f32], pos: ScrollPos, forward: bool) -> Option<usize> {
    if forward {
        if pos.offset >= pos.max - SAME_PLACE {
            return None;
        }
        ys.iter().position(|&y| y > pos.offset + SAME_PLACE)
    } else {
        ys.iter().rposition(|&y| y < pos.offset - SAME_PLACE)
    }
}

/// The hooked link clicked while the viewer last drew, if any.
fn take_clicked_hook(cache: &mut CommonMarkCache) -> Option<String> {
    let (link, clicked) = cache
        .link_hooks_mut()
        .iter_mut()
        .find(|(_, clicked)| **clicked)?;
    *clicked = false;
    Some(link.clone())
}

/// What a frame of the preview did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreviewOutput {
    /// Where the pane ended up.
    pub pos: ScrollPos,
    /// The source line of a heading the pane just jumped to, for the editor
    /// to follow.
    pub jumped_to_line: Option<usize>,
}

/// The preview pane's state, kept between frames.
#[derive(Default)]
pub struct MarkdownPreview {
    /// The viewer's layout cache.
    cache: CommonMarkCache,
    outline: Outline,
    /// Content offset of each heading, as laid out by the last frame.
    heading_ys: Vec<f32>,
    /// Where the pane was after the last frame.
    last_pos: ScrollPos,
    /// The heading to jump to on the next frame.
    pending: Option<usize>,
    /// The heading last jumped to and the offset that landed on. A heading
    /// near the end cannot reach the top, so the offset alone would name an
    /// earlier one.
    landed: Option<(usize, f32)>,
}

impl MarkdownPreview {
    /// Draws `text` as rendered Markdown, under a bar to move between its
    /// headings, or a notice when it exceeds [`MAX_PREVIEW_BYTES`].
    /// `scroll_to` forces the vertical offset for this frame, unless the pane
    /// jumps to a heading.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        i18n: &I18n,
        scroll_to: Option<f32>,
    ) -> PreviewOutput {
        let too_large = exceeds_limit(text);
        if too_large {
            self.pending = None;
        } else if self.outline.source != text {
            self.rebuild(text);
        }
        if !too_large && !self.outline.headings.is_empty() {
            self.nav_bar(ui, i18n);
            ui.separator();
        }

        let jump = self.pending.take().filter(|&i| i < self.heading_ys.len());
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(offset) = jump.map(|i| self.heading_ys[i]).or(scroll_to) {
            area = area.vertical_scroll_offset(offset);
        }
        let Self {
            cache,
            outline,
            heading_ys,
            ..
        } = self;
        let mut clicked = None;
        let output = area.show(ui, |ui| {
            if too_large {
                ui.weak(i18n.t("preview.too_large"));
                return;
            }
            heading_ys.clear();
            let origin = ui.cursor().min.y;
            for (i, section) in outline.sections.iter().enumerate() {
                if i > 0 {
                    heading_ys.push(ui.cursor().min.y - origin);
                }
                if !section.is_empty() {
                    ui.push_id(i, |ui| CommonMarkViewer::new().show(ui, cache, section));
                    // Each viewer resets every hook before it draws, so read
                    // them before the next section does.
                    if clicked.is_none() {
                        clicked = take_clicked_hook(cache);
                    }
                }
            }
        });
        let pos = ScrollPos::of(&output);
        self.last_pos = pos;
        if let Some(i) = jump {
            self.landed = Some((i, pos.offset));
        }
        // A clicked `#anchor` link is only known once drawn: jump next frame.
        let target = clicked.and_then(|anchor| {
            let headings = &self.outline.headings;
            headings.iter().position(|heading| heading.anchor == anchor)
        });
        if target.is_some() {
            self.pending = target;
            ui.ctx().request_repaint();
        }
        PreviewOutput {
            pos,
            jumped_to_line: jump.map(|i| self.outline.headings[i].line),
        }
    }

    /// Steps to the next heading (`forward`) or the previous one, if any.
    pub fn step(&mut self, forward: bool) {
        if let Some(i) = neighbour(&self.heading_ys, self.last_pos, forward) {
            self.pending = Some(i);
        }
    }

    /// The heading being read: the one just jumped to while the pane stays
    /// there, else the last one scrolled past.
    fn current(&self) -> Option<usize> {
        match self.landed {
            Some((i, offset)) if (offset - self.last_pos.offset).abs() < SAME_PLACE => Some(i),
            _ => heading_at(&self.heading_ys, self.last_pos.offset),
        }
    }

    fn rebuild(&mut self, text: &str) {
        self.outline = Outline::build(text);
        self.heading_ys.clear();
        self.pending = None;
        self.landed = None;
        // Hooked links are reported to us instead of being opened.
        self.cache.link_hooks_clear();
        for heading in &self.outline.headings {
            self.cache.add_link_hook(heading.anchor.clone());
        }
    }

    /// Previous / next buttons and the outline of every heading.
    fn nav_bar(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let current = self.current();
        let prev = neighbour(&self.heading_ys, self.last_pos, false);
        let next = neighbour(&self.heading_ys, self.last_pos, true);
        let headings = &self.outline.headings;
        let mut pick = None;
        ui.horizontal(|ui| {
            let hint =
                |key, shortcut| format!("{} ({})", i18n.t(key), ui.ctx().format_shortcut(shortcut));
            let prev_hint = hint("preview.prev_heading", &shortcuts::PREV_HEADING);
            let next_hint = hint("preview.next_heading", &shortcuts::NEXT_HEADING);
            if ui
                .add_enabled(prev.is_some(), egui::Button::new("\u{23f6}"))
                .on_hover_text(prev_hint)
                .clicked()
            {
                pick = prev;
            }
            if ui
                .add_enabled(next.is_some(), egui::Button::new("\u{23f7}"))
                .on_hover_text(next_hint)
                .clicked()
            {
                pick = next;
            }
            let selected = current.map_or(i18n.t("preview.outline"), |i| &headings[i].title);
            let top_level = headings.iter().map(|h| h.level).min().unwrap_or(1);
            egui::ComboBox::from_id_salt("preview_outline")
                .selected_text(selected)
                .width(ui.available_width())
                .height(ui.ctx().content_rect().height() * 0.6)
                .show_ui(ui, |ui| {
                    for (i, heading) in headings.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(f32::from(heading.level - top_level) * 12.0);
                            if ui
                                .selectable_label(current == Some(i), &heading.title)
                                .clicked()
                            {
                                pick = Some(i);
                            }
                        });
                    }
                });
        });
        if pick.is_some() {
            self.pending = pick;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exceeds_limit_is_inclusive_of_the_limit() {
        assert!(!exceeds_limit(&"a".repeat(MAX_PREVIEW_BYTES)));
        assert!(exceeds_limit(&"a".repeat(MAX_PREVIEW_BYTES + 1)));
    }

    fn pos(offset: f32, max: f32) -> ScrollPos {
        ScrollPos { offset, max }
    }

    #[test]
    fn test_scroll_pos_fraction() {
        assert_eq!(pos(50.0, 200.0).fraction(), 0.25);
        assert_eq!(pos(0.0, 0.0).fraction(), 0.0, "nothing to scroll");
        assert_eq!(pos(300.0, 200.0).fraction(), 1.0, "clamped");
    }

    #[test]
    fn test_sync_aligns_the_preview_when_it_opens() {
        let mut sync = ScrollSync::default();
        sync.observe(pos(100.0, 400.0), pos(0.0, 1000.0));
        assert_eq!(sync.take_preview_offset(), Some(250.0));
        assert_eq!(sync.take_editor_offset(), None);
    }

    #[test]
    fn test_sync_editor_scroll_drives_the_preview() {
        let mut sync = ScrollSync::default();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));
        let _ = sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));

        sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(200.0, 400.0), pos(0.0, 1000.0));
        assert_eq!(sync.take_preview_offset(), Some(500.0));
        assert_eq!(sync.take_editor_offset(), None);
    }

    #[test]
    fn test_sync_preview_scroll_drives_the_editor() {
        let mut sync = ScrollSync::default();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));
        sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));

        sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(0.0, 400.0), pos(750.0, 1000.0));
        assert_eq!(sync.take_editor_offset(), Some(300.0));
        assert_eq!(sync.take_preview_offset(), None);
    }

    /// The pane the sync just moved must not be read as the user scrolling it,
    /// or the two would drive each other forever.
    #[test]
    fn test_sync_does_not_echo_its_own_move() {
        let mut sync = ScrollSync::default();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));
        sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));

        sync.take_preview_offset();
        sync.take_editor_offset();
        sync.observe(pos(200.0, 400.0), pos(0.0, 1000.0));
        // Next frame the preview obeys and lands at 500.
        assert_eq!(sync.take_preview_offset(), Some(500.0));
        sync.take_editor_offset();
        sync.observe(pos(200.0, 400.0), pos(500.0, 1000.0));
        assert_eq!(sync.take_editor_offset(), None);
        assert_eq!(sync.take_preview_offset(), None);
    }

    #[test]
    fn test_sync_reset_forgets_history() {
        let mut sync = ScrollSync::default();
        sync.observe(pos(200.0, 400.0), pos(0.0, 1000.0));
        sync.reset();
        assert_eq!(sync.take_preview_offset(), None);
        sync.observe(pos(0.0, 400.0), pos(0.0, 1000.0));
        assert_eq!(sync.take_preview_offset(), Some(0.0), "reopens aligned");
    }

    fn run_frame(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(600.0, 400.0),
            )),
            ..Default::default()
        };
        let mut body = body;
        let mut output = ctx.run_ui(input, |ui| body(ui));
        // egui panics if a frame's texture updates are dropped unapplied.
        output.textures_delta.clear();
        output.shapes.len()
    }

    #[test]
    fn test_show_renders_markdown_without_panicking() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = "# Title\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n";
        let shapes = run_frame(&ctx, |ui| {
            preview.show(ui, text, &I18n::builtin(), None);
        });
        assert!(shapes > 0, "a rendered document paints something");
    }

    /// A forced offset really moves the pane, and is reported back.
    #[test]
    fn test_show_applies_a_forced_scroll_offset() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = "line\n\n".repeat(400);
        let mut seen = ScrollPos::default();
        for scroll_to in [None, Some(120.0), None] {
            run_frame(&ctx, |ui| {
                seen = preview.show(ui, &text, &I18n::builtin(), scroll_to).pos;
            });
        }
        assert!(seen.max > 120.0, "the document must be taller than the view");
        assert_eq!(seen.offset, 120.0);
    }

    #[test]
    fn test_show_replaces_an_oversized_document_with_the_notice() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = "word ".repeat(MAX_PREVIEW_BYTES / 4);
        assert!(exceeds_limit(&text));
        let shapes = run_frame(&ctx, |ui| {
            preview.show(ui, &text, &I18n::builtin(), None);
        });
        assert!(shapes > 0, "the notice is painted");
    }

    #[test]
    fn test_slug_follows_github() {
        assert_eq!(slug("Getting Started"), "getting-started");
        assert_eq!(slug("What's new? (v2)"), "whats-new-v2");
        assert_eq!(slug("  Ünïcode_Title-x "), "ünïcode_title-x");
        assert_eq!(slug("a & b"), "a--b");
    }

    fn titles(outline: &Outline) -> Vec<(u8, &str, &str, usize)> {
        outline
            .headings
            .iter()
            .map(|h| (h.level, h.title.as_str(), h.anchor.as_str(), h.line))
            .collect()
    }

    #[test]
    fn test_outline_lists_top_level_headings() {
        let text = "intro\n\n# One\ntext\n\n## Two `code` *em*\n\nSetext\n------\n";
        let outline = Outline::build(text);
        assert_eq!(
            titles(&outline),
            [
                (1, "One", "#one", 3),
                (2, "Two code em", "#two-code-em", 6),
                (2, "Setext", "#setext", 8),
            ]
        );
    }

    #[test]
    fn test_outline_skips_headings_in_code_lists_and_quotes() {
        let text = "```\n# not a heading\n```\n\n- # in a list\n\n> # in a quote\n\n# Real\n";
        let outline = Outline::build(text);
        assert_eq!(titles(&outline), [(1, "Real", "#real", 9)]);
    }

    #[test]
    fn test_outline_numbers_repeated_anchors() {
        let outline = Outline::build("# A\n\n# A\n\n# A\n");
        let anchors: Vec<_> = outline.headings.iter().map(|h| h.anchor.as_str()).collect();
        assert_eq!(anchors, ["#a", "#a-1", "#a-2"]);
    }

    #[test]
    fn test_outline_sections_split_at_headings() {
        let text = "intro\n\n# One\nbody\n# Two\nend\n";
        let outline = Outline::build(text);
        assert_eq!(
            outline.sections,
            ["intro\n\n", "# One\nbody\n", "# Two\nend\n"]
        );
        assert_eq!(outline.sections.concat(), text);
    }

    #[test]
    fn test_outline_blank_preamble_is_empty() {
        let outline = Outline::build("\n\n# One\n");
        assert_eq!(outline.sections, ["", "# One\n"]);
    }

    #[test]
    fn test_outline_gives_every_section_the_link_definitions() {
        let text = "# One\nsee [docs][d]\n\n# Two\n\n[d]: https://example.com\n";
        let outline = Outline::build(text);
        for section in &outline.sections[1..] {
            assert!(section.ends_with("[d]: https://example.com"), "{section:?}");
        }
    }

    #[test]
    fn test_heading_at_names_the_last_heading_above_the_top() {
        let ys = [0.0, 100.0, 300.0];
        assert_eq!(heading_at(&ys, 0.0), Some(0));
        assert_eq!(heading_at(&ys, 150.0), Some(1));
        assert_eq!(heading_at(&ys, 300.0), Some(2));
        assert_eq!(
            heading_at(&[50.0], 0.0),
            None,
            "still above the first heading"
        );
    }

    #[test]
    fn test_neighbour_steps_between_headings() {
        let ys = [0.0, 100.0, 300.0];
        assert_eq!(neighbour(&ys, pos(100.0, 1000.0), true), Some(2));
        assert_eq!(neighbour(&ys, pos(100.0, 1000.0), false), Some(0));
        assert_eq!(neighbour(&ys, pos(150.0, 1000.0), false), Some(1));
        assert_eq!(neighbour(&ys, pos(0.0, 1000.0), false), None);
        assert_eq!(neighbour(&ys, pos(300.0, 1000.0), true), None);
        assert_eq!(neighbour(&ys, pos(250.0, 250.0), true), None, "at the end");
    }

    fn long_document() -> String {
        (1..=5)
            .map(|n| format!("# Part {n}\n\n{}", "line\n\n".repeat(60)))
            .collect()
    }

    #[test]
    fn test_show_lays_out_every_heading_in_order() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = long_document();
        run_frame(&ctx, |ui| {
            preview.show(ui, &text, &I18n::builtin(), None);
        });
        let ys = &preview.heading_ys;
        assert_eq!(ys.len(), 5);
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
    }

    #[test]
    fn test_step_jumps_to_the_next_heading_and_reports_its_line() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = long_document();
        let mut out = PreviewOutput::default();
        run_frame(&ctx, |ui| {
            preview.show(ui, &text, &I18n::builtin(), None);
        });
        preview.step(true); // Part 1 sits at the top: the next one is Part 2.
        run_frame(&ctx, |ui| {
            out = preview.show(ui, &text, &I18n::builtin(), None);
        });
        assert_eq!(out.pos.offset, preview.heading_ys[1]);
        assert_eq!(out.jumped_to_line, Some(preview.outline.headings[1].line));
        assert_eq!(preview.current(), Some(1));

        preview.step(false);
        run_frame(&ctx, |ui| {
            out = preview.show(ui, &text, &I18n::builtin(), None);
        });
        assert_eq!(out.pos.offset, 0.0);
        assert_eq!(preview.current(), Some(0));
    }

    /// Runs a frame with `events` and returns where `needle` was painted.
    fn frame_finding(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        needle: &str,
        body: impl FnMut(&mut egui::Ui),
    ) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text().contains(needle) => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, needle)),
                _ => None,
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(600.0, 400.0),
            )),
            events,
            ..Default::default()
        };
        let mut body = body;
        let mut output = ctx.run_ui(input, |ui| body(ui));
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, needle))
    }

    /// The link sits in the first section: later sections must not wipe the
    /// click before it is read.
    #[test]
    fn test_clicked_anchor_link_jumps_to_its_heading() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = format!("[to four](#part-4)\n\n{}", long_document());
        let mut out = PreviewOutput::default();
        let mut show = |events| {
            frame_finding(&ctx, events, "to four", |ui| {
                out = preview.show(ui, &text, &I18n::builtin(), None);
            })
        };
        let link = show(vec![]).expect("the link is painted").center();
        let button = |pressed| egui::Event::PointerButton {
            pos: link,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        show(vec![egui::Event::PointerMoved(link)]);
        show(vec![button(true)]);
        show(vec![button(false)]);
        show(vec![]);
        assert_eq!(out.pos.offset, preview.heading_ys[3]);
        assert_eq!(out.jumped_to_line, Some(preview.outline.headings[3].line));
        assert_eq!(preview.current(), Some(3));
    }

    #[test]
    fn test_editing_the_text_rebuilds_the_outline() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        run_frame(&ctx, |ui| {
            preview.show(ui, "# A\n", &I18n::builtin(), None);
        });
        run_frame(&ctx, |ui| {
            preview.show(ui, "# A\n\n# B\n", &I18n::builtin(), None);
        });
        assert_eq!(preview.outline.headings.len(), 2);
        assert_eq!(preview.heading_ys.len(), 2);
        assert_eq!(preview.cache.get_link_hook("#b"), Some(false));
    }
}
