# Welcome Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A start page in the central area at launch and after the last tab closes: New File / Open File… / Open Folder… buttons and the recent folders and files.

**Architecture:** `TextEditorApp.show_welcome: bool` swaps the central panel to `welcome::show` (same early-return pattern as the Git Graph) while the single untitled tab stays underneath. `src/welcome.rs` is a pure egui view returning a `WelcomeAction`; `app.rs` maps it onto the existing open functions.

**Tech Stack:** Rust 2024, eframe/egui 0.37, headless tests via `ctx.run_ui`.

**Spec:** `docs/superpowers/specs/2026-11-01-welcome-page-design.md`

**Conventions:** `cargo test --quiet 2>&1 | tail -5`. `I18n::builtin()` starts in Indonesian, so headless tests assert `id.json` strings. Doc comments: one sentence saying what. Commit after every task.

---

## File map

| File | Change |
|---|---|
| `assets/lang/{id,en,de,es,zh}.json` | `welcome.*` keys |
| `src/welcome.rs` (new) | `WelcomeAction`, `show`, tests |
| `src/main.rs` | `mod welcome;` |
| `src/app.rs` | `show_welcome` field, central-panel swap, flag set/clear, shortcut guard, `previewing`, tests |
| `README.md`, `README.id.md` | Usage bullet |

---

### Task 1: Translations

**Files:** `assets/lang/id.json`, `en.json`, `de.json`, `es.json`, `zh.json` (insert after the `"file.no_recent"` line of each; that line ends with a comma).

- [ ] **Step 1: Add the keys**

`id.json`:
```json
  "welcome.new_file": "File Baru",
  "welcome.open_file": "Buka File…",
  "welcome.open_folder": "Buka Folder…",
  "welcome.recent_folders": "Folder Terbaru",
  "welcome.recent_files": "File Terbaru",
```
`en.json`:
```json
  "welcome.new_file": "New File",
  "welcome.open_file": "Open File…",
  "welcome.open_folder": "Open Folder…",
  "welcome.recent_folders": "Recent Folders",
  "welcome.recent_files": "Recent Files",
```
`de.json`:
```json
  "welcome.new_file": "Neue Datei",
  "welcome.open_file": "Datei öffnen…",
  "welcome.open_folder": "Ordner öffnen…",
  "welcome.recent_folders": "Zuletzt geöffnete Ordner",
  "welcome.recent_files": "Zuletzt geöffnete Dateien",
```
`es.json`:
```json
  "welcome.new_file": "Archivo nuevo",
  "welcome.open_file": "Abrir archivo…",
  "welcome.open_folder": "Abrir carpeta…",
  "welcome.recent_folders": "Carpetas recientes",
  "welcome.recent_files": "Archivos recientes",
```
`zh.json`:
```json
  "welcome.new_file": "新建文件",
  "welcome.open_file": "打开文件…",
  "welcome.open_folder": "打开文件夹…",
  "welcome.recent_folders": "最近的文件夹",
  "welcome.recent_files": "最近的文件",
```

- [ ] **Step 2: Validate and test**

`for f in assets/lang/*.json; do python3 -c "import json; json.load(open('$f'))" && echo "$f ok"; done` then `cargo test --quiet i18n 2>&1 | tail -3` → PASS.

- [ ] **Step 3: Commit**

```bash
git add assets/lang && git commit -m "feat(welcome): translations for the start page"
```

---

### Task 2: The view

**Files:**
- Create: `src/welcome.rs`
- Modify: `src/main.rs` (add `mod welcome;` after `mod theme;`)

- [ ] **Step 1: Write the module with its tests**

`src/welcome.rs`:

```rust
//! The start page: what the central area shows when there is nothing to
//! edit yet. It only reports what was clicked; `app.rs` opens things.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, Vec2, pos2, vec2};

use crate::file_browser::{self, ROW_HEIGHT};
use crate::history::{Entry, EntryKind, History};
use crate::i18n::I18n;
use crate::theme;

/// Widest the centred column gets.
const COLUMN_WIDTH: f32 = 520.0;
/// Below this column width the two recent lists stack.
const SIDE_BY_SIDE_MIN: f32 = 440.0;
const BUTTON_SIZE: Vec2 = vec2(150.0, 32.0);
/// Entries shown per recent list.
const MAX_RECENT: usize = 8;
const PADDING: f32 = 48.0;

/// What the user asked of the start page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WelcomeAction {
    NewFile,
    OpenFile,
    OpenFolder,
    Open(Entry),
}

/// Draws the start page over the whole available area.
pub fn show(ui: &mut egui::Ui, history: &History, i18n: &I18n) -> Option<WelcomeAction> {
    let theme = theme::active(ui.ctx());
    let palette = theme.sidebar;
    let area = ui.max_rect();
    ui.painter().rect_filled(area, CornerRadius::ZERO, theme.tab.tab_active);
    let mut action = None;

    let column_width = (area.width() - 2.0 * PADDING).clamp(240.0, COLUMN_WIDTH);
    let side_by_side = column_width >= SIDE_BY_SIDE_MIN;
    let folders: Vec<&Entry> = history.entries().iter().filter(|e| e.kind == EntryKind::Folder).take(MAX_RECENT).collect();
    let files: Vec<&Entry> = history.entries().iter().filter(|e| e.kind == EntryKind::File).take(MAX_RECENT).collect();
    let list_rows = |n: usize| n.max(1) as f32 * ROW_HEIGHT + ROW_HEIGHT;
    let lists_height = if side_by_side {
        list_rows(folders.len()).max(list_rows(files.len()))
    } else {
        list_rows(folders.len()) + list_rows(files.len()) + 16.0
    };
    // Title, buttons, gap, lists.
    let content_height = 40.0 + 24.0 + BUTTON_SIZE.y + 32.0 + lists_height;
    let top = if content_height < area.height() - 2.0 * PADDING {
        area.center().y - content_height / 2.0
    } else {
        area.top() + PADDING
    };
    let left = area.center().x - column_width / 2.0;

    egui::ScrollArea::vertical()
        .id_salt("welcome_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.allocate_space(vec2(ui.available_width(), (top - area.top()).max(0.0)));
            ui.allocate_ui_at_rect(Rect::from_min_size(pos2(left, ui.cursor().top()), vec2(column_width, content_height)), |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Red Text Editor").size(22.0).color(palette.foreground));
                    ui.label(RichText::new(env!("CARGO_PKG_VERSION")).size(13.0).color(palette.icon_inactive));
                });
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let ctx = ui.ctx().clone();
                    for (key, shortcut, result) in [
                        ("welcome.new_file", crate::app::shortcuts::NEW_TAB, WelcomeAction::NewFile),
                        ("welcome.open_file", crate::app::shortcuts::OPEN_FILE, WelcomeAction::OpenFile),
                        ("welcome.open_folder", crate::app::shortcuts::OPEN_FOLDER, WelcomeAction::OpenFolder),
                    ] {
                        let button = egui::Button::new(RichText::new(i18n.t(key)).color(palette.foreground))
                            .shortcut_text(RichText::new(ctx.format_shortcut(&shortcut)).color(palette.icon_inactive))
                            .min_size(BUTTON_SIZE);
                        if ui.add(button).clicked() {
                            action = Some(result);
                        }
                    }
                });
                ui.add_space(24.0);
                let list_width = if side_by_side { (column_width - 16.0) / 2.0 } else { column_width };
                if side_by_side {
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui(vec2(list_width, lists_height), |ui| {
                            if let Some(a) = show_list(ui, &palette, i18n.t("welcome.recent_folders"), &folders, list_width, i18n) {
                                action = Some(a);
                            }
                        });
                        ui.add_space(16.0);
                        ui.allocate_ui(vec2(list_width, lists_height), |ui| {
                            if let Some(a) = show_list(ui, &palette, i18n.t("welcome.recent_files"), &files, list_width, i18n) {
                                action = Some(a);
                            }
                        });
                    });
                } else {
                    if let Some(a) = show_list(ui, &palette, i18n.t("welcome.recent_folders"), &folders, list_width, i18n) {
                        action = Some(a);
                    }
                    ui.add_space(16.0);
                    if let Some(a) = show_list(ui, &palette, i18n.t("welcome.recent_files"), &files, list_width, i18n) {
                        action = Some(a);
                    }
                }
            });
        });
    action
}

/// A heading over the entries, or `file.no_recent` when there are none.
fn show_list(
    ui: &mut egui::Ui,
    palette: &file_browser::SidebarPalette,
    heading: &str,
    entries: &[&Entry],
    width: f32,
    i18n: &I18n,
) -> Option<WelcomeAction> {
    let mut action = None;
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        heading.to_uppercase(),
        FontId::proportional(11.0),
        palette.icon_inactive,
    );
    if entries.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            i18n.t("file.no_recent"),
            FontId::proportional(13.0),
            palette.icon_inactive,
        );
        return None;
    }
    for entry in entries {
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        let row = ui
            .interact(rect, egui::Id::unique(("welcome_recent", &entry.path)), Sense::click())
            .on_hover_text(entry.path.display().to_string());
        if row.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(4), palette.row_hover);
        }
        let (name, folder) = entry.label();
        let name = file_browser::elided(ui, name, FontId::proportional(13.0), palette.accent, width * 0.6);
        let name_width = name.size().x;
        ui.painter().galley(pos2(rect.left() + 4.0, rect.center().y - name.size().y / 2.0), name, palette.accent);
        let room = width - name_width - 16.0;
        if room > 40.0 {
            let folder = file_browser::elided(ui, folder, FontId::proportional(11.0), palette.icon_inactive, room);
            ui.painter().galley(pos2(rect.left() + 4.0 + name_width + 8.0, rect.center().y - folder.size().y / 2.0), folder, palette.icon_inactive);
        }
        if row.clicked() {
            action = Some(WelcomeAction::Open((*entry).clone()));
        }
    }
    let _ = (Layout::default(), Align::Min, Color32::TRANSPARENT);
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn history() -> History {
        let mut h = History::default();
        h.record(PathBuf::from("/tmp/proj-a"), EntryKind::Folder);
        h.record(PathBuf::from("/tmp/proj-b"), EntryKind::Folder);
        h.record(PathBuf::from("/tmp/notes.md"), EntryKind::File);
        h.record(PathBuf::from("/tmp/main.rs"), EntryKind::File);
        h
    }

    fn frame(ctx: &egui::Context, history: &History, events: Vec<egui::Event>) -> (Option<WelcomeAction>, Vec<(String, Rect)>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            action = show(ui, history, &I18n::builtin());
        });
        output.textures_delta.clear();
        let texts = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect();
        (action, texts)
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

    #[test]
    fn test_paints_buttons_headings_and_recent_names() {
        let ctx = egui::Context::default();
        let (_, texts) = frame(&ctx, &history(), Vec::new());
        let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for expected in ["File Baru", "Buka File…", "Buka Folder…", "FOLDER TERBARU", "FILE TERBARU", "proj-a/", "proj-b/", "notes.md", "main.rs"] {
            assert!(names.contains(&expected), "{expected} missing from {names:?}");
        }
    }

    #[test]
    fn test_buttons_and_recent_rows_report_actions() {
        let ctx = egui::Context::default();
        let history = history();
        let (_, texts) = frame(&ctx, &history, Vec::new());
        let (_, new_file) = texts.iter().find(|(t, _)| t == "File Baru").unwrap();
        let (action, _) = frame(&ctx, &history, click(new_file.center()));
        assert_eq!(action, Some(WelcomeAction::NewFile));

        let (_, texts) = frame(&ctx, &history, Vec::new());
        let (_, notes) = texts.iter().find(|(t, _)| t == "notes.md").unwrap();
        let (action, _) = frame(&ctx, &history, click(notes.center()));
        assert_eq!(action, Some(WelcomeAction::Open(Entry { path: PathBuf::from("/tmp/notes.md"), kind: EntryKind::File })));
    }

    #[test]
    fn test_empty_history_says_so_twice() {
        let ctx = egui::Context::default();
        let (_, texts) = frame(&ctx, &History::default(), Vec::new());
        let count = texts.iter().filter(|(t, _)| t == I18n::builtin().t("file.no_recent")).count();
        assert_eq!(count, 2, "one note per list");
    }
}
```

Notes for the implementer: `crate::app::shortcuts` must be reachable (`pub mod shortcuts` already; confirm `app` is a `pub mod` or the path `crate::app::shortcuts::NEW_TAB` resolves within the crate; `mod app;` in main.rs is enough for crate-internal paths). Remove the `let _ = (Layout::default(), …)` line and any unused imports once the compiler shows which are unused; it exists only so the import list above compiles if some items go unused. `History::record` and `Entry::label` exist in `src/history.rs`; a folder's label ends with the path separator (`proj-a/` on Linux).

Add `mod welcome;` to `src/main.rs` after `mod theme;`.

- [ ] **Step 2: Run the tests**

`cargo test --quiet welcome 2>&1 | tail -8` → 3 passed. Fix compile errors in the pasted code without changing behaviour (`allocate_ui_at_rect` is deprecated in 0.37 in favour of `ui.scope_builder(UiBuilder::new().max_rect(rect), ..)`; use whichever compiles without a warning).

- [ ] **Step 3: Commit**

```bash
git add src/welcome.rs src/main.rs && git commit -m "feat(welcome): start page view"
```

---

### Task 3: Hosting in the app

**Files:** `src/app.rs`

- [ ] **Step 1: Write the tests** (tests module, after `test_zen_mode_toggles_with_shortcut`)

```rust
    #[test]
    fn test_welcome_shows_at_start_and_after_the_last_tab_closes() {
        let mut app = TextEditorApp::default();
        assert!(app.show_welcome, "the start page opens the editor");
        app.open_new_tab();
        assert!(!app.show_welcome, "a new file dismisses it");
        app.close_tab(app.active_tab_index);
        assert!(!app.show_welcome, "one tab still open");
        app.close_tab(0);
        assert!(app.show_welcome, "the last tab closed");
        app.open_file_in_tab(PathBuf::from("Cargo.toml"));
        assert!(!app.show_welcome, "opening a file dismisses it");
    }

    #[test]
    fn test_editor_shortcuts_are_ignored_on_the_welcome_page() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(!app.find.visible);
        app.open_new_tab();
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(app.find.visible);
    }

    #[test]
    fn test_no_markdown_preview_on_the_welcome_page() {
        let mut app = markdown_app();
        app.show_markdown_preview = true;
        app.show_welcome = true;
        assert!(!app.previewing());
    }
```

- [ ] **Step 2: Run, expect compile errors** on `show_welcome`.

- [ ] **Step 3: Wire**

Imports: `use crate::welcome::{self, WelcomeAction};`.

Field after `git_graph`:
```rust
    /// The start page in place of the editor; back whenever no tab is left.
    pub show_welcome: bool,
```
`Default`: `show_welcome: true,`.

`open_new_tab`: first line `self.show_welcome = false;`. `open_file_in_tab`: after `self.git_graph = None;` add `self.show_welcome = false;`. `open_folder`: first line `self.show_welcome = false;`.

`close_tab`: in the `self.tabs.len() <= 1` branch add `self.show_welcome = true;` after resetting the tab.

`apply_shortcut`: change the guard to `if (self.git_graph.is_some() || self.show_welcome) && action.needs_editor() { return; }`. Update the `needs_editor` doc to `/// Acts on the editor, which the Git Graph and the start page hide.`

`previewing()`: add `&& !self.show_welcome`.

Central panel, right after the Git Graph early-return block:
```rust
            if self.show_welcome {
                welcome_action = welcome::show(ui, &self.history, &self.i18n);
                self.scroll_sync.reset();
                return;
            }
```
Declare `let mut welcome_action = None;` next to `graph_action`, and after the panel:
```rust
        if let Some(action) = welcome_action {
            match action {
                WelcomeAction::NewFile => self.open_new_tab(),
                WelcomeAction::OpenFile => self.trigger_open_file(),
                WelcomeAction::OpenFolder => self.trigger_open_folder(),
                WelcomeAction::Open(entry) => self.open_recent(entry),
            }
        }
```
When the page is dismissed the editor should get focus: in `open_new_tab` and `open_file_in_tab`, if the flag was true before clearing it, set `self.editor_request = EditorRequest { focus: true, ..Default::default() };` (write a tiny private helper `fn leave_welcome(&mut self)` that does both and call it from the three places).

- [ ] **Step 4: Run the suite** → `cargo test --quiet 2>&1 | tail -4` green; `cargo clippy --all-targets 2>&1 | grep -c "^warning"` → 0. Check existing tests that assume the editor is drawn at startup (grep `test_frame_` in app.rs): if one fails because the welcome page hides the editor, set `app.show_welcome = false` in that test's setup and say so.

- [ ] **Step 5: Smoke-run** `timeout 15 cargo run 2>&1 | tail -3` if a display exists.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs && git commit -m "feat(welcome): start page at launch and after the last tab closes"
```

---

### Task 4: README

**Files:** `README.md`, `README.id.md` (Usage section, first bullet list under `## Usage` / `## Cara pakai`).

- [ ] **Step 1: Add a bullet** at the top of that list.

`README.md`:
```markdown
- **Start page**: the editor opens on a page with **New File**, **Open File…** and **Open Folder…** and your recent folders and files; click one to reopen it. Closing the last tab brings the page back.
```
`README.id.md`:
```markdown
- **Halaman awal**: editor dibuka dengan halaman berisi **File Baru**, **Buka File…**, **Buka Folder…**, serta folder dan file yang baru dibuka; klik untuk membukanya lagi. Menutup tab terakhir memunculkan halaman ini kembali.
```

- [ ] **Step 2: Commit**

```bash
git add README.md README.id.md && git commit -m "docs(welcome): start page in both READMEs"
```

---

## Self-review

- Spec coverage: state (T3), view and layout (T2), wiring and shortcuts and preview (T3), i18n (T1), tests (T2, T3), README (T4). The spec's "command-line path" clause is moot: the app has no CLI file handling.
- Types: `WelcomeAction` variants used in T3 match T2; `shortcuts::{NEW_TAB, OPEN_FILE, OPEN_FOLDER}` exist in `app.rs`.
