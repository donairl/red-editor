# Auto-indent and auto-close — design and plan

Enter keeps the current line's indentation (one level deeper after an
opening bracket); typing an opening bracket or quote inserts its partner.
Always on, like VS Code's defaults; no setting.

Out of scope: a Tab key that inserts the indent unit, reindenting a paste,
language-aware indentation (Python `:`), multi-cursor.

## Pure layer (`src/auto_edit.rs`, new)

Same style as `src/line_ops.rs`: pure functions over `char` indices, which
is how egui's `TextEdit` counts its cursor, returning `line_ops::Edit`
(`text` + `selection`). `Range` is `line_ops::Range` (sorted `char` range;
equal ends are a caret).

```rust
/// One level of indentation in `text`: the leading whitespace of the first
/// indented line decides — a tab gives "\t", spaces give that many spaces
/// clamped to 2..=8. A document with no indented line gives 4 spaces.
pub fn indent_unit(text: &str) -> String;

/// Enter: replaces the selection with "\n" + the current line's leading
/// whitespace (only the part before the caret). After `{` `(` `[` (the last
/// non-space char before the caret), one more `unit`. When the caret also
/// sits right before the matching closer (`{|}`), the closer goes on its own
/// line at the original indentation and the caret stays on the middle line.
pub fn on_enter(text: &str, sel: Range, unit: &str) -> Edit;

/// A typed `ch`. `None` = not special, insert it normally.
/// - Opener `(` `[` `{`: with a selection, wraps it (selection kept, inside
///   the pair). With a caret, inserts the pair and puts the caret between —
///   only when the next char is whitespace, end of text, or a closer
///   `)` `]` `}` (else `None`).
/// - Quote `"`, `'`, `` ` ``: if the next char is the same quote, steps over
///   it. Else as an opener, but also never right after a letter, digit or
///   `_` (`don't`). Which quotes pair depends on `language` (below).
/// - Closer `)` `]` `}`: if the next char is the same, steps over it. `}`
///   (and `)` `]`) typed where everything before the caret on the line is
///   whitespace removes one `unit` from that whitespace first (if present),
///   then inserts.
/// `Language::PlainText` never auto-closes (returns `None` for openers and
/// quotes; stepping over and dedent still apply).
pub fn on_char(text: &str, sel: Range, ch: char, language: Language, unit: &str) -> Option<Edit>;

/// Backspace with a caret between an empty pair — `()` `[]` `{}` `""` `''`
/// `` `` `` — removes both. `None` otherwise (normal backspace).
pub fn on_backspace(text: &str, sel: Range) -> Option<Edit>;
```

Quotes that pair, by language:

| Quote | Pairs in |
|---|---|
| `"` | every language except PlainText |
| `'` | every language except PlainText, Rust (lifetimes `'a`), Markdown |
| `` ` `` | JavaScript, TypeScript, Go, Markdown only |

Look up the exact `Language` variant names in `src/highlighter.rs`.

## Hook (`src/editor_view.rs`, `src/app.rs`)

- `EditorRequest` (in `app.rs`) gains `pub auto_edit: Option<Language>`:
  `Some(language)` turns the behaviour on. `Default` leaves it `None`, so
  existing tests and other editors are unchanged. The two
  `render_editor_with` calls that draw the document editor (around
  `app.rs:2536` and `:2556`) set it to the active tab's effective language.
- In `render_editor_inner`, before the `TextEdit` is shown (next to where
  `request.select` and `request.key` are applied), when `auto_edit` is
  `Some`, the editor `id` has focus and the `TextEdit` state has a cursor:
  look at this frame's `i.events`. If any of them is special — `Key::Enter`
  pressed with no modifiers (Shift allowed), a `Text` of one char for which
  `on_char` returns `Some`, or `Key::Backspace` pressed with no modifiers for
  which `on_backspace` returns `Some` — then handle **every** `Text`, Enter
  and Backspace event of the frame in order yourself (plain `Text` = replace
  selection; plain Backspace = delete selection or the char before the
  caret), remove those events from `i.events`, write `text`, and store the
  final selection in the `TextEdit` state (same `load_state`/`store`
  pattern already used for `request.select`). Handling all of them keeps
  fast typing in order. Frames with nothing special are left to `TextEdit`.
- Undo: the edit goes straight into `text`, as Move Line / Toggle Comment
  already do; `TextEdit`'s undoer picks the new state up.
- IME: skip the hook while an `Event::Ime` is in the frame.

## Tests

- `auto_edit`: `indent_unit` (tab doc, 2-space doc, no indent → 4);
  `on_enter` (keeps indent; caret mid-line keeps only the whitespace before
  it; after `{` adds a unit; `{|}` splits into three lines with caret in
  the middle; replaces a selection); `on_char` (pair inserted; not before a
  word char; wraps selection; quote not after a letter; steps over closer
  and quote; `'` not paired in Rust; nothing pairs in PlainText; `}` on a
  blank indented line dedents one unit); `on_backspace` (empty pair removed;
  non-empty or no pair → `None`).
- `editor_view`: a focused editor with `auto_edit: Some(Rust)` fed the
  events `Text("(")` then (next frame) `Key::Enter` inside `{}` produces the
  expected text and caret; with `auto_edit: None` the same `(` input is left
  to egui (`text == "("`).
- Hand check in the running app.

## Tasks

1. `src/auto_edit.rs` + `mod auto_edit;` in `src/main.rs`, TDD on the pure
   functions. Commit `feat(editor): auto-indent and auto-close rules`.
2. The hook: `EditorRequest::auto_edit`, the event handling in
   `editor_view.rs`, set at the two app call sites, editor tests. Commit
   `feat(editor): auto-indent on Enter and auto-close brackets and quotes`.
3. `todo.md`: tick the item after the hand check.
