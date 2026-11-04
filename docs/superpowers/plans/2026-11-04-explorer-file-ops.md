# Explorer File Operations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Right-click the Explorer to create, rename, delete (to the Trash) and reveal files and folders, with names typed inline in the tree.

**Architecture:** A new pure module `src/file_ops.rs` does the file-system work and name checks. `src/file_browser.rs` owns the menus and the inline edit state and reports an `ExplorerAction` in `ExplorerResponse`. `src/app.rs` runs the action, fixes open tabs, asks before a delete and reports errors in the status bar.

**Tech Stack:** Rust 2024, eframe/egui 0.37, new crate `trash` 5.

**Spec:** `docs/superpowers/specs/2026-11-04-explorer-file-ops-design.md`

**Conventions in this repo**
- Tests live in a `#[cfg(test)] mod tests` at the bottom of each file.
- Temp folders: `std::env::temp_dir().join("rred_<module>_<name>")`, removed at the start and the end of the test.
- Run one test: `cargo test <name_fragment>`; all: `cargo test`; lint: `cargo clippy --all-targets`.
- Doc comments explain *why*, in plain sentences, as in the surrounding code.

---

## File map

| File | Change |
|---|---|
| `Cargo.toml` | add `trash = "5"` |
| `src/main.rs` | add `mod file_ops;` |
| `src/file_ops.rs` | **new**: `NameError`, `validate_name`, `rebase`, `create_file`, `create_dir`, `rename`, `move_to_trash`, `reveal` |
| `assets/lang/{en,id,de,es,zh}.json` | 13 new `explorer.*` keys |
| `src/file_browser.rs` | `EditTarget`, `InlineEdit`, `ExplorerAction`, edit state on `Explorer`, `TreeRow::edit`, menus, the edit row |
| `src/app.rs` | `pending_delete`, `apply_explorer_action`, `rebase_tabs`, `close_tabs_under`, delete confirm, wiring |
| `todo.md` | mark the item done after the hand check |

---

### Task 1: `file_ops` — name checks and path rebasing

**Files:**
- Create: `src/file_ops.rs`
- Modify: `src/main.rs` (module list, after `mod file_browser;`)

- [ ] **Step 1: Register the module**

In `src/main.rs`, after `mod file_browser;` add:

```rust
mod file_ops;
```

- [ ] **Step 2: Write the failing tests**

Create `src/file_ops.rs` with only the tests (and the imports they need):

```rust
//! File-system work behind the Explorer's right-click menu: checking a
//! typed name, creating, renaming, moving to the Trash and revealing.
//! Holds no egui or app state, so every function is tested on a temp folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_file_ops_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_validate_name_trims_and_accepts_a_free_name() {
        let dir = temp("validate_ok");
        assert_eq!(validate_name(&dir, "  main.rs ", None), Ok("main.rs".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_name_rejects_empty_reserved_and_separators() {
        let dir = temp("validate_bad");
        assert_eq!(validate_name(&dir, "   ", None), Err(NameError::Empty));
        assert_eq!(validate_name(&dir, ".", None), Err(NameError::Reserved));
        assert_eq!(validate_name(&dir, "..", None), Err(NameError::Reserved));
        assert_eq!(validate_name(&dir, "a/b.rs", None), Err(NameError::Separator));
        assert_eq!(validate_name(&dir, "a\\b.rs", None), Err(NameError::Separator));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_name_rejects_a_taken_name() {
        let dir = temp("validate_taken");
        fs::write(dir.join("a.rs"), "").unwrap();
        fs::create_dir(dir.join("src")).unwrap();
        assert_eq!(validate_name(&dir, "a.rs", None), Err(NameError::Exists));
        assert_eq!(validate_name(&dir, "src", None), Err(NameError::Exists));
        // Renaming b.rs onto the existing a.rs is refused too.
        fs::write(dir.join("b.rs"), "").unwrap();
        assert_eq!(
            validate_name(&dir, "a.rs", Some(&dir.join("b.rs"))),
            Err(NameError::Exists)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_name_lets_an_entry_keep_or_recase_its_name() {
        let dir = temp("validate_self");
        fs::write(dir.join("a.rs"), "").unwrap();
        let current = dir.join("a.rs");
        assert_eq!(validate_name(&dir, "a.rs", Some(&current)), Ok("a.rs".to_string()));
        assert_eq!(validate_name(&dir, "A.rs", Some(&current)), Ok("A.rs".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rebase_moves_the_path_and_its_children_only() {
        let from = Path::new("/p/src");
        let to = Path::new("/p/lib");
        assert_eq!(rebase(Path::new("/p/src"), from, to), Some(PathBuf::from("/p/lib")));
        assert_eq!(
            rebase(Path::new("/p/src/a/b.rs"), from, to),
            Some(PathBuf::from("/p/lib/a/b.rs"))
        );
        assert_eq!(rebase(Path::new("/p/srcx/b.rs"), from, to), None);
        assert_eq!(rebase(Path::new("/p/other.rs"), from, to), None);
    }
}
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test file_ops`
Expected: compile errors `cannot find function validate_name`, `cannot find type NameError`, `cannot find function rebase`.

- [ ] **Step 4: Implement `NameError`, `validate_name` and `rebase`**

Insert above `#[cfg(test)]` in `src/file_ops.rs`:

```rust
/// Why a typed name cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    /// Nothing left after trimming: the edit is simply cancelled.
    Empty,
    /// `.` or `..`.
    Reserved,
    /// Holds `/` or `\`; nested names are not supported.
    Separator,
    /// Something else in the folder already has that name.
    Exists,
}

/// Checks `name` for a new entry in `dir`, or for renaming `current` inside
/// `dir`, and returns it trimmed. A rename may keep the name or change only
/// its letter case.
pub fn validate_name(dir: &Path, name: &str, current: Option<&Path>) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name == "." || name == ".." {
        return Err(NameError::Reserved);
    }
    if name.contains('/') || name.contains('\\') {
        return Err(NameError::Separator);
    }
    let candidate = dir.join(name);
    let taken = fs::symlink_metadata(&candidate).is_ok();
    if taken && !current.is_some_and(|current| same_entry(&candidate, current)) {
        return Err(NameError::Exists);
    }
    Ok(name.to_string())
}

/// Whether `a` and `b` name the same file or folder, so a rename between
/// them only changes letter case on a case-insensitive file system.
fn same_entry(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let lower = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
        a.parent() == b.parent() && lower(a) == lower(b)
    }
}

/// `path` with its `from` prefix swapped for `to`, if it is `from` or lies
/// under it. Moves open tabs and expanded folders after a rename.
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    // `join("")` would add a trailing separator.
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test file_ops`
Expected: 5 passed. (`dead_code` warnings for unused items are expected until Task 4.)

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/file_ops.rs
git commit -m "feat(file_ops): validate Explorer names and rebase paths"
```

---

### Task 2: `file_ops` — create, rename, Trash, reveal

**Files:**
- Modify: `Cargo.toml` (`[dependencies]`)
- Modify: `src/file_ops.rs`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml` under `[dependencies]`, after `similar = "2"`:

```toml
trash = "5"
```

Run: `cargo build`
Expected: builds (downloads `trash`).

- [ ] **Step 2: Write the failing tests**

Add inside `mod tests` in `src/file_ops.rs`:

```rust
    #[test]
    fn test_create_file_and_dir_refuse_to_overwrite() {
        let dir = temp("create");
        let file = create_file(&dir, "a.rs").unwrap();
        assert_eq!(file, dir.join("a.rs"));
        assert!(file.is_file());
        assert_eq!(create_file(&dir, "a.rs").unwrap_err().kind(), io::ErrorKind::AlreadyExists);

        let folder = create_dir(&dir, "src").unwrap();
        assert_eq!(folder, dir.join("src"));
        assert!(folder.is_dir());
        assert_eq!(create_dir(&dir, "src").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename_moves_within_the_folder() {
        let dir = temp("rename");
        fs::write(dir.join("a.rs"), "body").unwrap();
        let to = rename(&dir.join("a.rs"), "b.rs").unwrap();
        assert_eq!(to, dir.join("b.rs"));
        assert!(!dir.join("a.rs").exists());
        assert_eq!(fs::read_to_string(&to).unwrap(), "body");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename_never_overwrites_another_entry() {
        let dir = temp("rename_taken");
        fs::write(dir.join("a.rs"), "a").unwrap();
        fs::write(dir.join("b.rs"), "b").unwrap();
        let err = rename(&dir.join("a.rs"), "b.rs").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(dir.join("b.rs")).unwrap(), "b");
        let _ = fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test file_ops`
Expected: compile errors `cannot find function create_file / create_dir / rename`.

- [ ] **Step 4: Implement**

Add above `#[cfg(test)]` in `src/file_ops.rs`:

```rust
/// Creates an empty file `dir/name`; fails if anything has that name.
pub fn create_file(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    Ok(path)
}

/// Creates the folder `dir/name`; fails if anything has that name.
pub fn create_dir(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

/// Renames `from` to `new_name` in the same folder and returns the new
/// path. Refuses to replace another entry, which `fs::rename` would do
/// silently on Unix; another program may have taken the name since the
/// name was checked.
pub fn rename(from: &Path, new_name: &str) -> io::Result<PathBuf> {
    let to = from.with_file_name(new_name);
    if fs::symlink_metadata(&to).is_ok() && !same_entry(&to, from) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    fs::rename(from, &to)?;
    Ok(to)
}

/// Moves a file or folder to the Trash / Recycle Bin.
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    trash::delete(path).map_err(|err| io::Error::other(err.to_string()))
}

/// Shows `path` in the system file manager. Linux opens the folder holding
/// it (no standard way to select an item); macOS and Windows select it.
/// The program is not waited on; a thread reaps it so no zombie is left.
pub fn reveal(path: &Path) -> io::Result<()> {
    let mut child = reveal_command(path).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn reveal_command(path: &Path) -> std::process::Command {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg("-R").arg(path);
        command
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    }
}
```

`move_to_trash` and `reveal` touch the desktop; they are checked by hand in Task 7.

- [ ] **Step 5: Run the tests**

Run: `cargo test file_ops`
Expected: 8 passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/file_ops.rs
git commit -m "feat(file_ops): create, rename, trash and reveal entries"
```

---

### Task 3: Strings

**Files:**
- Modify: `assets/lang/en.json`, `id.json`, `de.json`, `es.json`, `zh.json` — after the `"explorer.indexing"` line in each.

`src/i18n.rs` has `test_builtin_languages_have_the_same_keys`; every file needs the same 13 keys. Placeholders use `{name}` / `{error}` (as `scm.discard_one` does).

- [ ] **Step 1: `assets/lang/en.json`** — after `"explorer.indexing": "Indexing files…",` insert:

```json
  "explorer.new_file": "New File…",
  "explorer.new_folder": "New Folder…",
  "explorer.rename": "Rename…",
  "explorer.delete": "Delete",
  "explorer.reveal": "Reveal in Folder",
  "explorer.delete_title": "Delete",
  "explorer.delete_file_message": "Delete '{name}'? It will be moved to the Trash.",
  "explorer.delete_folder_message": "Delete '{name}' and its contents? It will be moved to the Trash.",
  "explorer.delete_confirm": "Move to Trash",
  "explorer.name_reserved": "'{name}' is not a valid name.",
  "explorer.name_separator": "A name cannot contain / or \\.",
  "explorer.name_exists": "'{name}' already exists here.",
  "explorer.op_failed": "Could not change '{name}': {error}",
```

- [ ] **Step 2: `assets/lang/id.json`** — after the `"explorer.indexing"` line:

```json
  "explorer.new_file": "File Baru…",
  "explorer.new_folder": "Folder Baru…",
  "explorer.rename": "Ganti Nama…",
  "explorer.delete": "Hapus",
  "explorer.reveal": "Tampilkan di Folder",
  "explorer.delete_title": "Hapus",
  "explorer.delete_file_message": "Hapus '{name}'? File akan dipindah ke Tempat Sampah.",
  "explorer.delete_folder_message": "Hapus '{name}' beserta isinya? Folder akan dipindah ke Tempat Sampah.",
  "explorer.delete_confirm": "Pindah ke Tempat Sampah",
  "explorer.name_reserved": "'{name}' bukan nama yang valid.",
  "explorer.name_separator": "Nama tidak boleh berisi / atau \\.",
  "explorer.name_exists": "'{name}' sudah ada di sini.",
  "explorer.op_failed": "Gagal mengubah '{name}': {error}",
```

- [ ] **Step 3: `assets/lang/de.json`** — after the `"explorer.indexing"` line:

```json
  "explorer.new_file": "Neue Datei…",
  "explorer.new_folder": "Neuer Ordner…",
  "explorer.rename": "Umbenennen…",
  "explorer.delete": "Löschen",
  "explorer.reveal": "Im Ordner anzeigen",
  "explorer.delete_title": "Löschen",
  "explorer.delete_file_message": "'{name}' löschen? Es wird in den Papierkorb verschoben.",
  "explorer.delete_folder_message": "'{name}' samt Inhalt löschen? Es wird in den Papierkorb verschoben.",
  "explorer.delete_confirm": "In den Papierkorb",
  "explorer.name_reserved": "'{name}' ist kein gültiger Name.",
  "explorer.name_separator": "Ein Name darf kein / oder \\ enthalten.",
  "explorer.name_exists": "'{name}' existiert hier bereits.",
  "explorer.op_failed": "'{name}' konnte nicht geändert werden: {error}",
```

- [ ] **Step 4: `assets/lang/es.json`** — after the `"explorer.indexing"` line:

```json
  "explorer.new_file": "Nuevo archivo…",
  "explorer.new_folder": "Nueva carpeta…",
  "explorer.rename": "Cambiar nombre…",
  "explorer.delete": "Eliminar",
  "explorer.reveal": "Mostrar en la carpeta",
  "explorer.delete_title": "Eliminar",
  "explorer.delete_file_message": "¿Eliminar '{name}'? Se moverá a la papelera.",
  "explorer.delete_folder_message": "¿Eliminar '{name}' y su contenido? Se moverá a la papelera.",
  "explorer.delete_confirm": "Mover a la papelera",
  "explorer.name_reserved": "'{name}' no es un nombre válido.",
  "explorer.name_separator": "Un nombre no puede contener / ni \\.",
  "explorer.name_exists": "'{name}' ya existe aquí.",
  "explorer.op_failed": "No se pudo cambiar '{name}': {error}",
```

- [ ] **Step 5: `assets/lang/zh.json`** — after the `"explorer.indexing"` line:

```json
  "explorer.new_file": "新建文件…",
  "explorer.new_folder": "新建文件夹…",
  "explorer.rename": "重命名…",
  "explorer.delete": "删除",
  "explorer.reveal": "在文件夹中显示",
  "explorer.delete_title": "删除",
  "explorer.delete_file_message": "删除“{name}”？它将被移到回收站。",
  "explorer.delete_folder_message": "删除“{name}”及其内容？它将被移到回收站。",
  "explorer.delete_confirm": "移到回收站",
  "explorer.name_reserved": "“{name}”不是有效的名称。",
  "explorer.name_separator": "名称不能包含 / 或 \\。",
  "explorer.name_exists": "此处已存在“{name}”。",
  "explorer.op_failed": "无法更改“{name}”：{error}",
```

- [ ] **Step 6: Run the i18n tests**

Run: `cargo test i18n`
Expected: all pass, including `test_builtin_languages_have_the_same_keys`.

- [ ] **Step 7: Commit**

```bash
git add assets/lang
git commit -m "feat(i18n): strings for Explorer file operations"
```

---

### Task 4: Explorer edit state

**Files:**
- Modify: `src/file_browser.rs` — `TreeRow` (line ~133), `Explorer` struct (~142) and `impl Explorer` (~255–419), the `TreeRow { .. }` literal in `show_matches` (~647)

- [ ] **Step 1: Write the failing tests**

Add in `mod tests` of `src/file_browser.rs` (the `fixture` and `outline` helpers already exist; `fixture` makes `docs/guide.md`, `src/main.rs`, `src/nested/deep.rs`, `Cargo.toml`):

```rust
    /// `(name, depth, edit)` of every visible row.
    fn edit_outline(explorer: &mut Explorer) -> Vec<(String, usize, bool)> {
        explorer
            .rows()
            .iter()
            .map(|row| (row.entry.name.clone(), row.depth, row.edit))
            .collect()
    }

    #[test]
    fn test_new_file_edit_row_is_first_under_its_expanded_parent() {
        let root = fixture("edit_new");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.join("src"), false);
        assert_eq!(
            edit_outline(&mut explorer),
            vec![
                ("docs".into(), 0, false),
                ("src".into(), 0, false),
                ("".into(), 1, true),
                ("nested".into(), 1, false),
                ("main.rs".into(), 1, false),
                ("Cargo.toml".into(), 0, false),
            ]
        );
        explorer.cancel_edit();
        assert!(explorer.editing().is_none());
        assert!(edit_outline(&mut explorer).iter().all(|(_, _, edit)| !edit));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_new_entry_in_the_root_is_the_first_row() {
        let root = fixture("edit_new_root");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), true);
        let rows = explorer.rows();
        assert!(rows[0].edit && rows[0].entry.is_dir && rows[0].depth == 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rename_marks_the_row_and_fills_the_name() {
        let root = fixture("edit_rename");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        assert_eq!(explorer.editing().unwrap().name, "Cargo.toml");
        let marked: Vec<_> = explorer.rows().iter().filter(|r| r.edit).map(|r| r.entry.name.clone()).collect();
        assert_eq!(marked, vec!["Cargo.toml".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_turns_a_valid_name_into_an_action() {
        let root = fixture("edit_commit");
        let mut explorer = Explorer::new(root.clone());

        explorer.begin_new(root.join("src"), false);
        explorer.edit_name_mut().unwrap().push_str(" lib.rs ");
        assert_eq!(
            explorer.commit_edit(),
            Some(ExplorerAction::Create { dir: root.join("src"), name: "lib.rs".into(), folder: false })
        );
        assert!(explorer.editing().is_none());

        explorer.begin_rename(root.join("Cargo.toml"));
        *explorer.edit_name_mut().unwrap() = "Cargo.lock".into();
        assert_eq!(
            explorer.commit_edit(),
            Some(ExplorerAction::Rename { from: root.join("Cargo.toml"), name: "Cargo.lock".into() })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_keeps_the_edit_open_on_a_bad_name() {
        let root = fixture("edit_bad");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), false);
        explorer.edit_name_mut().unwrap().push_str("Cargo.toml");
        assert_eq!(explorer.commit_edit(), None);
        assert_eq!(explorer.editing().unwrap().error, Some(NameError::Exists));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_edit_cancels_an_empty_or_unchanged_name() {
        let root = fixture("edit_noop");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), true);
        assert_eq!(explorer.commit_edit(), None);
        assert!(explorer.editing().is_none(), "empty name cancels");

        explorer.begin_rename(root.join("Cargo.toml"));
        assert_eq!(explorer.commit_edit(), None);
        assert!(explorer.editing().is_none(), "same name cancels");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_refresh_and_collapse_all_cancel_the_edit() {
        let root = fixture("edit_refresh");
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        explorer.refresh();
        assert!(explorer.editing().is_none());
        explorer.begin_rename(root.join("Cargo.toml"));
        explorer.collapse_all();
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rebase_expanded_keeps_a_renamed_folder_open() {
        let root = fixture("edit_rebase");
        let mut explorer = Explorer::new(root.clone());
        explorer.toggle(&root.join("src"));
        explorer.toggle(&root.join("src/nested"));
        fs::rename(root.join("src"), root.join("lib")).unwrap();
        explorer.rebase_expanded(&root.join("src"), &root.join("lib"));
        explorer.refresh();
        assert_eq!(
            outline(&mut explorer),
            owned(&[
                ("docs", 0),
                ("lib", 0),
                ("nested", 1),
                ("deep.rs", 2),
                ("main.rs", 1),
                ("Cargo.toml", 0),
            ])
        );
        let _ = fs::remove_dir_all(&root);
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test file_browser`
Expected: compile errors (`no field edit`, `no method begin_new`, `ExplorerAction` not found, …).

- [ ] **Step 3: Add the types**

In `src/file_browser.rs`, add to the imports:

```rust
use crate::file_ops::{self, NameError};
```

Give `TreeRow` a new field (after `expanded`):

```rust
    /// Drawn as a name field: the entry being renamed, or the placeholder
    /// row of a new entry (its `name` is empty, its `path` the parent).
    pub edit: bool,
```

Below `TreeRow`, add:

```rust
/// What the inline name field is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditTarget {
    NewFile { parent: PathBuf },
    NewFolder { parent: PathBuf },
    Rename { path: PathBuf },
}

impl EditTarget {
    /// The folder the typed name goes in.
    fn dir(&self) -> &Path {
        match self {
            EditTarget::NewFile { parent } | EditTarget::NewFolder { parent } => parent,
            EditTarget::Rename { path } => path.parent().unwrap_or(path),
        }
    }
}

/// The name being typed in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineEdit {
    pub target: EditTarget,
    pub name: String,
    /// Why the last Enter was refused; shown below the row.
    pub error: Option<NameError>,
    /// Focus the field (and, for a rename, select the name before its
    /// extension) on the next frame it is drawn.
    focus: bool,
}

/// A file operation the Explorer asks the app to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerAction {
    Create { dir: PathBuf, name: String, folder: bool },
    Rename { from: PathBuf, name: String },
    Delete(PathBuf),
    Reveal(PathBuf),
}
```

Add to `struct Explorer` (after `found`):

```rust
    /// The name field open in the tree, if any.
    editing: Option<InlineEdit>,
```

and `editing: None,` in `Explorer::new`.

- [ ] **Step 4: Add the methods**

Add inside `impl Explorer`:

```rust
    /// Expands `dir` if it is collapsed.
    pub fn expand(&mut self, dir: &Path) {
        if self.expanded.insert(dir.to_path_buf()) {
            self.rows = None;
        }
    }

    /// Opens a name field for a new file (or `folder`) as the first entry
    /// of `parent`, expanding it. Replaces any field already open.
    pub fn begin_new(&mut self, parent: PathBuf, folder: bool) {
        if parent != self.root {
            self.expanded.insert(parent.clone());
        }
        let target = if folder {
            EditTarget::NewFolder { parent }
        } else {
            EditTarget::NewFile { parent }
        };
        self.start_edit(target, String::new());
    }

    /// Turns the row of `path` into a name field holding its current name.
    pub fn begin_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.start_edit(EditTarget::Rename { path }, name);
    }

    fn start_edit(&mut self, target: EditTarget, name: String) {
        self.editing = Some(InlineEdit {
            target,
            name,
            error: None,
            focus: true,
        });
        self.rows = None;
    }

    pub fn cancel_edit(&mut self) {
        if self.editing.take().is_some() {
            self.rows = None;
        }
    }

    pub fn editing(&self) -> Option<&InlineEdit> {
        self.editing.as_ref()
    }

    /// The text of the open name field.
    pub fn edit_name_mut(&mut self) -> Option<&mut String> {
        self.editing.as_mut().map(|edit| &mut edit.name)
    }

    /// Closes the name field and returns what it asks for. An empty or
    /// unchanged name just closes it; a name that cannot be used keeps it
    /// open with [`InlineEdit::error`] set.
    pub fn commit_edit(&mut self) -> Option<ExplorerAction> {
        let edit = self.editing.as_ref()?;
        let current = match &edit.target {
            EditTarget::Rename { path } => Some(path.as_path()),
            _ => None,
        };
        let action = match file_ops::validate_name(edit.target.dir(), &edit.name, current) {
            Err(NameError::Empty) => None,
            Err(error) => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.error = Some(error);
                    edit.focus = true;
                }
                return None;
            }
            Ok(name) => match &edit.target {
                EditTarget::NewFile { parent } => Some(ExplorerAction::Create {
                    dir: parent.clone(),
                    name,
                    folder: false,
                }),
                EditTarget::NewFolder { parent } => Some(ExplorerAction::Create {
                    dir: parent.clone(),
                    name,
                    folder: true,
                }),
                EditTarget::Rename { path } => (path.file_name() != Some(std::ffi::OsStr::new(&name)))
                    .then(|| ExplorerAction::Rename {
                        from: path.clone(),
                        name,
                    }),
            },
        };
        self.cancel_edit();
        action
    }

    /// After `from` was renamed to `to`: folders that were expanded at or
    /// under `from` stay expanded under `to`.
    pub fn rebase_expanded(&mut self, from: &Path, to: &Path) {
        self.expanded = self
            .expanded
            .drain()
            .map(|dir| file_ops::rebase(&dir, from, to).unwrap_or(dir))
            .collect();
        self.rows = None;
    }

    /// Whether a new entry's field goes first in `dir`, and if so whether
    /// it is a folder.
    fn new_entry_in(&self, dir: &Path) -> Option<bool> {
        match &self.editing.as_ref()?.target {
            EditTarget::NewFile { parent } if parent == dir => Some(false),
            EditTarget::NewFolder { parent } if parent == dir => Some(true),
            _ => None,
        }
    }

    /// The entry being renamed.
    fn renaming(&self) -> Option<&Path> {
        match &self.editing.as_ref()?.target {
            EditTarget::Rename { path } => Some(path),
            _ => None,
        }
    }
```

In `collapse_all` and `refresh`, add as the first line:

```rust
        self.editing = None;
```

- [ ] **Step 5: Put the edit rows in the tree**

Replace `push_rows` with:

```rust
    fn push_rows(&mut self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        if let Some(folder) = self.new_entry_in(dir) {
            rows.push(TreeRow {
                entry: FsEntry {
                    path: dir.to_path_buf(),
                    is_dir: folder,
                    name: String::new(),
                },
                depth,
                expanded: false,
                edit: true,
            });
        }
        let renaming = self.renaming().map(Path::to_path_buf);
        let entries = self
            .children
            .entry(dir.to_path_buf())
            .or_insert_with(|| scan_directory(dir))
            .clone();
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let path = entry.path.clone();
            let edit = renaming.as_deref() == Some(path.as_path());
            rows.push(TreeRow {
                entry,
                depth,
                expanded,
                edit,
            });
            if expanded {
                self.push_rows(&path, depth + 1, rows);
            }
        }
    }
```

In `show_matches`, the `TreeRow { .. }` literal gets `edit: false,` for now (Task 5 changes it).

- [ ] **Step 6: Run the tests**

Run: `cargo test file_browser`
Expected: all pass (old and new).

- [ ] **Step 7: Commit**

```bash
git add src/file_browser.rs
git commit -m "feat(explorer): inline edit state for new and renamed entries"
```

---

### Task 5: Explorer menus and the name field

**Files:**
- Modify: `src/file_browser.rs` — `ExplorerResponse` (~438), `show` (~466–608), `show_matches` (~612–678), `file_menu` (~739, replaced), `paint_row` (~757)

- [ ] **Step 1: Write the failing tests**

Add in `mod tests` of `src/file_browser.rs` (`run` and `click` exist already):

```rust
    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn test_typing_a_name_and_enter_asks_to_create_it() {
        let root = fixture("ui_new");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_new(root.clone(), false);

        run(&ctx, Vec::new(), &mut explorer, None); // focuses the field
        run(&ctx, vec![egui::Event::Text("notes.md".into())], &mut explorer, None);
        let (response, _) = run(&ctx, vec![key(egui::Key::Enter)], &mut explorer, None);
        assert_eq!(
            response.action,
            Some(ExplorerAction::Create { dir: root.clone(), name: "notes.md".into(), folder: false })
        );
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_escape_closes_the_name_field_without_an_action() {
        let root = fixture("ui_escape");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));

        run(&ctx, Vec::new(), &mut explorer, None);
        run(&ctx, vec![egui::Event::Text("x".into())], &mut explorer, None);
        let (response, _) = run(&ctx, vec![key(egui::Key::Escape)], &mut explorer, None);
        assert_eq!(response.action, None);
        assert!(explorer.editing().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rename_field_starts_with_the_stem_selected() {
        let root = fixture("ui_stem");
        let ctx = egui::Context::default();
        let mut explorer = Explorer::new(root.clone());
        explorer.begin_rename(root.join("Cargo.toml"));
        run(&ctx, Vec::new(), &mut explorer, None);
        // Typing replaces the selected stem and keeps `.toml`.
        run(&ctx, vec![egui::Event::Text("Build".into())], &mut explorer, None);
        assert_eq!(explorer.editing().unwrap().name, "Build.toml");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_stem_chars_stops_before_the_extension() {
        assert_eq!(stem_chars("main.rs", false), 4);
        assert_eq!(stem_chars("archive.tar.gz", false), 11);
        assert_eq!(stem_chars(".gitignore", false), 10);
        assert_eq!(stem_chars("src.old", true), 7);
        assert_eq!(stem_chars("héllo.rs", false), 5);
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test file_browser`
Expected: compile errors (`no field action on ExplorerResponse`, `cannot find function stem_chars`).

- [ ] **Step 3: Add `action` to `ExplorerResponse`**

In `struct ExplorerResponse`, after `find_in_files`:

```rust
    /// A file operation picked from a menu or typed in a name field.
    pub action: Option<ExplorerAction>,
```

and `action: None,` in the `ExplorerResponse { .. }` literal in `show`.

- [ ] **Step 4: Replace `file_menu` with the menus**

Delete `fn file_menu` and its doc comment. In its place add:

```rust
/// A right-click menu entry, applied once the tree is drawn (the menus run
/// while the rows still borrow the Explorer).
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuChoice {
    New { parent: PathBuf, folder: bool },
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    History(PathBuf),
}

/// The right-click menu of a file or folder row. `offer_new` is false in
/// the flat list of filter matches, which has no obvious target folder.
fn entry_menu(
    row: &egui::Response,
    entry: &FsEntry,
    offer_new: bool,
    i18n: &I18n,
    choice: &mut Option<MenuChoice>,
) {
    row.context_menu(|ui| {
        let mut pick = |ui: &mut egui::Ui, key: &str, value: MenuChoice| {
            if ui.button(i18n.t(key)).clicked() {
                *choice = Some(value);
                ui.close();
            }
        };
        if offer_new {
            let parent = if entry.is_dir {
                entry.path.clone()
            } else {
                entry.path.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            pick(ui, "explorer.new_file", MenuChoice::New { parent: parent.clone(), folder: false });
            pick(ui, "explorer.new_folder", MenuChoice::New { parent, folder: true });
            ui.separator();
        }
        pick(ui, "explorer.rename", MenuChoice::Rename(entry.path.clone()));
        pick(ui, "explorer.delete", MenuChoice::Delete(entry.path.clone()));
        ui.separator();
        pick(ui, "explorer.reveal", MenuChoice::Reveal(entry.path.clone()));
        if !entry.is_dir {
            pick(ui, "history.file_history", MenuChoice::History(entry.path.clone()));
        }
    });
}

/// The right-click menu of the empty space below the tree: new entries in
/// the root.
fn blank_menu(blank: &egui::Response, root: &Path, i18n: &I18n, choice: &mut Option<MenuChoice>) {
    blank.context_menu(|ui| {
        for (key, folder) in [("explorer.new_file", false), ("explorer.new_folder", true)] {
            if ui.button(i18n.t(key)).clicked() {
                *choice = Some(MenuChoice::New { parent: root.to_path_buf(), folder });
                ui.close();
            }
        }
    });
}

/// Applies a menu entry: new and rename open a name field; the rest go to
/// the app through `response`.
fn apply_choice(explorer: &mut Explorer, choice: MenuChoice, response: &mut ExplorerResponse) {
    match choice {
        MenuChoice::New { parent, folder } => explorer.begin_new(parent, folder),
        MenuChoice::Rename(path) => explorer.begin_rename(path),
        MenuChoice::Delete(path) => response.action = Some(ExplorerAction::Delete(path)),
        MenuChoice::Reveal(path) => response.action = Some(ExplorerAction::Reveal(path)),
        MenuChoice::History(path) => response.file_history = Some(path),
    }
}
```

- [ ] **Step 5: Let `paint_row` say where the name starts**

Change `paint_row`'s signature to return the name's left edge:

```rust
fn paint_row(
    /* same parameters */
) -> f32 {
```

and right after `let mut x = chevron.right();` … the badge block (where `x` is final, before the git mark code) keep a copy:

```rust
    let name_x = x;
```

End the function with `name_x`. Existing callers ignore the value (`let _ =` is not needed; `paint_row(...);` as a statement compiles because the result is `f32`, not `#[must_use]`).

- [ ] **Step 6: Add the name field**

Add near `paint_row`:

```rust
/// How the name field ended this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditOutcome {
    Keep,
    Commit,
    Cancel,
}

/// Characters of `name` before its extension: what a rename selects.
/// Folders and dot-files (`.gitignore`) select the whole name.
fn stem_chars(name: &str, is_dir: bool) -> usize {
    let end = if is_dir { None } else { name.rfind('.').filter(|&i| i > 0) };
    name[..end.unwrap_or(name.len())].chars().count()
}

/// Draws `row` with a single-line field over its name. Enter or a click
/// elsewhere commits, Escape cancels; a refused name is shown below.
fn show_edit_row(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    row: &TreeRow,
    rect: Rect,
    edit: &mut InlineEdit,
    i18n: &I18n,
) -> EditOutcome {
    // Indent, chevron or badge as for a normal row, the badge after the
    // typed name; the name itself is the field's.
    let shown = TreeRow {
        entry: FsEntry {
            path: if row.entry.name.is_empty() {
                row.entry.path.join(&edit.name)
            } else {
                row.entry.path.clone()
            },
            is_dir: row.entry.is_dir,
            name: String::new(),
        },
        ..row.clone()
    };
    let name_x = paint_row(ui, palette, &shown, None, None, rect, false, true);

    let id = ui.id().with("explorer_edit");
    if edit.focus {
        edit.focus = false;
        ui.memory_mut(|m| m.request_focus(id));
        let end = if matches!(edit.target, EditTarget::Rename { .. }) {
            stem_chars(&edit.name, row.entry.is_dir)
        } else {
            edit.name.chars().count()
        };
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(end),
        )));
        state.store(ui.ctx(), id);
    }
    let field_rect = Rect::from_min_max(
        pos2(name_x - 2.0, rect.top() + 1.0),
        pos2(rect.right() - ROW_PADDING_X, rect.bottom() - 1.0),
    );
    let field = ui.put(
        field_rect,
        egui::TextEdit::singleline(&mut edit.name)
            .id(id)
            .font(FontId::proportional(13.0))
            .margin(vec2(2.0, 1.0)),
    );
    if field.changed() {
        edit.error = None;
    }
    if let Some(error) = edit.error {
        paint_name_error(ui, palette, field_rect, &error_text(error, &edit.name, i18n));
    }
    if field.lost_focus() {
        return if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            EditOutcome::Cancel
        } else {
            EditOutcome::Commit
        };
    }
    EditOutcome::Keep
}

/// The message for a refused name.
fn error_text(error: NameError, name: &str, i18n: &I18n) -> String {
    let name = name.trim();
    match error {
        NameError::Empty => String::new(),
        NameError::Reserved => i18n.tf("explorer.name_reserved", &[("name", name)]),
        NameError::Separator => i18n.t("explorer.name_separator").to_owned(),
        NameError::Exists => i18n.tf("explorer.name_exists", &[("name", name)]),
    }
}

/// A red-bordered note just below `field`, over the rows beneath it.
fn paint_name_error(ui: &egui::Ui, palette: &SidebarPalette, field: Rect, message: &str) {
    field_outline(ui, field, palette.git_deleted);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("explorer_edit_error"),
    ));
    let galley = painter.layout(
        message.to_owned(),
        FontId::proportional(12.0),
        palette.foreground,
        field.width() - 8.0,
    );
    let note = Rect::from_min_size(
        field.left_bottom(),
        vec2(field.width(), galley.size().y + 8.0),
    );
    painter.rect(
        note,
        CornerRadius::ZERO,
        palette.background,
        Stroke::new(1.0, palette.git_deleted),
        StrokeKind::Inside,
    );
    painter.galley(note.min + vec2(4.0, 4.0), galley, palette.foreground);
}

fn field_outline(ui: &egui::Ui, field: Rect, color: Color32) {
    ui.painter()
        .rect_stroke(field, CornerRadius::same(2), Stroke::new(1.0, color), StrokeKind::Outside);
}
```

- [ ] **Step 7: Use them in `show`**

Replace the tree part of `show` (from `// Only the rows in view are laid out` to the final `response`) with:

```rust
    // Only the rows in view are laid out: a folder can hold thousands.
    // The rows are built first, while the name field is still in place,
    // then the field is taken out so it can be edited as the rows draw.
    explorer.rows();
    let mut edit = explorer.editing.take();
    let mut outcome = EditOutcome::Keep;
    let mut choice = None;
    let mut toggled = None;
    let mut blank_top = None;
    let root = explorer.root().to_path_buf();
    let rows = explorer.rows();
    let scroll = egui::ScrollArea::vertical()
        .id_salt("explorer_tree")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let last = range.end == rows.len();
            for row in &rows[range] {
                let (rect, row_response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                if let (true, Some(edit)) = (row.edit, edit.as_mut()) {
                    outcome = show_edit_row(ui, &palette, row, rect, edit, i18n);
                    if !row.entry.name.is_empty() {
                        response.rows.push((row.entry.path.clone(), rect));
                    }
                    continue;
                }
                let selected = active_file == Some(row.entry.path.as_path());
                paint_row(
                    ui,
                    &palette,
                    row,
                    None,
                    git_kind(git, &row.entry),
                    rect,
                    row_response.hovered(),
                    selected,
                );
                if row_response.clicked() {
                    if row.entry.is_dir {
                        toggled = Some(row.entry.path.clone());
                    } else {
                        response.open_file = Some(row.entry.path.clone());
                    }
                }
                entry_menu(&row_response, &row.entry, true, i18n, &mut choice);
                response.rows.push((row.entry.path.clone(), rect));
            }
            if last {
                blank_top = Some(ui.min_rect().bottom());
            }
        });
    // The empty space below the last row: New File / New Folder in the root.
    if let Some(top) = blank_top.filter(|top| *top < scroll.inner_rect.bottom()) {
        let blank = Rect::from_min_max(
            pos2(scroll.inner_rect.left(), top),
            scroll.inner_rect.max,
        );
        let blank = ui.interact(blank, ui.id().with("explorer_blank"), Sense::click());
        blank_menu(&blank, &root, i18n, &mut choice);
    }

    explorer.editing = edit;
    match outcome {
        EditOutcome::Commit => response.action = explorer.commit_edit(),
        EditOutcome::Cancel => explorer.cancel_edit(),
        EditOutcome::Keep => {}
    }
    if let Some(dir) = toggled {
        explorer.toggle(&dir);
    }
    if let Some(choice) = choice {
        apply_choice(explorer, choice, &mut response);
    }
    response
```

Note `editing` is a private field read from `show` in the same module, so no accessor is needed.

- [ ] **Step 8: Use them in `show_matches`**

In `show_matches`, the same take-and-restore pattern. Change the start to:

```rust
    let root = explorer.root().to_path_buf();
    explorer.update_matches(ui.ctx());
    let mut edit = explorer.editing.take();
    let mut outcome = EditOutcome::Keep;
    let mut choice = None;
    let renaming = edit.as_ref().and_then(|e| match &e.target {
        EditTarget::Rename { path } => Some(path.clone()),
        _ => None,
    });
    let found = explorer.matches();
```

The early `return` for the empty list must first put the field back: replace that `return;` with:

```rust
        explorer.editing = edit;
        return;
```

In the row loop, build the row with `edit: renaming.as_ref() == Some(&entry.path),` and right after `allocate_exact_size`:

```rust
                if let (true, Some(edit)) = (row.edit, edit.as_mut()) {
                    outcome = show_edit_row(ui, palette, &row, rect, edit, i18n);
                    response.rows.push((entry.path.clone(), rect));
                    continue;
                }
```

Replace the `file_menu(&row_response, &entry.path, i18n, response);` call with:

```rust
                entry_menu(&row_response, entry, false, i18n, &mut choice);
```

After the `ScrollArea` call, end the function with:

```rust
    explorer.editing = edit;
    match outcome {
        EditOutcome::Commit => response.action = explorer.commit_edit(),
        EditOutcome::Cancel => explorer.cancel_edit(),
        EditOutcome::Keep => {}
    }
    if let Some(choice) = choice {
        apply_choice(explorer, choice, response);
    }
```

(`found` borrows `explorer` only inside the scroll area, so these lines compile after it.)

- [ ] **Step 9: Run the tests**

Run: `cargo test file_browser`
Expected: all pass, including the 4 new ones and the existing click / file history tests.

If `test_rename_field_starts_with_the_stem_selected` fails with `"BuildCargo.toml"`, the selection was stored after the field read its state: make sure the `load_state`/`store` block runs **before** `ui.put`, as above.

- [ ] **Step 10: Lint and commit**

Run: `cargo clippy --all-targets`
Expected: no new warnings in `file_browser.rs` / `file_ops.rs` (remaining `dead_code` on `ExplorerAction` variants go away in Task 6).

```bash
git add src/file_browser.rs
git commit -m "feat(explorer): right-click menus and inline name field"
```

---

### Task 6: Run the actions in the app

**Files:**
- Modify: `src/app.rs` — `TextEditorApp` struct (~258) and its `Default` (~370), dialog guards (~706, ~1048), sidebar wiring (~2170–2230), modal dispatch (~2488), new methods near `close_tabs_where` (~1212)

- [ ] **Step 1: Write the failing tests**

Add in `mod tests` of `src/app.rs` (`folder_with_notes` exists and makes `rred_app_<name>/notes.txt`):

```rust
    #[test]
    fn test_create_file_action_opens_the_new_file() {
        let dir = folder_with_notes("explorer_create");
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(dir.clone())),
            ..TextEditorApp::default()
        };
        app.apply_explorer_action(ExplorerAction::Create {
            dir: dir.clone(),
            name: "added.rs".into(),
            folder: false,
        });
        assert!(dir.join("added.rs").is_file());
        assert_eq!(app.active_tab().current_path.as_deref(), Some(dir.join("added.rs").as_path()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename_action_moves_the_tab_and_its_language() {
        let dir = folder_with_notes("explorer_rename");
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("notes.txt"));
        app.active_tab_mut().text_content.push_str(" edited");

        app.apply_explorer_action(ExplorerAction::Rename {
            from: dir.join("notes.txt"),
            name: "notes.rs".into(),
        });
        let tab = app.active_tab();
        assert_eq!(tab.current_path.as_deref(), Some(dir.join("notes.rs").as_path()));
        assert_eq!(tab.active_language, Language::Rust);
        assert_eq!(tab.text_content, "notes edited", "unsaved edits stay");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_renaming_a_folder_moves_every_tab_under_it() {
        let dir = folder_with_notes("explorer_rename_dir");
        fs::create_dir(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "a").unwrap();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("src/a.rs"));
        app.open_file_in_tab(dir.join("notes.txt"));

        app.apply_explorer_action(ExplorerAction::Rename { from: dir.join("src"), name: "lib".into() });
        let paths: Vec<_> = app.tabs.iter().filter_map(|t| t.current_path.clone()).collect();
        assert_eq!(paths, vec![dir.join("lib/a.rs"), dir.join("notes.txt")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_close_tabs_under_keeps_unsaved_tabs() {
        let dir = folder_with_notes("explorer_delete_tabs");
        fs::write(dir.join("other.txt"), "other").unwrap();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(dir.join("notes.txt"));
        app.open_file_in_tab(dir.join("other.txt"));
        app.active_tab_mut().text_content.push_str(" edited");

        app.close_tabs_under(&dir);
        let paths: Vec<_> = app.tabs.iter().filter_map(|t| t.current_path.clone()).collect();
        assert_eq!(paths, vec![dir.join("other.txt")], "the clean tab closes, the dirty one stays");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_action_waits_for_confirmation() {
        let dir = folder_with_notes("explorer_delete_ask");
        let mut app = TextEditorApp::default();
        app.apply_explorer_action(ExplorerAction::Delete(dir.join("notes.txt")));
        assert_eq!(app.pending_delete.as_deref(), Some(dir.join("notes.txt").as_path()));
        assert!(dir.join("notes.txt").exists(), "nothing is deleted before the answer");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_rename_reports_an_error() {
        let dir = folder_with_notes("explorer_rename_fail");
        let mut app = TextEditorApp::default();
        app.apply_explorer_action(ExplorerAction::Rename {
            from: dir.join("missing.txt"),
            name: "x.txt".into(),
        });
        assert!(matches!(app.status_message, Some((_, true))));
        let _ = fs::remove_dir_all(&dir);
    }
```

If `Explorer`, `ExplorerAction` or `Language` are not yet imported in the test module, add `use crate::file_browser::{Explorer, ExplorerAction};` / `use crate::highlighter::Language;` at its top.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test explorer_`
Expected: compile errors (`no method apply_explorer_action`, `no field pending_delete`, `no method close_tabs_under`).

- [ ] **Step 3: State and guards**

In `struct TextEditorApp`, after `git_confirm`:

```rust
    /// An Explorer entry waiting for the user to confirm its move to the Trash.
    pub pending_delete: Option<PathBuf>,
```

and `pending_delete: None,` in `Default`. Import at the top of `app.rs`: `use crate::file_browser::ExplorerAction;` (next to the existing `file_browser` import) and `use crate::file_ops;`.

Both dialog guards also wait on the delete question:

- `~706`: `self.show_about_modal || self.pending_action.is_some() || self.git_confirm.is_some() || self.pending_delete.is_some();`
- `~1048`: `if self.pending_action.is_some() || self.show_about_modal || self.git_confirm.is_some() || self.pending_delete.is_some() {`

- [ ] **Step 4: The methods**

Add after `close_tabs_where`:

```rust
    /// Runs a file operation asked for in the Explorer. A delete only
    /// stores the path: the user confirms it first.
    pub fn apply_explorer_action(&mut self, action: ExplorerAction) {
        match action {
            ExplorerAction::Create { dir, name, folder } => {
                let made = if folder {
                    file_ops::create_dir(&dir, &name)
                } else {
                    file_ops::create_file(&dir, &name)
                };
                match made {
                    Ok(path) => {
                        self.after_file_op();
                        if folder {
                            if let Some(explorer) = self.explorer.as_mut() {
                                explorer.expand(&path);
                            }
                        } else {
                            self.open_file_in_tab(path);
                        }
                    }
                    Err(err) => self.file_op_failed(&dir.join(&name), &err),
                }
            }
            ExplorerAction::Rename { from, name } => match file_ops::rename(&from, &name) {
                Ok(to) => {
                    self.rebase_tabs(&from, &to);
                    if let Some(explorer) = self.explorer.as_mut() {
                        explorer.rebase_expanded(&from, &to);
                    }
                    self.after_file_op();
                }
                Err(err) => self.file_op_failed(&from, &err),
            },
            ExplorerAction::Delete(path) => self.pending_delete = Some(path),
            ExplorerAction::Reveal(path) => {
                if let Err(err) = file_ops::reveal(&path) {
                    self.file_op_failed(&path, &err);
                }
            }
        }
    }

    /// Points the tabs open at or under `from` to the same files under
    /// `to`. Their text, undo and bookmarks stay; the language follows a
    /// new extension unless the user picked one, and git is asked for the
    /// new path's HEAD copy.
    pub fn rebase_tabs(&mut self, from: &Path, to: &Path) {
        for tab in &mut self.tabs {
            let Some(path) = tab
                .current_path
                .as_deref()
                .and_then(|path| file_ops::rebase(path, from, to))
            else {
                continue;
            };
            if tab.language_override.is_none() {
                tab.active_language = Language::from_path(&path);
            }
            tab.disk_stamp = disk_stamp(&path);
            tab.git_base = None;
            tab.marks_text = None;
            tab.current_path = Some(path);
        }
    }

    /// Closes the tabs of files at or under `path` (just moved to the
    /// Trash). Tabs with unsaved changes stay, so saving brings them back.
    pub fn close_tabs_under(&mut self, path: &Path) {
        let fallback = self.active_tab_index;
        self.close_tabs_where(fallback, |_, tab| {
            tab.current_path.as_deref().is_some_and(|p| p.starts_with(path))
        });
    }

    /// Asks before moving `pending_delete` to the Trash.
    fn show_delete_confirm(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.pending_delete else { return };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let key = if path.is_dir() {
            "explorer.delete_folder_message"
        } else {
            "explorer.delete_file_message"
        };
        let message = self.i18n.tf(key, &[("name", &name)]);
        let answer = modals::show_confirm_modal(
            ctx,
            self.i18n.t("explorer.delete_title"),
            &message,
            self.i18n.t("explorer.delete_confirm"),
            self.i18n.t("unsaved.cancel"),
        );
        match answer {
            Some(true) => {
                if let Some(path) = self.pending_delete.take() {
                    match file_ops::move_to_trash(&path) {
                        Ok(()) => {
                            self.close_tabs_under(&path);
                            self.after_file_op();
                        }
                        Err(err) => self.file_op_failed(&path, &err),
                    }
                }
            }
            Some(false) => self.pending_delete = None,
            None => {}
        }
    }

    /// The tree and git status after a file was created, renamed or removed.
    fn after_file_op(&mut self) {
        if let Some(explorer) = self.explorer.as_mut() {
            explorer.refresh();
        }
        self.git.request_refresh();
    }

    fn file_op_failed(&mut self, path: &Path, error: &std::io::Error) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let message = self
            .i18n
            .tf("explorer.op_failed", &[("name", &name), ("error", &error.to_string())]);
        self.status_message = Some((message, true));
    }
```

- [ ] **Step 5: Wire the sidebar and the dialog**

In the sidebar block (~2172) add `let mut explorer_action = None;` next to `let mut file_history = None;`, inside the panel closure add `explorer_action = tree.action;` after `find_in_files = tree.find_in_files;`, and after `if find_in_files { … }` add:

```rust
        if let Some(action) = explorer_action {
            self.apply_explorer_action(action);
        }
```

In the modal chain (~2488) extend the last branch:

```rust
        } else if self.git_confirm.is_some() {
            self.show_git_confirm(&ctx);
        } else if self.pending_delete.is_some() {
            self.show_delete_confirm(&ctx);
        }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test explorer_`
Expected: the 6 new tests pass.

Run: `cargo test`
Expected: the whole suite passes.

- [ ] **Step 7: Lint and commit**

Run: `cargo clippy --all-targets`
Expected: no warnings from the new code.

```bash
git add src/app.rs
git commit -m "feat(explorer): create, rename, delete and reveal from the tree"
```

---

### Task 7: Hand check and todo

**Files:**
- Modify: `todo.md`

- [ ] **Step 1: Run the app**

Run: `cargo run --release -- .` (or open the folder with File → Open Folder).

Check by hand, in a scratch folder:

1. Right-click a folder → New File → type `a.rs` → Enter: file appears first in the folder, opens in a tab.
2. Right-click below the tree → New Folder → `tmp` → Enter: folder in the root, expanded.
3. Right-click `a.rs` → Rename: `a` is selected, `.rs` not. Type `b`, Enter: tab title becomes `b.rs`.
4. Rename `b.rs` to an existing name: red note below the field, field stays open; Escape closes it.
5. Edit `b.rs` without saving; right-click → Delete → Move to Trash: tab stays open (dirty); file is in the desktop Trash; Ctrl+S writes it back.
6. Delete `tmp`: confirm text says "and its contents".
7. Reveal in Folder: the file manager opens the containing folder.
8. Type in the Explorer filter, right-click a match: Rename / Delete / Reveal / File History, no New.

- [ ] **Step 2: Update `todo.md`**

Replace the Explorer item with:

```markdown
- [x] **File operations in the Explorer** — done 2026-11-04
  (`src/file_ops.rs`): right-click New File, New Folder, Rename, Delete
  (to the Trash), Reveal in Folder; names typed inline. Later: F2 / Delete
  keys (needs a selected row), New buttons in the header, `a/b.rs` names.
```

and in **Suggested order** replace the line with:

```markdown
Command Palette hand check → auto-indent and auto-close → bracket matching.
```

- [ ] **Step 3: Commit**

```bash
git add todo.md
git commit -m "docs(todo): Explorer file operations done"
```
