//! Git through the `git` program on the `PATH`: finding the repository,
//! reading its status and running the everyday commands.
//!
//! Every call blocks until git exits; `git_worker.rs` runs them off the UI
//! thread. Git never prompts: a command that needs a password fails with
//! [`GitError::AuthRequired`] instead of waiting for input nobody can give.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::process::{self, RunError};

/// How long a local command (status, add, commit, ...) may run.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long fetch, pull and push may run.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a git command failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitError {
    /// `git` is not on the `PATH`.
    NotInstalled,
    /// The folder is not inside a work tree.
    NotRepo,
    /// The remote wants a password or key git may not ask for.
    AuthRequired,
    /// Anything else: git's own message.
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => f.write_str("git not installed"),
            GitError::NotRepo => f.write_str("not a git repository"),
            GitError::AuthRequired => f.write_str("authentication required"),
            GitError::Failed(message) => f.write_str(message),
        }
    }
}

/// What happened to a file, on one side (index or work tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl Kind {
    /// The letter VS Code shows beside the file.
    pub fn letter(self) -> &'static str {
        match self {
            Kind::Added => "A",
            Kind::Modified => "M",
            Kind::Deleted => "D",
            Kind::Renamed => "R",
            Kind::Untracked => "U",
            Kind::Conflicted => "C",
        }
    }

    /// One `XY` status character; `None` for `.` (unchanged).
    fn from_code(code: u8) -> Option<Kind> {
        match code {
            b'M' | b'T' => Some(Kind::Modified),
            b'A' | b'C' => Some(Kind::Added),
            b'D' => Some(Kind::Deleted),
            b'R' => Some(Kind::Renamed),
            b'U' => Some(Kind::Conflicted),
            _ => None,
        }
    }
}

/// One changed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Absolute.
    pub path: PathBuf,
    /// Where a renamed file came from, absolute.
    pub orig_path: Option<PathBuf>,
    /// The change staged in the index.
    pub staged: Option<Kind>,
    /// The change in the work tree not staged yet.
    pub unstaged: Option<Kind>,
}

impl FileChange {
    pub fn is_conflicted(&self) -> bool {
        self.unstaged == Some(Kind::Conflicted)
    }
}

/// `git status` of the whole work tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// The commit HEAD points at; `None` before the first commit.
    pub head: Option<String>,
    /// The checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Short commit id of a detached HEAD.
    pub detached_at: Option<String>,
    /// The repository has no commit yet.
    pub unborn: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileChange>,
}

impl RepoStatus {
    /// What the status bar calls HEAD.
    pub fn head_label(&self) -> String {
        match (&self.branch, &self.detached_at) {
            (Some(branch), _) => branch.clone(),
            (None, Some(oid)) => oid.clone(),
            (None, None) => "HEAD".to_owned(),
        }
    }
}

/// A local branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
}

/// One line of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// Line number in the old file, for context and removed lines.
    pub old: Option<usize>,
    /// Line number in the new file, for context and added lines.
    pub new: Option<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// `@@ -a,b +c,d @@`
    Hunk,
    Context,
    Added,
    Removed,
}

/// A work tree, by its top folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    root: PathBuf,
    /// The `.git` folder (elsewhere for linked work trees).
    git_dir: PathBuf,
}

impl Repo {
    /// The work tree holding `dir`. Its root is spelled the way `dir` is,
    /// so paths built from it match the editor's, symlinks and all.
    pub fn discover(dir: &Path) -> Result<Repo, GitError> {
        let out = run(
            dir,
            &["rev-parse", "--show-toplevel", "--absolute-git-dir"],
            None,
            LOCAL_TIMEOUT,
        )
        .map_err(|e| match e {
            GitError::Failed(_) => GitError::NotRepo,
            other => other,
        })?;
        let mut lines = out.lines().map(|l| l.trim_end_matches('\r'));
        let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else {
            return Err(GitError::NotRepo);
        };
        if top.is_empty() {
            return Err(GitError::NotRepo);
        }
        // git prints forward slashes on Windows too; Path accepts both.
        let top = PathBuf::from(top);
        Ok(Repo {
            root: respell_root(dir, &top).unwrap_or(top),
            git_dir: PathBuf::from(git_dir),
        })
    }

    /// Makes `dir` a repository.
    pub fn init(dir: &Path) -> Result<Repo, GitError> {
        run(dir, &["init"], None, LOCAL_TIMEOUT)?;
        Repo::discover(dir)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Files whose change means HEAD, the branch or the index moved.
    pub fn watched_files(&self) -> [PathBuf; 2] {
        [self.git_dir.join("HEAD"), self.git_dir.join("index")]
    }

    /// Whether `path` lies inside this work tree.
    pub fn contains(&self, path: &Path) -> bool {
        self.relative(path).is_some()
    }

    /// `path` relative to the root, with forward slashes as git spells it.
    fn relative(&self, path: &Path) -> Option<String> {
        let rel = match path.strip_prefix(&self.root) {
            Ok(rel) => rel.to_path_buf(),
            // The root git printed and the path the editor holds may differ
            // in spelling (symlinks, `C:/` vs `C:\`).
            Err(_) => {
                let root = self.root.canonicalize().ok()?;
                // A deleted file cannot be canonicalized; its folder can.
                let parent = path.parent()?.canonicalize().ok()?;
                parent.join(path.file_name()?).strip_prefix(&root).ok()?.to_path_buf()
            }
        };
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
        Some(parts.join("/"))
    }

    fn relatives(&self, paths: &[PathBuf]) -> Vec<String> {
        paths.iter().filter_map(|p| self.relative(p)).collect()
    }

    fn git(&self, args: &[&str]) -> Result<String, GitError> {
        run(&self.root, args, None, LOCAL_TIMEOUT)
    }

    /// Runs `args` followed by `-- paths`.
    fn git_paths(&self, args: &[&str], paths: &[PathBuf]) -> Result<String, GitError> {
        let rel = self.relatives(paths);
        if rel.is_empty() {
            return Ok(String::new());
        }
        let mut argv: Vec<&str> = args.to_vec();
        argv.push("--");
        argv.extend(rel.iter().map(String::as_str));
        self.git(&argv)
    }

    pub fn status(&self) -> Result<RepoStatus, GitError> {
        let out = self.git(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])?;
        Ok(parse_status(&self.root, &out))
    }

    pub fn branches(&self) -> Result<Vec<Branch>, GitError> {
        let out = self.git(&["for-each-ref", "--format=%(HEAD)%00%(refname:short)", "refs/heads"])?;
        Ok(parse_branches(&out))
    }

    pub fn stage(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        // `-A` also stages deletions of the listed paths.
        self.git_paths(&["add", "-A"], paths).map(drop)
    }

    pub fn unstage(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        match self.git_paths(&["restore", "--staged"], paths) {
            Ok(_) => Ok(()),
            // Before the first commit there is no HEAD to restore from.
            Err(GitError::Failed(_)) => self
                .git_paths(&["rm", "--cached", "-r", "-q"], paths)
                .map(drop),
            Err(e) => Err(e),
        }
    }

    /// Throws away the work-tree changes of `changes`: tracked files go back
    /// to the index, untracked ones are deleted.
    pub fn discard(&self, changes: &[FileChange]) -> Result<(), GitError> {
        let (untracked, tracked): (Vec<_>, Vec<_>) = changes
            .iter()
            .partition(|c| c.unstaged == Some(Kind::Untracked));
        let tracked: Vec<PathBuf> = tracked.iter().map(|c| c.path.clone()).collect();
        self.git_paths(&["restore", "--worktree"], &tracked)?;
        for change in untracked {
            std::fs::remove_file(&change.path)
                .map_err(|e| GitError::Failed(format!("{}: {e}", change.path.display())))?;
        }
        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<(), GitError> {
        run(&self.root, &["commit", "-F", "-"], Some(message), LOCAL_TIMEOUT).map(drop)
    }

    pub fn fetch(&self) -> Result<(), GitError> {
        run(&self.root, &["fetch"], None, NETWORK_TIMEOUT).map(drop)
    }

    pub fn pull(&self) -> Result<(), GitError> {
        run(&self.root, &["pull", "--ff-only"], None, NETWORK_TIMEOUT).map(drop)
    }

    /// Pushes the current branch. A branch with no upstream yet gets one on
    /// `origin`, or on the only remote there is.
    pub fn push(&self) -> Result<(), GitError> {
        let status = self.status()?;
        if status.upstream.is_some() {
            return run(&self.root, &["push"], None, NETWORK_TIMEOUT).map(drop);
        }
        let branch = status
            .branch
            .ok_or_else(|| GitError::Failed("HEAD is detached".to_owned()))?;
        let remotes = self.git(&["remote"])?;
        let remotes: Vec<&str> = remotes.lines().filter(|l| !l.is_empty()).collect();
        let remote = match remotes.as_slice() {
            [] => return Err(GitError::Failed("no remote configured".to_owned())),
            [only] => *only,
            several if several.contains(&"origin") => "origin",
            _ => return Err(GitError::Failed("several remotes; push from the console".to_owned())),
        };
        run(&self.root, &["push", "-u", remote, &branch], None, NETWORK_TIMEOUT).map(drop)
    }

    pub fn switch(&self, branch: &str) -> Result<(), GitError> {
        self.git(&["switch", branch]).map(drop)
    }

    pub fn create_branch(&self, branch: &str) -> Result<(), GitError> {
        self.git(&["switch", "-c", branch]).map(drop)
    }

    /// The file at `path` as HEAD has it, converted as a checkout would
    /// (line endings, filters). `None` when HEAD does not have it.
    pub fn head_text(&self, path: &Path) -> Option<String> {
        let rel = self.relative(path)?;
        self.git(&["cat-file", "--filters", &format!("HEAD:{rel}")]).ok()
    }

    /// The unified diff of `path`: staged changes against HEAD, or work-tree
    /// changes against the index.
    pub fn diff(&self, path: &Path, staged: bool) -> Result<Vec<DiffLine>, GitError> {
        let mut args = vec!["diff", "--no-ext-diff", "--no-color"];
        if staged {
            args.push("--cached");
        }
        let out = self.git_paths(&args, &[path.to_path_buf()])?;
        Ok(parse_diff(&out))
    }
}

/// `top`, git's spelling of the work tree holding `dir`, spelled as an
/// ancestor of `dir` instead: `/home/me/link/src` in a work tree git calls
/// `/data/repo` gives `/home/me/link`.
fn respell_root(dir: &Path, top: &Path) -> Option<PathBuf> {
    if dir.starts_with(top) {
        return None;
    }
    let depth = dir
        .canonicalize()
        .ok()?
        .strip_prefix(top.canonicalize().ok()?)
        .ok()?
        .components()
        .count();
    let mut root = dir.to_path_buf();
    for _ in 0..depth {
        root.pop();
    }
    Some(root)
}

/// Runs git in `dir` and returns its stdout.
fn run(dir: &Path, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<String, GitError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
        .args(args)
        // Fail instead of waiting for a password nobody can type.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        // English messages, so failures can be recognized.
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C");
    let output = process::run(command, stdin.map(str::as_bytes), timeout).map_err(|e| match e {
        RunError::NotInstalled => GitError::NotInstalled,
        RunError::Io(e) => GitError::Failed(format!("git: {e}")),
        RunError::TimedOut => GitError::Failed("git timed out".to_owned()),
    })?;
    if !output.status.success() {
        return Err(classify(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The error git's `stderr` describes.
fn classify(stderr: &str) -> GitError {
    const AUTH: [&str; 5] = [
        "terminal prompts disabled",
        "could not read Username",
        "could not read Password",
        "Authentication failed",
        "Permission denied (publickey",
    ];
    if AUTH.iter().any(|needle| stderr.contains(needle)) {
        return GitError::AuthRequired;
    }
    if stderr.contains("not a git repository") {
        return GitError::NotRepo;
    }
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    let line = lines()
        .find_map(|l| l.strip_prefix("fatal: ").or_else(|| l.strip_prefix("error: ")))
        .or_else(|| lines().next())
        .unwrap_or("git failed");
    GitError::Failed(line.to_owned())
}

/// Parses `git status --porcelain=v2 -z --branch`. Paths are joined to `root`.
pub fn parse_status(root: &Path, out: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut oid = None;
    let mut fields = out.split('\0');
    while let Some(record) = fields.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => oid = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some((tag, rest)) = record.split_once(' ') else {
            continue;
        };
        let change = match tag {
            "1" | "2" => {
                // 1 XY sub mH mI mW hH hI path
                // 2 XY sub mH mI mW hH hI Xscore path, then origPath
                let skip = if tag == "1" { 7 } else { 8 };
                let mut parts = rest.splitn(skip + 1, ' ');
                let xy = parts.next().unwrap_or("..").as_bytes();
                let Some(path) = parts.nth(skip - 1) else { continue };
                let orig_path = if tag == "2" {
                    fields.next().map(|p| root.join(p))
                } else {
                    None
                };
                FileChange {
                    path: root.join(path),
                    orig_path,
                    staged: xy.first().copied().and_then(Kind::from_code),
                    unstaged: xy.get(1).copied().and_then(Kind::from_code),
                }
            }
            "u" => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let Some(path) = rest.splitn(10, ' ').nth(9) else { continue };
                FileChange {
                    path: root.join(path),
                    orig_path: None,
                    staged: None,
                    unstaged: Some(Kind::Conflicted),
                }
            }
            "?" => FileChange {
                path: root.join(rest),
                orig_path: None,
                staged: None,
                unstaged: Some(Kind::Untracked),
            },
            _ => continue,
        };
        status.files.push(change);
    }
    match oid {
        Some(oid) if oid == "(initial)" => status.unborn = true,
        Some(oid) => {
            if status.branch.is_none() {
                status.detached_at = Some(oid.chars().take(7).collect());
            }
            status.head = Some(oid);
        }
        None => {}
    }
    status
}

/// Parses `git for-each-ref --format=%(HEAD)%00%(refname:short)`.
pub fn parse_branches(out: &str) -> Vec<Branch> {
    out.lines()
        .filter_map(|line| {
            let (head, name) = line.split_once('\0')?;
            Some(Branch {
                name: name.to_owned(),
                current: head == "*",
            })
        })
        .collect()
}

/// Parses a unified diff of one file, dropping the `diff --git` and `---` /
/// `+++` header lines.
pub fn parse_diff(out: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    for line in out.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            // @@ -a[,b] +c[,d] @@ context
            let mut ranges = header.split(' ');
            let start = |r: Option<&str>, sign| {
                r.and_then(|r| r.strip_prefix(sign))
                    .and_then(|r| r.split(',').next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0)
            };
            old = start(ranges.next(), '-');
            new = start(ranges.next(), '+');
            in_hunk = true;
            lines.push(DiffLine {
                kind: DiffKind::Hunk,
                old: None,
                new: None,
                text: line.to_owned(),
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (DiffKind::Added, &line[1..]),
            Some(b'-') => (DiffKind::Removed, &line[1..]),
            Some(b' ') => (DiffKind::Context, &line[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            None => (DiffKind::Context, ""),
            _ => {
                // The next file's header: one file was asked for.
                in_hunk = false;
                continue;
            }
        };
        let (o, n) = match kind {
            DiffKind::Added => (None, Some(new)),
            DiffKind::Removed => (Some(old), None),
            _ => (Some(old), Some(new)),
        };
        if o.is_some() {
            old += 1;
        }
        if n.is_some() {
            new += 1;
        }
        lines.push(DiffLine {
            kind,
            old: o,
            new: n,
            text: text.to_owned(),
        });
    }
    lines
}

/// A fresh repository in the temp folder, on branch `main`, or `None`
/// without git.
#[cfg(test)]
pub fn temp_repo(name: &str) -> Option<Repo> {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("red_git_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let repo = match Repo::init(&dir) {
        Ok(repo) => repo,
        Err(GitError::NotInstalled) => return None,
        Err(e) => panic!("{e}"),
    };
    // Keep the user's global settings (signing, hooks) out of the tests.
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
        ("core.autocrlf", "false"),
    ] {
        repo.git(&["config", key, value]).unwrap();
    }
    repo.git(&["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();
    Some(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    #[test]
    fn test_status_branch_headers() {
        let out = "# branch.oid 0123456789abcdef\0# branch.head main\0\
                   # branch.upstream origin/main\0# branch.ab +2 -3\0";
        let status = parse_status(&root(), out);
        assert_eq!(status.head.as_deref(), Some("0123456789abcdef"));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        assert!(!status.unborn);
        assert_eq!(status.detached_at, None);
        assert!(status.files.is_empty());
    }

    #[test]
    fn test_status_detached_and_unborn() {
        let detached = parse_status(&root(), "# branch.oid abcdef0123456\0# branch.head (detached)\0");
        assert_eq!(detached.branch, None);
        assert_eq!(detached.head_label(), "abcdef0");
        let unborn = parse_status(&root(), "# branch.oid (initial)\0# branch.head main\0");
        assert!(unborn.unborn);
        assert_eq!(unborn.head_label(), "main");
    }

    #[test]
    fn test_status_entries() {
        let out = "1 .M N... 100644 100644 100644 aaa bbb src/main.rs\0\
                   1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
                   1 A. N... 000000 100644 100644 000 bbb dir/new file.txt\0\
                   1 D. N... 100644 000000 000000 aaa 000 gone.rs\0\
                   2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs\0old name.rs\0\
                   u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
                   ? notes/todo.md\0\
                   ! target/\0";
        let files = parse_status(&root(), out).files;
        let r = root();
        assert_eq!(
            files,
            vec![
                FileChange { path: r.join("src/main.rs"), orig_path: None, staged: None, unstaged: Some(Kind::Modified) },
                FileChange { path: r.join("both.rs"), orig_path: None, staged: Some(Kind::Modified), unstaged: Some(Kind::Modified) },
                FileChange { path: r.join("dir/new file.txt"), orig_path: None, staged: Some(Kind::Added), unstaged: None },
                FileChange { path: r.join("gone.rs"), orig_path: None, staged: Some(Kind::Deleted), unstaged: None },
                FileChange {
                    path: r.join("new name.rs"),
                    orig_path: Some(r.join("old name.rs")),
                    staged: Some(Kind::Renamed),
                    unstaged: None,
                },
                FileChange { path: r.join("conflict.rs"), orig_path: None, staged: None, unstaged: Some(Kind::Conflicted) },
                FileChange { path: r.join("notes/todo.md"), orig_path: None, staged: None, unstaged: Some(Kind::Untracked) },
            ]
        );
        assert!(files[5].is_conflicted());
    }

    #[test]
    fn test_branches() {
        let branches = parse_branches(" \0feature/x\n*\0main\n");
        assert_eq!(
            branches,
            vec![
                Branch { name: "feature/x".into(), current: false },
                Branch { name: "main".into(), current: true },
            ]
        );
    }

    #[test]
    fn test_diff_numbers_lines() {
        let out = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n\
                   @@ -2,3 +2,3 @@ fn x\n a\n-b\n+B\n c\n\\ No newline at end of file\n";
        let lines = parse_diff(out);
        let kinds: Vec<_> = lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (DiffKind::Hunk, None, None, "@@ -2,3 +2,3 @@ fn x"),
                (DiffKind::Context, Some(2), Some(2), "a"),
                (DiffKind::Removed, Some(3), None, "b"),
                (DiffKind::Added, None, Some(3), "B"),
                (DiffKind::Context, Some(4), Some(4), "c"),
            ]
        );
    }

    #[test]
    fn test_classify_errors() {
        assert_eq!(
            classify("fatal: could not read Username for 'https://x': terminal prompts disabled\n"),
            GitError::AuthRequired
        );
        assert_eq!(classify("git@x: Permission denied (publickey).\n"), GitError::AuthRequired);
        assert_eq!(
            classify("hint: something\nerror: pathspec 'x' did not match\n"),
            GitError::Failed("pathspec 'x' did not match".into())
        );
        assert_eq!(classify("weird\n"), GitError::Failed("weird".into()));
    }

    #[test]
    fn test_discover_outside_a_repo() {
        let dir = std::env::temp_dir().join(format!("red_git_none_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let found = Repo::discover(&dir);
        // The temp folder may itself sit in a repository on some machines.
        if let Err(e) = found {
            assert!(matches!(e, GitError::NotRepo | GitError::NotInstalled), "{e:?}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_stage_commit_and_branch_round_trip() {
        let Some(repo) = temp_repo("round_trip") else { return };
        let root = repo.root().to_path_buf();
        let file = root.join("a.txt");
        fs::write(&file, "one\n").unwrap();

        let status = repo.status().unwrap();
        assert!(status.unborn);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].unstaged, Some(Kind::Untracked));
        assert!(repo.contains(&file));
        assert_eq!(repo.head_text(&file), None);

        repo.stage(std::slice::from_ref(&file)).unwrap();
        assert_eq!(repo.status().unwrap().files[0].staged, Some(Kind::Added));
        repo.unstage(std::slice::from_ref(&file)).unwrap();
        assert_eq!(repo.status().unwrap().files[0].unstaged, Some(Kind::Untracked));

        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first\n\nbody").unwrap();
        let status = repo.status().unwrap();
        assert!(!status.unborn);
        assert!(status.files.is_empty());
        assert_eq!(repo.head_text(&file).as_deref(), Some("one\n"));

        fs::write(&file, "two\n").unwrap();
        let change = repo.status().unwrap().files.remove(0);
        assert_eq!(change.unstaged, Some(Kind::Modified));
        let diff = repo.diff(&file, false).unwrap();
        assert!(diff.iter().any(|l| l.kind == DiffKind::Added && l.text == "two"));
        repo.discard(&[change]).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "one\n");

        let stray = root.join("stray.txt");
        fs::write(&stray, "x").unwrap();
        let untracked = repo.status().unwrap().files;
        repo.discard(&untracked).unwrap();
        assert!(!stray.exists());

        repo.create_branch("feature").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("feature"));
        let names: Vec<_> = repo.branches().unwrap().into_iter().map(|b| (b.name, b.current)).collect();
        assert_eq!(names, vec![("feature".into(), true), ("main".into(), false)]);
        repo.switch("main").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));

        assert_eq!(repo.push(), Err(GitError::Failed("no remote configured".into())));
        fs::remove_dir_all(&root).unwrap();
    }
}
