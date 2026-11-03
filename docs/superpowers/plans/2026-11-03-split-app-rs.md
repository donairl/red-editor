# Split `src/app.rs` — design and plan

> **For agentic workers:** one task per module, in order, each ending with `cargo build` free of warnings, `cargo test` at the same count as before (453 at the time of writing) and one commit. No behaviour change anywhere.

**Goal:** `src/app.rs` is 7,092 lines (≈3,800 code, ≈3,270 tests). Move five self-contained concerns into their own modules so each file holds one thing and future features (palette commands, menus, panels) touch a focused file.

**How:** Rust allows `impl TextEditorApp { … }` blocks in any module of the crate, so methods move without changing the struct. Each new module gets `use crate::app::TextEditorApp;` plus whatever it needs; `app.rs` keeps the struct, `Default`, `new`, tab/file/find/git glue and `update`. Fields are already `pub` (only `last_marks_at` is private; it stays with `update_line_marks` in `app.rs`). Items that were private to `app.rs` and are now used across the two files become `pub(crate)`.

**Tests:** a test moves with its code only when it is self-contained — it uses helpers that only moved tests use, or no helpers at all. Everything else stays in `app.rs`'s `mod tests`. A test helper used by both moved and staying tests is made `pub(crate)` inside `#[cfg(test)] pub(crate) mod test_support { … }` in `app.rs` and imported from there. Never duplicate a helper.

**Rules for every task**

- Cut and paste; do not rewrite, reorder or rename functions. Doc comments travel with their items.
- Keep the import style of the codebase (`use eframe::egui;`, `use crate::x::{...}`); trim unused imports in both files so the build has zero warnings.
- `cargo test 2>&1 | grep -E '^test result'` must print the same `passed` count as before the task; no test is deleted or `#[ignore]`d.
- `src/i18n.rs` `test_every_key_used_in_code_exists` scans a fixed list of source files with `include_str!`; add each new module that calls `i18n.t(` / `tf(` to that list.
- Do not run rustfmt on `app.rs`. Running it on the new file only is fine.
- One commit per task: `refactor(app): move <what> to src/<module>.rs`.

---

## Task 1: `src/shortcuts.rs`

Move, flattened (the constants sit at the module's top level so `shortcuts::NEW_TAB` and `shortcuts::label(ctx, &s)` keep reading the same from `app.rs`):

- the contents of `pub mod shortcuts { … }` (consts `NEW_TAB` … `PASTE`, helpers `cmd`, `cmd_shift`, `pub fn label`);
- `pub enum ShortcutAction` and its `impl` (`needs_editor`, `takes_keyboard`, `shortcut`);
- `pub fn consume_shortcut`.

In `app.rs` replace the module with `use crate::shortcuts::{self, ShortcutAction, consume_shortcut};`. Update other users: `grep -rn 'app::ShortcutAction\|app::shortcuts\|app::consume_shortcut' src/` (at least `src/palette.rs` and its tests) to the new path. Add `mod shortcuts;` to `src/main.rs` (alphabetical).

Tests that move: `test_consume_shortcut_prefers_shift_variant` (if it only builds an `InputState`). `test_ctrl_shift_g_is_source_control_not_goto_line` and the other `*_shortcut*` tests drive the whole app and stay.

## Task 2: `src/menus.rs`

Move the `impl TextEditorApp` methods: `show_file_menu`, `show_edit_menu`, `show_clipboard_items`, `show_view_menu`, `show_theme_menu`, `show_language_menu`, `show_ui_language_menu`, `show_help_menu`, `show_recent_menu`, `show_recent_items`, `show_editor_context_menu`, `context_line`; and the free functions `menu_item`, `recent_row`.

Keep in `app.rs` the actions those menus call (`open_new_tab`, `toggle_console`, `toggle_bookmark`, …).

Tests that move (with `painted_galley` and `shown_chars` if nothing else uses them): the four `test_open_recent_menu_*`, `test_file_menu_tab_actions`, `test_file_menu_offers_new_window_after_close_tab`, `test_language_menu_lives_in_view_menu_not_status_bar`, `test_help_home_opens_the_project_page`.

## Task 3: `src/status_bar.rs`

Move `show_status_bar`, `show_branch_menu`, `caret_position`.

Tests that move: `test_status_bar_shows_the_git_branch`, `test_status_bar_shows_the_caret_position_on_the_right`, `test_status_bar_position_follows_typing_and_arrows`, `test_status_bar_badge_counts_bookmarks_and_opens_the_list` — each only if self-contained per the rule above.

## Task 4: `src/console_panel.rs`

Move `toggle_console`, `show_console_panel`, `console_dir`, and the free functions `console_icon_button`, `paint_hide_icon`, `paint_kill_icon`.

No tests move (none target the console panel directly).

## Task 5: `src/editor_view.rs`

Move the free functions `render_editor` (`#[cfg(test)]`), `selection_rects`, `render_editor_with`, `render_editor_inner`, `text_content_exceeds_highlight_limit`, the const `HIGHLIGHT_SIZE_LIMIT`, and any type or helper used only by them (check what `render_editor_inner` touches: `EditorRequest`, `EditorOutput`-like types, layouter helpers; move a type only if `app.rs` no longer uses it directly, otherwise import it).

Tests that move (with `painted_number`, `render_with_gutter` and `render_editor` when only they use them): `test_highlight_size_limit`, `test_render_editor_plain_path_is_scrollable`, `test_render_editor_highlighted_path_is_scrollable`, `test_render_editor_lays_out_through_the_cache`, `test_render_editor_round_trips_text_on_both_paths`, `test_wide_line_keeps_line_starts_visible`, `test_wide_line_end_is_reachable_by_scrolling`, `test_plain_path_scrolls_horizontally_without_wrap`, `test_highlighted_path_scrolls_horizontally_without_wrap`, `test_plain_path_does_not_scroll_horizontally_with_wrap`, `test_highlighted_path_does_not_scroll_horizontally_with_wrap`, `test_selection_rects_cover_each_selected_row`, `test_line_numbers_paint_beside_each_line`, `test_hidden_line_numbers_paint_nothing`, `test_line_numbers_stick_when_scrolled_sideways`, `test_caret_line_number_is_highlighted`.

## Done when

`wc -l src/app.rs` is well under 5,000, `cargo build` has no warnings, `cargo test` passes at the original count, and `git log` shows five `refactor(app): …` commits.
