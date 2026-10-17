# Web Language Highlighting Design — JavaScript, TypeScript, HTML

**Date:** 2026-10-17
**Status:** Approved
**Project:** `rred_text_editor`

## Objective

Extend the editor's syntax highlighting to JavaScript, TypeScript, and HTML. Language is
auto-detected from the file extension and can be overridden from the "Bahasa" menu, exactly like
the existing Rust, C, C++, and PHP support.

## Motivation

The editor already highlights four languages through `syntect`. Web files (`.js`, `.ts`, `.html`)
currently open as plain text. Adding them reuses the existing pipeline without any change to the
rendering, caching, or background-highlighting code.

## Key Decision: TypeScript reuses the JavaScript grammar

`syntect`'s bundled grammar set (`SyntaxSet::load_defaults_newlines`) contains `JavaScript` and
`HTML` but **no TypeScript grammar**. Options considered:

1. **Reuse the JavaScript grammar for TypeScript (chosen).** No new dependency or vendored file.
   Keywords, strings, comments, and numbers colour correctly. Type annotations, `interface`, and
   generics are only partly coloured because the JavaScript grammar does not know them.
2. Bundle a TypeScript `.sublime-syntax` file. Full highlighting, but it adds a vendored file with
   its own license and a custom `SyntaxSet` build step, and slows startup.
3. Skip TypeScript. Rejected: the request includes it.

The status bar and menu still say "TypeScript"; only the grammar is shared.

## Architecture

All changes are inside `src/highlighter.rs`. `src/app.rs`, the layout cache, and the background
highlighting pass are unchanged: the new languages flow through the same path as the existing
ones, and the menu is built from `Language::all()`.

### `Language` enum

Add three variants:

| Variant | `display_name` | `syntax_name` (syntect grammar) |
|---|---|---|
| `JavaScript` | `JavaScript` | `JavaScript` |
| `TypeScript` | `TypeScript` | `JavaScript` |
| `Html` | `HTML` | `HTML` |

`Language::all()` lists them after `Php`, so the selector has 8 entries.

### Extension mapping

`Language::from_extension` (case-insensitive) gains:

| Language | Extensions |
|---|---|
| `JavaScript` | `js`, `mjs`, `cjs`, `jsx` |
| `TypeScript` | `ts`, `mts`, `cts`, `tsx` |
| `Html` | `html`, `htm` |

Unknown extensions still map to `PlainText`. `.vue`, `.svelte`, and `.json` are not mapped.

### Highlighting behaviour

- JavaScript, TypeScript, and HTML go through the existing `HighlighterLayouter` unchanged,
  including the 8 KB synchronous threshold, the background pass for larger documents, and the 1 MB
  highlight limit.
- The bundled HTML grammar embeds JavaScript and CSS, so the contents of `<script>` and `<style>`
  blocks are coloured with no extra work.
- A grammar that fails to resolve still falls back to plain text through `pick_syntax`.

## Error Handling

No new failure paths. Syntax-name lookup failure falls back to plain text; `highlight_line` errors
emit the raw line unformatted, as before.

## Testing

Written first, in `src/highlighter.rs`:

- `from_extension` maps every new extension (`js`, `mjs`, `cjs`, `jsx`, `ts`, `mts`, `cts`, `tsx`,
  `html`, `htm`), including upper-case variants.
- `from_path` handles a multi-dot name such as `app.test.ts`.
- `display_name` returns the expected label for each new variant.
- `all()` lists 8 languages (the existing length test changes from 5 to 8).
- Every new language's `syntax_name` resolves to a real grammar in the bundled set, not the
  plain-text fallback.
- A short sample per new language produces more than one colour section, which also proves
  TypeScript receives JavaScript colours.
- An HTML sample with a `<script>` block colours the script contents.
- The existing 37 tests keep passing; `cargo clippy --all-targets` and `cargo build --release`
  stay clean.

## Files Changed

| File | Change |
|---|---|
| `src/highlighter.rs` | New `Language` variants, extension mapping, display and grammar names, tests. |

## Non-Goals

- A bundled TypeScript grammar, so TypeScript type syntax is only partly coloured.
- `.vue`, `.svelte`, `.json`, `.css`, or other web formats.
- Changes to the theme, the layout cache, or the background highlighting.
