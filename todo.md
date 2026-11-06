# Todo

Improvement ideas for Red Text Editor, roughly in order of value per effort.
Last updated 2026-11-06.

## High priority (daily use)

- [x] **Command Palette** (`Ctrl+Shift+P`) and **Quick Open** (`Ctrl+P`) —
  done 2026-11-04 (`src/palette.rs`), hand-checked. Spec: `docs/superpowers/specs/2026-11-03-command-palette-design.md`,
  plan: `docs/superpowers/plans/2026-11-03-command-palette.md`.
- [x] **File operations in the Explorer** — done 2026-11-04
  (`src/file_ops.rs`): right-click New File, New Folder, Rename, Delete
  (to the Trash), Reveal in Folder; names typed inline. Later: F2 / Delete
  keys (needs a selected row), New buttons in the header, `a/b.rs` names.
- [x] **Restore session** — done 2026-11-03 (`src/session.rs`): folder,
  saved tabs and the active tab.
- [x] **Caret and scroll position per tab in the session** — done 2026-11-06
  (`src/session.rs`): each restored tab reopens at its caret and vertical
  scroll offset; the frame records both, the session saves them per file.
  A tab of an older session, saved without positions, opens at the top.
- [x] **Reload when a file changes on disk** — already there:
  `check_files_on_disk` polls on focus and every few seconds, reloads clean
  tabs and flags dirty ones with a conflict prompt.
- [x] **Font zoom** — done 2026-11-06 (`src/zoom.rs`): `Ctrl +` / `Ctrl -` /
  `Ctrl+0`, `Ctrl` + scroll, and View → Zoom. One editor font size, kept in
  storage; it feeds `FontId::monospace` and the layout cache. The status bar
  shows the size as a percentage when it is not the default one.
- [x] **Drag and drop** — done 2026-11-06 (`src/drop.rs`): files dropped on
  the window open in tabs and a folder opens as the workspace; folders are
  handled first, so the files dropped beside one land in it. A hint is
  painted over the window while files hover it.

## Editing experience

- [x] **Auto-indent and auto-close** — done 2026-11-04 (`src/auto_edit.rs`):
  Enter keeps the indentation (one level deeper after `{ ( [`, `{|}` splits
  into three lines); brackets and quotes pair, wrap a selection, step over
  a typed closer; Backspace removes an empty pair. Later: step over only
  auto-inserted closers, as VS Code does.
- [x] **Bracket matching** — done 2026-11-04 (`src/brackets.rs`): the pair
  at the caret gets a thin box; `Ctrl+Shift+\` (Edit → Go to Bracket) jumps
  to it. Plain scan: brackets in strings and comments are not told apart.
- [x] **Code folding** — done 2026-11-04, released in v0.8.0
  (`src/folding.rs`): fold blocks by indentation; gutter chevrons, a `⋯`
  marker, Fold / Unfold (`Ctrl+Shift+[` / `]`), Fold All, Unfold All.
- [x] **Indent guides** — done 2026-11-06 (`src/indent_guides.rs`): a thin
  line per indent level, read off the laid-out rows, so wrapped and folded
  rows are skipped. View → Indent Guides, on by default.
- [x] **Render whitespace** — done 2026-11-06 (`src/whitespace.rs`): faint
  dots for spaces and arrows for tabs, over the visible rows only.
  View → Render Whitespace, off by default.
- [x] **Split editor** — done 2026-11-06 (`Ctrl+\`, View → Split Editor):
  the same document in two views side by side, each with its own caret,
  scroll and layout cache; typing goes to the view with the keyboard.
  Still to do: two *different* tabs side by side, and dragging the seam.
- [ ] **Multi-cursor / column select** — `Ctrl+D` exists, but egui's
  `TextEdit` has no multi-cursor; needs a custom text widget. Later.
- [ ] **Snippets** — a short prefix + `Tab` expands to boilerplate (`fn` in
  Rust, `html:5` in HTML).

## Git

- [x] **Diff view** — done 2026-11-06 (`src/diff_view.rs`): side by side in
  place of the editor, the document (unsaved edits too) vs HEAD
  (`Ctrl+Shift+D`) or vs a picked file; step between changes, double-click
  to jump. Later: word-level highlights inside changed lines, editing on
  the right side, folding unchanged runs.
- [x] **Gutter diff popup** — done 2026-11-06: click a change marker for the
  old/new lines with Revert Change (editor only) and Stage Change (index
  only, via `hash-object` + `update-index`). Hunks are taken against HEAD;
  staging re-diffs against the index. Later: Unstage, next/prev change.
- [ ] **Overview ruler** — marks on the scrollbar for find matches,
  bookmarks and git changes.

## Languages and tooling

- [ ] **Minimal LSP client** — diagnostics first (squiggles + a Problems
  panel) from `rust-analyzer` / `gopls` / `tsserver`, then Go to Definition
  (`F12`) and hover. Biggest feature, biggest cost.
- [x] **Outline for more languages** — done 2026-11-06 (`src/symbols.rs`):
  JavaScript / TypeScript (classes, members, functions, arrow functions,
  interfaces, types, enums, namespaces), C / C++ (functions, methods,
  structs, classes, enums, namespaces, typedefs, `using`, `#define`) and
  Markdown headings (Go to Symbol only; Go to Definition stays code-only).
- [ ] **Several Console terminals** — a `+` for another shell tab in the
  Console panel, and a choice of shell (`bash`, `zsh`, `fish`).

## Settings

- [x] **Auto-save** — done 2026-11-06: File → Auto Save (off by default),
  kept in storage. Every dirty file with a path is written after a one
  second pause in the typing, and when the window loses focus; a file
  another program changed waits for the conflict dialog.
- [ ] **Settings** — one place for tab size, spaces vs tabs, format on
  save, trim trailing whitespace; a panel or a `settings.json`.

## Tech debt

- [x] **Split `src/app.rs`** — done 2026-11-03: 7,092 → 4,812 lines;
  `shortcuts.rs`, `menus.rs`, `status_bar.rs`, `console_panel.rs`,
  `editor_view.rs`. Left for later: `disk.rs` (poll/reload on disk change)
  and `git_actions.rs` (poll git, SCM actions, confirm dialogs), and
  `update` itself (~530 lines). `app.rs` is back up to about 5,700 lines; the
  hunk popup and diff view wiring could move to `git_actions.rs` too.

## Suggested order

Settings (one place for the toggles that piled up), then splitting
`src/app.rs`, then the minimal LSP client.
