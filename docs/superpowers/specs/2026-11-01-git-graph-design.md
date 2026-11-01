# Git Graph — design

Builds on the git history of v0.7.0 (`git.rs`, `git_worker.rs`,
`source_control.rs`). Everything goes through the `git` CLI, as before.

## Scope

A **Git Graph** view in the editor area: every branch and tag of the
repository as a commit graph with coloured lanes, a detail pane for the
selected commit, and branch actions from context menus.

1. **Graph view** replacing the editor while open; opened from Source
   Control, the View menu or `Ctrl+Alt+G`.
2. **Detail pane** under the graph: metadata, full message, parents, files;
   a file opens the existing diff panel.
3. **Actions**: checkout a commit, create a branch at a commit, switch to a
   branch, delete a branch, copy hash or name.

Out of scope: merge, rebase, cherry-pick, force delete (`-D`), checking out
remote branches, stashes, lanes in the sidebar COMMITS section.

## Data

- `Commit` gains `parents: Vec<String>`. The log format becomes
  `%H%x00%h%x00%an%x00%at%x00%s%x00%D%x00%P`; `parse_log` splits `%P` on
  spaces (empty for a root commit). Existing tests gain parents.
- `Repo::log_all(skip, count)`:
  `git log --all --topo-order --format=… -z -n 200 --skip N`. No `--follow`
  (it cannot combine with `--all`). An empty repository yields no commits.
- `Repo::commit_message(hash)`: `git show -s --format=%B <hash>`.
- `Repo::branch_at(name, hash)`: `git branch <name> <hash>`.
  `Repo::delete_branch(name)`: `git branch -d <name>` (never `-D`; an
  unmerged branch fails with git's message).
- `Repo::watched_files` also stamps `.git/packed-refs` and the
  `.git/refs/heads` directory, so creating or deleting a branch refreshes.

### Worker

- `Log` gains `all: bool`. `GitState.graph: Log` is a second log with
  `all: true`, loaded by the same `Message::Log { all, filter, skip, commits }`
  path; the message's `all` picks which `Log` it fills.
- `GitState::load_graph(ctx)` / `load_more_graph(ctx)`; the graph reloads
  when the status refresh sees a different HEAD or a refs stamp change
  (`graph_head`, mirroring `log_head`), and after `CreateBranchAt`,
  `DeleteBranch`, `Switch`, `Checkout`, `Commit*`, `Pull` finish.
- `commit_files` cache is shared with the sidebar. New
  `commit_messages: HashMap<String, Option<String>>` cache with
  `Message::CommitMessage { hash, message }`.
- `Op::CreateBranchAt { name: String, hash: String }`,
  `Op::DeleteBranch(String)`. Neither touches the work tree. Status-bar
  messages: `git.branch_created` (reused), `graph.branch_deleted`.

## Lane layout (`src/git_graph.rs`)

Pure, no egui types in the layout part.

- `pub struct GraphRow { lane: usize, color: usize, edges: Vec<Edge> }`,
  `pub struct Edge { from: usize, to: usize, color: usize }` — an edge runs
  from this row's lane `from` to the next row's lane `to`.
- `pub struct Layout { rows: Vec<GraphRow>, active: Vec<Option<String>>,
  lanes: usize }`; `Layout::extend(&mut self, commits: &[Commit])` lays out
  further rows, so Load more never recomputes earlier rows.
  `Layout::new(commits)` = `extend` on an empty layout.
- Algorithm, per commit in order:
  1. Its lane is the first `active` slot holding its hash; otherwise the
     first `None` slot, else a new slot. Slots holding the same hash
     (several children) all free; each draws an edge into the chosen lane.
  2. The first parent takes the commit's lane (if no other slot already
     waits for that parent; if one does, the lane frees and an edge goes to
     that slot). Every other parent takes the first slot already waiting
     for it, else the first free slot, else a new slot.
  3. Edges: one per active slot that stays occupied (`from == to`), plus the
     merge/fork edges from step 1–2. Colours: a lane's colour is assigned
     when it opens, `next_color % PALETTE_LEN`, and a parent inheriting the
     lane keeps it.
- `lanes` is the maximum width seen so far.

## UI (`src/git_graph.rs`, `show` part)

- `TextEditorApp.git_graph: Option<GitGraphUi>`:
  `selected: Option<String>`, `detail_height: f32`, `scroll_to: Option<usize>`.
  `Some` means open; the central panel calls `git_graph::show` instead of
  the editor, the tab bar stays above it. Zen mode hides nothing extra.
- Open/close: `ShortcutAction::GitGraph` (`Ctrl+Alt+G`), View → **Git
  Graph** check item (disabled without a repository), a fourth title-bar
  icon in Source Control (branching lines), `×` in the view header, Esc
  while the view has focus. Opening loads the graph if empty.
- Header strip (`TITLE_HEIGHT`): "Git Graph", `n branches`, `×`.
- Rows, `ROW_HEIGHT` 22, virtualised with `ScrollArea::show_rows`:
  - Lane column, `LANE_WIDTH` 14 × `min(layout.lanes, 10)`; lanes beyond 10
    share the last column. Lines are 2 px, colour from a palette of 8
    (`theme::GraphPalette`, light and dark). Merge/fork edges are cubic
    curves between row centres. Commit dot radius 3.5, filled; HEAD dot
    radius 4.5 with a ring in the accent colour.
  - Ref chips after the lanes, in `%D` order: local branch = accent fill,
    the checked-out branch bold; remote (`origin/…`) = outline, dimmed; tag
    = tag colour fill. Chips elide past 20 characters; past 4 chips, a
    `+n` chip.
  - Subject in the foreground colour, elided; right side `author · age`
    dimmed (reusing `git::age`) and the short hash in monospace.
  - Selected row = `row_selected`, hovered = `row_hover`. Hover tooltip:
    full hash.
  - Last row **Load 200 more** while `!graph.complete`; "No commits yet"
    when the log is empty and complete; the error text when loading failed.
- Click a row → `selected`. Right-click → menu: **Checkout Commit**
  (→ `ScmAction::Checkout`, existing confirm), **Create Branch Here…**
  (name modal → `Op::CreateBranchAt`), **Copy Hash**.
- Chip right-click: local branch → **Switch to Branch** (`Op::Switch`,
  disabled for the current branch), **Delete Branch…** (confirm →
  `Op::DeleteBranch`, disabled for the current branch); remote or tag →
  **Copy Name**.
- `GraphAction` enum returned by `show`:
  `Close, Select(hash), LoadMore, Checkout(hash), CreateBranchAt(hash),
  Switch(name), DeleteBranch(name), CommitDiff { hash, path }, Copy(String)`.
  `app.rs` maps it onto the existing `ScmAction`/`GitConfirm` paths;
  `GitConfirm::DeleteBranch(String)` is new.

### Detail pane

- Below the rows, a horizontal splitter (`detail_height`, default 35 % of
  the view, min 100 px), shown while `selected` is `Some`.
- Content: short hash with a copy button, author, absolute date
  (`YYYY-MM-DD HH:MM`, local time) and age, ref chips, full message
  (monospace, scrollable), parents as clickable short hashes → selects the
  parent and `scroll_to` its row (a parent beyond the loaded rows loads more
  until found or complete). Then the file list from `commit_files`, rows
  as in the sidebar (status letter, name, folder dimmed); click → existing
  `show_commit_diff` in the right diff panel.
- Buttons: **Checkout**, **Create Branch…** (same actions as the row menu).

### Branch name modal

- Reuses the existing create-branch text modal with a `from` hash shown in
  its title; empty or invalid names (`git check-ref-format --branch`
  is not called; the op's error shows instead) are refused by git.

## Theme

- `theme::GraphPalette { lanes: [Color32; 8], tag: Color32, remote: Color32 }`
  in both themes; lanes chosen to contrast with each theme's editor
  background.

## i18n

`graph.title`, `graph.branches` (`{count}`), `graph.load_more`,
`graph.no_commits`, `graph.checkout`, `graph.create_branch`,
`graph.create_branch_title` (`{hash}`), `graph.copy_hash`,
`graph.copy_name`, `graph.switch`, `graph.delete_branch`,
`graph.delete_branch_confirm` (`{branch}`), `graph.branch_deleted`
(`{branch}`), `graph.parents`, `graph.files`, `view.git_graph`,
`scm.graph` (tooltip) — in `id`, `en`, `de`, `es`, `zh`.

## Tests

- Pure (`git.rs`): `parse_log` reads one, two and zero parents.
- Pure (`git_graph.rs`): linear chain → one lane, every row `lane 0`;
  branch + merge → two lanes, the merge row has an edge from lane 1 to
  lane 0 and lane 1 frees after it; three-parent merge opens two extra
  lanes; a second root in the middle takes a free lane; `extend` in two
  halves equals `new` on the whole list.
- Temp repo (`git.rs`): `log_all` lists commits of two branches in topo
  order; `branch_at` then `branches()` sees the branch at that hash;
  `delete_branch` removes a merged branch and fails on an unmerged one
  with the branch name in the error.
- Headless UI (`git_graph.rs`): a graph with a loaded log paints subjects
  and chips; clicking a row returns `Select` and the detail pane lists the
  commit's files; the load-more row exists only while the log is
  incomplete.
- `app.rs`: `Ctrl+Alt+G` toggles `git_graph`; the View item is disabled
  without a repository.

## Order of work

1. `parents` in `Commit`, `log_all`, worker `graph` log, `Layout`.
2. `git_graph::show` rows + lanes + chips, hosting in `app.rs`, open/close.
3. Detail pane, commit message cache, file list → diff.
4. Context menus, `CreateBranchAt`, `DeleteBranch`, confirmations, refs
   stamps.
5. Theme palette, i18n in five languages, README EN/ID.
