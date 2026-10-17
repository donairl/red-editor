//! VS Code-style Explorer: the folder tree in the sidebar.
//!
//! [`Explorer`] remembers which folders are open and caches what is in them.

use eframe::egui::{self, Color32};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
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

pub fn render_tree_node(ui: &mut egui::Ui, dir: &Path, on_open_file: &mut Option<PathBuf>) {
    let entries = scan_directory(dir);

    for entry in entries {
        if entry.is_dir {
            egui::CollapsingHeader::new(format!("📁 {}", entry.name))
                .id_salt(&entry.path)
                .show(ui, |ui| {
                    render_tree_node(ui, &entry.path, on_open_file);
                });
        } else {
            let file_button = ui.button(format!("📄 {}", entry.name));
            if file_button.clicked() {
                *on_open_file = Some(entry.path);
            }
        }
    }
}

pub fn show_file_browser(
    ui: &mut egui::Ui,
    current_folder: &Path,
    on_open_file: &mut Option<PathBuf>,
    on_close_sidebar: &mut bool,
) {
    ui.horizontal(|ui| {
        let folder_name = current_folder
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_else(|| current_folder.to_string_lossy());
        ui.heading(format!("📂 {}", folder_name));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✖").clicked() {
                *on_close_sidebar = true;
            }
        });
    });

    ui.separator();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_tree_node(ui, current_folder, on_open_file);
        });
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
}
