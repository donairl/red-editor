//! VS Code-style Explorer: the folder tree in the sidebar.
//!
//! [`Explorer`] remembers which folders are open and caches what is in them;
//! [`show`] paints the sidebar and applies the clicks it receives.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::highlighter::Language;
use crate::tab_bar;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub name: String,
}

pub fn scan_directory(dir: &Path) -> Vec<FsEntry> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = match path.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            // Ignore hidden files / directories
            if name.starts_with('.') {
                continue;
            }

            let is_dir = path.is_dir();
            entries.push(FsEntry { path, is_dir, name });
        }
    }

    // Sort: directories first, then alphabetical by name (case-insensitive)
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    entries
}

/// Colours of the Activity Bar and the sidebar, after VS Code's
/// "Dark Modern" and "Light Modern".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarPalette {
    pub background: Color32,
    pub border: Color32,
    pub icon_active: Color32,
    pub icon_inactive: Color32,
    /// Marks the Activity Bar item whose view is open.
    pub accent: Color32,
    pub foreground: Color32,
    pub row_hover: Color32,
    /// The row of the file in the active tab.
    pub row_selected: Color32,
    pub indent_guide: Color32,
}

impl SidebarPalette {
    pub const DARK: SidebarPalette = SidebarPalette {
        background: Color32::from_rgb(0x18, 0x18, 0x18),
        border: Color32::from_rgb(0x2B, 0x2B, 0x2B),
        icon_active: Color32::from_rgb(0xD7, 0xD7, 0xD7),
        icon_inactive: Color32::from_rgb(0x86, 0x86, 0x86),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        foreground: Color32::from_rgb(0xCC, 0xCC, 0xCC),
        row_hover: Color32::from_rgb(0x2A, 0x2D, 0x2E),
        row_selected: Color32::from_rgb(0x37, 0x37, 0x3D),
        indent_guide: Color32::from_rgb(0x40, 0x40, 0x40),
    };

    pub const LIGHT: SidebarPalette = SidebarPalette {
        background: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        icon_active: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        icon_inactive: Color32::from_rgb(0x61, 0x61, 0x61),
        accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
        foreground: Color32::from_rgb(0x3B, 0x3B, 0x3B),
        row_hover: Color32::from_rgb(0xF2, 0xF2, 0xF2),
        row_selected: Color32::from_rgb(0xE4, 0xE6, 0xF1),
        indent_guide: Color32::from_rgb(0xD3, 0xD3, 0xD3),
    };

    /// The palette matching egui's current light or dark mode.
    pub fn for_visuals(visuals: &egui::Visuals) -> SidebarPalette {
        if visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// One visible line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub entry: FsEntry,
    /// 0 for the entries directly inside the root folder.
    pub depth: usize,
    /// Only ever `true` for folders.
    pub expanded: bool,
}

/// The tree under one root folder.
pub struct Explorer {
    root: PathBuf,
    /// The `▾ NAME` section that holds the whole tree.
    section_open: bool,
    expanded: HashSet<PathBuf>,
    /// What each folder held when it was last read from disk.
    children: HashMap<PathBuf, Vec<FsEntry>>,
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            section_open: true,
            expanded: HashSet::new(),
            children: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_section_open(&self) -> bool {
        self.section_open
    }

    /// Expands a collapsed folder, or collapses an expanded one.
    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
    }

    pub fn toggle_section(&mut self) {
        self.section_open = !self.section_open;
    }

    /// Collapses every folder; the section stays open.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Forgets what every folder held, so the next [`Self::rows`] reads them
    /// from disk again. Expanded folders stay expanded.
    pub fn refresh(&mut self) {
        self.children.clear();
    }

    /// The visible tree in display order, reading any folder not yet cached.
    pub fn rows(&mut self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        if self.section_open {
            let root = self.root.clone();
            self.push_rows(&root, 0, &mut rows);
        }
        rows
    }

    fn push_rows(&mut self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        let entries = self
            .children
            .entry(dir.to_path_buf())
            .or_insert_with(|| scan_directory(dir))
            .clone();
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let path = entry.path.clone();
            rows.push(TreeRow {
                entry,
                depth,
                expanded,
            });
            if expanded {
                self.push_rows(&path, depth + 1, rows);
            }
        }
    }
}

/// Height of the `EXPLORER` title row, level with the tab strip.
const TITLE_HEIGHT: f32 = tab_bar::TAB_HEIGHT;
const TITLE_PADDING_X: f32 = 20.0;
/// Height of the section header and of every tree row.
const ROW_HEIGHT: f32 = 22.0;
const ROW_PADDING_X: f32 = 8.0;
/// Extra left offset per depth level.
const INDENT: f32 = 8.0;
const CHEVRON_WIDTH: f32 = 16.0;
const BADGE_GAP: f32 = 6.0;
/// Side of the Refresh and Collapse All buttons.
const ACTION_SIZE: f32 = 22.0;

/// One frame of the sidebar: the file to open, and where each part was drawn.
#[derive(Clone, Debug)]
pub struct ExplorerResponse {
    pub open_file: Option<PathBuf>,
    // Only the tests read where things were drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub section_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub refresh_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub collapse_all_rect: Rect,
    #[cfg_attr(not(test), allow(dead_code))]
    pub rows: Vec<(PathBuf, Rect)>,
}

/// Draws the sidebar over the available width and applies folder, header,
/// Refresh and Collapse All clicks to `explorer`. A clicked file is returned
/// in [`ExplorerResponse::open_file`] for the caller to open.
pub fn show(
    ui: &mut egui::Ui,
    explorer: &mut Explorer,
    active_file: Option<&Path>,
) -> ExplorerResponse {
    let palette = SidebarPalette::for_visuals(ui.visuals());
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let width = ui.available_width();

    // Right border, over the full height of the panel.
    let panel = ui.max_rect();
    ui.painter().vline(
        panel.right() - 0.5,
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );

    let (title_rect, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(title_rect.left() + TITLE_PADDING_X, title_rect.center().y),
        Align2::LEFT_CENTER,
        "EXPLORER",
        FontId::proportional(11.0),
        palette.foreground,
    );

    let (section_rect, section) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    let collapse_all_rect = Rect::from_min_size(
        pos2(section_rect.right() - ACTION_SIZE, section_rect.top()),
        Vec2::splat(ACTION_SIZE),
    );
    let refresh_rect = collapse_all_rect.translate(vec2(-ACTION_SIZE, 0.0));
    // Registered after the header, so they win clicks inside their rects.
    let refresh = ui
        .interact(refresh_rect, section.id.with("refresh"), Sense::click())
        .on_hover_text("Refresh");
    let collapse_all = ui
        .interact(
            collapse_all_rect,
            section.id.with("collapse_all"),
            Sense::click(),
        )
        .on_hover_text("Collapse All");
    paint_section_header(
        ui,
        &palette,
        explorer,
        section_rect,
        section.hovered() || refresh.hovered() || collapse_all.hovered(),
        [
            (refresh_rect, refresh.hovered()),
            (collapse_all_rect, collapse_all.hovered()),
        ],
    );
    if refresh.clicked() {
        explorer.refresh();
    } else if collapse_all.clicked() {
        explorer.collapse_all();
    } else if section.clicked() {
        explorer.toggle_section();
    }

    let rows = explorer.rows();
    let mut response = ExplorerResponse {
        open_file: None,
        section_rect,
        refresh_rect,
        collapse_all_rect,
        rows: Vec::with_capacity(rows.len()),
    };
    let mut toggled = None;
    egui::ScrollArea::vertical()
        .id_salt("explorer_tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for row in &rows {
                let (rect, row_response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if ui.is_rect_visible(rect) {
                    let selected = active_file == Some(row.entry.path.as_path());
                    paint_row(ui, &palette, row, rect, row_response.hovered(), selected);
                }
                if row_response.clicked() {
                    if row.entry.is_dir {
                        toggled = Some(row.entry.path.clone());
                    } else {
                        response.open_file = Some(row.entry.path.clone());
                    }
                }
                response.rows.push((row.entry.path.clone(), rect));
            }
        });
    if let Some(dir) = toggled {
        explorer.toggle(&dir);
    }
    response
}

/// `▾ NAME`, plus the Refresh and Collapse All buttons while `hovered`.
fn paint_section_header(
    ui: &egui::Ui,
    palette: &SidebarPalette,
    explorer: &Explorer,
    rect: Rect,
    hovered: bool,
    actions: [(Rect, bool); 2],
) {
    let painter = ui.painter();
    let chevron = Rect::from_min_size(
        pos2(rect.left() + ROW_PADDING_X, rect.top()),
        vec2(CHEVRON_WIDTH, ROW_HEIGHT),
    );
    paint_chevron(
        painter,
        chevron.center(),
        explorer.is_section_open(),
        palette.foreground,
    );

    let name = explorer
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| explorer.root().display().to_string());
    let right = if hovered {
        actions[0].0.left()
    } else {
        rect.right()
    };
    let galley = elided(
        ui,
        name,
        FontId::proportional(11.0),
        palette.foreground,
        right - chevron.right() - ROW_PADDING_X,
    );
    let pos = pos2(chevron.right(), rect.center().y - galley.size().y / 2.0);
    // egui bundles no bold face: a second pass half a point to the right
    // thickens the strokes instead.
    painter.galley(pos, galley.clone(), palette.foreground);
    painter.galley(pos + vec2(0.5, 0.0), galley, palette.foreground);

    if hovered {
        let [
            (refresh, refresh_hovered),
            (collapse_all, collapse_all_hovered),
        ] = actions;
        for (action, action_hovered) in [
            (refresh, refresh_hovered),
            (collapse_all, collapse_all_hovered),
        ] {
            if action_hovered {
                painter.rect_filled(action.shrink(2.0), CornerRadius::same(4), palette.row_hover);
            }
        }
        paint_refresh_icon(painter, refresh.center(), palette.foreground);
        paint_collapse_all_icon(painter, collapse_all.center(), palette.foreground);
    }
}

/// Background, indent guides, chevron or badge, and the name of one row.
fn paint_row(
    ui: &egui::Ui,
    palette: &SidebarPalette,
    row: &TreeRow,
    rect: Rect,
    hovered: bool,
    selected: bool,
) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_selected);
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
    }

    // One guide per ancestor, through the middle of its chevron column.
    let guide = Stroke::new(1.0, palette.indent_guide);
    for level in 0..row.depth {
        let x = rect.left() + ROW_PADDING_X + level as f32 * INDENT + CHEVRON_WIDTH / 2.0;
        painter.vline(x.round() + 0.5, rect.y_range(), guide);
    }

    let chevron = Rect::from_min_size(
        pos2(
            rect.left() + ROW_PADDING_X + row.depth as f32 * INDENT,
            rect.top(),
        ),
        vec2(CHEVRON_WIDTH, ROW_HEIGHT),
    );
    let mut x = chevron.right();
    if row.entry.is_dir {
        paint_chevron(painter, chevron.center(), row.expanded, palette.foreground);
    } else {
        let (text, color) = tab_bar::badge(Language::from_path(&row.entry.path));
        let badge = painter.layout_no_wrap(
            text.to_owned(),
            FontId::monospace(10.0),
            color.unwrap_or(palette.icon_inactive),
        );
        let badge_width = badge.size().x;
        painter.galley(
            pos2(x, rect.center().y - badge.size().y / 2.0),
            badge,
            palette.foreground,
        );
        x += badge_width + BADGE_GAP;
    }

    let name = elided(
        ui,
        row.entry.name.clone(),
        FontId::proportional(13.0),
        palette.foreground,
        rect.right() - ROW_PADDING_X - x,
    );
    painter.galley(
        pos2(x, rect.center().y - name.size().y / 2.0),
        name,
        palette.foreground,
    );
}

/// `text` on one line, cut with `…` where it would pass `max_width`.
fn elided(
    ui: &egui::Ui,
    text: String,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font_id, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// `›` pointing right when collapsed, down when expanded. Drawn as lines,
/// so it never depends on the bundled fonts.
fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, expanded: bool, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let points = if expanded {
        [vec2(-4.0, -2.0), vec2(0.0, 2.0), vec2(4.0, -2.0)]
    } else {
        [vec2(-2.0, -4.0), vec2(2.0, 0.0), vec2(-2.0, 4.0)]
    };
    painter.line_segment([center + points[0], center + points[1]], stroke);
    painter.line_segment([center + points[1], center + points[2]], stroke);
}

/// A circular arrow, open at the top right.
fn paint_refresh_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    use std::f32::consts::PI;
    let stroke = Stroke::new(1.2, color);
    let radius = 5.0;
    // From the top (-90°) clockwise round to the right (≈ -20°).
    let start = -PI / 2.0;
    let end = start + 2.0 * PI * 0.8;
    let points: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let angle = start + (end - start) * i as f32 / 24.0;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect();
    let tip = points[0];
    painter.line(points, stroke);
    // Arrow head at the start of the arc, pointing back into the gap.
    painter.line_segment([tip, tip + vec2(3.0, -3.0)], stroke);
    painter.line_segment([tip, tip + vec2(3.0, 3.0)], stroke);
}

/// A box with a minus, in front of a second box: VS Code's "collapse all".
fn paint_collapse_all_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let front = Rect::from_center_size(center + vec2(1.0, 1.0), Vec2::splat(9.0));
    painter.rect_stroke(front, CornerRadius::ZERO, stroke, StrokeKind::Middle);
    painter.line_segment(
        [
            pos2(front.left() + 2.5, front.center().y),
            pos2(front.right() - 2.5, front.center().y),
        ],
        stroke,
    );
    // Top and left edges of the box behind.
    let back = front.translate(vec2(-2.5, -2.5));
    painter.line_segment([back.left_bottom(), back.left_top()], stroke);
    painter.line_segment([back.left_top(), back.right_top()], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn test_scan_directory_sorting_and_hidden_files() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_dir");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("sub_folder")).unwrap();
        fs::create_dir_all(temp_dir.join(".hidden_folder")).unwrap();
        File::create(temp_dir.join("a_file.txt")).unwrap();
        File::create(temp_dir.join("b_file.txt")).unwrap();
        File::create(temp_dir.join(".hidden_file.txt")).unwrap();

        let entries = scan_directory(&temp_dir);
        assert!(!entries.is_empty());
        // Directory must come first
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "sub_folder");
        // Followed by sorted files, hidden files/dirs skipped
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].name, "a_file.txt");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[2].name, "b_file.txt");
        assert!(!entries[2].is_dir);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_scan_directory_case_insensitive_sorting() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_case");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        File::create(temp_dir.join("b_file.txt")).unwrap();
        File::create(temp_dir.join("A_file.txt")).unwrap();
        File::create(temp_dir.join("c_file.txt")).unwrap();

        let entries = scan_directory(&temp_dir);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "A_file.txt");
        assert_eq!(entries[1].name, "b_file.txt");
        assert_eq!(entries[2].name, "c_file.txt");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_palettes_match_vscode_modern_themes() {
        let d = SidebarPalette::DARK;
        assert_eq!(d.background, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(d.border, Color32::from_rgb(0x2B, 0x2B, 0x2B));
        assert_eq!(d.icon_active, Color32::from_rgb(0xD7, 0xD7, 0xD7));
        assert_eq!(d.icon_inactive, Color32::from_rgb(0x86, 0x86, 0x86));
        assert_eq!(d.accent, Color32::from_rgb(0x00, 0x78, 0xD4));
        assert_eq!(d.foreground, Color32::from_rgb(0xCC, 0xCC, 0xCC));
        assert_eq!(d.row_hover, Color32::from_rgb(0x2A, 0x2D, 0x2E));
        assert_eq!(d.row_selected, Color32::from_rgb(0x37, 0x37, 0x3D));
        assert_eq!(d.indent_guide, Color32::from_rgb(0x40, 0x40, 0x40));

        let l = SidebarPalette::LIGHT;
        assert_eq!(l.background, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(l.border, Color32::from_rgb(0xE5, 0xE5, 0xE5));
        assert_eq!(l.icon_active, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(l.icon_inactive, Color32::from_rgb(0x61, 0x61, 0x61));
        assert_eq!(l.accent, Color32::from_rgb(0x00, 0x5F, 0xB8));
        assert_eq!(l.foreground, Color32::from_rgb(0x3B, 0x3B, 0x3B));
        assert_eq!(l.row_hover, Color32::from_rgb(0xF2, 0xF2, 0xF2));
        assert_eq!(l.row_selected, Color32::from_rgb(0xE4, 0xE6, 0xF1));
        assert_eq!(l.indent_guide, Color32::from_rgb(0xD3, 0xD3, 0xD3));

        assert_eq!(
            SidebarPalette::for_visuals(&egui::Visuals::dark()),
            SidebarPalette::DARK
        );
        assert_eq!(
            SidebarPalette::for_visuals(&egui::Visuals::light()),
            SidebarPalette::LIGHT
        );
    }

    /// A fresh folder named `rred_explorer_<name>` in the temp directory:
    ///
    /// ```text
    /// docs/guide.md
    /// src/main.rs
    /// src/nested/deep.rs
    /// Cargo.toml
    /// .hidden
    /// ```
    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rred_explorer_{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        File::create(root.join("docs/guide.md")).unwrap();
        File::create(root.join("src/main.rs")).unwrap();
        File::create(root.join("src/nested/deep.rs")).unwrap();
        File::create(root.join("Cargo.toml")).unwrap();
        File::create(root.join(".hidden")).unwrap();
        root
    }

    /// `(name, depth)` of every visible row.
    fn outline(explorer: &mut Explorer) -> Vec<(String, usize)> {
        explorer
            .rows()
            .into_iter()
            .map(|row| (row.entry.name, row.depth))
            .collect()
    }

    fn owned(rows: &[(&str, usize)]) -> Vec<(String, usize)> {
        rows.iter().map(|(n, d)| (n.to_string(), *d)).collect()
    }

    #[test]
    fn test_rows_list_root_entries_folders_first() {
        let root = fixture("root_rows");
        let mut explorer = Explorer::new(root.clone());
        let rows = explorer.rows();
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        assert!(rows.iter().all(|row| !row.expanded));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_toggle_expands_and_collapses_folders() {
        let root = fixture("toggle");
        let mut explorer = Explorer::new(root.clone());

        explorer.toggle(&root.join("src"));
        let rows = explorer.rows();
        assert!(rows[1].expanded, "src is expanded");
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );

        explorer.toggle(&root.join("src/nested"));
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("deep.rs", 2),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );

        explorer.toggle(&root.join("src"));
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_collapse_all_leaves_root_entries() {
        let root = fixture("collapse_all");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.toggle(&root.join("src/nested"));
        explorer.toggle(&root.join("docs"));

        explorer.collapse_all();
        assert!(explorer.is_section_open(), "the section stays open");
        assert_eq!(
            outline(&mut explorer),
            owned(&[("docs", 0), ("src", 0), ("Cargo.toml", 0)])
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_folders_are_cached_until_refresh() {
        let root = fixture("refresh");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.rows();

        File::create(root.join("src/added.rs")).unwrap();
        let names = |explorer: &mut Explorer| -> Vec<String> {
            explorer
                .rows()
                .into_iter()
                .map(|row| row.entry.name)
                .collect()
        };
        assert!(
            !names(&mut explorer).contains(&"added.rs".to_string()),
            "a cached folder is not read again"
        );

        explorer.refresh();
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("src", 0),
                ("nested", 1),
                ("added.rs", 1),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ]),
            "refresh reads folders again and keeps src expanded"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_closed_section_has_no_rows() {
        let root = fixture("section");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle_section();
        assert!(!explorer.is_section_open());
        assert!(explorer.rows().is_empty());
        explorer.toggle_section();
        assert_eq!(explorer.rows().len(), 3);
        let _ = fs::remove_dir_all(&root);
    }

    /// One headless frame of the sidebar in a 240 × 600 window; returns its
    /// response and the painted shapes.
    fn run(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        explorer: &mut Explorer,
        active_file: Option<&Path>,
    ) -> (ExplorerResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(240.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| {
            response = Some(show(ui, explorer, active_file));
        });
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

    fn row_rect(response: &ExplorerResponse, path: &Path) -> Rect {
        response
            .rows
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("no row for {path:?}"))
    }

    /// Rects painted with `fill`.
    fn filled(shapes: &[egui::epaint::ClippedShape], fill: Color32) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_clicking_a_folder_row_expands_it() {
        let root = fixture("click_folder");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        let src = row_rect(&response, &root.join("src"));

        let (response, _) = run(&ctx, click(src.center()), &mut explorer, None);
        assert_eq!(response.open_file, None, "a folder is not opened as a file");
        assert!(explorer.rows()[1].expanded, "clicking src expands it");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_clicking_a_file_row_opens_it() {
        let root = fixture("click_file");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let file = root.join("Cargo.toml");
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);

        let (response, _) = run(
            &ctx,
            click(row_rect(&response, &file).center()),
            &mut explorer,
            None,
        );
        assert_eq!(response.open_file, Some(file));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_active_file_row_is_selected() {
        let root = fixture("selected");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let active = root.join("Cargo.toml");
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, Some(&active));

        let palette = SidebarPalette::for_visuals(&ctx.global_style().visuals);
        assert_eq!(
            filled(&shapes, palette.row_selected),
            vec![row_rect(&response, &active)],
            "only the active file's row is selected"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_section_header_buttons() {
        let root = fixture("header_buttons");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        let (response, _) = run(&ctx, Vec::new(), &mut explorer, None);
        File::create(root.join("added.txt")).unwrap();

        run(
            &ctx,
            click(response.refresh_rect.center()),
            &mut explorer,
            None,
        );
        let names: Vec<String> = explorer.rows().into_iter().map(|r| r.entry.name).collect();
        assert!(
            names.contains(&"added.txt".to_string()),
            "Refresh reads the folder again"
        );
        assert!(
            names.contains(&"main.rs".to_string()),
            "Refresh keeps src expanded"
        );

        run(
            &ctx,
            click(response.collapse_all_rect.center()),
            &mut explorer,
            None,
        );
        assert!(
            explorer.is_section_open(),
            "the buttons do not toggle the section"
        );
        assert!(
            explorer.rows().iter().all(|row| row.depth == 0),
            "Collapse All"
        );

        run(
            &ctx,
            click(response.section_rect.left_center() + vec2(30.0, 0.0)),
            &mut explorer,
            None,
        );
        assert!(
            !explorer.is_section_open(),
            "clicking the header closes the section"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_long_names_are_cut_to_the_sidebar_width() {
        let root = fixture("long_name");
        let long = "a_file_name_far_too_long_to_fit_in_a_narrow_sidebar.txt";
        File::create(root.join(long)).unwrap();
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        let (response, shapes) = run(&ctx, Vec::new(), &mut explorer, None);
        let row = row_rect(&response, &root.join(long));

        let name = shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == long => Some(text),
                _ => None,
            })
            .expect("the long name is painted");
        assert!(name.galley.elided, "the name is cut with …");
        assert!(
            name.pos.x + name.galley.size().x <= row.right(),
            "the name stays inside its row"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
