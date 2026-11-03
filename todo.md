# Todo

Improvement ideas for Red Text Editor, roughly in order of value per effort.
Last updated 2026-11-03.

## High priority (daily use)

- [ ] **Command Palette** (`Ctrl+Shift+P`) and **Quick Open** (`Ctrl+P`) —
  code done and tested; waiting on the hand check in the running app. Spec: `docs/superpowers/specs/2026-11-03-command-palette-design.md`,
  plan: `docs/superpowers/plans/2026-11-03-command-palette.md`.
- [ ] **File operations in the Explorer** — right-click: New File, New Folder,
  Rename, Delete, Reveal in Folder.
- [ ] **Restore session** — reopen the tabs, folder and caret positions from
  the last run. `History` and eframe `persistence` already exist; save the
  tab list.
- [ ] **Reload when a file changes on disk** — `DiskStamp` exists; add a
  watcher (`notify` crate) or a poll on focus, then a "file changed, reload?"
  prompt.

## Editing experience

- [ ] **Auto-indent and auto-close** — Enter keeps the previous line's
  indentation; `{` inserts `{}`, same for `(`, `[`, quotes.
- [ ] **Bracket matching** — highlight the pair at the caret,
  `Ctrl+Shift+\` jumps to the match.
- [ ] **Code folding** — fold blocks by indentation (simpler than parsing
  syntax).
- [ ] **Multi-cursor / column select** — `Ctrl+D` exists, but egui's
  `TextEdit` has no multi-cursor; needs a custom text widget. Later.

## Languages and tooling

- [ ] **Minimal LSP client** — diagnostics first (squiggles + a Problems
  panel) from `rust-analyzer` / `gopls` / `tsserver`, then completion and
  hover. Biggest feature, biggest cost.
- [ ] **Diff view** — active file vs HEAD, or two files, in a tab
  (`Ctrl+Shift+D`). The SCM diff view exists; reuse it.

## Tech debt

- [x] **Split `src/app.rs`** — done 2026-11-03: 7,092 → 4,812 lines;
  `shortcuts.rs`, `menus.rs`, `status_bar.rs`, `console_panel.rs`,
  `editor_view.rs`. Left for later: `disk.rs` (poll/reload on disk change)
  and `git_actions.rs` (poll git, SCM actions, confirm dialogs), and
  `update` itself (~530 lines).

## Suggested order

Command Palette → split `app.rs` → Explorer file operations → restore
session → reload on disk change.
