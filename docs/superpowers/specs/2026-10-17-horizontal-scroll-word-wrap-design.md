# Horizontal Scrollbar and Word-Wrap Toggle Design

**Date:** 2026-10-17
**Status:** Draft, awaiting user review
**Project:** `rred_text_editor`

## Objective

Add a horizontal scrollbar to the editor so lines wider than the window can be scrolled sideways,
and add a menu toggle that switches word wrap on or off.

## Motivation

Today the two render paths disagree:

- **Highlighted text never wraps.** `src/highlighter.rs` sets `job.wrap.max_width = f32::INFINITY`
  (lines 266, 297, 346) and the layouter closure in `render_editor` ignores its `wrap_width`
  argument. A long line runs off-screen, and the editor's `ScrollArea::vertical()` gives no way to
  reach it.
- **Plain text wraps.** Plain text and files over 1 MB (`HIGHLIGHT_SIZE_LIMIT`) use the stock
  `TextEdit` wrapping, so they never need a horizontal scrollbar.

## Decisions

- Wrapping is user-controlled through a toggle (chosen over "never wrap" and "keep plain wrapping").
- Default is **wrap off**, like a code editor. Plain text therefore stops wrapping by default.
- The toggle lives in a new **"Tampilan"** menu between "File" and "Help", as a checkbox labelled
  **"Bungkus Baris"**.
- The setting is not persisted; it resets to off when the app restarts.

## Architecture

Changes are limited to `src/app.rs`. `src/highlighter.rs`, the layout cache, and the background
highlighting pass are untouched, because wrapping is a layout concern, not a highlighting one.

### State

`TextEditorApp` gains `pub word_wrap: bool`, set to `false` in `Default`.

### Menu

A "Tampilan" `menu_button` between "File" and "Help" holds one `ui.checkbox(&mut self.word_wrap,
"Bungkus Baris")`.

### `render_editor`

`render_editor` takes a `word_wrap: bool` parameter.

| `word_wrap` | Scroll area | Text width |
|---|---|---|
| `false` | `ScrollArea::both()` | Never wraps; as wide as the longest line, at least the viewport width. |
| `true` | `ScrollArea::vertical()` (as today) | Wraps to the viewport width. |

The text area keeps at least the viewport width so clicks in empty space still land in the editor.

### Highlighted path

The layouter closure stops ignoring `wrap_width`. It already clones the cached job each frame, so it
sets `job.wrap.max_width` to `wrap_width` when wrapping and to `f32::INFINITY` when not, before
calling `layout_job`. The cache key and background pass do not change.

### Plain path

The stock `TextEdit` wraps to the available width on its own. Turning wrap off means giving it an
unbounded width inside the horizontal scroll area, so only the scroll area and width settings change.

### Open question for planning

How egui 0.37 sizes a multiline `TextEdit` inside `ScrollArea::both()` is unverified. The existing
`add_sized(ui.available_size(), ...)` call may need to change when the width is unbounded. The plan
starts with a throwaway probe that measures the content width for a very long line, on both paths,
with wrap on and off. The probe's findings decide the final width settings; the behaviour above does
not change.

## Error Handling

No new failure paths. The toggle is a plain bool and nothing is parsed or persisted.

## Testing

Written first, in `src/app.rs`:

- `word_wrap` defaults to `false`.
- With wrap off, a very long line makes the scroll area's content wider than the viewport, on the
  plain path and on the highlighted path.
- With wrap on, the content width stays within the viewport on both paths.
- A wheel or drag event with a horizontal delta moves the horizontal offset when wrap is off.
- The buffer round-trips unchanged in every combination.
- The existing scroll and round-trip tests keep passing, updated for the new `render_editor`
  signature. `cargo clippy --all-targets` and `cargo build --release` stay clean.

## Files Changed

| File | Change |
|---|---|
| `src/app.rs` | `word_wrap` field, "Tampilan" menu, `render_editor` signature and scroll area, layouter wrap width, tests. |

## Non-Goals

- Persisting the setting across restarts.
- A keyboard shortcut for the toggle.
- Line numbers, a minimap, or a wrap indicator.
- Any change to `src/highlighter.rs`.
