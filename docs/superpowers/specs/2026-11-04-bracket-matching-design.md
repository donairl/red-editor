# Bracket matching — design and plan

With the caret next to a bracket, both it and its partner get a thin box,
as in VS Code. `Ctrl+Shift+\` (Edit → Go to Bracket) jumps to the partner.

Out of scope: telling brackets inside strings or comments apart (a plain
character scan; a `")"` in a string can mismatch), bracket-pair colouring,
selecting to the bracket, `<` `>`.

## Pure layer (`src/brackets.rs`, new)

Pure functions over `char` indices (how egui's `TextEdit` counts its caret),
in the style of `src/line_ops.rs`.

```rust
/// Most chars scanned in each direction, so a huge file stays fast.
pub const SCAN_LIMIT: usize = 100_000;

/// The bracket touching `caret` and its partner, as `(char index of the
/// bracket at the caret, char index of its partner)`. The char just before
/// the caret is tried first, then the char just after it. Pairs are
/// `()` `[]` `{}`; nesting is counted for the same kind only. `None` when no
/// bracket touches the caret or no partner is found within `SCAN_LIMIT`.
pub fn matching_pair(text: &str, caret: usize) -> Option<(usize, usize)>;

/// Where Go to Bracket puts the caret: next to the partner, on its outer
/// side — before an opener, after a closer — so pressing again returns.
pub fn jump_target(text: &str, caret: usize) -> Option<usize>;
```

Byte/char conversion must not be quadratic: walk `char_indices` once from
the start to the caret area, then scan within the limit.

## Highlight (`src/editor_view.rs`)

After the `TextEdit` is shown (where the find match `request.highlight` is
painted, using the same `span` helper), when the editor id has focus, the
cursor is a caret (no selection) and the document is not over the
highlight size limit (`text_content_exceeds_highlight_limit` /
`too_large`, already used there), call `matching_pair` and paint a 1-px
rounded outline around each of the two one-char spans. Colour: the theme's
foreground at about 40% alpha, or an existing theme colour if one fits
(look in `src/theme.rs`). `EditorRequest` (in `src/app.rs`) gains
`pub match_brackets: bool`, default `false`, set `true` by the app for the
document editor (next to where `request.auto_edit` is set), so other
editors and the tests are unchanged.

## Go to Bracket

- `ShortcutAction::GoToBracket` in `src/shortcuts.rs` with
  `Ctrl+Shift+\` (`Key::Backslash`), following how the other actions are
  declared, listed and labelled. The Command Palette picks actions up from
  there; check it does.
- The app handles it like the other editor commands (`edit_lines` /
  `goto`-style): read the caret with `editor_selection`, call `jump_target`,
  set `editor_request.select = Some((t, t))`, `reveal`, `focus`.
- Menu entry Edit → Go to Bracket with its shortcut shown, next to the
  other navigation/edit entries in `src/menus.rs`.
- String key `edit.go_to_bracket` in all five `assets/lang/*.json`:
  en "Go to Bracket", id "Ke Pasangan Kurung", de "Zur Klammer springen",
  es "Ir al corchete", zh "跳转到匹配括号".

## Tests

- `brackets`: before-caret preferred over after; nested same kind; mixed
  kinds don't count each other; unmatched → `None`; caret not touching a
  bracket → `None`; multibyte text before the bracket (char indices, not
  bytes); `jump_target` lands before an opener / after a closer and
  pressing twice returns.
- App: Go to Bracket moves the caret (via `editor_request.select`).
- Shortcut: `Ctrl+Shift+\` maps to `GoToBracket` (follow the existing
  shortcut tests).
- Hand check in the running app.

## Tasks

1. `src/brackets.rs` + `mod brackets;`, TDD. Commit
   `feat(editor): find the matching bracket`.
2. Highlight, shortcut, menu, strings, app handling, tests. Commit
   `feat(editor): highlight the bracket pair and go to bracket`.
3. `todo.md` after the hand check.
