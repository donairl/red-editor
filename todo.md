# Todo

Improvement ideas for Red Text Editor, roughly in order of value per effort.
Last updated 2026-11-04.

## High priority (daily use)

- [x] **Command Palette** (`Ctrl+Shift+P`) and **Quick Open** (`Ctrl+P`) —
  done 2026-11-04 (`src/palette.rs`), hand-checked. Spec: `docs/superpowers/specs/2026-11-03-command-palette-design.md`,
  plan: `docs/superpowers/plans/2026-11-03-command-palette.md`.
- [x] **File operations in the Explorer** — done 2026-11-04
  (`src/file_ops.rs`): right-click New File, New Folder, Rename, Delete
  (to the Trash), Reveal in Folder; names typed inline. Later: F2 / Delete
  keys (needs a selected row), New buttons in the header, `a/b.rs` names.
- [x] **Restore session** — done 2026-11-03 (`src/session.rs`): folder,
  saved tabs and the active tab. Later: caret and scroll position per tab.
- [x] **Reload when a file changes on disk** — already there:
  `check_files_on_disk` polls on focus and every few seconds, reloads clean
  tabs and flags dirty ones with a conflict prompt.

## Editing experience

- [x] **Auto-indent and auto-close** — done 2026-11-04 (`src/auto_edit.rs`):
  Enter keeps the indentation (one level deeper after `{ ( [`, `{|}` splits
  into three lines); brackets and quotes pair, wrap a selection, step over
  a typed closer; Backspace removes an empty pair. Later: step over only
  auto-inserted closers, as VS Code does.
- [x] **Bracket matching** — done 2026-11-04 (`src/brackets.rs`): the pair
  at the caret gets a thin box; `Ctrl+Shift+\` (Edit → Go to Bracket) jumps
  to it. Plain scan: brackets in strings and comments are not told apart.
- [ ] **Code folding** — fold blocks by indentation (simpler than parsing
  syntax).
- [ ] **Multi-cursor / column select** — `Ctrl+D` exists, but egui's
  `TextEdit` has no multi-cursor; needs a custom text widget. Later.

## Languages and tooling

- [ ] **Minimal LSP client** — diagnostics first (squiggles + a Problems
  panel) from `rust-analyzer` / `gopls` / `tsserver`, then completion and
  hover. Biggest feature, biggest cost.
- [x] **Diff view** — done 2026-11-06 (`src/diff_view.rs`): side by side in
  place of the editor, the document (unsaved edits too) vs HEAD
  (`Ctrl+Shift+D`) or vs a picked file; step between changes, double-click
  to jump. Later: word-level highlights inside changed lines, editing on
  the right side, folding unchanged runs.

- [x] **Gutter diff popup** — done 2026-11-06: click a change marker for the
  old/new lines with Revert Change (editor only) and Stage Change (index
  only, via `hash-object` + `update-index`). Hunks are taken against HEAD;
  staging re-diffs against the index. Later: Unstage, next/prev change.

## Tech debt

- [x] **Split `src/app.rs`** — done 2026-11-03: 7,092 → 4,812 lines;
  `shortcuts.rs`, `menus.rs`, `status_bar.rs`, `console_panel.rs`,
  `editor_view.rs`. Left for later: `disk.rs` (poll/reload on disk change)
  and `git_actions.rs` (poll git, SCM actions, confirm dialogs), and
  `update` itself (~530 lines).

## Suggested order

Minimal LSP client (diagnostics first).
