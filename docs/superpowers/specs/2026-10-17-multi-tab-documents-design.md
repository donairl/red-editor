# Multi-Tab Document Support Design

**Date:** 2026-10-17
**Status:** Approved by user
**Project:** `rred_text_editor`

## Objective

Add multi-tab document editing support to Red Text Editor, allowing users to open and switch between multiple files in tabs, close tabs with unsaved changes verification, and open new blank tabs.

## Motivation

Currently, the editor only supports a single document in memory. Opening a file either replaces the current document (prompting if unsaved) or cancels. Multi-tab editing enables standard code-editor workflows: editing several files at once, switching contexts without closing work, and opening files from the sidebar into tabs.

## Requirements & Key Decisions

1. **DocumentTab Structure:**
   - Document state is extracted from `TextEditorApp` into a `DocumentTab` struct:
     - `text_content: String`
     - `saved_content: String`
     - `current_path: Option<PathBuf>`
     - `active_language: Language`
     - `language_override: Option<Language>`
   - `TextEditorApp` holds `tabs: Vec<DocumentTab>` and `active_tab_index: usize`.
   - Global states (highlighter, word_wrap, opened_folder, show_file_browser, modals, status_message) remain on `TextEditorApp`.

2. **Always at least one tab:**
   - The editor starts with 1 empty tab ("Ketik sesuatu di sini...").
   - If the user closes the last tab, a new blank tab is automatically created so `tabs` is never empty and `active_tab_index` is always valid (`< tabs.len()`).

3. **Opening files & Duplicate prevention:**
   - When a user opens a file (via file dialog or sidebar), the app checks if `current_path == Some(path)` in any open tab.
   - If found, it simply activates that tab (switches `active_tab_index`) without reloading from disk or creating a duplicate.
   - If not found:
     - If the currently active tab is pristine (no `current_path`, clean/not dirty, content is the default template), it replaces the active tab with the opened file instead of leaving an unwanted empty tab behind.
     - Otherwise, a new tab is created and activated (`active_tab_index = tabs.len() - 1`).

4. **Tab Bar UI:**
   - Rendered horizontally directly above the editor area (inside `CentralPanel`).
   - Each tab displays:
     - Display name: file name or `[Dokumen Baru]`.
     - Dirty indicator `*` if `tab.is_dirty()`.
     - Close button (`x`).
     - Selected styling when `index == active_tab_index`.
   - Clicking a tab switches `active_tab_index`.
   - A `+` button at the right of the tab bar opens a new blank tab ("Tab Baru").
   - Menus:
     - "File > Tab Baru" creates a new blank tab.
     - "File > Tutup Tab" closes the active tab.

5. **Closing Tabs & Unsaved Changes:**
   - `PendingAction` in `modals.rs` is expanded:
     - `CloseTab(usize)`: prompt to save/discard before closing specific tab index.
     - `Exit`: prompt to save/discard when closing the app with unsaved tabs.
   - When closing a dirty tab:
     - Modal pops up: "Perubahan Belum Disimpan" for that specific document.
     - `Save`: saves that tab, then closes it.
     - `Discard`: closes the tab without saving.
     - `Cancel`: aborts closing.
   - When closing an active tab, `active_tab_index` safely adjusts (e.g., clamp to `tabs.len() - 1`).

6. **Application Exit:**
   - If any tab is dirty when the user triggers exit, prompt on the first dirty tab.

## Architecture & Data Flow

```
+--------------------------------------------------------------+
| Top Panel (Menu Bar: File, Tampilan, Help)                   |
+--------------------------------------------------------------+
| Left Panel (File Browser) | Central Panel                    |
|                           |  +-----------------------------+ |
|                           |  | Tab Bar: [Tab 1* x] [Tab 2 x] +|
|                           |  +-----------------------------+ |
|                           |  | Active Tab TextEdit Editor  | |
|                           |  | (ScrollArea both/vertical)  | |
|                           |  +-----------------------------+ |
+--------------------------------------------------------------+
| Status Bar (Document Name, Language Menu, Status Message)    |
+--------------------------------------------------------------+
```

### Methods on `DocumentTab`:
- `fn default_new() -> Self`: creates blank tab with initial text.
- `fn from_file(path: PathBuf) -> Result<Self, std::io::Error>`: loads file content and detects language.
- `fn is_dirty(&self) -> bool`
- `fn file_display_name(&self) -> String`
- `fn effective_language(&self) -> Language`
- `fn save(&mut self) -> Result<(), std::io::Error>`

### Methods on `TextEditorApp`:
- `fn active_tab(&self) -> &DocumentTab`
- `fn active_tab_mut(&mut self) -> &mut DocumentTab`
- `fn open_new_tab(&mut self)`
- `fn open_file_in_tab(&mut self, path: PathBuf)`
- `fn close_tab(&mut self, index: usize)`
- `fn trigger_close_tab(&mut self, index: usize)`

## Testing Strategy

Written in `src/app.rs`:
1. `test_default_starts_with_one_pristine_tab`: verify default state has 1 tab, not dirty, default title.
2. `test_open_new_tab`: adds a tab, switches active index to the new tab.
3. `test_open_file_replaces_pristine_tab`: opening file into empty default tab replaces it without creating tab count 2.
4. `test_open_file_creates_new_tab_when_dirty_or_non_empty`: opening file when current tab has content creates a second tab and activates it.
5. `test_open_existing_file_focuses_tab`: opening a file already open in tab 0 switches active index to 0 without opening duplicate tab.
6. `test_close_tab_adjusts_active_index`: closing tab 0 when 2 tabs open shifts focus to remaining tab.
7. `test_close_last_tab_creates_new_blank_tab`: closing the only open tab leaves 1 new blank tab.
8. `test_dirty_tab_triggers_pending_action_on_close`: trying to close dirty tab sets `PendingAction::CloseTab`.
9. Existing scrolling, syntax highlighting, and round-trip tests continue to pass.

## Non-Goals

- Draggable tab reordering (egui drag-and-drop complexity is not requested).
- Tab splitting (side-by-side / split view).
- Persistent session / tab restoration across application restarts.
