//! Runs git off the UI thread and keeps what it last reported: the
//! repository of the open folder, its status and branches, and HEAD's copy
//! of the file in the active tab.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;

use eframe::egui;

use crate::blame::{self, Blame};
use crate::git::{Branch, Commit, CommitDetails, CommitFile, DiffLine, FileChange, GitError, Kind, Repo, RepoStatus};

/// Commits the log loads at a time.
pub const LOG_PAGE: usize = 100;

/// Commits the Git Graph loads at a time.
pub const GRAPH_PAGE: usize = 200;

/// A command that changes the repository. One runs at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    /// Stage the change at a 1-based line of `text`, the editor's copy of
    /// the file.
    StageHunk { path: PathBuf, text: String, line: usize },
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
    /// Make the work tree's file what it was in the commit.
    RestoreFile { hash: String, path: PathBuf },
    /// Check the commit out, detaching HEAD.
    Checkout(String),
    /// Make a branch point at the commit, without switching to it.
    CreateBranchAt { name: String, hash: String },
    /// Delete a fully merged local branch.
    DeleteBranch(String),
}

impl Op {
    /// Whether the files on disk may have changed under the open tabs.
    pub fn touches_work_tree(&self) -> bool {
        matches!(
            self,
            Op::Discard(_)
                | Op::Pull
                | Op::Switch(_)
                | Op::CreateBranch(_)
                | Op::RestoreFile { .. }
                | Op::Checkout(_)
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
            Op::StageHunk { path, text, line } => repo.stage_hunk(path, text, *line),
            Op::Discard(changes) => repo.discard(changes),
            Op::Commit(message) => repo.commit(message),
            Op::CommitAll(message) => repo.stage_all().and_then(|()| repo.commit(message)),
            Op::Fetch => repo.fetch(),
            Op::Pull => repo.pull(),
            Op::Push => repo.push(),
            Op::Switch(branch) => repo.switch(branch),
            Op::CreateBranch(branch) => repo.create_branch(branch),
            Op::RestoreFile { hash, path } => repo.restore_from(hash, path),
            Op::Checkout(hash) => repo.checkout_detached(hash),
            Op::CreateBranchAt { name, hash } => repo.branch_at(name, hash),
            Op::DeleteBranch(name) => repo.delete_branch(name),
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
    Diff {
        path: PathBuf,
        source: DiffSource,
        lines: Result<Vec<DiffLine>, GitError>,
    },
    Log {
        all: bool,
        generation: u64,
        filter: Option<PathBuf>,
        skip: usize,
        commits: Result<Vec<Commit>, GitError>,
    },
    CommitDetails {
        hash: String,
        details: Result<CommitDetails, GitError>,
    },
    CommitFiles {
        hash: String,
        files: Result<Vec<CommitFile>, GitError>,
    },
    Blame {
        path: PathBuf,
        head: String,
        blame: Option<Blame>,
    },
}

/// What a file's diff is taken against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffSource {
    /// Work-tree changes against the index.
    WorkTree,
    /// Staged changes against HEAD.
    Staged,
    /// What this commit changed.
    Commit(String),
}

/// The changes of one file, shown beside the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub source: DiffSource,
    /// `None` until git first answers.
    pub lines: Option<Result<Vec<DiffLine>, GitError>>,
}

/// The commit log as loaded so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    /// Every ref's history (the Git Graph) rather than HEAD's.
    pub all: bool,
    /// Bumped each time the log starts over, so a page of the old log is
    /// dropped when it arrives late.
    pub generation: u64,
    /// Only commits touching this file.
    pub filter: Option<PathBuf>,
    pub commits: Vec<Commit>,
    /// Every commit is loaded; no more to ask for.
    pub complete: bool,
    pub loading: bool,
    pub error: Option<GitError>,
    /// The commit whose files are shown.
    pub expanded: Option<String>,
    /// Commits received so far, repeats included; the next page skips
    /// this many.
    pub fetched: usize,
    /// Hashes received so far; a page that shifted under the log can
    /// repeat one.
    seen: std::collections::HashSet<String>,
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
    /// The diff shown beside the editor; read again after each refresh.
    diff: Option<FileDiff>,
    pub log: Log,
    /// HEAD when the log was loaded; it reloads when HEAD moves.
    log_head: Option<String>,
    /// The log of every ref, for the Git Graph.
    pub graph: Log,
    /// HEAD and the branch names when the graph was loaded; it reloads
    /// when either changes.
    graph_key: Option<(Option<String>, Vec<String>)>,
    /// Date and message of commits the detail pane showed; None while loading.
    pub(crate) commit_details: HashMap<String, Option<Result<CommitDetails, GitError>>>,
    /// A command moved refs the graph key cannot see; reload on the next refresh.
    graph_stale: bool,
    /// The files of commits already shown; None while loading.
    pub(crate) commit_files: HashMap<String, Option<Result<Vec<CommitFile>, GitError>>>,
    /// Blame by file and the HEAD it was read at; `None` while loading or
    /// when git has no blame for the file.
    blames: HashMap<(PathBuf, String), Option<Blame>>,
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
            diff: None,
            log: Log::default(),
            log_head: None,
            graph: Log::default(),
            graph_key: None,
            commit_details: HashMap::new(),
            graph_stale: false,
            commit_files: HashMap::new(),
            blames: HashMap::new(),
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

    /// Whether the repository is still being looked up.
    pub fn is_discovering(&self) -> bool {
        self.discovering
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

    /// The blame of `path` at the current HEAD, asking git for it the first
    /// time. `None` until it arrives, or when git has none (untracked).
    pub fn blame(&mut self, ctx: &egui::Context, path: &Path) -> Option<&Blame> {
        let repo = self.repo.clone()?;
        let head = self.status.as_ref()?.head.clone()?;
        let key = (path.to_path_buf(), head.clone());
        if !self.blames.contains_key(&key) {
            if !repo.contains(path) {
                return None;
            }
            self.blames.insert(key.clone(), None);
            let path = path.to_path_buf();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let blame = repo
                    .blame(&path)
                    .ok()
                    .map(|out| Blame::new(blame::parse_porcelain(&out), blame::now()));
                let _ = sender.send(Message::Blame { path, head, blame });
                ctx.request_repaint();
            });
        }
        self.blames.get(&key)?.as_ref()
    }

    pub fn diff(&self) -> Option<&FileDiff> {
        self.diff.as_ref()
    }

    /// Shows the changes of `path` against `source` beside the editor.
    pub fn show_diff(&mut self, ctx: &egui::Context, path: PathBuf, source: DiffSource) {
        self.diff = Some(FileDiff {
            path,
            source,
            lines: None,
        });
        self.load_diff(ctx);
    }

    /// The files `hash` changed, once loaded; asks for them otherwise.
    pub fn commit_files(&mut self, ctx: &egui::Context, hash: &str) -> Option<&Result<Vec<CommitFile>, GitError>> {
        if !self.commit_files.contains_key(hash) {
            let repo = self.repo.clone()?;
            // Marked pending so one request goes out per commit.
            self.commit_files.insert(hash.to_owned(), None);
            let hash = hash.to_owned();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let files = repo.commit_files(&hash);
                let _ = sender.send(Message::CommitFiles { hash, files });
                ctx.request_repaint();
            });
            return None;
        }
        self.commit_files.get(hash)?.as_ref()
    }

    /// Starts the log over, for `filter`'s history or the whole branch's.
    pub fn load_log(&mut self, ctx: &egui::Context, filter: Option<PathBuf>) {
        self.log = Log {
            filter,
            generation: self.log.generation + 1,
            ..Log::default()
        };
        self.log_head = self.status.as_ref().and_then(|s| s.head.clone());
        self.load_more(ctx, false);
    }

    /// Asks for the next page of the log.
    pub fn load_more_log(&mut self, ctx: &egui::Context) {
        self.load_more(ctx, false);
    }

    /// Starts the Git Graph's log over.
    pub fn load_graph(&mut self, ctx: &egui::Context) {
        self.graph = Log {
            all: true,
            generation: self.graph.generation + 1,
            ..Log::default()
        };
        self.graph_key = Some(self.graph_key_now());
        self.load_more(ctx, true);
    }

    /// Asks for the next page of the Git Graph's log.
    pub fn load_more_graph(&mut self, ctx: &egui::Context) {
        self.load_more(ctx, true);
    }

    fn load_more(&mut self, ctx: &egui::Context, all: bool) {
        let Some(repo) = self.repo.clone() else { return };
        let log = if all { &mut self.graph } else { &mut self.log };
        if log.loading || log.complete {
            return;
        }
        log.loading = true;
        let generation = log.generation;
        let filter = log.filter.clone();
        let skip = log.fetched;
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let commits = if all {
                repo.log_all(skip, GRAPH_PAGE)
            } else {
                repo.log(skip, LOG_PAGE, filter.as_deref())
            };
            let _ = sender.send(Message::Log { all, generation, filter, skip, commits });
            ctx.request_repaint();
        });
    }

    /// What the graph depends on: HEAD and the local branches.
    fn graph_key_now(&self) -> (Option<String>, Vec<String>) {
        (
            self.status.as_ref().and_then(|s| s.head.clone()),
            self.branches.iter().map(|b| b.name.clone()).collect(),
        )
    }

    /// Reloads the graph when HEAD, a branch or a remote ref moved since it
    /// was loaded. Only a graph that was asked for once is kept up to date.
    fn refresh_graph(&mut self, ctx: &egui::Context) {
        let stale = std::mem::take(&mut self.graph_stale);
        if self.graph_key.as_ref().is_some_and(|k| stale || *k != self.graph_key_now()) {
            self.load_graph(ctx);
        }
    }

    /// The date and message of `hash`, once loaded; asks for them otherwise.
    pub fn commit_details(&mut self, ctx: &egui::Context, hash: &str) -> Option<&Result<CommitDetails, GitError>> {
        if !self.commit_details.contains_key(hash) {
            let repo = self.repo.clone()?;
            // Marked pending so one request goes out per commit.
            self.commit_details.insert(hash.to_owned(), None);
            let hash = hash.to_owned();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let details = repo.commit_details(&hash);
                let _ = sender.send(Message::CommitDetails { hash, details });
                ctx.request_repaint();
            });
            return None;
        }
        self.commit_details.get(hash)?.as_ref()
    }

    /// Reloads the log when HEAD moved since it was loaded. Only a log
    /// that was asked for once is kept up to date.
    fn refresh_log(&mut self, ctx: &egui::Context) {
        let head = self.status.as_ref().and_then(|s| s.head.clone());
        let loaded = !self.log.commits.is_empty() || self.log.complete;
        if loaded && head != self.log_head {
            self.commit_files.clear();
            let filter = self.log.filter.clone();
            self.load_log(ctx, filter);
        }
    }

    pub fn close_diff(&mut self) {
        self.diff = None;
    }

    /// Reads the shown diff again, keeping the old lines until it arrives.
    fn load_diff(&mut self, ctx: &egui::Context) {
        let (Some(repo), Some(diff)) = (self.repo.clone(), &self.diff) else { return };
        let (path, source) = (diff.path.clone(), diff.source.clone());
        let untracked = source == DiffSource::WorkTree
            && self
                .change(&path)
                .is_some_and(|c| c.unstaged == Some(Kind::Untracked));
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let lines = match &source {
                _ if untracked => repo.diff_new_file(&path),
                DiffSource::Commit(hash) => repo.commit_diff(hash, &path),
                DiffSource::Staged => repo.diff(&path, true),
                DiffSource::WorkTree => repo.diff(&path, false),
            };
            let _ = sender.send(Message::Diff { path, source, lines });
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
                            self.load_diff(ctx);
                            self.refresh_log(ctx);
                            self.refresh_graph(ctx);
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
                    if matches!(op, Op::Fetch | Op::Pull | Op::Push) && result.is_ok() {
                        self.graph_stale = true;
                    }
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
                Message::Diff { path, source, lines } => {
                    if let Some(diff) = &mut self.diff
                        && diff.path == path
                        && diff.source == source
                    {
                        diff.lines = Some(lines);
                    }
                }
                Message::Log { all, generation, filter, skip, commits } => {
                    let page = if all { GRAPH_PAGE } else { LOG_PAGE };
                    let log = if all { &mut self.graph } else { &mut self.log };
                    // A stale page, from before the log was started over.
                    if log.generation != generation || log.filter != filter || log.fetched != skip {
                        continue;
                    }
                    log.loading = false;
                    match commits {
                        Ok(commits) => {
                            log.complete = commits.len() < page;
                            log.fetched += commits.len();
                            let seen = &mut log.seen;
                            log.commits.extend(commits.into_iter().filter(|c| seen.insert(c.hash.clone())));
                        }
                        Err(e) => log.error = Some(e),
                    }
                }
                Message::CommitDetails { hash, details } => {
                    self.commit_details.insert(hash, Some(details));
                }
                Message::CommitFiles { hash, files } => {
                    self.commit_files.insert(hash, Some(files));
                }
                Message::Blame { path, head, blame } => {
                    // Blames of older HEADs are not coming back.
                    self.blames.retain(|(_, h), _| *h == head);
                    self.blames.insert((path, head), blame);
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

    /// Polls until `done` holds or 5 s pass.
    fn wait(ctx: &egui::Context, git: &mut GitState, done: impl Fn(&GitState) -> bool) {
        for _ in 0..250 {
            git.poll(ctx);
            if done(git) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("timed out");
    }

    #[test]
    fn test_graph_log_loads_all_branches_and_reloads_when_refs_move() {
        let Some(repo) = crate::git::temp_repo("worker_graph") else { return };
        let root = repo.root().to_path_buf();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("first").unwrap();
        repo.create_branch("feature").unwrap();
        std::fs::write(root.join("b.txt"), "bee\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("second").unwrap();
        repo.switch("main").unwrap();

        let ctx = egui::Context::default();
        let mut git = GitState::default();
        git.open(&ctx, root.clone());
        wait(&ctx, &mut git, |g| g.status.is_some());
        assert!(git.graph.commits.is_empty() && !git.graph.complete, "nothing asked for yet");

        git.load_graph(&ctx);
        assert!(git.graph.loading && git.graph.all);
        wait(&ctx, &mut git, |g| !g.graph.loading);
        let subjects: Vec<&str> = git.graph.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["second", "first"]);
        assert!(git.graph.complete);
        assert!(git.log.commits.is_empty(), "the sidebar log is untouched");

        // Details are fetched once and cached.
        let hash = git.graph.commits[1].hash.clone();
        assert!(git.commit_details(&ctx, &hash).is_none());
        wait(&ctx, &mut git, |g| g.commit_details.get(&hash).is_some_and(|d| d.as_ref().is_some_and(|d| d.is_ok())));
        assert_eq!(git.commit_details(&ctx, &hash).unwrap().as_ref().unwrap().message, "first");

        // A branch made in the editor reloads the graph.
        let first = hash.clone();
        assert!(git.run(&ctx, Op::CreateBranchAt { name: "old".into(), hash: first.clone() }));
        wait(&ctx, &mut git, |g| g.busy().is_none());
        wait(&ctx, &mut git, |g| !g.is_refreshing() && !g.graph.loading);
        assert!(git.graph.commits[1].refs.contains(&"old".to_owned()), "{:?}", git.graph.commits[1].refs);
        assert_eq!(git.take_finished().len(), 1);

        assert!(git.run(&ctx, Op::DeleteBranch("old".into())));
        wait(&ctx, &mut git, |g| g.busy().is_none());
        wait(&ctx, &mut git, |g| !g.is_refreshing() && !g.graph.loading);
        assert!(!git.graph.commits[1].refs.contains(&"old".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
