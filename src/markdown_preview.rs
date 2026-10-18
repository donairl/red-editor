//! Rendered preview of Markdown documents, shown beside the editor.
//!
//! `app.rs` decides when the preview is visible (a tab whose language is
//! `Language::Markdown`); this module only knows how to draw one.

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

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

/// The preview pane's state: the viewer's layout cache, kept between frames.
#[derive(Default)]
pub struct MarkdownPreview {
    cache: CommonMarkCache,
}

impl MarkdownPreview {
    /// Draws `text` as rendered Markdown, or `too_large` when it exceeds
    /// [`MAX_PREVIEW_BYTES`]. `scroll_to` forces the vertical offset for this
    /// frame; the returned position is where the pane ended up.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        too_large: &str,
        scroll_to: Option<f32>,
    ) -> ScrollPos {
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(offset) = scroll_to {
            area = area.vertical_scroll_offset(offset);
        }
        let output = area.show(ui, |ui| {
            if exceeds_limit(text) {
                ui.weak(too_large);
            } else {
                CommonMarkViewer::new().show(ui, &mut self.cache, text);
            }
        });
        ScrollPos::of(&output)
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
            preview.show(ui, text, "too large", None);
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
                seen = preview.show(ui, &text, "too large", scroll_to);
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
            preview.show(ui, &text, "too large", None);
        });
        assert!(shapes > 0, "the notice is painted");
    }
}
