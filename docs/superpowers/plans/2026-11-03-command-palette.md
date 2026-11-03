# Command Palette Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A VS Code-style popup that runs any editor command by name (`Ctrl+Shift+P`) or opens any file in the open folder by name (`Ctrl+P`).

**Architecture:** New module `src/palette.rs` owns the state, the fuzzy scorer, the static command table and the popup widget; it returns a `PaletteAction` and never touches the app. `app.rs` builds the item list each frame (commands from the table, files from the Explorer's existing background `FileIndex`), draws the popup, and runs the result through the existing `apply_shortcut` / `open_file_in_tab`.

**Tech Stack:** Rust 2024, egui/eframe 0.37, existing `I18n`, `theme::active(ctx).sidebar` palette, `file_browser::ROW_HEIGHT`.

**Spec:** `docs/superpowers/specs/2026-11-03-command-palette-design.md`

---

## Before starting

The working tree holds uncommitted Git Log work (`src/git_log.rs`, `src/git.rs`, `src/main.rs`, part of `src/app.rs`, `git_log.*` keys in `assets/lang/*.json`). Every task below commits with explicit paths, and several touch `app.rs` and the language files. **Commit or stash the Git Log work first** so each palette commit holds only palette changes:

```bash
git status --short
# either: git add -A && git commit -m "feat(git): git log panel"   (if it builds and tests pass)
# or:     git stash push -u -m "git log wip"
```

Run `cargo test` once before Task 1 to confirm the baseline is green (436 tests at the time of writing).

## File structure

| File | Responsibility |
|---|---|
| `src/palette.rs` (new) | `Palette` state, `PaletteMode`, `score`/`rank`, `Item`, `Command`/`COMMANDS`, `PaletteAction`, `show` widget, unit tests |
| `src/app.rs` | `ShortcutAction::{CommandPalette, QuickOpen, SelectTheme}`, `shortcuts::{COMMAND_PALETTE, QUICK_OPEN}`, `ShortcutAction::shortcut()`, `TextEditorApp.palette`, `open_palette`, `show_palette`, two menu items |
| `src/file_browser.rs` | `Explorer::indexed_files` |
| `src/main.rs` | `mod palette;` |
| `src/i18n.rs` | add `palette.rs` to the key scanner |
| `assets/lang/{en,id,es,de,zh}.json` | 15 new keys |
| `README.md`, `README.id.md` | feature bullet |

---

### Task 1: Fuzzy scorer

**Files:**
- Create: `src/palette.rs`
- Modify: `src/main.rs` (add `mod palette;` after `mod markdown_preview;`)
- Modify: `docs/superpowers/specs/2026-11-03-command-palette-design.md` (Matching section)

- [ ] **Step 1: Create the module with the failing tests**

`src/palette.rs`:

```rust
//! The Command Palette: a popup that runs any command, or opens any file
//! in the open folder, by typing part of its name.

/// How well `query` matches `text`, higher is better; `None` when it does
/// not match. Case-insensitive subsequence: every char of the query must
/// appear in the text, in order. The first occurrence of each char is
/// taken (greedy), which is good enough for short queries.
///
/// A char matched at the start of a word (the start of the text, or right
/// after `/`, `\`, `_`, `-`, `.` or a space) scores 3; one right after the
/// previous match scores 2; any other 1, minus one per text char skipped
/// since the previous match. Jumping to a word start costs nothing, so
/// `tc` prefers "Toggle Console" to "Watch".
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score: i64 = 0;
    let mut next = 0;
    let mut previous: Option<usize> = None;
    for (at, &c) in text.iter().enumerate() {
        if next == query.len() {
            break;
        }
        if c != query[next] {
            continue;
        }
        let word_start = at == 0 || matches!(text[at - 1], '/' | '\\' | '_' | '-' | '.' | ' ');
        let adjacent = previous.is_some_and(|p| p + 1 == at);
        score += if word_start {
            3
        } else if adjacent {
            2
        } else {
            let skipped = previous.map_or(0, |p| at - p - 1) as i64;
            1 - skipped
        };
        previous = Some(at);
        next += 1;
    }
    (next == query.len()).then(|| score.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_everything_with_zero() {
        assert_eq!(score("", ""), Some(0));
        assert_eq!(score("", "Toggle Console"), Some(0));
    }

    #[test]
    fn test_chars_out_of_order_or_missing_do_not_match() {
        assert_eq!(score("xyz", "Toggle"), None);
        assert_eq!(score("ct", "Toggle Console"), None);
        assert_eq!(score("togglee", "Toggle"), None);
    }

    #[test]
    fn test_word_starts_outrank_mid_word_matches() {
        assert!(score("tc", "Toggle Console") > score("tc", "Watch"));
        assert!(score("app", "src/app.rs") > score("app", "src/wrapper.rs"));
    }

    #[test]
    fn test_adjacent_chars_outrank_spread_ones() {
        assert!(score("app", "app") > score("app", "apxp"));
    }

    #[test]
    fn test_score_ignores_case() {
        assert_eq!(score("TC", "toggle console"), score("tc", "Toggle Console"));
    }

    #[test]
    fn test_score_never_goes_negative() {
        assert_eq!(score("ae", "axxxxxxxxxe"), Some(0));
    }
}
```

`src/main.rs`: add `mod palette;` between `mod modals;` and `mod prettify;`.

- [ ] **Step 2: Run the tests to see them fail to compile or fail**

Run: `cargo test palette::tests -- --nocapture`
Expected: all six tests PASS already if the implementation above is correct. If any fails, fix `score` before moving on — the tests define the contract.

- [ ] **Step 3: Align the spec with the scorer**

In the spec's **Matching** section replace

> Minus 1 per label char skipped between matches, floored at 0.

with

> +1 otherwise, minus one per label char skipped since the previous match; a jump to a word start costs nothing. Total floored at 0.

- [ ] **Step 4: Commit**

```bash
git add src/palette.rs src/main.rs docs/superpowers/specs/2026-11-03-command-palette-design.md
git commit -m "feat(palette): fuzzy scorer"
```

---

### Task 2: Palette state and ranking

**Files:**
- Modify: `src/palette.rs`

- [ ] **Step 1: Add the failing tests**

Append inside `mod tests`:

```rust
    fn items(labels: &[&str]) -> Vec<Item<usize>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| Item {
                label: l.to_string(),
                detail: String::new(),
                search: l.to_string(),
                payload: i,
            })
            .collect()
    }

    #[test]
    fn test_rank_keeps_original_order_for_empty_query_and_ties() {
        let items = items(&["b", "a", "c"]);
        assert_eq!(rank("", &items), vec![0, 1, 2]);
    }

    #[test]
    fn test_rank_sorts_by_score_then_original_order() {
        let items = items(&["Watch", "Toggle Console", "Toggle Comment", "Save"]);
        assert_eq!(rank("tc", &items), vec![1, 2, 0]);
    }

    #[test]
    fn test_open_same_mode_twice_toggles_closed() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        assert!(palette.visible && palette.focus);
        palette.open(PaletteMode::Commands);
        assert!(!palette.visible);
    }

    #[test]
    fn test_open_other_mode_switches_and_clears_query() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        palette.query = "tog".into();
        palette.selected = 2;
        palette.open(PaletteMode::Files);
        assert!(palette.visible);
        assert_eq!(palette.mode, PaletteMode::Files);
        assert_eq!(palette.query, "");
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn test_selection_moves_within_bounds() {
        let mut palette = Palette::default();
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        assert_eq!(palette.selected, 2);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        assert_eq!(palette.selected, 0);
        palette.selected = 5;
        palette.clamp_selection(2);
        assert_eq!(palette.selected, 1);
        palette.clamp_selection(0);
        assert_eq!(palette.selected, 0);
    }
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test palette::tests`
Expected: compile error — `Item`, `rank`, `Palette`, `PaletteMode` not found.

- [ ] **Step 3: Add the types**

Insert after the module doc comment, before `pub fn score`:

```rust
use std::path::PathBuf;

use crate::app::ShortcutAction;

/// What the palette lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteMode {
    /// Every editor command, run through `apply_shortcut`.
    #[default]
    Commands,
    /// Every file under the open folder.
    Files,
}

#[derive(Debug, Default)]
pub struct Palette {
    pub visible: bool,
    pub mode: PaletteMode,
    pub query: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// Index into the ranked list.
    selected: usize,
}

impl Palette {
    /// Opens in `mode` with an empty query. Opening again in the same mode
    /// closes; in the other mode switches and clears the query.
    pub fn open(&mut self, mode: PaletteMode) {
        if self.visible && self.mode == mode {
            self.visible = false;
            return;
        }
        *self = Self {
            visible: true,
            mode,
            focus: true,
            ..Self::default()
        };
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Moves the selection by `delta` rows within `count` rows, no wrap.
    fn move_selection(&mut self, delta: isize, count: usize) {
        let last = count.saturating_sub(1);
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
    }

    fn clamp_selection(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }
}

/// One row of the list. The app builds these each frame.
#[derive(Clone, Debug)]
pub struct Item<T> {
    pub label: String,
    /// Dimmed text at the right: the shortcut, or the folder of a file.
    pub detail: String,
    /// What the query is scored against: the label for a command, the path
    /// relative to the folder for a file (so `src/app` finds `src/app.rs`).
    pub search: String,
    pub payload: T,
}

/// What the user asked of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Run(ShortcutAction),
    Open(PathBuf),
    Close,
}

/// Indices of the items matching `query`, best first; ties keep their
/// original order.
pub fn rank<T>(query: &str, items: &[Item<T>]) -> Vec<usize> {
    let mut ranked: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score(query, &item.search).map(|s| (s, i)))
        .collect();
    ranked.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
    ranked.into_iter().map(|(_, i)| i).collect()
}
```

`sort_by_key` is stable, so equal scores keep index order.

- [ ] **Step 4: Run the tests**

Run: `cargo test palette::tests`
Expected: 11 passed. (`PaletteAction` is unused for now; a dead-code warning is fine until Task 6.)

- [ ] **Step 5: Commit**

```bash
git add src/palette.rs
git commit -m "feat(palette): state, items and ranking"
```

---

### Task 3: Shortcut actions and the command table

**Files:**
- Modify: `src/app.rs:441-515` (`shortcuts` module), `src/app.rs:518-556` (`ShortcutAction`), `src/app.rs:586-631` (`consume_shortcut`)
- Modify: `src/palette.rs`

- [ ] **Step 1: Add the failing table tests to `src/palette.rs`**

```rust
    #[test]
    fn test_commands_are_unique_and_leave_out_the_palette_itself() {
        let mut seen = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(seen.insert(command.action), "duplicate {:?}", command.action);
            assert!(
                !matches!(
                    command.action,
                    ShortcutAction::CommandPalette | ShortcutAction::QuickOpen
                ),
                "{:?} must not list itself",
                command.action
            );
        }
    }

    #[test]
    fn test_every_command_label_is_translated() {
        let i18n = crate::i18n::I18n::builtin();
        for command in COMMANDS {
            assert_ne!(i18n.t(command.key), command.key, "missing key {}", command.key);
        }
    }

    #[test]
    fn test_every_keyboard_action_is_a_command() {
        use crate::app::consume_shortcut;
        use eframe::egui::{Event, InputOptions, InputState, Key, Modifiers, RawInput};
        // Drive consume_shortcut with every key/modifier combination it
        // knows, and check each action it produces is in the table.
        let listed: std::collections::HashSet<ShortcutAction> =
            COMMANDS.iter().map(|c| c.action).collect();
        let keys = [
            Key::N, Key::W, Key::O, Key::S, Key::Q, Key::B, Key::G, Key::Tab, Key::F,
            Key::I, Key::F11, Key::V, Key::Backtick, Key::H, Key::F3, Key::ArrowUp,
            Key::ArrowDown, Key::K, Key::Slash, Key::D, Key::R, Key::F12, Key::P,
        ];
        let modifiers = [
            Modifiers::NONE,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::COMMAND,
            Modifiers::COMMAND.plus(Modifiers::SHIFT),
            Modifiers::COMMAND.plus(Modifiers::ALT),
            Modifiers::SHIFT.plus(Modifiers::ALT),
        ];
        for key in keys {
            for modifiers in modifiers {
                let raw = RawInput {
                    events: vec![Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                };
                let mut input =
                    InputState::default().begin_pass(raw, false, 1.0, InputOptions::default());
                if let Some(action) = consume_shortcut(&mut input)
                    && !matches!(action, ShortcutAction::CommandPalette | ShortcutAction::QuickOpen)
                {
                    assert!(listed.contains(&action), "{action:?} is not in COMMANDS");
                }
            }
        }
    }
```

(`InputState::begin_pass(self, new: RawInput, requested_immediate_repaint_prev_frame: bool, pixels_per_point: f32, options: InputOptions) -> Self` in egui 0.37; `Event::Key { key, physical_key, pressed, repeat, modifiers }`.)

- [ ] **Step 2: Run to see them fail**

Run: `cargo test palette::tests`
Expected: compile error — `COMMANDS`, `CommandPalette`, `QuickOpen` not found.

- [ ] **Step 3: Add shortcuts and actions in `src/app.rs`**

In `pub mod shortcuts`, after `pub const FIND_REFERENCES`:

```rust
    pub const COMMAND_PALETTE: KeyboardShortcut = cmd_shift(Key::P);
    pub const QUICK_OPEN: KeyboardShortcut = cmd(Key::P);
```

In `pub enum ShortcutAction`, after `SelectNextMatch,`:

```rust
    /// Open the Command Palette listing every command.
    CommandPalette,
    /// Open the Command Palette listing the folder's files.
    QuickOpen,
    /// Switch to this color theme (a Command Palette entry; no key).
    SelectTheme(ThemeId),
```

`ThemeId` is already imported (`use crate::theme::{self, ThemeId};`). It derives `Copy, Debug, PartialEq, Eq, Hash`, so `ShortcutAction` keeps its derives; add `Hash` to `ShortcutAction`'s derive so it can go in a `HashSet`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
```

In `impl ShortcutAction` (the block holding `needs_editor`), add:

```rust
    /// The key the action is bound to, as the menus print it; `None` for
    /// actions only the Command Palette offers.
    pub fn shortcut(self) -> Option<egui::KeyboardShortcut> {
        use ShortcutAction::*;
        Some(match self {
            NewTab => shortcuts::NEW_TAB,
            NewWindow => shortcuts::NEW_WINDOW,
            CloseTab => shortcuts::CLOSE_TAB,
            OpenFile => shortcuts::OPEN_FILE,
            OpenFolder => shortcuts::OPEN_FOLDER,
            Save => shortcuts::SAVE,
            SaveAs => shortcuts::SAVE_AS,
            Exit => shortcuts::EXIT,
            ToggleSidebar => shortcuts::TOGGLE_SIDEBAR,
            SourceControl => shortcuts::SOURCE_CONTROL,
            Blame => shortcuts::BLAME,
            GitGraph => shortcuts::GIT_GRAPH,
            NextTab => shortcuts::NEXT_TAB,
            PrevTab => shortcuts::PREV_TAB,
            GotoLine => shortcuts::GOTO_LINE,
            Prettify => shortcuts::PRETTIFY,
            ZenMode => shortcuts::ZEN_MODE,
            MarkdownPreview => shortcuts::MARKDOWN_PREVIEW,
            ToggleConsole => shortcuts::TOGGLE_CONSOLE,
            FindInFiles => shortcuts::FIND_IN_FILES,
            Find => shortcuts::FIND,
            Replace => shortcuts::REPLACE,
            FindNext => shortcuts::FIND_NEXT,
            FindPrev => shortcuts::FIND_PREV,
            PrevHeading => shortcuts::PREV_HEADING,
            NextHeading => shortcuts::NEXT_HEADING,
            GotoSymbol => shortcuts::GOTO_SYMBOL,
            GotoDefinition => shortcuts::GOTO_DEFINITION,
            FindReferences => shortcuts::FIND_REFERENCES,
            MoveLineUp => shortcuts::MOVE_LINE_UP,
            MoveLineDown => shortcuts::MOVE_LINE_DOWN,
            CopyLineUp => shortcuts::COPY_LINE_UP,
            CopyLineDown => shortcuts::COPY_LINE_DOWN,
            DeleteLine => shortcuts::DELETE_LINE,
            ToggleComment => shortcuts::TOGGLE_COMMENT,
            SelectNextMatch => shortcuts::SELECT_NEXT_MATCH,
            CommandPalette => shortcuts::COMMAND_PALETTE,
            QuickOpen => shortcuts::QUICK_OPEN,
            SelectTheme(_) => return None,
        })
    }
```

Make `needs_editor` reachable from `palette.rs` and the `show_palette` code: change `fn needs_editor(self)` to `pub fn needs_editor(self)`.

In `consume_shortcut`, the array is tried in order and Shift variants go first. Insert `(shortcuts::COMMAND_PALETTE, CommandPalette),` as the **first** entry, and `(shortcuts::QUICK_OPEN, QuickOpen),` as the **last** entry (after `(shortcuts::GOTO_DEFINITION, GotoDefinition),`).

- [ ] **Step 4: Add the command table to `src/palette.rs`**

After `PaletteAction`:

```rust
use crate::theme::ThemeId;

/// One command the palette can run.
pub struct Command {
    /// Translation key of the label. `cmd.theme` takes a `{name}`.
    pub key: &'static str,
    pub action: ShortcutAction,
}

const fn command(key: &'static str, action: ShortcutAction) -> Command {
    Command { key, action }
}

/// Every command, in menu order: File, Edit, View, Git, then the themes.
pub const COMMANDS: &[Command] = &[
    command("file.new_tab", ShortcutAction::NewTab),
    command("file.new_window", ShortcutAction::NewWindow),
    command("file.close_tab", ShortcutAction::CloseTab),
    command("file.open_file", ShortcutAction::OpenFile),
    command("file.open_folder", ShortcutAction::OpenFolder),
    command("file.save", ShortcutAction::Save),
    command("file.save_as", ShortcutAction::SaveAs),
    command("file.exit", ShortcutAction::Exit),
    command("edit.find", ShortcutAction::Find),
    command("edit.replace", ShortcutAction::Replace),
    command("cmd.find_next", ShortcutAction::FindNext),
    command("cmd.find_prev", ShortcutAction::FindPrev),
    command("edit.find_in_files", ShortcutAction::FindInFiles),
    command("edit.goto_line", ShortcutAction::GotoLine),
    command("edit.goto_symbol", ShortcutAction::GotoSymbol),
    command("edit.goto_definition", ShortcutAction::GotoDefinition),
    command("edit.find_references", ShortcutAction::FindReferences),
    command("edit.prettify", ShortcutAction::Prettify),
    command("edit.move_line_up", ShortcutAction::MoveLineUp),
    command("edit.move_line_down", ShortcutAction::MoveLineDown),
    command("edit.copy_line_up", ShortcutAction::CopyLineUp),
    command("edit.copy_line_down", ShortcutAction::CopyLineDown),
    command("edit.delete_line", ShortcutAction::DeleteLine),
    command("edit.toggle_comment", ShortcutAction::ToggleComment),
    command("edit.select_next_match", ShortcutAction::SelectNextMatch),
    command("cmd.prev_heading", ShortcutAction::PrevHeading),
    command("cmd.next_heading", ShortcutAction::NextHeading),
    command("cmd.toggle_sidebar", ShortcutAction::ToggleSidebar),
    command("cmd.next_tab", ShortcutAction::NextTab),
    command("cmd.prev_tab", ShortcutAction::PrevTab),
    command("view.zen_mode", ShortcutAction::ZenMode),
    command("view.markdown_preview", ShortcutAction::MarkdownPreview),
    command("view.console", ShortcutAction::ToggleConsole),
    command("view.source_control", ShortcutAction::SourceControl),
    command("view.blame", ShortcutAction::Blame),
    command("view.git_graph", ShortcutAction::GitGraph),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::System)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::LightModern)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::DarkModern)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::SolarizedLight)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::SolarizedDark)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::OceanDark)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::MochaDark)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::EightiesDark)),
];
```

Move the `use crate::theme::ThemeId;` line up next to the other `use` lines.

- [ ] **Step 5: Build and run the tests**

Run: `cargo build 2>&1 | grep -E '^(warning|error)' ; cargo test palette::tests`
Expected: `test_every_command_label_is_translated` FAILS on `cmd.find_next` (keys not yet added); the other two new tests pass. `apply_shortcut` does not compile yet because its `match` is not exhaustive — add these arms to the `match action` in `apply_shortcut` (next to `ShortcutAction::GotoSymbol => ...`), as placeholders completed in Task 7:

```rust
            ShortcutAction::CommandPalette | ShortcutAction::QuickOpen => {}
            ShortcutAction::SelectTheme(id) => self.theme = id,
```

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/palette.rs
git commit -m "feat(palette): command table and palette shortcuts"
```

---

### Task 4: Translations

**Files:**
- Modify: `assets/lang/en.json`, `assets/lang/id.json`, `assets/lang/es.json`, `assets/lang/de.json`, `assets/lang/zh.json`
- Modify: `src/i18n.rs:391-401` (scanner sources)

- [ ] **Step 1: Add the keys**

Each file ends with a last key and `}`. Add a comma after the current last value and append the block below before `}`.

`en.json`:
```json
  "view.command_palette": "Command Palette...",
  "file.quick_open": "Quick Open...",
  "palette.commands_hint": "Type a command name",
  "palette.files_hint": "Type a file name",
  "palette.no_results": "No matches",
  "palette.no_folder": "Open a folder to search its files",
  "palette.indexing": "Indexing files…",
  "cmd.theme": "Theme: {name}",
  "cmd.toggle_sidebar": "Toggle Sidebar",
  "cmd.next_tab": "Next Tab",
  "cmd.prev_tab": "Previous Tab",
  "cmd.find_next": "Find Next",
  "cmd.find_prev": "Find Previous",
  "cmd.prev_heading": "Previous Heading",
  "cmd.next_heading": "Next Heading"
```

`id.json`:
```json
  "view.command_palette": "Palet Perintah...",
  "file.quick_open": "Buka Cepat...",
  "palette.commands_hint": "Ketik nama perintah",
  "palette.files_hint": "Ketik nama file",
  "palette.no_results": "Tidak ada yang cocok",
  "palette.no_folder": "Buka folder untuk mencari file",
  "palette.indexing": "Mengindeks file…",
  "cmd.theme": "Tema: {name}",
  "cmd.toggle_sidebar": "Tampilkan/Sembunyikan Sidebar",
  "cmd.next_tab": "Tab Berikutnya",
  "cmd.prev_tab": "Tab Sebelumnya",
  "cmd.find_next": "Cari Berikutnya",
  "cmd.find_prev": "Cari Sebelumnya",
  "cmd.prev_heading": "Judul Sebelumnya",
  "cmd.next_heading": "Judul Berikutnya"
```

`es.json`:
```json
  "view.command_palette": "Paleta de comandos...",
  "file.quick_open": "Apertura rápida...",
  "palette.commands_hint": "Escribe el nombre de un comando",
  "palette.files_hint": "Escribe el nombre de un archivo",
  "palette.no_results": "Sin coincidencias",
  "palette.no_folder": "Abre una carpeta para buscar sus archivos",
  "palette.indexing": "Indexando archivos…",
  "cmd.theme": "Tema: {name}",
  "cmd.toggle_sidebar": "Alternar barra lateral",
  "cmd.next_tab": "Pestaña siguiente",
  "cmd.prev_tab": "Pestaña anterior",
  "cmd.find_next": "Buscar siguiente",
  "cmd.find_prev": "Buscar anterior",
  "cmd.prev_heading": "Encabezado anterior",
  "cmd.next_heading": "Encabezado siguiente"
```

`de.json`:
```json
  "view.command_palette": "Befehlspalette...",
  "file.quick_open": "Schnell öffnen...",
  "palette.commands_hint": "Befehlsnamen eingeben",
  "palette.files_hint": "Dateinamen eingeben",
  "palette.no_results": "Keine Treffer",
  "palette.no_folder": "Ordner öffnen, um Dateien zu suchen",
  "palette.indexing": "Dateien werden indiziert…",
  "cmd.theme": "Design: {name}",
  "cmd.toggle_sidebar": "Seitenleiste umschalten",
  "cmd.next_tab": "Nächster Tab",
  "cmd.prev_tab": "Vorheriger Tab",
  "cmd.find_next": "Weitersuchen",
  "cmd.find_prev": "Rückwärts suchen",
  "cmd.prev_heading": "Vorherige Überschrift",
  "cmd.next_heading": "Nächste Überschrift"
```

`zh.json`:
```json
  "view.command_palette": "命令面板...",
  "file.quick_open": "快速打开...",
  "palette.commands_hint": "输入命令名称",
  "palette.files_hint": "输入文件名",
  "palette.no_results": "无匹配项",
  "palette.no_folder": "打开文件夹以搜索文件",
  "palette.indexing": "正在索引文件…",
  "cmd.theme": "主题：{name}",
  "cmd.toggle_sidebar": "切换侧边栏",
  "cmd.next_tab": "下一个标签页",
  "cmd.prev_tab": "上一个标签页",
  "cmd.find_next": "查找下一个",
  "cmd.find_prev": "查找上一个",
  "cmd.prev_heading": "上一个标题",
  "cmd.next_heading": "下一个标题"
```

- [ ] **Step 2: Add `palette.rs` to the key scanner**

In `src/i18n.rs`, `test_every_key_used_in_code_exists`, add `include_str!("palette.rs"),` after `include_str!("about.rs"),`.

- [ ] **Step 3: Run the i18n and palette tests**

Run: `cargo test i18n:: palette::`
Expected: all pass, including `test_builtin_languages_have_the_same_keys` (every language has every key) and `test_every_command_label_is_translated`. A JSON syntax slip (missing comma) shows as a panic in `builtin`; fix the file.

- [ ] **Step 4: Commit**

```bash
git add assets/lang/en.json assets/lang/id.json assets/lang/es.json assets/lang/de.json assets/lang/zh.json src/i18n.rs
git commit -m "feat(palette): translations"
```

---

### Task 5: Explorer exposes its file index

**Files:**
- Modify: `src/file_browser.rs` (next to `update_matches`, around line 316)

- [ ] **Step 1: Add the failing test**

In `file_browser.rs`'s `mod tests`, next to the existing index tests (search for `explorer.index = Some(FileIndex::spawn`), add:

```rust
    #[test]
    fn test_indexed_files_starts_the_index_and_reports_when_done() {
        let dir = std::env::temp_dir().join("rred_test_indexed_files");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join("b.rs")).unwrap();
        File::create(dir.join("a.rs")).unwrap();
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(dir.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (_, done) = explorer.indexed_files(&ctx);
            if done {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "index never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (files, done) = explorer.indexed_files(&ctx);
        assert!(done);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        let _ = fs::remove_dir_all(&dir);
    }
```

This follows the temp-folder pattern of `test_scan_directory_*` in the same module (`std::env::temp_dir().join("rred_test_…")`, `fs` and `File` are already imported there); `Explorer::new(root: PathBuf)` is the constructor the other tests use.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test file_browser::tests::test_indexed_files`
Expected: compile error — no method `indexed_files`.

- [ ] **Step 3: Add the method**

In `impl Explorer`, right before `pub fn update_matches`:

```rust
    /// Every file under the root found so far, and whether the walk is
    /// done. Starts the background index the first time; never waits for
    /// it (`ctx` is woken as more files arrive).
    pub fn indexed_files(&mut self, ctx: &egui::Context) -> (&[FsEntry], bool) {
        let index = self
            .index
            .get_or_insert_with(|| FileIndex::start(ctx, self.root.clone()));
        index.poll();
        (&index.files, index.is_done())
    }
```

`update_matches` keeps its own `get_or_insert_with` + `poll`: it needs `index` and `self.found` borrowed at once, which a call through `self.indexed_files` would not allow. Both share the one `self.index`, so there is still a single index per folder.

- [ ] **Step 4: Run the test**

Run: `cargo test file_browser::tests::test_indexed_files`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/file_browser.rs
git commit -m "feat(explorer): expose the background file index"
```

---

### Task 6: The popup widget

**Files:**
- Modify: `src/palette.rs`

No unit test: it is egui drawing, checked by hand in Task 8 like the other panels.

- [ ] **Step 1: Add the widget**

Add to the `use` lines:

```rust
use eframe::egui::{self, Align2, CornerRadius, Key, Modifiers, Sense, pos2, vec2};

use crate::file_browser::ROW_HEIGHT;
use crate::i18n::I18n;
use crate::theme;
```

Append after `rank`:

```rust
/// Width of the popup.
const WIDTH: f32 = 520.0;
/// Rows of the list shown at once.
const VISIBLE_ROWS: f32 = 12.0;
const PADDING: f32 = 8.0;

/// Draws the popup at the top center of the window and returns what the
/// user asked. `status` is a dimmed line under the list ("Indexing…").
/// Up and Down move the selection, Enter picks it, Escape or a click
/// outside closes. `to_action` turns the picked item's payload into the
/// action to return.
pub fn show<T>(
    ctx: &egui::Context,
    state: &mut Palette,
    items: &[Item<T>],
    status: Option<&str>,
    i18n: &I18n,
    to_action: impl Fn(&T) -> PaletteAction,
) -> Option<PaletteAction> {
    let palette = theme::active(ctx).sidebar;
    let ranked = rank(&state.query, items);
    state.clamp_selection(ranked.len());
    let mut action = None;

    let field_id = egui::Id::unique("command_palette_query");
    // Focused before it draws, so its key filter holds from the first frame.
    if std::mem::take(&mut state.focus) {
        ctx.memory_mut(|m| m.request_focus(field_id));
    }
    // Taken before the field draws, which would move its caret.
    if ctx.memory(|m| m.has_focus(field_id)) {
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            state.move_selection(-1, ranked.len());
        }
        if down {
            state.move_selection(1, ranked.len());
        }
        if enter && let Some(&index) = ranked.get(state.selected) {
            action = Some(to_action(&items[index].payload));
        }
        if escape {
            action = Some(PaletteAction::Close);
        }
    }

    let mut frame = egui::Frame::popup(&ctx.style());
    frame.fill = palette.background;
    frame.stroke = egui::Stroke::new(1.0, palette.border);
    let area = egui::Area::new(egui::Id::unique("command_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    if state.mode == PaletteMode::Commands {
                        ui.label(
                            egui::RichText::new(">")
                                .monospace()
                                .color(palette.icon_inactive),
                        );
                    }
                    let hint = match state.mode {
                        PaletteMode::Commands => "palette.commands_hint",
                        PaletteMode::Files => "palette.files_hint",
                    };
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut state.query)
                            .id(field_id)
                            // Up and Down pick a row instead of moving focus.
                            .event_filter(egui::EventFilter {
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                ..Default::default()
                            })
                            .hint_text(i18n.t(hint))
                            .desired_width(f32::INFINITY),
                    );
                    if field.changed() {
                        state.selected = 0;
                    }
                });
                ui.separator();
                if ranked.is_empty() {
                    ui.weak(i18n.t("palette.no_results"));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(ROW_HEIGHT * VISIBLE_ROWS)
                        .auto_shrink([false, true])
                        .show_rows(ui, ROW_HEIGHT, ranked.len(), |ui, rows| {
                            for row in rows {
                                let item = &items[ranked[row]];
                                let selected = row == state.selected;
                                let (rect, response) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), ROW_HEIGHT),
                                    Sense::click(),
                                );
                                if selected {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(3),
                                        palette.row_selected,
                                    );
                                } else if response.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(3),
                                        palette.row_hover,
                                    );
                                }
                                let font = egui::TextStyle::Body.resolve(ui.style());
                                let detail = truncated(
                                    ui,
                                    &item.detail,
                                    font.clone(),
                                    palette.icon_inactive,
                                    rect.width() * 0.5,
                                );
                                let label = truncated(
                                    ui,
                                    &item.label,
                                    font,
                                    palette.foreground,
                                    rect.width() - detail.size().x - 3.0 * PADDING,
                                );
                                let y = rect.center().y;
                                ui.painter().galley(
                                    pos2(rect.left() + PADDING, y - label.size().y / 2.0),
                                    label,
                                    palette.foreground,
                                );
                                ui.painter().galley(
                                    pos2(
                                        rect.right() - PADDING - detail.size().x,
                                        y - detail.size().y / 2.0,
                                    ),
                                    detail,
                                    palette.icon_inactive,
                                );
                                if selected {
                                    ui.scroll_to_rect(rect, None);
                                }
                                if response.clicked() {
                                    action = Some(to_action(&item.payload));
                                }
                            }
                        });
                }
                if let Some(status) = status {
                    ui.weak(status);
                }
            });
        });
    if action.is_none() && area.response.clicked_elsewhere() {
        action = Some(PaletteAction::Close);
    }
    action
}

/// `text` laid out on one line, cut with `…` to fit `max_width`.
fn truncated(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | grep -E '^(warning|error)' -A5`
Expected: no errors (`Painter::layout_job`, `Painter::galley`, `TextWrapping::truncate_at_width`, `Frame::popup(&Style)`, `Response::clicked_elsewhere` and `Ui::scroll_to_rect` all exist in egui/epaint 0.37). Dead-code warnings for `show` are fine until Task 7.

- [ ] **Step 3: Commit**

```bash
git add src/palette.rs
git commit -m "feat(palette): popup widget"
```

---

### Task 7: Wire the palette into the app

**Files:**
- Modify: `src/app.rs` — struct and `Default` (~line 300-390), `apply_shortcut` (~line 1614), `show_view_menu` (~line 757), `show_file_menu` (~line 1575), `update` after the `CentralPanel` (~line 3470)

- [ ] **Step 1: Import and state**

Add to the imports: `use crate::palette::{self, Item, Palette, PaletteAction, PaletteMode};`

In `pub struct TextEditorApp`, after `pub git_log: GitLogPanel,`:

```rust
    /// The Command Palette / Quick Open popup.
    pub palette: Palette,
```

In `impl Default for TextEditorApp`, after `git_log: GitLogPanel::default(),`:

```rust
            palette: Palette::default(),
```

- [ ] **Step 2: Open and dispatch**

Replace the placeholder arm from Task 3 in `apply_shortcut`:

```rust
            ShortcutAction::CommandPalette => self.open_palette(PaletteMode::Commands),
            ShortcutAction::QuickOpen => self.open_palette(PaletteMode::Files),
            ShortcutAction::SelectTheme(id) => self.theme = id,
```

Add to `impl TextEditorApp`, right after `open_symbol_picker`:

```rust
    /// Opens the Command Palette in `mode`, or closes it when already open
    /// in that mode. Takes over from the other bars that read the keys.
    pub fn open_palette(&mut self, mode: PaletteMode) {
        self.palette.open(mode);
        if self.palette.visible {
            self.symbol_picker.visible = false;
            self.goto.visible = false;
        } else {
            self.editor_request.focus = true;
        }
    }

    /// Draws the palette when open, with this frame's commands or files,
    /// and acts on what the user picked.
    fn show_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.visible {
            return;
        }
        let action = match self.palette.mode {
            PaletteMode::Commands => {
                let editor_hidden = self.git_graph.is_some() || self.show_welcome;
                let items: Vec<Item<ShortcutAction>> = palette::COMMANDS
                    .iter()
                    .filter(|c| !(editor_hidden && c.action.needs_editor()))
                    .map(|c| {
                        let label = match c.action {
                            ShortcutAction::SelectTheme(id) => {
                                let name = id.name().unwrap_or_else(|| self.i18n.t("theme.system"));
                                self.i18n.tf(c.key, &[("name", name)])
                            }
                            _ => self.i18n.t(c.key).to_owned(),
                        };
                        let detail = c
                            .action
                            .shortcut()
                            .map(|s| shortcuts::label(ctx, &s))
                            .unwrap_or_default();
                        Item { search: label.clone(), label, detail, payload: c.action }
                    })
                    .collect();
                palette::show(ctx, &mut self.palette, &items, None, &self.i18n, |a| {
                    PaletteAction::Run(*a)
                })
            }
            PaletteMode::Files => {
                let (items, status): (Vec<Item<PathBuf>>, Option<&str>) = match self.explorer.as_mut() {
                    None => (Vec::new(), Some(self.i18n.t("palette.no_folder"))),
                    Some(explorer) => {
                        let root = explorer.root().to_path_buf();
                        let (files, done) = explorer.indexed_files(ctx);
                        let items = files
                            .iter()
                            .map(|f| {
                                let relative = f.path.strip_prefix(&root).unwrap_or(&f.path);
                                let folder = relative
                                    .parent()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default();
                                Item {
                                    label: f.name.clone(),
                                    detail: folder,
                                    search: relative.display().to_string(),
                                    payload: f.path.clone(),
                                }
                            })
                            .collect();
                        (items, (!done).then(|| self.i18n.t("palette.indexing")))
                    }
                };
                palette::show(ctx, &mut self.palette, &items, status, &self.i18n, |p| {
                    PaletteAction::Open(p.clone())
                })
            }
        };
        match action {
            Some(PaletteAction::Run(action)) => {
                self.palette.close();
                self.apply_shortcut(ctx, action);
            }
            Some(PaletteAction::Open(path)) => {
                self.palette.close();
                self.open_file_in_tab(path);
            }
            Some(PaletteAction::Close) => {
                self.palette.close();
                self.editor_request.focus = true;
            }
            None => {}
        }
    }
```

If the borrow checker rejects `self.i18n` being read while `self.explorer` is borrowed mutably (they are disjoint fields, so it should compile), hoist `let no_folder = self.i18n.t("palette.no_folder");` and `let indexing = self.i18n.t("palette.indexing");` to `let`-bound `&str`s before the `match`.

- [ ] **Step 3: Draw it each frame**

In `update` (the `impl eframe::App` block), right after the `egui::CentralPanel::default().frame(editor_frame).show(ui, |ui| { ... });` call ends (search for `welcome_action` handling just below it), add:

```rust
        self.show_palette(&ctx);
```

It must run after the Explorer sidebar is drawn (it is: the sidebar is a `SidePanel` shown before the `CentralPanel`), so Quick Open sees this frame's index.

- [ ] **Step 4: Menu items**

In `show_view_menu`, after the Git Log button and before `self.show_theme_menu(ui);`:

```rust
        if menu_item(
            ui,
            self.i18n.t("view.command_palette"),
            &shortcuts::label(&ctx, &shortcuts::COMMAND_PALETTE),
        ) {
            self.open_palette(PaletteMode::Commands);
        }
```

In `show_file_menu`, after the `file.open_folder` item and before `self.show_recent_menu(ui);`:

```rust
        if item(ui, self.i18n.t("file.quick_open"), &shortcuts::QUICK_OPEN) {
            self.open_palette(PaletteMode::Files);
        }
```

- [ ] **Step 5: Build and run everything**

Run: `cargo build 2>&1 | grep -E '^(warning|error)' -A5; cargo test 2>&1 | grep -E '^test result|FAILED|panicked'`
Expected: no warnings, no errors; `test result: ok.` with the baseline count from "Before starting" plus 15 new tests (6 scorer, 5 state, 3 table, 1 explorer).

- [ ] **Step 6: Commit**

```bash
git add src/app.rs
git commit -m "feat(palette): command palette and quick open"
```

---

### Task 8: Check by hand, document, finish

**Files:**
- Modify: `README.md` (Features list, after the **Find in Files** bullet), `README.id.md` (same place)

- [ ] **Step 1: Run the app and check each behaviour**

Run: `cargo run` and open this repository's folder (File → Open Folder…). Check:

1. `Ctrl+Shift+P` opens the popup at the top center with `>` and the hint; the field has focus at once. Typing `tc` lists "Console" (Toggle Console) before others; `Down`, `Up` move the highlight; `Enter` toggles the console and closes the popup; the editor has focus again.
2. `Ctrl+Shift+P` twice: opens then closes. `Esc` closes. Clicking the editor outside the popup closes.
3. `Ctrl+P` opens Files mode with no `>`; "Indexing files…" shows briefly on a big folder; typing `src/app` ranks `app.rs` in `src` first; `Enter` opens it in a tab.
4. `Ctrl+P` while in Commands mode switches to Files mode with an empty query, and back.
5. Type `theme`: eight "Theme: …" rows; picking one recolors the UI at once.
6. From the welcome page (close all tabs): `Ctrl+Shift+P` lists no Find/Prettify/Move Line commands; from an open file it does.
7. Without a folder: `Ctrl+P` shows "Open a folder to search its files" and no rows.
8. View → Command Palette… and File → Quick Open… open the right mode.
9. Long file names and deep folders are cut with `…` and never overlap.
10. Switch the interface language (View → Interface Language): labels and hints follow.

Fix anything that fails before moving on, re-running `cargo test` after each fix.

- [ ] **Step 2: README bullets**

`README.md`, after the **Find in Files** bullet:

```markdown
- **Command Palette** (`Ctrl+Shift+P`) and **Quick Open** (`Ctrl+P`): a popup at the top of the window. Type part of a command name to run it, or part of a file name or path (`src/app`) to open that file from the opened folder. `Up`/`Down` pick, `Enter` runs, `Esc` closes. Also in **View → Command Palette…** and **File → Quick Open…**.
```

`README.id.md`, same place:

```markdown
- **Palet Perintah** (`Ctrl+Shift+P`) dan **Buka Cepat** (`Ctrl+P`): popup di bagian atas jendela. Ketik sebagian nama perintah untuk menjalankannya, atau sebagian nama atau path file (`src/app`) untuk membuka file itu dari folder yang terbuka. `Up`/`Down` memilih, `Enter` menjalankan, `Esc` menutup. Juga ada di **View → Palet Perintah…** dan **File → Buka Cepat…**.
```

- [ ] **Step 3: Final test run and commit**

Run: `cargo test 2>&1 | grep -E '^test result|FAILED'`
Expected: `test result: ok.`

```bash
git add README.md README.id.md
git commit -m "docs: command palette and quick open"
```

- [ ] **Step 4: Hand back**

Report: commits made, test count, anything from the hand check that was fixed or left open.
