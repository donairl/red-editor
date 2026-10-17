# Design Specification: File Browser and Menus (File & Help)

**Date:** 2026-10-17  
**Status:** Approved  
**Project:** `rred_text_editor`

---

## 1. Overview & Goals

The goal of this enhancement is to transform `rred_text_editor` from a minimal standalone scratchpad into a usable desktop text editor with native file management, folder browsing, and help dialogs.

### Key Requirements
1. **File Browser (Sidebar):**
   - Left panel displaying an expandable/collapsible hierarchical tree view of an opened folder.
   - Clicking a file opens it in the main editor.
   - Resizable sidebar with a toggle option.
2. **File Menu Actions:**
   - **Open:** Pick and open a single text file using native OS file dialog.
   - **Open Folder:** Pick a folder using native OS folder picker, populating the file browser.
   - **Save:** Save current buffer to active file path, or invoke Save As if untitled.
   - **Save As:** Save buffer to a chosen location using native OS save file dialog.
   - **Exit:** Close the application window gracefully.
3. **Help Menu Actions:**
   - **About:** Open a centered modal window detailing application name, version, description, and credits.
4. **Safety & Dirty State Management:**
   - Detect unsaved modifications (`is_dirty`).
   - Prompt user with a confirmation dialog (Save, Discard, Cancel) before opening another file or exiting if there are unsaved changes.

---

## 2. Architecture & Modules

The application will be refactored into modular components to maintain clean separation of concerns and avoid monolithic growth:

```
src/
├── main.rs          # Application entry point, window configuration, and eframe initialization
├── app.rs           # Core TextEditorApp state, top menu rendering, status bar, and central editor panel
├── file_browser.rs  # Left sidebar panel, folder scanning, sorting, and recursive tree rendering
└── modals.rs        # About modal dialog and Unsaved Changes confirmation prompt
```

### Dependencies
- `eframe = "0.37"` (GUI framework)
- `rfd = "0.17"` (Rusty File Dialog - native cross-platform OS dialogs)

---

## 3. Data Structures & State Management

### 3.1 `TextEditorApp` (in `src/app.rs`)
```rust
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

#[derive(Clone, Debug, PartialEq)]
pub enum PendingAction {
    OpenFile(PathBuf),
    PromptOpenFile,
    Exit,
}
```

### 3.2 File Browser Representation (in `src/file_browser.rs`)
```rust
pub struct FileBrowser {
    pub root: Option<PathBuf>,
}
```
The tree directory scanner will read directory entries on demand or recursively:
- Entries are sorted with subdirectories first (alphabetically), followed by regular files (alphabetically).
- Hidden files and folders (starting with `.`) are ignored or filtered by default.
- Directories render using `egui::CollapsingHeader`.
- Files render as clickable labels / buttons.

---

## 4. Workflows & Detailed Interactions

### 4.1 File Operations
- **Open File:**
  1. If `app.is_dirty()`, set `pending_action = Some(PendingAction::PromptOpenFile)` and prompt modal.
  2. If clean or user confirms discard: trigger `rfd::FileDialog::new().pick_file()`.
  3. On selection: load file content into `text_content`, update `saved_content = text_content.clone()`, set `current_path = Some(path)`.
- **Open Folder:**
  1. Trigger `rfd::FileDialog::new().pick_folder()`.
  2. On selection: update `opened_folder = Some(path)`, set `show_file_browser = true`.
- **Save:**
  1. If `current_path` is `Some(path)`: write `text_content` to disk, set `saved_content = text_content.clone()`, set success status message.
  2. If `current_path` is `None`: delegate to **Save As**.
- **Save As:**
  1. Trigger `rfd::FileDialog::new().save_file()`.
  2. On selection: write `text_content` to disk, set `current_path = Some(path)`, update `saved_content`.
- **Exit:**
  1. If `app.is_dirty()`, set `pending_action = Some(PendingAction::Exit)` and prompt modal.
  2. If clean or confirmed: call `ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)`.

### 4.2 Modals
- **About Modal:**
  - Rendered when `show_about_modal == true`.
  - Display centered dialog with title "Tentang Red Text Editor", version `0.1.0`, description, and "Tutup" button.
- **Unsaved Changes Confirmation Modal:**
  - Rendered when `pending_action.is_some()`.
  - Buttons:
    - **Simpan (Save):** Executes save logic for current document, then proceeds with pending action.
    - **Abaikan (Discard):** Discards changes and directly proceeds with pending action.
    - **Batal (Cancel):** Clears `pending_action` and stays in current editor state.

---

## 5. UI Layout

1. **Top Panel:**
   - Menu bar:
     - `File`: Open, Open Folder, Separator, Save, Save As, Separator, Exit.
     - `Help`: About.
   - Toggle button for File Browser sidebar (`📁`).
   - Character count indicator on the right.
2. **Left Panel (Conditional when `show_file_browser` is true):**
   - Title: Folder name + close/collapse button.
   - Scrollable tree view of files and folders.
3. **Central Panel:**
   - Full-area monospace `TextEdit::multiline` with line-wrapping and focus lock.
4. **Bottom Status Bar:**
   - Shows current file path (or `[Dokumen Baru]`), dirty indicator (`*`), and status/error messages.

---

## 6. Error Handling

- Non-UTF8 or unreadable files produce an informative non-blocking status message instead of panicking.
- I/O failures on save (e.g. read-only filesystem or permission denied) set an error status message.

---

## 7. Verification & Testing Strategy

- Unit tests for:
  - `is_dirty` logic: comparison between active and saved content.
  - Directory sorting logic: folders before files, alphabetical sorting.
  - Title/status formatting with modified indicator (`*`).
- Verification via `cargo check`, `cargo clippy`, and `cargo test`.
