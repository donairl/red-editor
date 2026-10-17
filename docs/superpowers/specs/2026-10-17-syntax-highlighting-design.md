# Syntax Highlighting Design — PHP, C, C++, Rust

**Date:** 2026-10-17
**Status:** Approved
**Project:** `rred_text_editor`

## Objective

Add syntax highlighting to the central editor for PHP, C, C++, and Rust, using
[`syntect`](https://github.com/trishume/syntect) (Sublime Text grammars) rendered through
a custom `egui::TextEdit::layouter`. Language is auto-detected from the file extension, with a
manual override available from the File menu and the status bar.

## Motivation

The editor currently renders every document as plain monospace text. Highlighting the four
languages the project is used for improves readability without changing the editing model.
The `layouter` route was chosen over a static re-render because it keeps the caret, selection,
scrolling, and undo behaviour of the stock `TextEdit` completely intact.

## Tech Stack

- Rust (edition 2024), `eframe` 0.37, `egui` 0.37, `rfd` 0.17
- `syntect` 5.3 with the pure-Rust regex backend (`regex-fancy`)

## Architecture

### Dependency

```toml
syntect = { version = "5.3", default-features = false, features = ["parsing", "default-syntaxes", "default-themes", "regex-fancy"] }
```

`default-features = false` disables the default `onig` backend, which links against a C library
(`libonig`) and would impose a system build dependency. `regex-fancy` is pure Rust and provides
comparable matching quality for the grammars in scope.

### Module: `src/highlighter.rs`

A new module owns every syntax-highlighting concern. `app.rs` never touches `syntect` directly —
it holds a `Language` value and passes it to the highlighter. Language detection, the selector
UI, and editor wiring stay separate and independently testable.

Public surface:

| Item | Purpose |
|---|---|
| `enum Language { PlainText, Rust, C, Cpp, Php }` | Supported languages; each variant knows its file extensions and its `syntect` syntax name. |
| `Language::from_extension(ext: &str) -> Language` | Pure extension → language mapping. Case-insensitive. Unknown → `PlainText`. |
| `Language::from_path(path: &Path) -> Language` | Convenience wrapper over `from_extension`. |
| `Language::all() -> &'static [Language]` | Populates the manual selector. |
| `Language::display_name(&self) -> &'static str` | Human-readable label for the menu and status bar. |
| `struct SyntaxHighlighter` | Holds `SyntaxSet`, `ThemeSet`, and reusable `HighlightLines` state. |
| `SyntaxHighlighter::new() -> Self` | Loads bundled defaults. Infallible. |
| `SyntaxHighlighter::layouter(&mut self, lang: Language) -> impl FnMut(&str) -> egui::text::LayoutJob` | Produces the closure handed to `TextEdit::layouter`. |

`SyntaxHighlighter` also exposes a `theme` field so a future theme picker can swap it without
touching the layouter.

## Language Detection & Manual Selector

### Automatic detection

- `Language::from_path` maps extensions as follows:

| Extension | Language |
|---|---|
| `.rs` | `Rust` |
| `.c` | `C` |
| `.h` | `C` |
| `.cpp`, `.hpp`, `.cc`, `.cxx` | `Cpp` |
| `.php` | `Php` |
| anything else | `PlainText` |

- Called from `app.rs` wherever the current document changes: `load_file()` and after `save_as()`
  sets a new path. The result is stored in `TextEditorApp::active_language`.
- New/untitled documents start as `PlainText`.

### Manual override

- `TextEditorApp` gains two fields:
  - `pub active_language: Language` — the auto-detected language.
  - `pub language_override: Option<Language>` — the user's explicit choice.
- The **effective** language is `language_override.unwrap_or(active_language)`. The override wins
  when set; otherwise auto-detection applies. Clearing the override falls back cleanly.
- A dropdown labelled **"Bahasa"** in the **File** menu lists every `Language::all()` entry plus
  an **"Otomatis"** entry. Selecting a language sets `language_override = Some(lang)`; selecting
  "Otomatis" sets it to `None`.
- The status bar gains a clickable label showing the effective language name, which opens the same
  dropdown, so the current mode is always visible without opening a menu.
- Opening a different file always refreshes `active_language` from the new path but leaves a set
  `language_override` in place — the user's explicit choice sticks until they choose "Otomatis".

## Highlight Rendering & Theme

### Theme

Bundled `ThemeSet` defaults, with **InspiredGitHub** as the initial theme: light, high contrast,
and legible on the editor's existing white background.

### Color conversion

`syntect` themes supply colors as `syntect::highlighting::Color { r, g, b, a }`. Conversion is a
direct 8-bit pass-through to `egui::Color32::from_rgb(r, g, b)`. Alpha is ignored; the editor
background provides contrast. No gamma work is needed since egui renders in the same sRGB space.

### Layouter behaviour

- The closure receives each visible line as `&str` and returns an `egui::text::LayoutJob`.
- `HighlightLines::highlight_line` is stateful across lines, which is how multi-line constructs
  (block comments, `/* */`, PHP heredocs) retain their color. egui's `TextEdit` layout pass calls
  the layouter in line order, so the state stays correct. The state resets whenever the language
  changes or a file is loaded.
- Tokens matching the theme's default foreground color are emitted as plain unformatted text, so
  untouched text costs nothing extra and renders identically to the pre-highlighting behaviour.
- Formatted runs use the `Monospace` font family through the `LayoutJob`'s `TextFormat`, matching
  the current editor font exactly so there is no mismatch with the caret or selection overlay.

## Error Handling

- `SyntaxHighlighter::new()` is infallible — it loads syntect's bundled defaults. An internal
  syntax-name lookup failure falls back to `PlainText` rather than erroring.
- `load_file()` already reports I/O errors to the status bar; language detection adds no new
  failure path.
- Files above **1 MB** skip highlighting and render plain, with the status bar showing
  `"File terlalu besar untuk syntax highlighting."` at info level — the file still opens and edits
  normally.
- `highlight_line` can fail on pathological grammars. On error the highlighter emits the raw line
  as unformatted text, so the editor never blanks or panics on malformed source.

## Performance

- Highlighting runs for visible lines only (egui lays out only what is on screen).
- `SyntaxSet` and `ThemeSet` load once at startup, not per frame.
- No incremental re-highlighting cache: egui's visible-line-only layout makes it unnecessary at
  current scale.

## Testing

Pure and headless functions only — no GUI interaction required.

- `Language::from_extension` — each supported extension maps correctly; unknown extensions map to
  `PlainText`; matching is case-insensitive (`.RS`, `.PHP`).
- `Language::from_path` — full paths, extensionless files, and files with multiple dots.
- `Language::display_name` — each variant returns its expected label.
- `SyntaxHighlighter::layouter` — headless: build an `egui::Context`, run the layouter over Rust,
  C, C++, and PHP snippets, assert the returned `LayoutJob` contains the expected sections, and
  assert a multi-line block comment keeps consistent formatting across lines. Also assert the
  plain-text fallback produces unformatted output.
- The existing 10 tests must continue to pass.
- `cargo clippy -- -D warnings` and `cargo build --release` must stay clean.

## Files Changed

| File | Change |
|---|---|
| `Cargo.toml` | Add the `syntect` dependency. |
| `src/highlighter.rs` | New module. |
| `src/app.rs` | Add `active_language` / `language_override` fields, the language menu and status bar label, and wire the layouter into `TextEdit`. |
| `src/main.rs` | Add `mod highlighter;`. |

## Non-Goals

Explicitly out of scope for this feature:

- Line-number gutter.
- Theme picker UI (the `theme` field exists; the selector does not).
- Bracket matching or code folding.
- Incremental re-highlighting cache.