# JSON Syntax Highlighting Design

**Date:** 2026-10-17
**Status:** Approved
**Project:** `rred_text_editor`

## Objective

Extend the editor's syntax highlighting to JSON files (`.json` and `.jsonc`). Language is auto-detected from the file extension and can be manually overridden from the "Tampilan → Bahasa" menu, exactly like existing supported languages (Rust, C, C++, PHP, JavaScript, TypeScript, and HTML).

## Motivation

JSON is one of the most common configuration and data interchange formats (used directly by the editor itself in `assets/lang/*.json`). Opening JSON files currently falls back to `PlainText` without syntax highlighting. Adding JSON leverages `syntect`'s bundled `"JSON"` grammar without adding new dependencies or modifying the core rendering and background caching pipeline.

## Architecture

All syntax detection and highlighting changes are encapsulated in `src/highlighter.rs`. Visual badge representation is updated in `src/tab_bar.rs`. The explorer sidebar (`src/file_browser.rs`) and the menus (`src/app.rs`) automatically inherit the changes through `tab_bar::badge` and `Language::all()`.

### `Language` enum (`src/highlighter.rs`)

Add the `Json` variant:

| Variant | `display_name` | `syntax_name` (syntect grammar) |
|---|---|---|
| `Json` | `JSON` | `JSON` |

`Language::all()` lists `Json` after `Html`, bringing the total supported language count to 9.

### Extension mapping (`src/highlighter.rs`)

`Language::from_extension` (case-insensitive) maps:

| Language | Extensions |
|---|---|
| `Json` | `json`, `jsonc` |

### Tab bar & explorer badge (`src/tab_bar.rs`)

In `tab_bar::badge(language: Language)`:

| Language | Badge Text | Badge Color |
|---|---|---|
| `Json` | `"{}"` | `Some(Color32::from_rgb(0xCB, 0xB0, 0x41))` |

This gives JSON tabs and tree rows in the file explorer the familiar VS Code gold `{}` badge.

### Highlighting behaviour

- JSON documents follow the standard highlighting pipeline: synchronous inside the frame when <= 8 KB (`SYNC_HIGHLIGHT_LIMIT`), and asynchronously on a background thread when > 8 KB.
- Files exceeding 1 MB are rendered as plain text without highlighting, preserving existing editor safety limits.
- If grammar lookup were to fail, `pick_syntax` cleanly falls back to plain text.

## Error Handling

No new failure modes are introduced. If `highlight_line` encounters an error, the raw line is emitted unformatted. Unknown file extensions continue falling back to `PlainText`.

## Testing

Tests to be added/updated:
1. `src/highlighter.rs`:
   - `test_from_extension_maps_supported_extensions`: test `"json"` and `"jsonc"` mapping to `Language::Json`.
   - `test_from_extension_is_case_insensitive`: test `"JSON"` and `"JSONC"`.
   - `test_display_name_labels_each_language`: verify `Language::Json.display_name() == "JSON"`.
   - `test_syntax_names_resolve_in_bundled_syntax_set`: verifies syntect resolves `"JSON"`.
   - `test_all_lists_every_variant`: count updated to 9.
   - `test_json_is_coloured`: verifies a sample JSON string produces colored tokens (keys, strings, numbers).
2. `src/tab_bar.rs`:
   - `test_every_language_has_a_badge`: verifies `Language::Json` has non-empty badge `"{}"` and color `Color32::from_rgb(0xCB, 0xB0, 0x41)`.

## Files Changed

| File | Change |
|---|---|
| `src/highlighter.rs` | Add `Language::Json`, extension mapping (`json`, `jsonc`), `display_name`, `syntax_name`, and tests |
| `src/tab_bar.rs` | Add badge mapping for `Language::Json` and unit test assertion |
| `README.md` | Update feature description list to include JSON |
