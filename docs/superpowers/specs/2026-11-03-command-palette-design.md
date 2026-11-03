# Command Palette — design

A popup that runs any editor command by name (`Ctrl+Shift+P`) or opens any
file in the open folder by name (`Ctrl+P`). One widget, two modes.

## Scope

- **Commands mode**: every action that has a `ShortcutAction`, plus one
  entry per color theme. Typing filters the list; Enter runs the selected
  command through the existing `apply_shortcut` dispatch.
- **Files mode**: every file under the open folder, from the Explorer's
  existing background `FileIndex`. Enter opens the selected file in a tab.
- A floating popup at the top center of the window, VS Code style, closed by
  Escape, by a click outside it, or by running an item.
- Menu entries **View → Command Palette…** and **File → Quick Open…**.

Out of scope for v1: interface language entries, Open Recent and Clear
Recent as commands, Go to Symbol inside the palette (`@` prefix), recently
used ordering, a `>` prefix that switches modes from the query text.

## Module (`src/palette.rs`, new)

```rust
pub enum PaletteMode { Commands, Files }

pub struct Palette {
    pub visible: bool,
    pub mode: PaletteMode,
    pub query: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// Index into the filtered list.
    selected: usize,
}

impl Palette {
    /// Opens in `mode` with an empty query. Reopening in the other mode
    /// switches mode and clears the query; reopening in the same mode
    /// closes (toggle).
    pub fn open(&mut self, mode: PaletteMode);
}

/// One command the palette can run.
pub struct Command {
    /// i18n key of the label, e.g. `"cmd.toggle_console"`.
    pub key: &'static str,
    pub action: ShortcutAction,
}

/// Every command, in menu order: File, Edit, View, Git, then Theme: *.
pub const COMMANDS: &[Command];

/// One row of the list, built by the app each frame.
pub struct Item<T> {
    pub label: String,
    /// Dimmed text at the right: the shortcut, or the folder of a file.
    pub detail: String,
    pub payload: T,
}

pub enum PaletteAction {
    Run(ShortcutAction),
    Open(PathBuf),
    Close,
}

/// Draws the popup. `status` is an optional dimmed line under the list
/// ("Indexing…", "Open a folder first").
pub fn show<T: Clone>(
    ctx: &egui::Context,
    state: &mut Palette,
    items: &[Item<T>],
    status: Option<&str>,
    i18n: &I18n,
    to_action: impl Fn(&T) -> PaletteAction,
) -> Option<PaletteAction>;
```

`show` is generic over the payload so the module never depends on how the
app builds its items; the app passes `ShortcutAction` items in Commands mode
and `PathBuf` items in Files mode.

## Matching

`pub fn score(query: &str, label: &str) -> Option<u32>`, pure, in
`palette.rs`:

- Case-insensitive subsequence: every `char` of the query must appear in
  the label in order. No match → `None`. Empty query → `Some(0)` for every
  label, so the list keeps its original order.
- Score, higher is better: +3 for a query char that matches at the start of
  the label or right after `/`, `\`, `_`, `-`, `.` or a space; +2 for a char
  adjacent to the previous match; +1 otherwise. Minus 1 per label char
  skipped between matches, floored at 0.
- The list is the items with `Some` score, sorted by score descending, then
  by their original index (stable sort), so ties keep menu or index order.

Files mode scores against the path relative to the folder root (so
`src/app` finds `src/app.rs`), with the file name as the label and the
parent folder as the detail.

## UI and keyboard

- `egui::Area` with `Order::Foreground`, anchored `Align2::CENTER_TOP` at
  `(0, 60)`, fixed width 520, inside `Frame::popup` recolored from
  `SidebarPalette` (`background`, `border`).
- Row 1: a mode marker (`>` in Commands mode, nothing in Files mode) and a
  `TextEdit::singleline` for the query with the hint `palette.commands_hint`
  / `palette.files_hint`. The `>` is painted, not part of the query.
- A 1px `border` line, then a `ScrollArea` of at most 12 rows of
  `ROW_HEIGHT` (same constant as the Explorer). Each row: `label`
  (`foreground`) and `detail` right-aligned (`icon_inactive`). The selected
  row is filled `row_selected`; a hovered row `row_hover`; the selected row
  is scrolled into view when the selection moves.
- Empty list: one dimmed row `palette.no_results`. `status`, when given, is
  one dimmed row under the list.
- Keys, consumed before the `TextEdit` draws so it never sees them, as in
  `symbols::show_picker`: Up/Down move `selected` (clamped, no wrap);
  Enter → action for the selected item; Escape → `Close`.
  Clicking a row → its action. A click outside the area → `Close`.
- `selected` resets to 0 whenever the query changes, and is clamped to the
  list length every frame.
- Focus: `state.focus` requests focus on the field on the frame it opens;
  while the popup is visible the editor does not get its focus requests.

## App integration (`src/app.rs`)

- `ShortcutAction::CommandPalette` on `Ctrl+Shift+P` and
  `ShortcutAction::QuickOpen` on `Ctrl+P`; both are free today. They are
  the only actions missing from `COMMANDS`.
- `ShortcutAction::SelectTheme(ThemeId)`: sets `self.theme`, exactly what
  `show_theme_menu` does. Eight `COMMANDS` entries, labelled
  `"Theme: <name>"` through `cmd.theme` with the theme name as a parameter.
- `TextEditorApp.palette: Palette`. `CommandPalette` / `QuickOpen` call
  `self.palette.open(mode)` and also set `self.symbol_picker.visible` and
  `self.goto.visible` to `false`.
- Menu items `view.command_palette` (View, next to Go to Symbol) and
  `file.quick_open` (File, under Open Folder), using `menu_item` with the
  shortcut text.
- Each frame, when `self.palette.visible`, the app builds the items:
  - Commands: `COMMANDS` mapped to `Item { label: i18n.t(key), detail:
    shortcut text, payload: action }`. Shortcut text comes from a new
    `ShortcutAction::shortcut(self) -> Option<KeyboardShortcut>` (the
    `shortcuts::*` constant each action is bound to, `None` for
    `SelectTheme`) formatted with the existing `shortcuts::label`, so it
    matches the menus. Entries whose `needs_editor()` is true are left out
    while the Git Graph or the welcome page hides the editor, mirroring
    the guard in `apply_shortcut`.
  - Files: `self.explorer.indexed_files(ctx)` mapped to `Item { label: file
    name, detail: parent folder relative to root, payload: path }`. The
    palette is drawn after the Explorer so the index is already updated for
    this frame. No folder open → empty items with status `palette.no_folder`.
    Index not done → status `palette.indexing`.
- `PaletteAction::Run(a)` → close the palette, then `self.apply_shortcut(ctx, a)`.
  `Open(p)` → close, then `self.open_file_in_tab(p)`. `Close` → close and
  `self.editor_request.focus = true`. Close means `visible = false`.
- `Explorer` gains `pub fn indexed_files(&mut self, ctx: &egui::Context)
  -> (&[FsEntry], bool)`: starts the `FileIndex` if it has not started,
  drains any finished batches, returns the files so far and whether the
  walk is done. The existing filter code switches to the same method so the
  index has one owner.

Known limit: the index only starts on first use, so in a big project the
file list fills over the first seconds Quick Open is open; the
`palette.indexing` status says so.

## i18n

New keys in all five language files (`en`, `id`, `es`, `de`, `zh`):
`view.command_palette`, `file.quick_open`, `palette.commands_hint`
("Type a command name"), `palette.files_hint` ("Type a file name"),
`palette.no_results`, `palette.no_folder`, `palette.indexing`,
`cmd.theme` ("Theme: {name}"), and one `cmd.*` key per `COMMANDS` entry
whose label differs from its menu key; entries whose menu text already
reads well reuse the menu key. The existing i18n tests enforce that every
language has every key.

## Tests (`palette.rs`, unit)

- `score`: empty query matches everything with 0; non-subsequence → `None`;
  word-start match outranks mid-word match (`"tc"` ranks "Toggle Console"
  over "Watch"); adjacent chars outrank spread ones; case-insensitive.
- Ordering: ties keep original order.
- `COMMANDS` holds no duplicate `action` and no `CommandPalette` /
  `QuickOpen`; every `key` exists in `en.json`; every `ShortcutAction`
  that `consume_shortcut` can produce, other than those two, is listed.
- `Palette::open`: same mode twice toggles closed; other mode switches and
  clears the query.
- Selection: clamped when the list shrinks below `selected`; reset to 0 on
  query change.

Widget drawing is checked by hand in the running app, as for the other
panels.
