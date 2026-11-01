# Welcome page — design

A start page in place of the empty editor: the editor opens on it, and it
comes back when the last tab is closed.

## Scope

- A **welcome page** drawn in the central area instead of the tab bar and
  editor: title, three buttons (New File, Open File…, Open Folder…), and
  the recently opened folders and files from `History`.
- Shown at startup, and again
  whenever the last tab is closed.
- Dismissed by anything that puts a document in a tab or opens a folder.

Out of scope: a "show on startup" setting, tips, project templates.

## State

- `TextEditorApp.show_welcome: bool`. `Default` sets it to `true`. The app
  takes no file on the command line, so the page always shows at launch.
- The single untitled tab stays in `tabs` underneath, so every
  `active_tab()` caller is unchanged.
- Set to `true` in `close_tab` when the last tab is closed (the branch that
  replaces `tabs[0]` with `DocumentTab::default_new()`).
- Set to `false` in `open_new_tab`, `open_file_in_tab` and the folder-open
  path (`open_folder` or whatever `trigger_open_folder` resolves to), and
  in `open_recent`. Opening the Git Graph does not clear it; the graph
  takes precedence while open and the welcome page returns when it closes.

## View (`src/welcome.rs`, new)

- `pub enum WelcomeAction { NewFile, OpenFile, OpenFolder, Open(Entry) }`.
- `pub fn show(ui, history: &History, i18n: &I18n) -> Option<WelcomeAction>`.
- Layout: the editor fill as background (`theme.tab.tab_active`); a column
  of at most 520 px, horizontally centred, vertically centred when the
  content is shorter than the view (otherwise top-aligned with 48 px
  padding, scrollable).
  - Title "Red Text Editor" (22 pt, foreground) with the crate version
    dimmed beside it.
  - Three buttons in a row, each `min_size(150 × 32)`: label plus the
    shortcut in the dimmed colour (`Ctrl+N`, `Ctrl+O`, `Ctrl+Shift+O`
    via `ctx.format_shortcut`).
  - Two groups, side by side when the column is wider than 440 px, else
    stacked: **Recent Folders** and **Recent Files** (11 pt uppercase
    dimmed heading), each the first 8 entries of that kind from
    `history.entries()` (already newest first). Row: name in the
    foreground colour, parent folder dimmed after it, elided to the group
    width; hover shows the full path; click → `Open(entry)`. A group with
    no entries shows `file.no_recent` dimmed.
- The page hides the tab bar, find/goto bars and the editor. Sidebar,
  menus, status bar and console are unchanged.

## App wiring (`app.rs`)

- Central panel: before `render_tab_bar`, `if self.show_welcome &&
  self.git_graph.is_none() { welcome_action = welcome::show(ui,
  &self.history, &self.i18n); return; }` (same early-return pattern as the
  graph). Actions after the panel: `NewFile → open_new_tab`, `OpenFile →
  trigger_open_file`, `OpenFolder → trigger_open_folder`, `Open(entry) →
  open_recent(entry)`.
- Shortcuts keep working: `Ctrl+N/O/Shift+O` go through the same functions
  and clear the flag. `needs_editor` shortcuts are ignored while the page
  shows (reuse the Git Graph guard: `git_graph.is_some() || show_welcome`).
- `previewing()` is false while the page shows; `editor_request` is
  reset the same way as on graph close when the page is dismissed.

## i18n

`welcome.new_file` ("New File"), `welcome.open_file` ("Open File…"),
`welcome.open_folder` ("Open Folder…"), `welcome.recent_folders`
("Recent Folders"), `welcome.recent_files` ("Recent Files"); reuse
`file.no_recent`. Five languages.

## Tests

- `welcome.rs` headless: with two folder and two file entries the page
  paints the three button labels, both headings and the entry names;
  clicking New File returns `NewFile`; clicking a recent name returns
  `Open(entry)`; empty history paints `file.no_recent`.
- `app.rs`: `TextEditorApp::default().show_welcome` is true; `close_tab`
  on the only tab sets it; `open_new_tab` and `open_file_in_tab` clear it;
  `apply_shortcut(Find)` is ignored while it shows.

## README

One bullet under Usage in both READMEs: the start page, its buttons and
recent lists, and that closing the last tab brings it back.
