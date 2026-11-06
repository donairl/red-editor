# Settings — design

One place for the editor's settings: a **Settings window** (`Ctrl+,`,
File → Settings…, and the command palette) that holds four new editor
settings and the toggles that already live in menus.

Out of scope: a `settings.json` file, per-language settings, a
"detect indentation" toggle, changing how wide egui draws a `\t`.

## New settings (`src/settings.rs`, new)

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorSettings {
    /// Spaces per indent level when `insert_spaces` is on. 1..=8, default 4.
    pub tab_size: u8,
    /// Indent with spaces (default) or with `\t`.
    pub insert_spaces: bool,
    /// Strip spaces and tabs at the ends of lines on an explicit save.
    /// Default off.
    pub trim_trailing_whitespace: bool,
    /// Run the document's formatter (`prettify.rs`) on an explicit save.
    /// Default off.
    pub format_on_save: bool,
}

impl EditorSettings {
    /// One indent level: `"\t"`, or `tab_size` spaces.
    pub fn indent_unit(&self) -> String;
    pub fn to_storage(&self) -> String;            // JSON object
    pub fn from_storage(text: Option<&str>) -> Self;
}
```

- Stored under one eframe storage key, `editor_settings`, as a JSON object.
- `from_storage` is forgiving, as `session.rs` is: missing or unreadable
  text gives the defaults; each field that is missing or has the wrong type
  takes its default; `tab_size` is clamped to `1..=8`.
- `TextEditorApp` gets `pub editor_settings: EditorSettings`, loaded in
  `new` and written in `save` next to the other keys.
- The existing toggles keep their own storage keys (`theme`, `font_size`,
  `auto_save`, indent guides, render whitespace, `ui_language`). There's
  nothing to migrate.

## Indentation

- `auto_edit::indent_unit(text, fallback: &str)`: detection from the
  document stays first (first indented line decides). Only a document
  with no indented line falls back, now to `editor_settings.indent_unit()`
  instead of the hard-coded four spaces. Every caller passes the setting.
- **Tab** in the editor (focus in the text, no modifier):
  - No selection or a selection within one line: the selection is replaced
    by the indent unit, inserted at the caret.
  - A selection that spans lines: every line it touches gets one indent
    unit in front. A line the selection only touches at column 0 of its
    last line is left out, as in VS Code. The selection grows to keep
    covering the same text.
- **Shift+Tab**: every line the caret or selection touches loses one
  indent level from its front: one `\t`, or up to one indent unit of
  spaces — the unit detected from the document, as Tab uses (4 when the
  unit is `\t`); fewer if fewer are there. Lines with no leading whitespace are
  untouched. The selection follows the text.
- Pure functions over `char` indices, in the style of `line_ops.rs`, return
  `Option<line_ops::Edit>` (`None` = nothing to do):

  ```rust
  pub fn indent(text: &str, selection: Range, unit: &str) -> Option<Edit>;
  pub fn outdent(text: &str, selection: Range, unit: &str) -> Option<Edit>;
  ```

  Wired through the existing `edit_lines` path. egui's own `\t` insertion
  for Tab must be consumed so it does not happen as well. Ctrl+Tab and
  Ctrl+Shift+Tab stay Next/Prev Tab.
- Known limit: egui draws a `\t` four columns wide whatever `tab_size` is,
  so `tab_size` means "spaces per indent", not "width of a tab". Indent
  guides and folding keep counting a `\t` as 4 columns.

## On save

- Applies on **explicit saves only**: Save (`Ctrl+S`), Save As, Save All,
  and saving from the close-dirty-tab dialog. It doesn't apply on
  auto-save, which would strip a space the user just typed and move text
  under the caret mid-typing.
- Order: trim trailing whitespace, then format. Both change the tab's
  `text_content` before it is written, so the editor shows what was saved.
- Trim: a pure `fn trim_trailing_whitespace(text: &str) -> Option<String>`
  (`None` when nothing changes). It removes `' '` and `'\t'` before every
  `\n` and at the end of the text, and keeps `\r\n` line endings intact.
- Format: reuse `prettify::supports` and `prettify::prettify` with the
  tab's effective language. When the language is not supported, nothing
  happens. When the formatter fails or is missing, the file is still
  saved unformatted and the status bar shows `msg.prettify_failed`. That
  warning replaces "Saved" so it is not lost.
- The caret is clamped into the new text. Placing it exactly isn't
  required.

## Settings window (`src/settings_window.rs`, new)

- A non-modal `egui::Window` titled "Settings", closable, open state on
  the app (`show_settings: bool`). Opened by `Ctrl+,`
  (`ShortcutAction::OpenSettings`), File → Settings…, and a palette command.
  The shortcut toggles it.
- Three groups:
  - **Editor**: Tab size (drag value / slider 1..=8), Insert spaces,
    Trim trailing whitespace on save, Format on save.
  - **Appearance**: Font size (same range and step as `zoom.rs`), Theme
    (combo box of the existing themes), Indent guides, Render whitespace.
  - **General**: Auto save, Interface language (combo box of the loaded
    languages).
- Every control edits the same app field the menus edit. A change applies
  at once and is saved with the rest of storage. Toggling in a menu shows
  in the window and the other way round. Theme, language and auto save go
  through the existing setter or toggle functions so their side effects
  (repaint, i18n reload, auto-save timers) still happen.
- All labels come from i18n. The new keys go into every file in
  `assets/lang/` (`en`, `id`, `de`, `es`, `zh`).

## Testing

- `settings.rs`: storage round trip; defaults from `None`, `""`, junk and
  wrong types; `tab_size` clamped (0 → 1, 99 → 8); `indent_unit` for both
  modes.
- `auto_edit::indent_unit` falls back to the given unit only when no line
  is indented.
- `indent` / `outdent`: caret only, a selection in one line, multi-line
  selections, the column-0 last line, tabs vs spaces, partial space
  indent on outdent, lines without indentation, selection kept on the
  same text.
- `trim_trailing_whitespace`: spaces and tabs, last line without `\n`,
  `\r\n` kept, untouched text gives `None`.
- App tests, in the style of the existing ones in `app.rs`:
  - an explicit save trims and formats when on, and leaves the text alone
    when off;
  - auto-save does neither;
  - a failing formatter still saves and reports;
  - `Ctrl+,` opens the window;
  - a checkbox in the window flips the same field the menu does.
- Every language file has every key (the existing i18n test, if there is
  one, covers it).
