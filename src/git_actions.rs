//! Git wiring: polling the git worker, the Source Control and Git Graph
//! actions, the confirmation of commands that cannot be undone, the gutter's
//! hunk popup and the diff view.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::app::{EditorRequest, GitConfirm, MARKS_INTERVAL, SidebarView, TextEditorApp, read_document};
use crate::diff_view::{self, Against, DiffViewAction, DiffViewUi};
use crate::editor_view::text_content_exceeds_highlight_limit;
use crate::git::GitError;
use crate::git_graph::{GitGraphUi, GraphAction};
use crate::git_worker::{DiffSource, Op};
use crate::line_diff::{self, LineMarks};
use crate::modals;
use crate::source_control::ScmAction;
use crate::theme;

/// What the hunk popup asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HunkAction {
    Close,
    Revert,
    Stage,
}

impl TextEditorApp {
    /// The hunk the open popup is about, while its tab is shown and git
    /// still marks a change there.
    fn popup_hunk(&self) -> Option<line_diff::Hunk> {
        let popup = self.hunk_popup.as_ref()?;
        if popup.tab != self.active_tab_index
            || self.zen_mode
            || self.show_welcome
            || self.git_graph.is_some()
            || self.diff_view.is_some()
        {
            return None;
        }
        let tab = self.tabs.get(popup.tab)?;
        let base = tab.git_base.as_ref()?;
        let base_text = base.text.as_deref().unwrap_or("");
        line_diff::hunk_at(base_text, &tab.text_content, popup.line)
    }

    /// The popup of a clicked gutter change marker: the lines as HEAD has
    /// them, the lines now, and Revert / Stage.
    pub(crate) fn show_hunk_popup(&mut self, ctx: &egui::Context) {
        if self.hunk_popup.is_none() {
            return;
        }
        let Some(hunk) = self.popup_hunk() else {
            self.hunk_popup = None;
            return;
        };
        let popup = self.hunk_popup.clone().expect("checked above");
        let palette = theme::active(ctx);
        let removed_bg = palette.gutter.deleted.gamma_multiply(0.18);
        let added_bg = palette.gutter.added.gamma_multiply(0.18);
        let mut action = None;
        let area = egui::Area::new(egui::Id::unique("hunk_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ui.horizontal(|ui| {
                        let lines = &hunk.new_line_range;
                        ui.label(egui::RichText::new(format!("{}–{}", lines.start(), lines.end())).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new("×").frame(false))
                                .on_hover_text(self.i18n.t("scm.diff_close"))
                                .clicked()
                            {
                                action = Some(HunkAction::Close);
                            }
                            if ui.small_button(self.i18n.t("scm.stage_hunk")).clicked() {
                                action = Some(HunkAction::Stage);
                            }
                            if ui.small_button(self.i18n.t("scm.revert_hunk")).clicked() {
                                action = Some(HunkAction::Revert);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let row = |ui: &mut egui::Ui, sign: &str, text: &str, fill: egui::Color32| {
                            egui::Frame::new().fill(fill).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.label(egui::RichText::new(format!("{sign} {text}")).monospace());
                            });
                        };
                        for line in &hunk.old_lines {
                            row(ui, "-", line, removed_bg);
                        }
                        for line in &hunk.new_lines {
                            row(ui, "+", line, added_bg);
                        }
                    });
                });
            });
        // A click elsewhere, or Escape, closes it; the click that opened it
        // lands outside the area on the marker and must not.
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && !ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| {
                area.response.rect.contains(p) || (p - popup.pos).length() < 16.0
            });
        if action.is_none() && (clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            action = Some(HunkAction::Close);
        }
        match action {
            None => {}
            Some(HunkAction::Close) => self.hunk_popup = None,
            Some(HunkAction::Revert) => {
                self.revert_hunk(&hunk);
                self.hunk_popup = None;
            }
            Some(HunkAction::Stage) => {
                let tab = self.active_tab();
                if let Some(path) = tab.current_path.clone() {
                    let op = Op::StageHunk { path, text: tab.text_content.clone(), line: popup.line };
                    self.run_git(ctx, op);
                }
                self.hunk_popup = None;
            }
        }
    }

    /// Puts HEAD's lines back in place of `hunk` in the active document.
    /// The file is not saved.
    fn revert_hunk(&mut self, hunk: &line_diff::Hunk) {
        let tab = self.active_tab_mut();
        let byte = |text: &str, at: usize| text.char_indices().nth(at).map_or(text.len(), |(b, _)| b);
        let (start, end) = (byte(&tab.text_content, hunk.char_range.0), byte(&tab.text_content, hunk.char_range.1));
        tab.text_content.replace_range(start..end, &hunk.revert_replacement);
        let at = hunk.char_range.0;
        self.editor_request.goto = Some(at);
        self.editor_request.focus = true;
    }

    /// The folder whose repository git features act on: the open folder,
    /// else the active file's.
    pub(crate) fn git_dir(&self) -> Option<PathBuf> {
        match &self.explorer {
            Some(explorer) => Some(explorer.root().to_path_buf()),
            None => self.active_tab().current_path.as_deref()?.parent().map(Path::to_path_buf),
        }
    }

    /// Takes in what git reported and tells the user how commands ended.
    pub fn poll_git(&mut self, ctx: &egui::Context) {
        if let Some(dir) = self.git_dir() {
            self.git.open(ctx, dir);
        }
        self.git.poll(ctx);
        for (op, result) in self.git.take_finished() {
            if op.touches_work_tree() {
                self.check_files_on_disk();
            }
            if matches!(op, Op::Commit(_) | Op::CommitAll(_)) && result.is_ok() {
                self.scm.message.clear();
            }
            if matches!(op, Op::Fetch | Op::Pull | Op::Push) {
                self.scm.auth_failed = result == Err(GitError::AuthRequired);
            }
            self.status_message = self.git_outcome_message(&op, result);
        }
        for base in self.git.take_bases() {
            for tab in &mut self.tabs {
                if tab.current_path.as_ref() == Some(&base.path) {
                    tab.git_base = Some(base.clone());
                    tab.marks_text = None;
                }
            }
        }
        self.update_line_marks(ctx);
    }

    /// Keeps the active tab's gutter markers in step with its text, asking
    /// git for HEAD's copy of the file when HEAD moved.
    fn update_line_marks(&mut self, ctx: &egui::Context) {
        let head = self.git.status.as_ref().map(|s| s.head.clone());
        let tab = &self.tabs[self.active_tab_index];
        let path = match (&tab.current_path, self.git.repo(), &head) {
            (Some(path), Some(repo), Some(_)) if repo.contains(path) => path.clone(),
            _ => {
                let tab = &mut self.tabs[self.active_tab_index];
                tab.git_base = None;
                tab.line_marks = LineMarks::default();
                tab.marks_text = None;
                return;
            }
        };
        if tab.git_base.as_ref().is_none_or(|b| Some(&b.head) != head.as_ref()) {
            self.git.request_base(ctx, &path);
        }
        let Some(base) = &tab.git_base else { return };
        if tab.marks_text.as_ref() == Some(&tab.text_content) {
            return;
        }
        if text_content_exceeds_highlight_limit(&tab.text_content) {
            let tab = &mut self.tabs[self.active_tab_index];
            tab.line_marks = LineMarks::default();
            return;
        }
        // While typing, at most a few diffs a second.
        let since = self.last_marks_at.map(|at| at.elapsed());
        if let Some(since) = since.filter(|s| tab.marks_text.is_some() && *s < MARKS_INTERVAL) {
            ctx.request_repaint_after(MARKS_INTERVAL - since);
            return;
        }
        self.last_marks_at = Some(Instant::now());
        // HEAD lacks a new file: all of it is added. Without a change git
        // ignores the file, and nothing is marked.
        let marks = match (&base.text, self.git.change(&path)) {
            (Some(text), _) => line_diff::marks(text, &tab.text_content),
            (None, Some(_)) => line_diff::marks("", &tab.text_content),
            (None, None) => LineMarks::default(),
        };
        let tab = &mut self.tabs[self.active_tab_index];
        tab.line_marks = marks;
        tab.marks_text = Some(tab.text_content.clone());
    }

    /// Starts a git command, unless one is running.
    pub fn run_git(&mut self, ctx: &egui::Context, op: Op) {
        if !self.git.run(ctx, op) {
            self.status_message = Some((self.i18n.t("git.busy").to_owned(), true));
        }
    }

    /// The status bar message for a finished git command; staging is
    /// silent, the lists show it.
    fn git_outcome_message(&self, op: &Op, result: Result<(), GitError>) -> Option<(String, bool)> {
        let message = match result {
            Err(GitError::AuthRequired) => (self.i18n.t("git.auth_required").to_owned(), true),
            Err(GitError::NotInstalled) => (self.i18n.t("git.not_installed").to_owned(), true),
            // `pull --ff-only` refuses to merge.
            Err(GitError::Failed(e)) if *op == Op::Pull && e.contains("fast-forward") => {
                (self.i18n.t("git.pull_diverged").to_owned(), true)
            }
            Err(e) => (self.i18n.tf("git.failed", &[("error", &e.to_string())]), true),
            Ok(()) => {
                let text = match op {
                    Op::Stage(_) | Op::Unstage(_) => return self.status_message.clone(),
                    Op::StageHunk { .. } => self.i18n.t("scm.hunk_staged").to_owned(),
                    Op::Discard(_) => self.i18n.t("git.discarded").to_owned(),
                    Op::Commit(_) | Op::CommitAll(_) => self.i18n.t("git.committed").to_owned(),
                    Op::Fetch => self.i18n.t("git.fetched").to_owned(),
                    Op::Pull => self.i18n.t("git.pulled").to_owned(),
                    Op::Push => self.i18n.t("git.pushed").to_owned(),
                    Op::Switch(b) => self.i18n.tf("git.switched", &[("branch", b)]),
                    Op::CreateBranch(b) => self.i18n.tf("git.branch_created", &[("branch", b)]),
                    Op::CreateBranchAt { name, .. } => self.i18n.tf("git.branch_created", &[("branch", name)]),
                    Op::DeleteBranch(name) => self.i18n.tf("graph.branch_deleted", &[("branch", name)]),
                    Op::Init(_) => self.i18n.t("git.initialized").to_owned(),
                    Op::RestoreFile { .. } => self.i18n.t("history.restored").to_owned(),
                    Op::Checkout(hash) => {
                        self.i18n.tf("history.checked_out", &[("hash", &hash[..hash.len().min(7)])])
                    }
                };
                (text, false)
            }
        };
        Some(message)
    }

    /// Shows the commits touching `path` in the Source Control view.
    pub fn open_file_history(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.git.load_log(ctx, Some(path));
        self.show_sidebar = true;
        self.sidebar_view = SidebarView::SourceControl;
        self.scm.commits_open = true;
    }

    /// Shows the active document beside `against` in place of the editor,
    /// or hides the view when it already shows that.
    pub fn toggle_diff_view(&mut self, ctx: &egui::Context, against: Against) {
        if self.diff_view.as_ref().is_some_and(|v| v.tab == self.active_tab_index && v.against == against) {
            self.close_diff_view();
            return;
        }
        if self.show_welcome {
            return;
        }
        if against == Against::Head {
            // HEAD's copy is what the gutter compares with; ask for it.
            let tab = self.active_tab();
            match (&tab.current_path, self.git.repo()) {
                (Some(path), Some(repo)) if repo.contains(path) => {
                    if tab.git_base.is_none() {
                        let path = path.clone();
                        self.git.request_base(ctx, &path);
                    }
                }
                _ => {
                    self.status_message = Some((self.i18n.t("diff.no_head").to_owned(), true));
                    return;
                }
            }
        }
        if self.find.visible {
            self.close_find();
        }
        self.goto.visible = false;
        self.symbol_picker.visible = false;
        self.git_graph = None;
        self.hunk_popup = None;
        self.diff_view = Some(DiffViewUi::new(self.active_tab_index, against));
    }

    /// Asks for a file and shows the active document beside it.
    pub(crate) fn pick_and_diff_file(&mut self, ctx: &egui::Context) {
        if self.show_welcome {
            return;
        }
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.active_tab().current_path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.diff_with_file(ctx, path);
        }
    }

    /// Shows the active document beside the file at `path`.
    pub fn diff_with_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        match read_document(&path) {
            Ok((text, _)) => self.toggle_diff_view(ctx, Against::File { path, text }),
            Err(err) => self.status_message = Some((self.open_failed_message(&err), true)),
        }
    }

    pub fn close_diff_view(&mut self) {
        if self.diff_view.take().is_some() {
            self.editor_request = EditorRequest { focus: true, ..Default::default() };
        }
    }

    /// Draws the diff view for the active tab; it closes when another tab
    /// is shown.
    pub(crate) fn show_diff_view(&mut self, ui: &mut egui::Ui) -> Option<DiffViewAction> {
        let mut view = self.diff_view.take()?;
        if view.tab != self.active_tab_index {
            return None;
        }
        let untitled = self.i18n.t("doc.untitled");
        let tab = &self.tabs[self.active_tab_index];
        let right_title = tab.file_display_name(untitled);
        // The file's text moves out for the frame, so the view can be
        // borrowed mutably beside it.
        let file_text = match &mut view.against {
            Against::File { text, .. } => Some(std::mem::take(text)),
            Against::Head => None,
        };
        let (left, left_title) = match (&view.against, &file_text) {
            (Against::File { path, .. }, Some(text)) => (
                diff_view::Left::Text(text.as_str()),
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            ),
            // HEAD lacks a new file: all of it is added.
            _ => {
                let left = match &tab.git_base {
                    Some(base) => diff_view::Left::Text(base.text.as_deref().unwrap_or("")),
                    None => diff_view::Left::Loading,
                };
                (left, format!("{right_title} (HEAD)"))
            }
        };
        let titles = diff_view::Titles { left: &left_title, right: &right_title };
        let action = diff_view::show(ui, &mut view, left, &tab.text_content, &titles, &self.i18n);
        if let (Against::File { text, .. }, Some(file_text)) = (&mut view.against, file_text) {
            *text = file_text;
        }
        self.diff_view = Some(view);
        action
    }

    /// Shows the Git Graph in place of the editor, or hides it again.
    /// Needs a repository; the first opening loads the graph.
    pub fn toggle_git_graph(&mut self, ctx: &egui::Context) {
        if self.git_graph.is_some() {
            self.git_graph = None;
            self.editor_request = EditorRequest { focus: true, ..Default::default() };
            return;
        }
        if self.git.repo().is_none() {
            return;
        }
        if self.find.visible {
            self.close_find();
        }
        self.goto.visible = false;
        self.symbol_picker.visible = false;
        self.ensure_graph_loaded(ctx, true);
        self.git_graph = Some(GitGraphUi::default());
    }

    /// Loads the graph unless it is loaded or loading; a failed load is
    /// tried again only when `retry` (the graph being opened).
    pub(crate) fn ensure_graph_loaded(&mut self, ctx: &egui::Context, retry: bool) {
        let graph = &self.git.graph;
        let untouched = !graph.complete && graph.error.is_none();
        let failed = retry && graph.error.is_some();
        if graph.commits.is_empty() && !graph.loading && (untouched || failed) {
            self.git.load_graph(ctx);
        }
    }

    /// Does what the Git Graph asked.
    pub(crate) fn apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction) {
        match action {
            GraphAction::Close => self.toggle_git_graph(ctx),
            GraphAction::Select(_) => {}
            GraphAction::LoadMore => self.git.load_more_graph(ctx),
            GraphAction::Checkout(hash) => self.git_confirm = Some(GitConfirm::Checkout(hash)),
            GraphAction::CreateBranch { name, hash } => self.run_git(ctx, Op::CreateBranchAt { name, hash }),
            GraphAction::Switch(name) => self.run_git(ctx, Op::Switch(name)),
            GraphAction::DeleteBranch(name) => self.git_confirm = Some(GitConfirm::DeleteBranch(name)),
            GraphAction::CommitDiff { hash, path } => self.git.show_diff(ctx, path, DiffSource::Commit(hash)),
            GraphAction::Copy(text) => ctx.copy_text(text),
        }
    }

    /// Does what the Source Control view asked.
    pub(crate) fn apply_scm_action(&mut self, ctx: &egui::Context, action: ScmAction) {
        match action {
            ScmAction::Run(op) => self.run_git(ctx, op),
            ScmAction::Discard(changes) => self.git_confirm = Some(GitConfirm::Discard(changes)),
            ScmAction::CommitAll(message) => self.git_confirm = Some(GitConfirm::CommitAll(message)),
            ScmAction::Open(path) => self.trigger_open_specific_file(path),
            ScmAction::Diff { path, source } => {
                if path.is_file() {
                    self.trigger_open_specific_file(path.clone());
                }
                self.git.show_diff(ctx, path, source);
            }
            ScmAction::Refresh => self.git.refresh(ctx),
            ScmAction::LoadMoreLog => self.git.load_more_log(ctx),
            ScmAction::ExpandCommit(hash) => self.git.log.expanded = hash,
            ScmAction::ClearHistoryFilter => self.git.load_log(ctx, None),
            ScmAction::Checkout(hash) => self.git_confirm = Some(GitConfirm::Checkout(hash)),
            ScmAction::OpenGraph => {
                if self.git_graph.is_none() {
                    self.toggle_git_graph(ctx);
                }
            }
            ScmAction::OpenConsole => {
                self.scm.auth_failed = false;
                if !self.show_console {
                    self.toggle_console(ctx);
                }
            }
        }
    }

    /// Asks before a git command that waits for confirmation.
    pub(crate) fn show_git_confirm(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.git_confirm else { return };
        let (title, message, yes) = match confirm {
            GitConfirm::Discard(changes) => {
                let message = match changes.as_slice() {
                    [one] => {
                        let name = one.path.file_name().unwrap_or_default().to_string_lossy();
                        self.i18n.tf("scm.discard_one", &[("name", &name)])
                    }
                    many => self.i18n.tf("scm.discard_many", &[("count", &many.len().to_string())]),
                };
                (self.i18n.t("scm.discard_title"), message, self.i18n.t("scm.discard_confirm"))
            }
            GitConfirm::CommitAll(_) => (
                self.i18n.t("scm.commit_all_title"),
                self.i18n.t("scm.commit_all_message").to_owned(),
                self.i18n.t("scm.commit_all_confirm"),
            ),
            GitConfirm::Checkout(hash) => (
                self.i18n.t("history.checkout"),
                self.i18n.tf("history.checkout_message", &[("hash", &hash[..hash.len().min(7)])]),
                self.i18n.t("history.checkout_confirm"),
            ),
            GitConfirm::RestoreFile { hash, path } => {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                (
                    self.i18n.t("history.restore"),
                    self.i18n.tf(
                        "history.restore_message",
                        &[("name", &name), ("hash", &hash[..hash.len().min(7)])],
                    ),
                    self.i18n.t("history.restore_confirm"),
                )
            }
            GitConfirm::DeleteBranch(branch) => (
                self.i18n.t("graph.delete_branch"),
                self.i18n.tf("graph.delete_branch_message", &[("branch", branch)]),
                self.i18n.t("graph.delete_branch_confirm"),
            ),
        };
        let answer = modals::show_confirm_modal(ctx, title, &message, yes, self.i18n.t("unsaved.cancel"));
        match answer {
            Some(true) => {
                let op = match self.git_confirm.take() {
                    Some(GitConfirm::Discard(changes)) => Op::Discard(changes),
                    Some(GitConfirm::CommitAll(message)) => Op::CommitAll(message),
                    Some(GitConfirm::Checkout(hash)) => Op::Checkout(hash),
                    Some(GitConfirm::RestoreFile { hash, path }) => Op::RestoreFile { hash, path },
                    Some(GitConfirm::DeleteBranch(name)) => Op::DeleteBranch(name),
                    None => return,
                };
                self.run_git(ctx, op);
            }
            Some(false) => self.git_confirm = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::HunkPopup;
    use crate::app::test_support::*;
    use crate::file_browser::Explorer;
    use crate::shortcuts::ShortcutAction;
    use std::fs;

    #[test]
    fn test_git_graph_opens_only_with_a_repository() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_none(), "no repository, no graph");

        let Some(repo) = crate::git::temp_repo("app_graph") else { return };
        let root = repo.root().to_path_buf();
        app.git.open(&ctx, root.clone());
        for _ in 0..250 {
            app.git.poll(&ctx);
            if app.git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_some());
        assert!(app.git.graph.loading || app.git.graph.complete, "opening loads the graph");
        app.apply_shortcut(&ctx, ShortcutAction::GitGraph);
        assert!(app.git_graph.is_none(), "the shortcut toggles");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_reopening_the_graph_retries_a_failed_load() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        let Some(repo) = crate::git::temp_repo("app_graph_retry") else { return };
        let root = repo.root().to_path_buf();
        app.git.open(&ctx, root.clone());
        for _ in 0..250 {
            app.git.poll(&ctx);
            if app.git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.git.graph.error = Some(GitError::Failed("x".into()));
        app.git.graph.loading = false;
        app.git.graph.commits.clear();
        app.ensure_graph_loaded(&ctx, false);
        assert!(app.git.graph.error.is_some(), "no retry every frame");
        app.toggle_git_graph(&ctx);
        assert!(app.git_graph.is_some());
        assert!(app.git.graph.error.is_none() || app.git.graph.loading, "opening retries");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_closing_the_graph_drops_stale_editor_requests() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp { git_graph: Some(GitGraphUi::default()), ..Default::default() };
        app.editor_request.goto = Some(5);
        app.toggle_git_graph(&ctx);
        assert!(app.git_graph.is_none());
        assert!(app.editor_request.goto.is_none() && app.editor_request.focus);
    }

    #[test]
    fn test_file_history_opens_source_control_filtered_to_the_file() {
        let Some(repo) = crate::git::temp_repo("app_history") else { return };
        let a = repo.root().join("a.txt");
        let b = repo.root().join("b.txt");
        fs::write(&a, "a\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("add a").unwrap();
        fs::write(&b, "b\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("add b").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(repo.root().to_path_buf())),
            ..Default::default()
        };
        poll_git_until(&mut app, &ctx, |app| app.git.status.is_some());
        app.open_file_history(&ctx, b.clone());
        assert!(app.show_sidebar);
        assert_eq!(app.sidebar_view, SidebarView::SourceControl);
        assert!(app.scm.commits_open);
        poll_git_until(&mut app, &ctx, |app| app.git.log.complete);
        let subjects: Vec<&str> = app.git.log.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["add b"]);
        assert_eq!(app.git.log.filter.as_ref(), Some(&b));

        app.apply_scm_action(&ctx, ScmAction::ClearHistoryFilter);
        poll_git_until(&mut app, &ctx, |app| app.git.log.complete);
        assert_eq!(app.git.log.commits.len(), 2);
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_clicking_a_change_opens_the_file_and_its_diff() {
        let Some(repo) = crate::git::temp_repo("app_diff") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();
        fs::write(&file, "one\ntwo\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp {
            explorer: Some(Explorer::new(repo.root().to_path_buf())),
            ..Default::default()
        };
        poll_git_until(&mut app, &ctx, |app| app.git.change_count() == 1);
        app.apply_scm_action(
            &ctx,
            ScmAction::Diff { path: file.clone(), source: crate::git_worker::DiffSource::WorkTree },
        );
        assert_eq!(app.active_tab().current_path.as_ref(), Some(&file));
        assert!(!app.previewing());
        poll_git_until(&mut app, &ctx, |app| app.git.diff().is_some_and(|d| d.lines.is_some()));
        let diff = app.git.diff().unwrap();
        let lines = diff.lines.as_ref().unwrap().as_ref().unwrap();
        assert!(lines.iter().any(|l| l.kind == crate::git::DiffKind::Added && l.text == "two"));

        // Staging it from the panel moves it to Staged Changes.
        app.apply_scm_action(&ctx, ScmAction::Run(Op::Stage(vec![file.clone()])));
        poll_git_until(&mut app, &ctx, |app| {
            app.git.busy().is_none() && app.git.change(&file).is_some_and(|c| c.staged.is_some())
        });
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_gutter_marks_lines_changed_since_head() {
        let Some(repo) = crate::git::temp_repo("app_marks") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        assert!(app.active_tab().line_marks.is_empty());

        app.active_tab_mut().text_content = "one\nTWO\nthree\nfour\n".into();
        poll_git_until(&mut app, &ctx, |app| !app.active_tab().line_marks.is_empty());
        let marks = &app.active_tab().line_marks;
        use crate::line_diff::Mark;
        assert_eq!(marks.get(1), None);
        assert_eq!(marks.get(2), Some(Mark::Modified));
        assert_eq!(marks.get(4), Some(Mark::Added));
        assert_eq!(app.git.status.as_ref().unwrap().branch.as_deref(), Some("main"));
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_diff_view_against_head_toggles_and_blocks_editor_shortcuts() {
        let Some(repo) = crate::git::temp_repo("app_diff_view") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        app.active_tab_mut().text_content = "one\nTWO\nthree\n".into();

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_some());
        // The editor is hidden: its shortcuts do nothing.
        app.apply_shortcut(&ctx, ShortcutAction::Find);
        assert!(!app.find.visible);
        let mut ui_action = None;
        run_frame_with_events(&ctx, Vec::new(), |ui| ui_action = app.show_diff_view(ui));
        assert_eq!(ui_action, None);
        let kinds: Vec<_> = app.diff_view.as_ref().unwrap().rows().iter().map(|r| r.kind).collect();
        use crate::diff_view::RowKind::*;
        assert_eq!(kinds, [Equal, Changed, Added]);

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_none(), "the shortcut toggles");
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn test_diff_view_with_a_file_and_outside_a_repository() {
        let dir = std::env::temp_dir().join(format!("red_diff_file_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        fs::write(&a, "x\ny\n").unwrap();
        fs::write(&b, "x\nz\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(a.clone());

        app.apply_shortcut(&ctx, ShortcutAction::DiffWithHead);
        assert!(app.diff_view.is_none(), "no repository, no HEAD");
        assert!(app.status_message.as_ref().is_some_and(|(_, error)| *error));

        app.diff_with_file(&ctx, b.clone());
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_diff_view(ui);
        });
        let view = app.diff_view.as_ref().unwrap();
        assert_eq!(view.rows()[1].left, Some((2, "z".into())));
        assert_eq!(view.rows()[1].right, Some((2, "y".into())));

        // Showing another tab closes it.
        app.open_new_tab();
        run_frame_with_events(&ctx, Vec::new(), |ui| {
            app.show_diff_view(ui);
        });
        assert!(app.diff_view.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hunk_popup_reverts_one_change_and_closes() {
        let Some(repo) = crate::git::temp_repo("app_hunk") else { return };
        let file = repo.root().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("first").unwrap();

        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_file_in_tab(file.clone());
        poll_git_until(&mut app, &ctx, |app| app.active_tab().git_base.is_some());
        app.active_tab_mut().text_content = "one\nTWO\nthree\nfour\n".into();

        app.hunk_popup = Some(HunkPopup { tab: app.active_tab_index, line: 2, pos: egui::Pos2::ZERO });
        let hunk = app.popup_hunk().expect("a change at line 2");
        assert_eq!(hunk.old_lines, ["two"]);
        assert_eq!(hunk.new_lines, ["TWO"]);
        app.revert_hunk(&hunk);
        assert_eq!(app.active_tab().text_content, "one\ntwo\nthree\nfour\n");

        // Line 1 is unchanged: the popup has nothing to show and closes.
        app.hunk_popup = Some(HunkPopup { tab: app.active_tab_index, line: 1, pos: egui::Pos2::ZERO });
        run_frame_with_events(&ctx, Vec::new(), |ui| app.show_hunk_popup(ui.ctx()));
        assert!(app.hunk_popup.is_none());
        let _ = fs::remove_dir_all(repo.root());
    }
}
