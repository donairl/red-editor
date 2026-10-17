# Multi-Tab Document Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable multi-tab editing in Red Text Editor, allowing users to open multiple files in separate tabs, switch between tabs via a top tab bar, close tabs with unsaved-changes protection, and prevent duplicate tabs when reopening existing files.

**Architecture:** Extract document-specific fields into a `DocumentTab` struct. `TextEditorApp` holds `tabs: Vec<DocumentTab>` and `active_tab_index: usize`. A horizontal tab bar is rendered directly above the editor area in the CentralPanel. Opening a file focuses existing tabs or opens a new tab (or replaces an untouched pristine tab). Closing a dirty tab invokes the unsaved changes modal. Closing the last tab creates a fresh blank tab.

**Tech Stack:** Rust (edition 2024), `eframe`/`egui` 0.37.

## Global Constraints

- Follow existing naming conventions and Indonesian UI localization (e.g., "Tab Baru", "Tutup Tab", "[Dokumen Baru]").
- `DocumentTab` fields: `text_content`, `saved_content`, `current_path`, `active_language`, `language_override`.
- `tabs` is never empty; closing the last tab always resets or inserts an empty `[Dokumen Baru]`.
- `active_tab_index` is always valid (`< tabs.len()`).
- Reopening an already open file path activates that tab instead of duplicating it.
- Closing a dirty tab prompts with the unsaved changes modal (`PendingAction::CloseTab(index)`).
- All 45 existing tests (updated for tab signatures where applicable) and new tab tests must pass.
- `cargo clippy --all-targets` and `cargo build --release` must pass without warnings.

---

### Task 1: Introduce `DocumentTab` and update `PendingAction::CloseTab`

**Files:**
- Modify: `src/modals.rs:6-12` (add `CloseTab(usize)` to `PendingAction`)
- Modify: `src/app.rs` (define `DocumentTab`, refactor `TextEditorApp` to use `tabs` and `active_tab_index`)
- Test: `src/app.rs` (`mod tests`)

**Interfaces:**
- Produces:
  ```rust
  pub struct DocumentTab {
      pub text_content: String,
      pub saved_content: String,
      pub current_path: Option<PathBuf>,
      pub active_language: Language,
      pub language_override: Option<Language>,
  }
  impl DocumentTab {
      pub fn default_new() -> Self;
      pub fn is_dirty(&self) -> bool;
      pub fn file_display_name(&self) -> String;
      pub fn effective_language(&self) -> Language;
      pub fn is_pristine(&self) -> bool;
  }
  ```
  And in `TextEditorApp`:
  ```rust
  pub struct TextEditorApp {
      pub tabs: Vec<DocumentTab>,
      pub active_tab_index: usize,
      ...
  }
  impl TextEditorApp {
      pub fn active_tab(&self) -> &DocumentTab;
      pub fn active_tab_mut(&mut self) -> &mut DocumentTab;
      pub fn open_new_tab(&mut self);
      pub fn close_tab(&mut self, index: usize);
      pub fn trigger_close_tab(&mut self, index: usize);
  }
  ```

- [ ] **Step 1: Update `PendingAction` in `src/modals.rs`**

Add `CloseTab(usize)` to `PendingAction`:
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAction {
    OpenFile(PathBuf),
    PromptOpenFile,
    CloseTab(usize),
    Exit,
}
```

- [ ] **Step 2: Add `DocumentTab` and update `TextEditorApp` in `src/app.rs`**

Define `DocumentTab`:
```rust
pub const INITIAL_TEXT: &str = "Ketik sesuatu di sini...";

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentTab {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub active_language: Language,
    pub language_override: Option<Language>,
}

impl Default for DocumentTab {
    fn default() -> Self {
        Self::default_new()
    }
}

impl DocumentTab {
    pub fn default_new() -> Self {
        let initial = String::from(INITIAL_TEXT);
        Self {
            text_content: initial.clone(),
            saved_content: initial,
            current_path: None,
            active_language: Language::PlainText,
            language_override: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.text_content != self.saved_content
    }

    pub fn is_pristine(&self) -> bool {
        self.current_path.is_none() && !self.is_dirty() && self.text_content == INITIAL_TEXT
    }

    pub fn file_display_name(&self) -> String {
        self.current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[Dokumen Baru]".to_string())
    }

    pub fn effective_language(&self) -> Language {
        self.language_override.unwrap_or(self.active_language)
    }
}
```

Replace `text_content`, `saved_content`, `current_path`, `active_language`, `language_override` in `TextEditorApp` with:
```rust
    pub tabs: Vec<DocumentTab>,
    pub active_tab_index: usize,
```

Implement convenience and tab management methods on `TextEditorApp`:
- `active_tab(&self) -> &DocumentTab`
- `active_tab_mut(&mut self) -> &mut DocumentTab`
- `is_dirty(&self) -> bool`: returns `self.tabs.iter().any(|t| t.is_dirty())` (or `self.active_tab().is_dirty()`, with `any_dirty` for app exit).
- `file_display_name(&self) -> String`: delegates to `self.active_tab().file_display_name()`.
- `effective_language(&self) -> Language`: delegates to `self.active_tab().effective_language()`.
- `open_new_tab(&mut self)`: pushes `DocumentTab::default_new()` and sets `self.active_tab_index = self.tabs.len() - 1`.
- `open_file_in_tab(&mut self, path: PathBuf)`:
  - If existing tab has `current_path == Some(&path)`, set `active_tab_index` to that tab's index and set status message.
  - Else if `self.active_tab().is_pristine()`, load file into active tab.
  - Else load file into a new `DocumentTab`, push to `tabs`, and set `active_tab_index = self.tabs.len() - 1`.
- `close_tab(&mut self, index: usize)`:
  - If `self.tabs.len() <= 1`, reset the only tab with `DocumentTab::default_new()`.
  - Else remove tab at `index`. If `self.active_tab_index >= self.tabs.len()`, set `self.active_tab_index = self.tabs.len() - 1`.
- `trigger_close_tab(&mut self, index: usize)`:
  - If tab at `index` is dirty, set `self.pending_action = Some(PendingAction::CloseTab(index))`.
  - Else `self.close_tab(index)`.

- [ ] **Step 3: Update `save`, `save_as`, `load_file`, and dialog action executions**

Make `save` and `save_as` operate on `self.active_tab_mut()`:
- When saving, update `saved_content` and `current_path` on the active tab.
- In `execute_pending_action`:
  - `PendingAction::CloseTab(index) => self.close_tab(index)`
  - Handle `PendingAction::OpenFile(path)` and `PendingAction::PromptOpenFile` using `open_file_in_tab` and `pick_and_open_file`.
  - Update `trigger_exit`: check if any tab is dirty (`self.tabs.iter().any(|t| t.is_dirty())`), prompt with `PendingAction::Exit` if dirty.

- [ ] **Step 4: Update existing tests in `src/app.rs`**

Update any direct field accesses in existing tests (e.g., `app.text_content` -> `app.active_tab().text_content` or `app.active_tab_mut().text_content`).
Add new unit tests:
- `test_default_starts_with_one_pristine_tab`
- `test_open_new_tab`
- `test_open_file_replaces_pristine_tab`
- `test_open_file_creates_new_tab_when_active_tab_is_not_pristine`
- `test_open_existing_file_focuses_tab_without_duplicate`
- `test_close_tab_adjusts_active_index`
- `test_close_last_tab_creates_new_blank_tab`
- `test_dirty_tab_triggers_close_tab_pending_action`

- [ ] **Step 5: Verify tests and commit Task 1**

Run: `cargo test && cargo clippy --all-targets`
Commit: `feat(editor): extract DocumentTab and implement multi-tab data structures`

---

### Task 2: Render Tab Bar UI and wire Menu Items

**Files:**
- Modify: `src/app.rs:260-460` (render tab bar in CentralPanel, add "Tab Baru" and "Tutup Tab" to File menu)
- Test: `src/app.rs` (`mod tests`)

- [ ] **Step 1: Add "Tab Baru" and "Tutup Tab" to File Menu**

In `ui.menu_button("File", ...)`:
- `if ui.button("Tab Baru").clicked() { self.open_new_tab(); ui.close(); }`
- `if ui.button("Tutup Tab").clicked() { self.trigger_close_tab(self.active_tab_index); ui.close(); }`
- Place them appropriately (e.g. at the top of the File menu with a separator).

- [ ] **Step 2: Render Tab Bar above Editor in CentralPanel**

Inside `egui::CentralPanel::default().show(ui, |ui| { ... })`:
Render horizontal scrollable tab bar:
```rust
egui::ScrollArea::horizontal()
    .id_salt("tab_bar_scroll")
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            let mut tab_to_close = None;
            let mut tab_to_select = None;
            for (idx, tab) in self.tabs.iter().enumerate() {
                let is_active = idx == self.active_tab_index;
                let title = format!(
                    "{}{}",
                    tab.file_display_name(),
                    if tab.is_dirty() { " *" } else { "" }
                );

                ui.group(|ui| {
                    ui.style_mut().visuals.widgets.inactive.bg_fill = if is_active {
                        ui.visuals().selection.bg_fill.linear_multiply(0.4)
                    } else {
                        egui::Color32::TRANSPARENT
                    };

                    if ui.selectable_label(is_active, title).clicked() {
                        tab_to_select = Some(idx);
                    }
                    if ui.small_button("✕").clicked() {
                        tab_to_close = Some(idx);
                    }
                });
            }

            if ui.button("➕").clicked() {
                self.open_new_tab();
            }

            if let Some(idx) = tab_to_select {
                self.active_tab_index = idx;
            }
            if let Some(idx) = tab_to_close {
                self.trigger_close_tab(idx);
            }
        });
    });
ui.separator();
```
Below the separator, render the active tab's editor using `render_editor`.

- [ ] **Step 3: Write tests for Tab Bar rendering and UI interaction**

Add tests simulating tab switching and close button clicks via `run_frame_with_events`.

- [ ] **Step 4: Full verification and Commit Task 2**

Run:
- `cargo test`
- `cargo clippy --all-targets`
- `cargo build --release`
Commit: `feat(editor): render tab bar and wire tab menu actions`
