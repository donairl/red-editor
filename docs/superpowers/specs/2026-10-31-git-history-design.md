# Git History — design

Builds on the git integration of v0.7.0 (`git.rs`, `git_worker.rs`,
`source_control.rs`). Everything goes through the `git` CLI, as before.

## Scope

1. **Commit log** of the current branch, as a COMMITS section in the Source
   Control view. Click a commit to see its files; click a file to see its
   diff in the existing diff panel.
2. **File history**: the same section filtered to one file, from the tab's
   or the Explorer's right-click menu.
3. **Blame**: a column left of the gutter with author and age per line,
   toggled from View → Git Blame (`Ctrl+Alt+B`).
4. **Restore / checkout**: restore a file to a commit's version from the
   commit diff; check out a commit (detached HEAD) from its row.

Out of scope: branch graph lines, interactive rebase, blame of unsaved text.

## Commit log

- `git log --format=%H%x00%h%x00%an%x00%at%x00%s%x00%D -z -n 100 [--skip N] [-- path]`
  → `Commit { hash, short, author, time, subject, head: bool, refs }`.
  Parsed in `git.rs` (`parse_log`), unit-tested without git.
- Section **COMMITS (n)** below Changes, collapsed by default, remembered in
  `ScmUi`. Row: `short  subject` in the foreground colour, then `author ·
  age` dimmed. Age is relative (`5m`, `3h`, `2d`, `3w`, `1y`). HEAD's row
  has the accent dot; rows also carry branch/tag names from `%D` after the
  subject, small.
- Click a row → expands it: the files of the commit, from
  `git show --name-status --format= -z <hash>` → `(Kind, path)` rows
  indented under the commit. Click a file → `GitState::show_commit_diff`:
  the diff panel shows `git show --format= <hash> -- path` with the
  title `name (short)`.
- One commit expanded at a time. Files are fetched once per commit and
  cached in `GitState` (`HashMap<hash, Vec<CommitFile>>`).
- The last row is **Load 100 more** while the last load returned 100. The
  log reloads whenever the status refreshes with a different HEAD (the
  `.git/HEAD` stamp is already watched).
- Repository without commits: the section says *No commits yet*.

## File history

- Tab bar context menu and Explorer file right-click gain **File History**.
  Sets `GitState` log filter to that path (absolute), reloads the log,
  opens the sidebar on Source Control and expands COMMITS.
- Header of the section shows a chip `name ×` while filtered; `×` clears
  the filter. A filtered commit row click shows that file's diff in that
  commit directly (no file list).

## Blame

- `git blame --porcelain -- path` at HEAD (not the work tree), parsed in
  `blame.rs` → `Vec<BlameLine { hash, short, author, time, summary }>`
  indexed by line. Porcelain repeats a commit's header only once; the
  parser keeps a map by hash.
- Fetched by the worker on request, cached per `(path, head)`. Only for a
  tab whose text equals `saved_content` and whose file git tracks;
  otherwise the column is drawn empty with *Save to see blame* once
  (dirty) or nothing (untracked).
- Rendering: `gutter::paint` gets `Option<&BlameView>`; the editor margin
  grows by the blame column width (`author · age`, author elided to 12
  chars; measured once per blame). Column text in the gutter's `number`
  colour. Hovering a row shows `short summary` in a tooltip (via
  `ui.interact` rects per visible row).
- Toggle: `TextEditorApp::show_blame`, View menu check item, shortcut
  `Ctrl+Alt+B`, remembered for the session only.

## Restore / checkout

- Commit diff panel header: button **Restore this version** →
  `GitConfirm::RestoreFile { hash, path }` → `Op::RestoreFile` →
  `git restore --source=<hash> --worktree -- path`. Touches the work tree,
  so the tab reloads through `check_files_on_disk` (conflict dialog when
  dirty).
- Commit row hover button **Checkout** → `GitConfirm::Checkout(hash)` →
  `Op::Checkout` → `git switch --detach <hash>`. Refused by git with local
  changes; the error shows in the status bar. The status bar then shows
  the short hash, and the branch menu switches back.

## Code layout

- `git.rs`: `Commit`, `CommitFile`, `parse_log`, `parse_name_status`,
  `Repo::{log, commit_files, commit_diff, blame, restore_from,
  checkout_detached}`.
- `blame.rs` (new): porcelain parser, `BlameLine`, relative-age helper
  (`age(seconds_ago) -> String`) shared with the log rows.
- `git_worker.rs`: `Message::{Log, CommitFiles, Blame}`, log state
  (`commits`, `log_filter`, `log_complete`, `loading_log`),
  `commit_files` cache, `blame` cache, `Op::{RestoreFile, Checkout}`,
  `show_commit_diff`.
- `source_control.rs`: `show_commits_section`, `ScmAction::{LoadMore,
  ExpandCommit, CommitDiff, Checkout, RestoreFile, ClearHistoryFilter}`,
  diff panel header variant for a commit diff.
- `gutter.rs`: blame column; `app.rs`: toggles, menus, wiring.
- i18n keys `history.*`, `blame.*` in all five languages.

## Tests

- Pure: `parse_log` (refs, empty `%D`, non-ASCII subject), `parse_name_status`
  (M/A/D/R with two paths), blame porcelain (repeated commit, boundary
  commit), `age`.
- Temp repo: three commits → log order and HEAD flag; file history lists
  only commits touching the file; `restore_from` rewrites the file;
  `checkout_detached` makes `status().detached_at` the short hash; blame
  attributes lines to the right commits.
- Headless UI: COMMITS section lists subjects; clicking a row expands its
  files; blame column paints the author name; Load more row appears only
  when the log is not complete.

## Order of work

1. `parse_log`, `Repo::log`, worker log state, COMMITS section with
   expand + file list + commit diff.
2. File history filter from tab and Explorer menus.
3. `blame.rs`, worker cache, gutter column, toggle.
4. Restore and checkout with confirmations.
5. README (EN/ID) and translations check.
