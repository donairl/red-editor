//! Rendered preview of Markdown documents, shown beside the editor.
//!
//! `app.rs` decides when the preview is visible; this module only knows which
//! files are Markdown and how to draw one.

use std::path::Path;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

/// Documents past this size are not rendered. The viewer is immediate-mode, so
/// its cost grows with the document and is paid every frame.
pub const MAX_PREVIEW_BYTES: usize = 512 * 1024;

/// True when `path` names a Markdown file. Matching is case-insensitive.
pub fn is_markdown(path: Option<&Path>) -> bool {
    path.and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mdown", "mkd"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// True when `text` is too large to render every frame.
pub fn exceeds_limit(text: &str) -> bool {
    text.len() > MAX_PREVIEW_BYTES
}

/// The preview pane's state: the viewer's layout cache, kept between frames.
#[derive(Default)]
pub struct MarkdownPreview {
    cache: CommonMarkCache,
}

impl MarkdownPreview {
    /// Draws `text` as rendered Markdown, or `too_large` when it exceeds
    /// [`MAX_PREVIEW_BYTES`].
    pub fn show(&mut self, ui: &mut egui::Ui, text: &str, too_large: &str) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if exceeds_limit(text) {
                    ui.weak(too_large);
                } else {
                    CommonMarkViewer::new().show(ui, &mut self.cache, text);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_is_markdown_recognises_markdown_extensions() {
        for name in ["a.md", "a.markdown", "a.mdown", "a.mkd", "README.MD", "x.Markdown"] {
            assert!(is_markdown(Some(&PathBuf::from(name))), "{name}");
        }
    }

    #[test]
    fn test_is_markdown_rejects_other_files() {
        for name in ["a.rs", "a.txt", "md", "a.md.bak", "a."] {
            assert!(!is_markdown(Some(&PathBuf::from(name))), "{name}");
        }
        assert!(!is_markdown(None));
    }

    #[test]
    fn test_exceeds_limit_is_inclusive_of_the_limit() {
        assert!(!exceeds_limit(&"a".repeat(MAX_PREVIEW_BYTES)));
        assert!(exceeds_limit(&"a".repeat(MAX_PREVIEW_BYTES + 1)));
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
        let shapes = run_frame(&ctx, |ui| preview.show(ui, text, "too large"));
        assert!(shapes > 0, "a rendered document paints something");
    }

    #[test]
    fn test_show_replaces_an_oversized_document_with_the_notice() {
        let ctx = egui::Context::default();
        let mut preview = MarkdownPreview::default();
        let text = "word ".repeat(MAX_PREVIEW_BYTES / 4);
        assert!(exceeds_limit(&text));
        let shapes = run_frame(&ctx, |ui| preview.show(ui, &text, "too large"));
        assert!(shapes > 0, "the notice is painted");
    }
}
