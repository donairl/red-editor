# Code folding — design and plan

Fold blocks by indentation. A folded block's lines stay in the text; they
are only drawn at (near) zero height, so every `char` index — find, Go to
Line, bookmarks, git marks, blame, line ops, auto-edit, bracket matching —
keeps working unchanged.

Out of scope: syntax-aware folding, saving folds in the session, fold
regions (`#region`), Fold Level N, folding documents over the highlight
size limit (folding is off there).

## Rules (`src/folding.rs`, new, pure)

- A line is **foldable** when the next non-blank line is indented deeper
  (tabs count as 4 columns). Its block runs to the last following line whose
  indent is deeper than the header's; blank lines inside the block belong to
  it, trailing blank lines do not.
- A fold is stored as its **header line** (0-based). Folds nest; folding an
  outer block hides inner headers too.

```rust
/// Line ranges `(header, last)` of every foldable block, in header order.
pub fn foldable(text: &str) -> Vec<(usize, usize)>;

/// The folds of one document: header lines that are folded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds { headers: BTreeSet<usize> }

impl Folds {
    pub fn toggle(&mut self, header: usize);
    pub fn fold(&mut self, header: usize);
    pub fn unfold(&mut self, header: usize);
    pub fn clear(&mut self);
    pub fn is_folded(&self, header: usize) -> bool;
    pub fn is_empty(&self) -> bool;
    /// Keeps the headers on their lines after `old` became `new`
    /// (reuse the logic of `bookmarks::follow_edit`; make it `pub(crate)`
    /// and share it rather than copying). Headers that are no longer
    /// foldable in `new` are dropped.
    pub fn follow_edit(&mut self, old: &str, new: &str);
    /// Unfolds every fold whose hidden lines contain `line`.
    pub fn reveal_line(&mut self, text: &str, line: usize);
}

/// The hidden `char` ranges for `folds` in `text`, sorted, merged: for a
/// block `(header, last)`, from the `\n` ending the header line up to (not
/// including) the `\n` ending line `last` — so the header row is followed
/// directly by the next visible line.
pub fn hidden_ranges(text: &str, folds: &Folds) -> Vec<(usize, usize)>;
```

The fold header for the caret's line (Fold / Unfold commands): the
innermost foldable block containing the caret's line, its header included.

## Task 0 — feasibility spike (do first, stop if it fails)

The visual collapse relies on egui laying out the hidden chars as rows of
(near) zero height. In a scratch test, build a `LayoutJob` where a middle
run of lines (including their `\n`) uses a `TextFormat` with
`line_height: Some(0.0)` (or the smallest font size egui accepts, e.g.
`0.1`) and a transparent colour, lay it out with `ctx.fonts`/`layout_job`,
and check:

1. the hidden rows' total height is under ~1 px per 10 hidden lines;
2. `galley.pos_from_cursor` for the line after the hidden run sits right
   below the header row;
3. nothing panics, wrapping on and off.

If this does not hold, **stop and report BLOCKED** with what you measured;
the user will decide. Keep the spike as a regular test in
`src/layout_cache.rs` or `src/editor_view.rs` if it passes (it guards the
behaviour against egui upgrades).

## Drawing (`src/layout_cache.rs`, `src/editor_view.rs`, `src/gutter.rs`)

- `EditorRequest` (in `src/app.rs`) gains `pub hidden: Vec<(usize, usize)>`
  (char ranges, default empty) and `pub foldable: Vec<(usize, usize)>`
  (line ranges, default empty) set by the app for the document editor from
  the active tab's `Folds` (next to `request.auto_edit` /
  `request.match_brackets`). Both stay empty in other editors and tests.
- Layout: both the plain and the highlighted path go through
  `layout_cache`. Apply the hidden ranges to the `LayoutJob` there (split
  sections at the range edges, give hidden sections the collapsed format
  from Task 0). The hidden ranges are part of the cache key, so folding or
  unfolding re-lays out once and the cache keeps working.
- Fold marker: after the end of each folded header line paint a small
  rounded `⋯` box; clicking it unfolds that fold.
- Gutter: hidden lines get no line number (their rows are skipped). Each
  foldable line shows a chevron left of the number: `▸` when folded
  (always visible), `▾` when unfolded (only while the pointer is over the
  gutter, as VS Code). Clicking a chevron toggles the fold. Bookmark and
  git marks on hidden lines are not drawn.
- `EditorRequest` → the editor returns what the user clicked (toggle this
  header) in its output (extend the returned tuple or add a small output
  struct); the app applies it to the tab's `Folds`.

## Caret (`src/editor_view.rs`)

After the `TextEdit` handled the frame, if the caret (primary end) lies
inside a hidden range:
- if `ArrowDown` / `PageDown` was pressed this frame: move it to the first
  char after the range (start of the next visible line); `ArrowUp` /
  `PageUp`: move it to the range start (end of the header line). Keep a
  Shift selection's other end where it was. Store the cursor as
  `request.select` is stored today.
- otherwise (the caret was placed there by find, Go to Line, a symbol,
  undo…): report "reveal this line" to the app, which calls
  `Folds::reveal_line`.

## Edits (`src/app.rs`)

When the document text changes (any path), call
`tab.folds.follow_edit(old, new)`. If the changed `char` span (common
prefix/suffix of old and new) touches a hidden range, unfold that fold
first. Find in the app where bookmarks already follow edits and hook in
there.

## Commands

| Command | Shortcut | Where |
|---|---|---|
| Fold | `Ctrl+Shift+[` | View menu, palette |
| Unfold | `Ctrl+Shift+]` | View menu, palette |
| Fold All | — | View menu, palette |
| Unfold All | — | View menu, palette |

- New `ShortcutAction`s in `src/shortcuts.rs`. Logical keys: on a US layout
  Shift+[ arrives as `Key::OpenCurlyBracket` and Shift+] as
  `Key::CloseCurlyBracket`; register those as fallbacks the way
  `GO_TO_BRACKET_SHIFTED` (`Key::Pipe`) does for Go to Bracket.
- Add them to the Command Palette's hand-written `COMMANDS` list in
  `src/palette.rs` and to its key-coverage test.
- Strings in all five `assets/lang/*.json` (same keys everywhere). Follow
  the existing menu key prefix for the View menu; English: "Fold",
  "Unfold", "Fold All", "Unfold All"; id: "Lipat", "Buka Lipatan",
  "Lipat Semua", "Buka Semua Lipatan"; de: "Falten", "Entfalten",
  "Alle falten", "Alle entfalten"; es: "Plegar", "Desplegar",
  "Plegar todo", "Desplegar todo"; zh: "折叠", "展开", "全部折叠", "全部展开".
- `DocumentTab` gains `pub folds: Folds`; folds are per tab, not saved.

## Tests

- `folding`: `foldable` (nested blocks, blank lines inside vs trailing,
  tabs, no indent → none); `hidden_ranges` (one fold, nested folds merge,
  fold at end of text without trailing `\n`); `follow_edit` (line inserted
  above moves the header; header made unfoldable is dropped);
  `reveal_line`; header for the caret line.
- Layout: the spike test; a folded document's galley is shorter by about
  the hidden rows' height.
- Caret: ArrowDown from the header line of a folded block lands on the line
  after it.
- App: Fold / Unfold / Fold All / Unfold All change `folds`; Go to Line into
  a folded block unfolds it; typing on a folded header keeps the fold.
- Hand check in the running app.

## Tasks

0. Spike (above). Stop if it fails.
1. `src/folding.rs` + `mod folding;`, TDD. Commit
   `feat(editor): indentation fold rules`.
2. Drawing: hidden ranges in the layout, marker, gutter chevrons and line
   numbers, the editor's click output. Commit
   `feat(editor): draw folded blocks collapsed`.
3. Caret skipping and reveal, edits following folds, commands, menus,
   palette, strings. Commit `feat(editor): fold and unfold commands`.
4. `todo.md` after the hand check.
