# File Browser & Menus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform `rred_text_editor` into a full-featured desktop text editor with hierarchical sidebar file browsing, native OS file dialogs (Open, Open Folder, Save, Save As, Exit), Help: About modal, and unsaved changes confirmation prompts.

**Architecture:** Refactor the codebase from a single monolithic file into modular components: `src/modals.rs` (About & Unsaved confirmation dialogs), `src/file_browser.rs` (recursive folder scanner and UI tree view), `src/app.rs` (main `TextEditorApp` state, dirty detection, menus, and layout), and `src/main.rs` (entry point). Use `rfd` for native cross-platform file and folder picker dialogs.

**Tech Stack:** Rust (edition 2024), `eframe` 0.37, `egui` 0.37, `rfd` 0.17.

## Global Constraints

- Dependency `rfd` version `0.17` added to `Cargo.toml`.
- All operations must be non-blocking to prevent UI crashes; file reading and saving must handle I/O and UTF-8 decode errors gracefully with status messages.
- Must preserve monospace editing layout in central panel.
- Must include unit tests for dirty tracking, path formatting, and directory entry sorting.

---

### Task 1: Add Dependencies & Project Architecture Setup

**Files:**
- Modify: `Cargo.toml`
- Create: `src/modals.rs`
- Create: `src/file_browser.rs`
- Create: `src/app.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces:
  - `Cargo.toml` with `rfd = "0.17"`
  - Module exports in `src/main.rs` or `lib.rs` (`mod modals; mod file_browser; mod app;`)

- [ ] **Step 1: Update Cargo.toml with `rfd` dependency**

Update `Cargo.toml` dependencies to include `rfd = "0.17"`:

```toml
[package]
name = "rred_text_editor"
version = "0.1.0"
edition = "2024"

[dependencies]
eframe = "0.37"
rfd = "0.17"
```

- [ ] **Step 2: Run `cargo check` to verify dependencies download and compile**

Run: `cargo check`
Expected: downloads `rfd` and completes successfully.

- [ ] **Step 3: Create stub files for new modules**

Create `src/modals.rs`, `src/file_browser.rs`, and `src/app.rs` with empty or basic module declarations.

- [ ] **Step 4: Verify module stubs build cleanly**

Run: `cargo check`
Expected: compilation succeeds.

---

### Task 2: Modals & Confirmation Actions (`src/modals.rs`)

**Files:**
- Create/Modify: `src/modals.rs`
- Test: Unit tests in `src/modals.rs`

**Interfaces:**
- Produces:
  - `enum PendingAction { OpenFile(PathBuf), PromptOpenFile, Exit }`
  - `enum ModalResponse { Save, Discard, Cancel }`
  - `fn show_about_modal(ctx: &egui::Context, open: &mut bool)`
  - `fn show_unsaved_changes_modal(ctx: &egui::Context, file_name: &str) -> Option<ModalResponse>`

- [ ] **Step 1: Write failing unit test for `PendingAction` and modal responses**

In `src/modals.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_pending_action_equality() {
        let action1 = PendingAction::OpenFile(PathBuf::from("test.txt"));
        let action2 = PendingAction::OpenFile(PathBuf::from("test.txt"));
        let action3 = PendingAction::Exit;

        assert_eq!(action1, action2);
        assert_ne!(action1, action3);
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --bin rred_text_editor modals::tests`
Expected: FAIL because `PendingAction` is not yet defined.

- [ ] **Step 3: Implement `src/modals.rs`**

```rust
use eframe::egui;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    OpenFile(PathBuf),
    PromptOpenFile,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalResponse {
    Save,
    Discard,
    Cancel,
}

pub fn show_about_modal(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }

    egui::Window::new("Tentang Red Text Editor")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Red Text Editor");
                ui.label("Versi 0.1.0");
                ui.add_space(8.0);
                ui.label("Editor teks sederhana dan cepat berbasis Rust & egui.");
                ui.add_space(12.0);
                if ui.button("Tutup").clicked() {
                    *open = false;
                }
            });
        });
}

pub fn show_unsaved_changes_modal(ctx: &egui::Context, file_name: &str) -> Option<ModalResponse> {
    let mut response = None;

    egui::Window::new("Perubahan Belum Disimpan")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Terdapat perubahan belum disimpan pada \"{}\". Simpan sekarang?",
                file_name
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Simpan").clicked() {
                    response = Some(ModalResponse::Save);
                }
                if ui.button("Abaikan").clicked() {
                    response = Some(ModalResponse::Discard);
                }
                if ui.button("Batal").clicked() {
                    response = Some(ModalResponse::Cancel);
                }
            });
        });

    response
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo test --bin rred_text_editor modals::tests`
Expected: PASS

---

### Task 3: File Browser Tree View & Directory Scanning (`src/file_browser.rs`)

**Files:**
- Create/Modify: `src/file_browser.rs`
- Test: Unit tests in `src/file_browser.rs`

**Interfaces:**
- Produces:
  - `struct FsEntry { pub path: PathBuf, pub is_dir: bool, pub name: String }`
  - `fn scan_directory(dir: &Path) -> Vec<FsEntry>`
  - `fn show_file_browser(ui: &mut egui::Ui, current_folder: &Path, on_open_file: &mut Option<PathBuf>)`

- [ ] **Step 1: Write failing unit test for `scan_directory`**

In `src/file_browser.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn test_scan_directory_sorting() {
        let temp_dir = std::env::temp_dir().join("rred_test_scan_dir");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir.join("sub_folder")).unwrap();
        File::create(temp_dir.join("a_file.txt")).unwrap();
        File::create(temp_dir.join("b_file.txt")).unwrap();

        let entries = scan_directory(&temp_dir);
        assert!(!entries.is_empty());
        // Directory must come first
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "sub_folder");
        // Followed by sorted files
        assert_eq!(entries[1].name, "a_file.txt");
        assert_eq!(entries[2].name, "b_file.txt");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --bin rred_text_editor file_browser::tests`
Expected: FAIL because `scan_directory` does not exist.

- [ ] **Step 3: Implement `src/file_browser.rs`**

```rust
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
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo test --bin rred_text_editor file_browser::tests`
Expected: PASS

---

### Task 4: Core Application State & Actions (`src/app.rs`)

**Files:**
- Create/Modify: `src/app.rs`
- Test: Unit tests in `src/app.rs`

**Interfaces:**
- Consumes:
  - `src/modals::{PendingAction, ModalResponse, show_about_modal, show_unsaved_changes_modal}`
  - `src/file_browser::{show_file_browser}`
- Produces:
  - `struct TextEditorApp`
  - `impl TextEditorApp`: `is_dirty()`, `file_display_name()`, `window_title()`, `save()`, `save_as()`, `open_file_dialog()`, `open_folder_dialog()`, `load_file()`
  - `impl eframe::App for TextEditorApp`

- [ ] **Step 1: Write failing unit test for `is_dirty` and title formatting**

In `src/app.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_is_dirty_detection() {
        let mut app = TextEditorApp::default();
        assert!(!app.is_dirty());

        app.text_content = "Berubah".to_string();
        assert!(app.is_dirty());

        app.saved_content = "Berubah".to_string();
        assert!(!app.is_dirty());
    }

    #[test]
    fn test_window_title_formatting() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.window_title(), "[Dokumen Baru] - Red Text Editor");

        app.text_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "[Dokumen Baru] * - Red Text Editor");

        app.current_path = Some(PathBuf::from("/home/user/document.txt"));
        app.saved_content = "Teks baru".to_string();
        assert_eq!(app.window_title(), "document.txt - Red Text Editor");

        app.text_content = "Teks diedit".to_string();
        assert_eq!(app.window_title(), "document.txt * - Red Text Editor");
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --bin rred_text_editor app::tests`
Expected: FAIL because `TextEditorApp` and methods are not defined.

- [ ] **Step 3: Implement `src/app.rs`**

```rust
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};

use crate::file_browser;
use crate::modals::{self, ModalResponse, PendingAction};

pub struct TextEditorApp {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub opened_folder: Option<PathBuf>,
    pub show_file_browser: bool,
    pub pending_action: Option<PendingAction>,
    pub show_about_modal: bool,
    pub status_message: Option<(String, bool)>, // (message, is_error)
}

impl Default for TextEditorApp {
    fn default() -> Self {
        let initial_text = String::from("Ketik sesuatu di sini...");
        Self {
            text_content: initial_text.clone(),
            saved_content: initial_text,
            current_path: None,
            opened_folder: None,
            show_file_browser: false,
            pending_action: None,
            show_about_modal: false,
            status_message: None,
        }
    }
}

impl TextEditorApp {
    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
    }

    pub fn file_display_name(&self) -> String {
        self.current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[Dokumen Baru]".to_string())
    }

    pub fn window_title(&self) -> String {
        let dirty_flag = if self.is_dirty() { " *" } else { "" };
        format!("{}{dirty_flag} - Red Text Editor", self.file_display_name())
    }

    pub fn load_file(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(content) => {
                self.text_content = content.clone();
                self.saved_content = content;
                self.current_path = Some(path);
                self.status_message = Some(("File berhasil dibuka.".to_string(), false));
            }
            Err(err) => {
                self.status_message = Some((format!("Gagal membuka file: {err}"), true));
            }
        }
    }

    pub fn save(&mut self) -> bool {
        if let Some(path) = self.current_path.clone() {
            match fs::write(&path, &self.text_content) {
                Ok(_) => {
                    self.saved_content = self.text_content.clone();
                    self.status_message = Some(("File berhasil disimpan.".to_string(), false));
                    true
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal menyimpan file: {err}"), true));
                    false
                }
            }
        } else {
            self.save_as()
        }
    }

    pub fn save_as(&mut self) -> bool {
        let dialog = rfd::FileDialog::new();
        let dialog = if let Some(ref path) = self.current_path {
            if let Some(parent) = path.parent() {
                dialog.set_directory(parent)
            } else {
                dialog
            }
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            match fs::write(&path, &self.text_content) {
                Ok(_) => {
                    self.saved_content = self.text_content.clone();
                    self.current_path = Some(path);
                    self.status_message = Some(("File berhasil disimpan.".to_string(), false));
                    true
                }
                Err(err) => {
                    self.status_message = Some((format!("Gagal menyimpan file: {err}"), true));
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn trigger_open_file(&mut self) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::PromptOpenFile);
        } else {
            self.pick_and_open_file();
        }
    }

    pub fn pick_and_open_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.load_file(path);
        }
    }

    pub fn trigger_open_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.opened_folder = Some(folder);
            self.show_file_browser = true;
            self.status_message = Some(("Folder dibuka.".to_string(), false));
        }
    }

    pub fn trigger_open_specific_file(&mut self, path: PathBuf) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::OpenFile(path));
        } else {
            self.load_file(path);
        }
    }

    pub fn trigger_exit(&mut self, ctx: &egui::Context) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::Exit);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn execute_pending_action(&mut self, ctx: &egui::Context, action: PendingAction) {
        match action {
            PendingAction::OpenFile(path) => {
                self.load_file(path);
            }
            PendingAction::PromptOpenFile => {
                self.pick_and_open_file();
            }
            PendingAction::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for TextEditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 1. Top Panel (Menu Bar & Toolbar)
        egui::Panel::top("top_panel").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Menu File
                ui.menu_button("File", |ui| {
                    if ui.button("Buka File...").clicked() {
                        self.trigger_open_file();
                        ui.close();
                    }
                    if ui.button("Buka Folder...").clicked() {
                        self.trigger_open_folder();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Simpan").clicked() {
                        self.save();
                        ui.close();
                    }
                    if ui.button("Simpan Sebagai...").clicked() {
                        self.save_as();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Keluar").clicked() {
                        self.trigger_exit(&ctx);
                        ui.close();
                    }
                });

                // Menu Help
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.show_about_modal = true;
                        ui.close();
                    }
                });

                ui.separator();

                // Toggle Sidebar File Browser button
                if self.opened_folder.is_some() {
                    let toggle_text = if self.show_file_browser { "📂 Sembunyikan Sidebar" } else { "📁 Tampilkan Sidebar" };
                    if ui.button(toggle_text).clicked() {
                        self.show_file_browser = !self.show_file_browser;
                    }
                }

                // Info Karakter di sisi kanan
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Karakter: {}", self.text_content.len()));
                });
            });
        });

        // 2. Bottom Status Bar Panel
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let doc_label = self.file_display_name();
                let dirty_str = if self.is_dirty() { " [diubah]" } else { "" };
                ui.label(format!("Dokumen: {doc_label}{dirty_str}"));

                if let Some((msg, is_error)) = &self.status_message {
                    ui.separator();
                    let color = if *is_error {
                        egui::Color32::from_rgb(220, 60, 60)
                    } else {
                        egui::Color32::from_rgb(60, 180, 60)
                    };
                    ui.colored_label(color, msg);
                }
            });
        });

        // 3. Left Panel (File Browser Sidebar)
        let mut file_to_open = None;
        let mut close_sidebar = false;

        if self.show_file_browser {
            if let Some(ref folder) = self.opened_folder {
                egui::Panel::left("file_browser_panel")
                    .resizable(true)
                    .default_size(200.0)
                    .min_size(120.0)
                    .max_size(400.0)
                    .show(ui, |ui| {
                        file_browser::show_file_browser(
                            ui,
                            folder,
                            &mut file_to_open,
                            &mut close_sidebar,
                        );
                    });
            }
        }

        if close_sidebar {
            self.show_file_browser = false;
        }

        if let Some(path) = file_to_open {
            self.trigger_open_specific_file(path);
        }

        // 4. Central Panel (Main Text Editor Area)
        egui::CentralPanel::default().show(ui, |ui| {
            let text_style = egui::TextStyle::Monospace;
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.text_content)
                        .font(text_style)
                        .lock_focus(true)
                        .desired_width(f32::INFINITY),
                );
            });
        });

        // 5. Modals & Dialogs
        modals::show_about_modal(&ctx, &mut self.show_about_modal);

        if self.pending_action.is_some() {
            if let Some(response) = modals::show_unsaved_changes_modal(&ctx, &self.file_display_name()) {
                let action = self.pending_action.take().unwrap();
                match response {
                    ModalResponse::Save => {
                        if self.save() {
                            self.execute_pending_action(&ctx, action);
                        }
                    }
                    ModalResponse::Discard => {
                        self.execute_pending_action(&ctx, action);
                    }
                    ModalResponse::Cancel => {
                        // Action cancelled, do nothing
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo test --bin rred_text_editor app::tests`
Expected: PASS

---

### Task 5: Integration & Entry Point (`src/main.rs`)

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes:
  - `src/app::TextEditorApp`
- Produces:
  - Running application executable with native window title updating

- [ ] **Step 1: Wire modules in `src/main.rs`**

```rust
mod app;
mod file_browser;
mod modals;

use app::TextEditorApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_title("Red Text Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Red Text Editor",
        options,
        Box::new(|_cc| Ok(Box::new(TextEditorApp::default()))),
    )
}
```

- [ ] **Step 2: Run `cargo check` and `cargo test`**

Run: `cargo test`
Expected: All tests pass (0 failures).

- [ ] **Step 3: Run `cargo clippy` and `cargo build`**

Run: `cargo clippy -- -D warnings && cargo build`
Expected: Exit code 0, clean build with no warnings.

---

### Task 6: End-to-End Verification

- [ ] **Step 1: Run complete test suite**

Run: `cargo test`
Expected: PASS all unit tests across `app`, `file_browser`, and `modals`.

- [ ] **Step 2: Run verification check via `cargo clippy`**

Run: `cargo clippy`
Expected: Clean output.
