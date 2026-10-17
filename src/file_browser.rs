//! File browser tree navigation and state.

use eframe::egui;
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
}
