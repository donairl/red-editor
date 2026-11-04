//! File-system work behind the Explorer's right-click menu: checking a
//! typed name, creating, renaming, moving to the Trash and revealing.
//! Holds no egui or app state, so every function is tested on a temp folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
}
