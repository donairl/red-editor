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
