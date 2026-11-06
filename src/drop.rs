//! Files dragged onto the window: what a drop opens, and the hint painted
//! while the drag still hovers over it.
//!
//! The windowing backend reports a drop as one entry per file, all in the
//! frame the user let go. The app turns each entry back into a path and
//! decides by looking at the disk whether it is a folder to open as the
//! workspace or a file to open in a tab.

use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, StrokeKind};

use crate::i18n::I18n;

/// How opaque the scrim over the window is, out of 255: enough to make the
/// hint read as a target, not so much that the window is lost.
const SCRIM_ALPHA: u8 = 170;

/// The size of the hint box.
const HINT_SIZE: egui::Vec2 = egui::vec2(360.0, 112.0);

/// The paths dropped on the window in this frame, in the order they came.
pub fn paths(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    })
}

/// What a drop asks for: the folders to open as the workspace and the files
/// to open in tabs, each in the order it came.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Folders, each of which replaces the workspace.
    pub folders: Vec<PathBuf>,
    /// Files, each of which opens in a tab.
    pub files: Vec<PathBuf>,
}

impl Dropped {
    /// Sorts `paths` into the folders and the files among them.
    ///
    /// A path that is no longer on disk counts as a file: opening one says
    /// which path failed, while a folder that is not there would be skipped
    /// without a word.
    pub fn split(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut dropped = Self::default();
        for path in paths {
            if path.is_dir() {
                dropped.folders.push(path);
            } else {
                dropped.files.push(path);
            }
        }
        dropped
    }

    /// Whether the drop held nothing the app could use.
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.files.is_empty()
    }
}

/// Paints the hint over the whole window while files hover it, so the drop
/// is expected and its effect is no surprise.
///
/// The hint goes on the foreground layer, above every panel and dialog: it
/// only describes the drag and takes no input.
pub fn show_hint(ctx: &egui::Context, i18n: &I18n) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let visuals = ctx.global_style().visuals.clone();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("rred_drop_hint"),
    ));
    let screen = ctx.content_rect();
    // A scrim of black over a dark window, white over a light one: the
    // content under it dims without changing colour.
    let scrim = if visuals.dark_mode {
        Color32::from_black_alpha(SCRIM_ALPHA)
    } else {
        Color32::from_white_alpha(SCRIM_ALPHA)
    };
    painter.rect_filled(screen, CornerRadius::ZERO, scrim);
    let toast = egui::Rect::from_center_size(screen.center(), HINT_SIZE);
    painter.rect(
        toast,
        CornerRadius::same(12),
        visuals.window_fill,
        Stroke::new(2.0, visuals.selection.bg_fill),
        StrokeKind::Inside,
    );
    painter.text(
        toast.center(),
        egui::Align2::CENTER_CENTER,
        i18n.t("drop.hint"),
        FontId::proportional(18.0),
        visuals.strong_text_color(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::painted_text;

    /// A folder `rred_drop_<name>` in the temp directory, holding a file and
    /// a subfolder, so the two kinds can be told apart on disk.
    fn drop_folder(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rred_drop_{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        let file = root.join("a.txt");
        let sub = root.join("sub");
        (root, file, sub)
    }

    #[test]
    fn test_a_drop_is_split_into_its_folders_and_files() {
        let (root, file, sub) = drop_folder("split");
        let dropped = Dropped::split(vec![file.clone(), sub.clone(), file.clone()]);
        assert_eq!(dropped.folders, vec![sub]);
        assert_eq!(dropped.files, vec![file.clone(), file]);
        assert!(!dropped.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_a_path_that_is_gone_counts_as_a_file() {
        let gone = std::env::temp_dir().join("rred_drop_gone_never_there");
        let dropped = Dropped::split(vec![gone.clone()]);
        assert!(
            dropped.folders.is_empty(),
            "a missing folder opened silently"
        );
        assert_eq!(dropped.files, vec![gone]);
    }

    #[test]
    fn test_an_empty_drop_is_empty() {
        assert!(Dropped::split(Vec::new()).is_empty());
    }

    /// A dropped file the backend hands over as a path; nothing reads it.
    #[derive(Debug)]
    struct TestFile(PathBuf);

    impl egui::DroppedFile for TestFile {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("the tests never read a dropped file".to_owned())
        }
    }

    fn frame(ctx: &egui::Context, input: egui::RawInput) -> Vec<egui::epaint::ClippedShape> {
        let mut output = ctx.run_ui(input, |ui| {
            show_hint(ui.ctx(), &english());
        });
        output.textures_delta.clear();
        output.shapes
    }

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    fn english() -> I18n {
        let mut i18n = I18n::builtin();
        assert!(i18n.select("en"));
        i18n
    }

    #[test]
    fn test_the_dropped_paths_are_read_from_the_input() {
        let ctx = egui::Context::default();
        let path = PathBuf::from("/tmp/rred_drop_read_me.txt");
        let input = egui::RawInput {
            dropped_files: vec![std::sync::Arc::new(TestFile(path.clone()))],
            ..raw_input()
        };
        let mut read = Vec::new();
        ctx.run_ui(input, |_| read.extend(paths(&ctx)))
            .textures_delta
            .clear();
        assert_eq!(read, vec![path.clone()]);
        // A drop belongs to the frame it arrives in: the next one has none.
        let mut again = Vec::new();
        ctx.run_ui(raw_input(), |_| again.extend(paths(&ctx)))
            .textures_delta
            .clear();
        assert!(again.is_empty(), "the drop was read twice");
    }

    #[test]
    fn test_the_hint_appears_only_while_files_hover() {
        let ctx = egui::Context::default();
        let hint = english().t("drop.hint").to_owned();
        let hovering = egui::RawInput {
            hovered_files: vec![egui::HoveredFile {
                path: Some(PathBuf::from("/tmp/rred_drop_hint.txt")),
                ..Default::default()
            }],
            ..raw_input()
        };
        let shapes = frame(&ctx, hovering);
        assert!(
            painted_text(&shapes, &hint).is_some(),
            "no hint while a file hovers"
        );

        let shapes = frame(&ctx, raw_input());
        assert!(
            painted_text(&shapes, &hint).is_none(),
            "the hint stayed with nothing hovering"
        );
    }

    /// The hint names the scrim and the box over the window, so the drag is
    /// visible even over a busy editor.
    #[test]
    fn test_the_hint_dims_the_window_and_draws_a_box() {
        let ctx = egui::Context::default();
        let hovering = egui::RawInput {
            hovered_files: vec![egui::HoveredFile::default()],
            ..raw_input()
        };
        let shapes = frame(&ctx, hovering);
        let rects: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) => Some(rect.rect),
                _ => None,
            })
            .collect();
        assert!(
            rects.contains(&egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0)
            )),
            "no scrim over the window: {rects:?}"
        );
        assert!(
            rects.iter().any(|rect| rect.size() == HINT_SIZE),
            "no hint box: {rects:?}"
        );
    }
}
