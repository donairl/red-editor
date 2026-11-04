# Explorer file operations — design

Right-click the Explorer to create, rename, delete and reveal files and
folders, VS Code style. Names are typed inline in the tree; deleted entries
go to the system Trash.

## Scope

- **New File**, **New Folder**, **Rename**, **Delete**, **Reveal in Folder**
  from the Explorer's right-click menus.
- Inline name editing in the tree: Enter or a click elsewhere commits,
  Escape cancels.
- Delete asks first, then moves the entry to the Trash / Recycle Bin.
- Open tabs follow a rename or move and close on delete (see *Open tabs*).

Out of scope for v1: keyboard shortcuts (F2, Delete) — the Explorer has no
selected row yet; New File / New Folder buttons in the section header;
nested names (`a/b.rs` creating folder `a`); drag-and-drop moves; copy,
cut and paste; undo of a delete other than restoring from the Trash.

## Menus

| Where | Entries |
|---|---|
| File row (tree) | New File, New Folder, —, Rename, Delete, —, Reveal in Folder, File History |
| Folder row (tree) | New File, New Folder, —, Rename, Delete, —, Reveal in Folder |
| Empty space below the tree | New File, New Folder |
| Match row (filter active) | Rename, Delete, —, Reveal in Folder, File History |

- On a file row, New File / New Folder create in the file's folder; on a
  folder row, inside that folder, which is expanded; below the tree, in the
  root.
- The flat match list has no obvious target folder, so it offers no New
  entries.
- File History stays where it is today; only its position in the menu moves.

## Pure file-system layer (`src/file_ops.rs`, new)

No egui, no app state; every function is tested against a temp folder.

```rust
/// Why a typed name cannot be used.
pub enum NameError { Empty, Reserved, Separator, Exists }

/// Checks `name` for a new entry in `dir`, or for renaming `current`
/// (`Some`) inside `dir`. Trims surrounding whitespace first.
/// - `Empty`: nothing left after trimming.
/// - `Reserved`: `.` or `..`.
/// - `Separator`: holds `/` or `\`.
/// - `Exists`: `dir/name` exists and is not `current` itself (a rename
///   that only changes letter case is allowed).
pub fn validate_name(dir: &Path, name: &str, current: Option<&Path>) -> Result<String, NameError>;

/// Creates an empty file; fails if it exists (`create_new`).
pub fn create_file(dir: &Path, name: &str) -> io::Result<PathBuf>;
pub fn create_dir(dir: &Path, name: &str) -> io::Result<PathBuf>;
/// Renames `from` to `new_name` in the same folder; returns the new path.
pub fn rename(from: &Path, new_name: &str) -> io::Result<PathBuf>;
/// Moves `path` (file or folder) to the Trash via the `trash` crate.
pub fn move_to_trash(path: &Path) -> io::Result<()>;
/// Shows `path` in the system file manager:
/// Linux `xdg-open <parent>`, macOS `open -R <path>`,
/// Windows `explorer /select,<path>`. Spawned, never waited on.
pub fn reveal(path: &Path) -> io::Result<()>;
/// `path` with its `from` prefix swapped for `to`, if it lies under `from`
/// (or is `from`). Used to move open tabs after a rename.
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf>;
```

`validate_name` runs before every create or rename, so the I/O functions
only fail on real OS errors (permissions, a race with another program).

New dependency: `trash` (cross-platform Trash / Recycle Bin).

## Explorer state (`src/file_browser.rs`)

```rust
pub enum EditTarget {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { path: PathBuf },
}

pub struct InlineEdit {
    pub target: EditTarget,
    pub name: String,
    /// Shown in red below the row; the edit stays open.
    pub error: Option<NameError>,
    /// Focus the field (and select the stem for Rename) on the next frame.
    focus: bool,
}

impl Explorer {
    pub fn begin_new(&mut self, parent: PathBuf, folder: bool); // expands `parent`
    pub fn begin_rename(&mut self, path: PathBuf);              // name = current name
    pub fn cancel_edit(&mut self);
    pub fn editing(&self) -> Option<&InlineEdit>;
}
```

- `TreeRow` gains `edit: bool`. With a New edit open, `rows()` puts one edit
  row as the first child of `parent` (the root's first row when `parent` is
  the root); with a Rename open, the renamed entry's row is marked `edit`.
  Opening or closing an edit clears the cached rows, like `toggle` does.
- An edit row paints the usual indent, badge or chevron, then a single-line
  `TextEdit` over the name. Rename selects the name up to the last `.`
  (the whole name for folders and dot-files).
- **Enter** or **focus lost**: an empty name cancels; otherwise
  `validate_name` runs. A valid name is sent to the app as an action and the
  edit closes; an invalid one sets `error` and keeps the edit open with
  focus.
- **Escape** cancels.
- Starting another edit replaces the open one. Refresh and Collapse All
  cancel it.
- In filter mode a Rename edit replaces the match row the same way.

## Actions to the app

`ExplorerResponse` gains `action: Option<ExplorerAction>`:

```rust
pub enum ExplorerAction {
    Create { dir: PathBuf, name: String, folder: bool },
    Rename { from: PathBuf, name: String },
    Delete(PathBuf),
    Reveal(PathBuf),
}
```

`TextEditorApp::apply_explorer_action` (in `app.rs`, next to the other
sidebar wiring) runs it:

- **Create file**: `file_ops::create_file`, then open the new file in a tab.
- **Create folder**: `file_ops::create_dir`; the folder is expanded.
- **Rename**: `file_ops::rename`, then fix the open tabs (below). A renamed
  folder that was expanded stays expanded, and so do the expanded folders
  under it: `Explorer::rebase_expanded(from, to)` rebases those paths.
- **Delete**: stores `pending_delete: Option<PathBuf>`; the next frames show
  `modals::show_confirm_modal` with
  "Delete '{name}'? It will be moved to the Trash." /
  "Delete '{name}' and its contents? It will be moved to the Trash."
  for a folder. On confirm: `file_ops::move_to_trash`, then close tabs.
- **Reveal**: `file_ops::reveal`.

After every successful operation: `explorer.refresh()` and
`git.request_refresh()`. On failure the OS error goes to the status bar as
an error (`status_message` with `true`), e.g.
"Could not rename 'a.rs': Permission denied".

## Open tabs

- **Rename / move**: every tab whose `current_path` passes
  `file_ops::rebase(path, from, to)` gets the new path. Text, undo history,
  dirty state and bookmarks stay. `active_language` is recomputed from the
  new path unless `language_override` is set; `disk_stamp` is read again.
- **Delete**: tabs under the deleted path with no unsaved changes close.
  Tabs with unsaved changes stay open with the same path; the existing disk
  check ignores a missing file (`changed_on_disk` is false with no stamp),
  so no conflict prompt appears, and saving writes the file back.
- The session file needs no change: it stores tab paths when it saves.

## Strings (`assets/lang/*.json`)

New keys in `en`, `id`, `de`, `es`, `zh`:

`explorer.new_file`, `explorer.new_folder`, `explorer.rename`,
`explorer.delete`, `explorer.reveal`, `explorer.delete_title`,
`explorer.delete_file_message`, `explorer.delete_folder_message`,
`explorer.delete_confirm`, `explorer.name_empty`, `explorer.name_reserved`,
`explorer.name_separator`, `explorer.name_exists`, `explorer.op_failed`.

## Testing

- `file_ops`: `validate_name` (each error, trimming, case-only rename
  allowed), `create_file` / `create_dir` (and refusal when existing),
  `rename`, `rebase` (same path, child path, unrelated path). `move_to_trash`
  and `reveal` touch the desktop and are checked by hand.
- `Explorer`: the New edit row lands first under its parent and expands it;
  the Rename row is marked; Escape cancels; Enter with a taken name sets
  `error` and keeps the edit; Enter with a valid name returns the action.
- `app`: renaming a file updates its tab's path and language; renaming a
  folder moves every tab under it; deleting closes clean tabs and keeps
  dirty ones; Create File opens the new file.
- Hand check in the running app: each menu, Trash on Linux, Reveal.
