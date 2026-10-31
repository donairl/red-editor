//! Runs git off the UI thread and keeps what it last reported: the
//! repository of the open folder, its status and branches, and HEAD's copy
//! of the file in the active tab.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;

use eframe::egui;

use crate::git::{Branch, FileChange, GitError, Kind, Repo, RepoStatus};

/// A command that changes the repository. One runs at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Discard(Vec<FileChange>),
    Commit(String),
    /// Stages everything, then commits.
    CommitAll(String),
    Fetch,
    Pull,
    Push,
    Switch(String),
    CreateBranch(String),
    Init(PathBuf),
}

impl Op {
    /// Whether the files on disk may have changed under the open tabs.
    pub fn touches_work_tree(&self) -> bool {
        matches!(
            self,
            Op::Discard(_) | Op::Pull | Op::Switch(_) | Op::CreateBranch(_)
        )
    }

    fn run(&self, repo: Option<&Repo>) -> Result<(), GitError> {
        let repo = match (self, repo) {
            (Op::Init(dir), _) => return Repo::init(dir).map(drop),
            (_, Some(repo)) => repo,
            (_, None) => return Err(GitError::NotRepo),
        };
        match self {
            Op::Stage(paths) => repo.stage(paths),
            Op::Unstage(paths) => repo.unstage(paths),
            Op::Discard(changes) => repo.discard(changes),
            Op::Commit(message) => repo.commit(message),
            Op::CommitAll(message) => repo.stage_all().and_then(|()| repo.commit(message)),
            Op::Fetch => repo.fetch(),
            Op::Pull => repo.pull(),
            Op::Push => repo.push(),
            Op::Switch(branch) => repo.switch(branch),
            Op::CreateBranch(branch) => repo.create_branch(branch),
            Op::Init(_) => unreachable!(),
        }
    }
}

enum Message {
    Discovered {
        dir: PathBuf,
        repo: Result<Repo, GitError>,
    },
    Status(Result<(RepoStatus, Vec<Branch>), GitError>),
    Done(Op, Result<(), GitError>),
    Base {
        path: PathBuf,
        head: Option<String>,
        text: Option<String>,
    },
}

/// HEAD's copy of a file, for the gutter's change markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub path: PathBuf,
    /// The commit it was read from.
    pub head: Option<String>,
    /// `None` when HEAD does not have the file.
    pub text: Option<String>,
}

/// Status colours for the Explorer, by absolute path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decorations {
    files: HashMap<PathBuf, Kind>,
    /// Folders holding a changed file, with the most notable change.
    dirs: HashMap<PathBuf, Kind>,
}

impl Decorations {
    pub fn new(root: &Path, status: &RepoStatus) -> Self {
        let mut decorations = Decorations::default();
        for change in &status.files {
            let Some(kind) = shown_kind(change) else { continue };
            decorations.files.insert(change.path.clone(), kind);
            let mut dir = change.path.parent();
            while let Some(d) = dir.filter(|d| d.starts_with(root) && *d != root) {
                let slot = decorations.dirs.entry(d.to_path_buf()).or_insert(kind);
                if rank(kind) > rank(*slot) {
                    *slot = kind;
                }
                dir = d.parent();
            }
        }
        decorations
    }

    pub fn file(&self, path: &Path) -> Option<Kind> {
        self.files.get(path).copied()
    }

    pub fn dir(&self, path: &Path) -> Option<Kind> {
        self.dirs.get(path).copied()
    }
}

/// The change a file is marked with: the work tree's if any, else the
/// staged one.
pub fn shown_kind(change: &FileChange) -> Option<Kind> {
    change.unstaged.or(change.staged)
}

/// Which change colours a folder holding several.
fn rank(kind: Kind) -> u8 {
    match kind {
        Kind::Conflicted => 4,
        Kind::Deleted => 3,
        Kind::Modified | Kind::Renamed => 2,
        Kind::Added | Kind::Untracked => 1,
    }
}

/// Git state of the open folder.
pub struct GitState {
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    /// The folder the repository was looked up from.
    dir: Option<PathBuf>,
    discovering: bool,
    repo: Option<Repo>,
    /// Why there is no repository, once looked up.
    pub missing: Option<GitError>,
    pub status: Option<RepoStatus>,
    pub branches: Vec<Branch>,
    pub decorations: Decorations,
    refreshing: bool,
    /// Another refresh was asked for while one ran, or by code with no
    /// `egui::Context` at hand.
    refresh_again: bool,
    /// The running command.
    busy: Option<Op>,
    /// Finished commands, for the app to report.
    finished: Vec<(Op, Result<(), GitError>)>,
    /// Modified times of [`Repo::watched_files`] at the last refresh.
    stamps: Vec<Option<SystemTime>>,
    /// Bases asked for and not delivered yet.
    base_requests: HashSet<PathBuf>,
    bases: Vec<Base>,
}

impl Default for GitState {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        GitState {
            sender,
            receiver,
            dir: None,
            discovering: false,
            repo: None,
            missing: None,
            status: None,
            branches: Vec::new(),
            decorations: Decorations::default(),
            refreshing: false,
            refresh_again: false,
            busy: None,
            finished: Vec::new(),
            stamps: Vec::new(),
            base_requests: HashSet::new(),
            bases: Vec::new(),
        }
    }
}

impl GitState {
    pub fn repo(&self) -> Option<&Repo> {
        self.repo.as_ref()
    }

    pub fn busy(&self) -> Option<&Op> {
        self.busy.as_ref()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing || self.discovering
    }

    /// Number of changed files, for the Activity Bar badge.
    pub fn change_count(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.files.len())
    }

    /// The change recorded for `path`, if it is a changed file.
    pub fn change(&self, path: &Path) -> Option<&FileChange> {
        self.status.as_ref()?.files.iter().find(|c| c.path == path)
    }

    /// Looks up the repository holding `dir`, forgetting the previous one.
    /// A folder inside the current repository keeps it.
    pub fn open(&mut self, ctx: &egui::Context, dir: PathBuf) {
        if self.dir.as_ref() == Some(&dir) {
            return;
        }
        if let Some(repo) = &self.repo
            && dir.starts_with(repo.root())
        {
            self.dir = Some(dir);
            return;
        }
        *self = GitState {
            dir: Some(dir.clone()),
            discovering: true,
            ..GitState::default()
        };
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let repo = Repo::discover(&dir);
            let _ = sender.send(Message::Discovered { dir, repo });
            ctx.request_repaint();
        });
    }

    /// Reads the status again.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        let Some(repo) = self.repo.clone() else { return };
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        self.stamps = stamps(&repo);
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = repo
                .status()
                .and_then(|status| Ok((status, repo.branches()?)));
            let _ = sender.send(Message::Status(result));
            ctx.request_repaint();
        });
    }

    /// Refreshes on the next [`Self::poll`].
    pub fn request_refresh(&mut self) {
        self.refresh_again = true;
    }

    /// Refreshes when git's HEAD or index changed since the last refresh,
    /// as a commit or `git add` in the console does. Costs two `stat`s.
    pub fn refresh_if_changed(&mut self, ctx: &egui::Context) {
        if let Some(repo) = &self.repo
            && stamps(repo) != self.stamps
        {
            self.refresh(ctx);
        }
    }

    /// Starts `op` unless another command is running; returns whether it
    /// started.
    pub fn run(&mut self, ctx: &egui::Context, op: Op) -> bool {
        if self.busy.is_some() {
            return false;
        }
        if self.repo.is_none() && !matches!(op, Op::Init(_)) {
            return false;
        }
        self.busy = Some(op.clone());
        let repo = self.repo.clone();
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = op.run(repo.as_ref());
            let _ = sender.send(Message::Done(op, result));
            ctx.request_repaint();
        });
        true
    }

    /// Reads HEAD's copy of `path`, delivered by [`Self::take_bases`].
    pub fn request_base(&mut self, ctx: &egui::Context, path: &Path) {
        let Some(repo) = self.repo.clone() else { return };
        if !repo.contains(path) || !self.base_requests.insert(path.to_path_buf()) {
            return;
        }
        let head = self.status.as_ref().and_then(|s| s.head.clone());
        let path = path.to_path_buf();
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let text = if head.is_some() { repo.head_text(&path) } else { None };
            let _ = sender.send(Message::Base { path, head, text });
            ctx.request_repaint();
        });
    }

    pub fn take_bases(&mut self) -> Vec<Base> {
        std::mem::take(&mut self.bases)
    }

    pub fn take_finished(&mut self) -> Vec<(Op, Result<(), GitError>)> {
        std::mem::take(&mut self.finished)
    }

    /// Takes in what the git threads sent since the last frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                Message::Discovered { dir, repo } => {
                    if self.dir.as_ref() != Some(&dir) {
                        continue;
                    }
                    self.discovering = false;
                    match repo {
                        Ok(repo) => {
                            self.repo = Some(repo);
                            self.refresh(ctx);
                        }
                        Err(e) => self.missing = Some(e),
                    }
                }
                Message::Status(result) => {
                    self.refreshing = false;
                    match result {
                        Ok((status, branches)) => {
                            if let Some(repo) = &self.repo {
                                self.decorations = Decorations::new(repo.root(), &status);
                            }
                            self.status = Some(status);
                            self.branches = branches;
                        }
                        // A repository deleted under the editor, most likely.
                        Err(_) => {
                            self.status = None;
                            self.decorations = Decorations::default();
                        }
                    }
                    if std::mem::take(&mut self.refresh_again) {
                        self.refresh(ctx);
                    }
                }
                Message::Done(op, result) => {
                    self.busy = None;
                    if let (Op::Init(dir), Ok(())) = (&op, &result) {
                        let dir = dir.clone();
                        self.dir = None;
                        self.open(ctx, dir);
                    } else {
                        self.refresh(ctx);
                    }
                    self.finished.push((op, result));
                }
                Message::Base { path, head, text } => {
                    self.base_requests.remove(&path);
                    self.bases.push(Base { path, head, text });
                }
            }
        }
        if self.refresh_again && !self.refreshing {
            self.refresh_again = false;
            self.refresh(ctx);
        }
    }
}

fn stamps(repo: &Repo) -> Vec<Option<SystemTime>> {
    repo.watched_files()
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decorations_mark_files_and_their_folders() {
        let root = PathBuf::from("/repo");
        let change = |path: &str, staged, unstaged| FileChange {
            path: root.join(path),
            orig_path: None,
            staged,
            unstaged,
        };
        let status = RepoStatus {
            files: vec![
                change("src/a.rs", Some(Kind::Modified), None),
                change("src/ui/b.rs", None, Some(Kind::Untracked)),
                change("src/ui/c.rs", Some(Kind::Added), Some(Kind::Deleted)),
            ],
            ..RepoStatus::default()
        };
        let d = Decorations::new(&root, &status);
        assert_eq!(d.file(&root.join("src/a.rs")), Some(Kind::Modified));
        assert_eq!(d.file(&root.join("src/ui/c.rs")), Some(Kind::Deleted));
        assert_eq!(d.dir(&root.join("src/ui")), Some(Kind::Deleted));
        assert_eq!(d.dir(&root.join("src")), Some(Kind::Deleted));
        assert_eq!(d.dir(&root), None);
        assert_eq!(d.file(&root.join("README.md")), None);
    }
}
