# Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add syntax highlighting for PHP, C, C++, and Rust to the central editor, using `syntect` grammars rendered through a custom `egui::TextEdit::layouter`, with language auto-detected from the file extension and overridable from the File menu and status bar.

**Architecture:** A new `src/highlighter.rs` module owns all syntax-highlighting concerns behind a small, testable surface: a `Language` enum (pure extension→language mapping, unit-testable without a GUI) and a `SyntaxHighlighter` struct (holds the `SyntaxSet`, `ThemeSet`, and stateful `HighlightLines`). `src/app.rs` gains `active_language` and `language_override` fields and passes the effective language into the highlighter — it never touches `syntect` types directly. `src/main.rs` adds `mod highlighter;`.

**Tech Stack:** Rust (edition 2024), `eframe` 0.37, `egui` 0.37, `rfd` 0.17, `syntect` 5.3 with the pure-Rust `regex-fancy` backend.

## Global Constraints

- Dependency added to `Cargo.toml` exactly as: `syntect = { version = "5.3", default-features = false, features = ["parsing", "default-syntaxes", "default-themes", "regex-fancy"] }`
- `default-features = false` is mandatory: it disables the default `onig` backend, which links against a C library (`libonig`) and would impose a system build dependency.
- Theme is `InspiredGitHub`. Its default foreground is `Color { r: 50, g: 50, b: 50, a: 255 }`, and its comment color for C/C++/Rust is `Color { r: 150, g: 152, b: 150, a: 255 }`. Both values are asserted in tests.
- Grammar names resolved via `SyntaxSet::find_syntax_by_name` are exactly: `Rust`, `C`, `C++`, `PHP`, `Plain Text`. All five are present in syntect 5.3's bundled set.
- Use `SyntaxSet::load_defaults_newlines()`, not `load_defaults_nonewlines()`, because the layouter feeds it lines split on `\n`.
- Extension mapping: `.rs` → Rust, `.c` → C, `.h` → C, `.cpp`/`.hpp`/`.cc`/`.cxx` → C++, `.php` → Php, everything else → `PlainText`. Matching is case-insensitive.
- Effective language is `language_override.unwrap_or(active_language)`.
- The initial document text stays exactly `Ketik sesuatu di sini...`; changing it breaks `test_is_dirty_detection`.
- Files larger than 1 MB skip highlighting and set the status message `"File terlalu besar untuk syntax highlighting."` at info level (`is_error == false`).
- The existing 10 tests must continue to pass. `cargo clippy -- -D warnings` and `cargo build --release` must stay clean.
- All user-facing UI strings in this feature are in Indonesian, matching the existing codebase.

### egui 0.37 API notes (verified against the installed crate)

These differ from older egui versions and are the most likely source of compile errors:

- `TextEdit::layouter` takes `&mut dyn FnMut(&Ui, &dyn TextBuffer, f32) -> Arc<Galley>`.
- `LayoutJob` fields are `text`, `sections`, `wrap`, `first_row_min_height`, `break_on_newline`, `halign`, `justify`, `round_output_to_gui`, `keep_trailing_whitespace`.
- `LayoutSection` has exactly two fields: `leading_space` and `format`. There is no `font_id` or `extra` field on it — both live inside `format`.
- `LayoutSection::byte_range` is `std::ops::Range<ByteIndex>`, and `ByteIndex` is a newtype over `usize`. Construct with `egui::text::ByteIndex(n)`.
- `egui::FontId` is **not** `Copy`; clone it when storing it in a format.
- `LayoutJob::debug_sanity_check()` asserts that sections cover the whole text with no gaps or overlaps — use it in tests.
- `ui.fonts(|f| f.resolve_text_style(egui::TextStyle::Monospace))` returns the `FontId` for the editor font.

---

### Task 1: `Language` Enum and Extension Detection

**Files:**
- Create: `src/highlighter.rs`
- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Test: unit tests in `src/highlighter.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces:
  - `pub enum Language { PlainText, Rust, C, Cpp, Php }`
  - `Language::from_extension(ext: &str) -> Language`
  - `Language::from_path(path: &Path) -> Language`
  - `Language::all() -> &'static [Language]`
  - `Language::display_name(&self) -> &'static str`
  - `Language::syntax_name(&self) -> &'static str`
  - `pub struct SyntaxHighlighter` with `new()`, `syntax_set()`, `theme()`, `syntax_for()`, `start()`

- [ ] **Step 1: Add the `syntect` dependency to `Cargo.toml`**

Replace the `[dependencies]` section with:

```toml
[dependencies]
eframe = "0.37" # Gunakan versi terbaru yang stabil
rfd = "0.17"
syntect = { version = "5.3", default-features = false, features = ["parsing", "default-syntaxes", "default-themes", "regex-fancy"] }
```

- [ ] **Step 2: Create `src/highlighter.rs` with the failing tests**

Create `src/highlighter.rs` containing only the test module:

```rust
//! Syntax highlighting for the central editor.
//!
//! Owns language detection and the `syntect`-backed highlighter. `app.rs` never
//! touches `syntect` types directly — it holds a `Language` value and passes it here.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_from_extension_maps_supported_extensions() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("c"), Language::C);
        assert_eq!(Language::from_extension("h"), Language::C);
        assert_eq!(Language::from_extension("cpp"), Language::Cpp);
        assert_eq!(Language::from_extension("hpp"), Language::Cpp);
        assert_eq!(Language::from_extension("cc"), Language::Cpp);
        assert_eq!(Language::from_extension("cxx"), Language::Cpp);
        assert_eq!(Language::from_extension("php"), Language::Php);
    }

    #[test]
    fn test_from_extension_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Language::Rust);
        assert_eq!(Language::from_extension("PHP"), Language::Php);
        assert_eq!(Language::from_extension("Cpp"), Language::Cpp);
    }

    #[test]
    fn test_from_extension_unknown_defaults_to_plain_text() {
        assert_eq!(Language::from_extension("txt"), Language::PlainText);
        assert_eq!(Language::from_extension(""), Language::PlainText);
        assert_eq!(Language::from_extension("py"), Language::PlainText);
    }

    #[test]
    fn test_from_path_handles_full_paths_and_missing_extension() {
        assert_eq!(
            Language::from_path(Path::new("/home/user/main.rs")),
            Language::Rust
        );
        assert_eq!(
            Language::from_path(Path::new("/tmp/archive.tar.gz")),
            Language::PlainText
        );
        assert_eq!(
            Language::from_path(Path::new("/tmp/Makefile")),
            Language::PlainText
        );
        assert_eq!(
            Language::from_path(Path::new("/tmp/a/b/script.php")),
            Language::Php
        );
    }

    #[test]
    fn test_display_name_labels_each_language() {
        assert_eq!(Language::PlainText.display_name(), "Teks Biasa");
        assert_eq!(Language::Rust.display_name(), "Rust");
        assert_eq!(Language::C.display_name(), "C");
        assert_eq!(Language::Cpp.display_name(), "C++");
        assert_eq!(Language::Php.display_name(), "PHP");
    }

    #[test]
    fn test_all_lists_every_variant() {
        assert_eq!(Language::all().len(), 5);
    }

    #[test]
    fn test_syntax_names_resolve_in_bundled_syntax_set() {
        let highlighter = SyntaxHighlighter::new();
        for lang in Language::all() {
            assert!(
                highlighter.syntax_set().find_syntax_by_name(lang.syntax_name()).is_some(),
                "missing bundled grammar for {lang:?} ({})",
                lang.syntax_name()
            );
        }
    }

    #[test]
    fn test_theme_default_foreground_matches_inspected_value() {
        let fg = SyntaxHighlighter::new()
            .theme()
            .settings
            .foreground
            .expect("InspiredGitHub defines a default foreground");
        assert_eq!((fg.r, fg.g, fg.b), (50, 50, 50));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --bin rred_text_editor highlighter::tests`

Expected: FAIL to compile with `cannot find type 'Language' in this scope` and `cannot find function 'SyntaxHighlighter' in this scope`. Both are expected — neither exists yet.

- [ ] **Step 4: Register the module in `src/main.rs`**

Add `mod highlighter;` to the module list in `src/main.rs`, keeping alphabetical order:

```rust
mod app;
mod file_browser;
mod highlighter;
mod modals;
```

- [ ] **Step 5: Implement the `Language` enum and `SyntaxHighlighter`**

Replace the entire contents of `src/highlighter.rs` with this complete file:

```rust
//! Syntax highlighting for the central editor.
//!
//! Owns language detection and the `syntect`-backed highlighter. `app.rs` never
//! touches `syntect` types directly — it holds a `Language` value and passes it here.

use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Theme used for all highlighting.
const THEME_NAME: &str = "InspiredGitHub";

/// Languages the editor can highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Language {
    /// No highlighting — plain monospace text.
    #[default]
    PlainText,
    Rust,
    C,
    Cpp,
    Php,
}

impl Language {
    /// Every language, in the order they appear in the selector.
    pub fn all() -> &'static [Language] {
        &[
            Language::PlainText,
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::Php,
        ]
    }

    /// Maps a file extension (without the leading dot) to a language.
    ///
    /// Matching is case-insensitive. Unknown extensions map to [`Language::PlainText`].
    pub fn from_extension(ext: &str) -> Language {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cpp" | "hpp" | "cc" | "cxx" => Language::Cpp,
            "php" => Language::Php,
            _ => Language::PlainText,
        }
    }

    /// Maps a file path to a language via its extension.
    ///
    /// Paths without an extension map to [`Language::PlainText`].
    pub fn from_path(path: &Path) -> Language {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::PlainText)
    }

    /// Human-readable label for the menu and status bar.
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::PlainText => "Teks Biasa",
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Php => "PHP",
        }
    }

    /// The grammar name inside syntect's bundled `SyntaxSet`.
    pub fn syntax_name(&self) -> &'static str {
        match self {
            Language::PlainText => "Plain Text",
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Php => "PHP",
        }
    }
}

/// Holds syntect's grammars and themes for the lifetime of the application.
///
/// Construct once and reuse: loading the bundled sets is expensive.
pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl SyntaxHighlighter {
    /// Loads syntect's bundled grammars and themes. Infallible.
    pub fn new() -> Self {
        Self {
            // The layouter feeds this set lines split on '\n', so it needs the
            // newline-terminated grammar variant.
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    /// The bundled grammar set.
    pub fn syntax_set(&self) -> &SyntaxSet {
        &self.syntax_set
    }

    /// The active theme, falling back to the first bundled theme if the
    /// configured one is ever missing.
    pub fn theme(&self) -> &Theme {
        self.theme_set
            .themes
            .get(THEME_NAME)
            .or_else(|| self.theme_set.themes.values().next())
            .expect("syntect always bundles at least one theme")
    }

    /// Finds the grammar for a language.
    ///
    /// An unknown grammar name falls back to plain text rather than erroring.
    pub fn syntax_for(&self, lang: Language) -> &SyntaxReference {
        self.syntax_set
            .find_syntax_by_name(lang.syntax_name())
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }

    /// Starts a fresh, stateful highlighter for `lang`.
    ///
    /// The returned value must be fed lines in order for multi-line constructs
    /// such as block comments to keep their color.
    pub fn start(&self, lang: Language) -> HighlightLines<'_> {
        HighlightLines::new(self.syntax_for(lang), self.theme())
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --bin rred_text_editor highlighter::tests`

Expected: PASS — 8 tests, 0 failures.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/highlighter.rs src/main.rs
git commit -m "feat(highlighter): add Language enum and syntect-backed SyntaxHighlighter"
```

---

### Task 2: Layouter and Color Conversion

**Files:**
- Modify: `src/highlighter.rs`
- Test: unit tests in `src/highlighter.rs`

**Interfaces:**
- Consumes: `Language`, `SyntaxHighlighter` from Task 1.
- Produces:
  - `SyntaxHighlighter::layouter(&mut self, lang: Language, font_id: egui::FontId) -> HighlighterLayouter<'_>`
  - `pub struct HighlighterLayouter<'a>` with:
    - `pub fn highlight_line(&mut self, line: &str) -> egui::text::LayoutJob`
    - `pub fn default_format(&self) -> &egui::text::TextFormat`

- [ ] **Step 1: Write the failing tests**

Add `use eframe::egui;` as the first line inside the existing `mod tests` block in `src/highlighter.rs`, then append these tests:

```rust
    #[test]
    fn test_color_conversion_is_direct_passthrough() {
        let color = syntect::highlighting::Color {
            r: 10,
            g: 20,
            b: 30,
            a: 255,
        };
        assert_eq!(
            super::color_to_egui(color),
            egui::Color32::from_rgb(10, 20, 30)
        );
    }

    #[test]
    fn test_layouter_emits_multiple_sections_for_rust_keyword() {
        let mut highlighter = SyntaxHighlighter::new();
        let mut layouter = highlighter.layouter(Language::Rust, egui::FontId::monospace(14.0));
        let job = layouter.highlight_line("fn main() {}");
        job.debug_sanity_check();
        assert_eq!(job.text, "fn main() {}");
        assert!(
            job.sections.len() > 1,
            "expected multiple sections, got {}",
            job.sections.len()
        );
    }

    #[test]
    fn test_layouter_uses_supplied_font_id() {
        let mut highlighter = SyntaxHighlighter::new();
        let font_id = egui::FontId::monospace(18.0);
        let layouter = highlighter.layouter(Language::Rust, font_id.clone());
        assert_eq!(layouter.default_format().font_id, font_id);
    }

    #[test]
    fn test_layouter_plain_text_returns_single_unformatted_section() {
        let mut highlighter = SyntaxHighlighter::new();
        let mut layouter = highlighter.layouter(Language::PlainText, egui::FontId::monospace(14.0));
        let job = layouter.highlight_line("fn main() {}");
        job.debug_sanity_check();
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.text, "fn main() {}");
    }

    #[test]
    fn test_layouter_preserves_text_exactly_for_every_language() {
        let lines = ["let x = 1;", "\tif (a && b) {", "  // comment", "", "  "];
        for lang in Language::all() {
            let mut highlighter = SyntaxHighlighter::new();
            let mut layouter = highlighter.layouter(*lang, egui::FontId::monospace(14.0));
            for line in lines {
                let job = layouter.highlight_line(line);
                job.debug_sanity_check();
                assert_eq!(job.text, line, "text mangled for {lang:?}");
            }
        }
    }

    #[test]
    fn test_block_comment_keeps_comment_color_across_lines() {
        let comment = egui::Color32::from_rgb(150, 152, 150);
        let mut highlighter = SyntaxHighlighter::new();
        let mut layouter = highlighter.layouter(Language::C, egui::FontId::monospace(14.0));

        let first = layouter.highlight_line("/* start");
        let second = layouter.highlight_line("   still inside */");

        let runs_in_comment_color = |job: &egui::text::LayoutJob| {
            job.sections
                .iter()
                .filter(|section| section.format.color == comment)
                .map(|section| &job.text[section.byte_range.clone()])
                .collect::<String>()
        };

        // Both lines keep the comment scope, proving multi-line state survives
        // between calls. Without carried state the second line would be plain.
        assert_eq!(runs_in_comment_color(&first), "/* start");
        assert_eq!(runs_in_comment_color(&second), "   still inside */");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin rred_text_editor highlighter::tests`

Expected: FAIL to compile with `no method named 'layouter'`, `cannot find function 'color_to_egui'`, and `cannot find type 'HighlighterLayouter' in this scope`.

- [ ] **Step 3: Implement the layouter**

Add these imports at the top of `src/highlighter.rs`, directly after the existing `use syntect::parsing::...` line:

```rust
use eframe::egui;
```

Append this code to the end of `src/highlighter.rs`:

```rust
/// Converts a syntect theme color into the equivalent egui color.
///
/// Alpha is ignored — the editor background supplies contrast.
fn color_to_egui(color: syntect::highlighting::Color) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

/// Builds an [`egui::text::LayoutJob`] per line from syntect token colors.
///
/// Hand this to `egui::TextEdit::layouter` to get syntax highlighting that keeps
/// the caret, selection, scrolling, and undo behaviour of the stock `TextEdit`.
pub struct HighlighterLayouter<'a> {
    syntax_set: &'a SyntaxSet,
    default_foreground: Option<syntect::highlighting::Color>,
    default_format: egui::text::TextFormat,
    lines: HighlightLines<'a>,
}

impl<'a> HighlighterLayouter<'a> {
    /// Highlights one line and returns its layout job.
    ///
    /// Lines must be supplied in document order: state carried between calls is
    /// what keeps multi-line constructs colored. A line the grammar cannot parse
    /// falls back to unformatted text rather than failing.
    pub fn highlight_line(&mut self, line: &str) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob::default();

        match self.lines.highlight_line(line, self.syntax_set) {
            Ok(ranges) => {
                for (style, piece) in ranges {
                    if self.is_default_color(style.foreground) {
                        job.append(piece, 0.0, self.default_format.clone());
                    } else {
                        job.append(
                            piece,
                            0.0,
                            egui::text::TextFormat {
                                // egui::FontId is not Copy.
                                font_id: self.default_format.font_id.clone(),
                                color: color_to_egui(style.foreground),
                                ..self.default_format.clone()
                            },
                        );
                    }
                }
            }
            Err(_) => {
                job.append(line, 0.0, self.default_format.clone());
            }
        }

        job.wrap.max_width = f32::INFINITY;
        job
    }

    /// The format used for uncolored runs, so callers can append their own
    /// separators (newlines) without guessing the font.
    pub fn default_format(&self) -> &egui::text::TextFormat {
        &self.default_format
    }

    /// True when a token uses the theme's default foreground color.
    fn is_default_color(&self, color: syntect::highlighting::Color) -> bool {
        self.default_foreground
            .is_some_and(|fg| fg.r == color.r && fg.g == color.g && fg.b == color.b)
    }
}

impl SyntaxHighlighter {
    /// Builds a layouter for `lang`, using `font_id` for every text run.
    ///
    /// Pass the same `FontId` the editor resolves for its monospace style so the
    /// caret and selection overlay line up with the highlighted glyphs.
    pub fn layouter(
        &mut self,
        lang: Language,
        font_id: egui::FontId,
    ) -> HighlighterLayouter<'_> {
        let theme = self.theme();
        HighlighterLayouter {
            syntax_set: &self.syntax_set,
            default_foreground: theme.settings.foreground,
            default_format: egui::text::TextFormat {
                font_id,
                ..Default::default()
            },
            lines: self.start(lang),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin rred_text_editor highlighter::tests`

Expected: PASS — 14 tests, 0 failures. `debug_sanity_check()` passing on every assertion confirms each job's sections tile its text exactly.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -- -D warnings`

Expected: exit code 0.

- [ ] **Step 6: Commit**

```bash
git add src/highlighter.rs
git commit -m "feat(highlighter): add layouter and egui color conversion"
```

---

### Task 3: Track Detected and Overridden Language in App State

**Files:**
- Modify: `src/app.rs`
- Test: unit tests in `src/app.rs`

**Interfaces:**
- Consumes: `Language` from Task 1.
- Produces:
  - `TextEditorApp` fields `pub active_language: Language` and `pub language_override: Option<Language>`
  - `TextEditorApp::effective_language(&self) -> Language`
  - `TextEditorApp::show_language_menu(&mut self, ui: &mut egui::Ui)`
  - free function `fn text_content_exceeds_highlight_limit(content: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `mod tests` in `src/app.rs`:

```rust
    #[test]
    fn test_effective_language_prefers_override() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.active_language, crate::highlighter::Language::PlainText);
        assert_eq!(app.language_override, None);
        assert_eq!(
            app.effective_language(),
            crate::highlighter::Language::PlainText
        );

        app.active_language = crate::highlighter::Language::Rust;
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        app.language_override = Some(crate::highlighter::Language::Php);
        assert_eq!(app.effective_language(), crate::highlighter::Language::Php);

        app.language_override = None;
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);
    }

    #[test]
    fn test_load_file_updates_detected_language() {
        let dir = std::env::temp_dir().join("rred_app_lang_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.rs");
        std::fs::write(&path, "fn main() {}").unwrap();

        let mut app = TextEditorApp::default();
        app.load_file(path);
        assert_eq!(app.active_language, crate::highlighter::Language::Rust);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_file_keeps_explicit_override() {
        let dir = std::env::temp_dir().join("rred_app_lang_override_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("script.php");
        std::fs::write(&path, "<?php echo 1;").unwrap();

        let mut app = TextEditorApp::default();
        app.language_override = Some(crate::highlighter::Language::Rust);
        app.load_file(path);
        assert_eq!(app.active_language, crate::highlighter::Language::Php);
        assert_eq!(app.effective_language(), crate::highlighter::Language::Rust);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_highlight_size_limit() {
        assert!(!super::text_content_exceeds_highlight_limit("small"));
        assert!(!super::text_content_exceeds_highlight_limit(&"a".repeat(super::HIGHLIGHT_SIZE_LIMIT)));
        assert!(super::text_content_exceeds_highlight_limit(&"a".repeat(super::HIGHLIGHT_SIZE_LIMIT + 1)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin rred_text_editor app::tests`

Expected: FAIL to compile with `no field 'active_language' on type 'TextEditorApp'` and `cannot find function 'text_content_exceeds_highlight_limit'`.

- [ ] **Step 3: Add the import to `src/app.rs`**

Add this line next to the existing `use crate::file_browser;`:

```rust
use crate::highlighter::Language;
```

- [ ] **Step 4: Add the fields to the struct and `Default` impl**

Add to the `TextEditorApp` struct:

```rust
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub active_language: Language,
    pub language_override: Option<Language>,
```

Add to the `Default` impl:

```rust
            active_language: Language::PlainText,
            language_override: None,
```

- [ ] **Step 5: Add `effective_language` and `show_language_menu`**

Add this method to the `impl TextEditorApp` block, right after `window_title()`:

```rust
    /// The language actually used for highlighting: the manual override if set,
    /// otherwise the language auto-detected from the current file.
    pub fn effective_language(&self) -> Language {
        self.language_override.unwrap_or(self.active_language)
    }

    /// Draws the "Bahasa" dropdown used by both the File menu and the status bar.
    ///
    /// Re-selecting the already active language clears the override and returns
    /// to automatic detection.
    pub fn show_language_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Bahasa", |ui| {
            let current = self.effective_language();
            for lang in Language::all() {
                let selected = *lang == current;
                if ui.selectable_label(selected, lang.display_name()).clicked() {
                    self.language_override = if selected { None } else { Some(*lang) };
                    ui.close();
                }
            }
        });
    }
```

- [ ] **Step 6: Add the size-limit helper**

Add this after the `impl TextEditorApp` block:

```rust
/// Documents above this size skip syntax highlighting and render plain.
const HIGHLIGHT_SIZE_LIMIT: usize = 1024 * 1024;

/// True when a document is too large to highlight cheaply.
fn text_content_exceeds_highlight_limit(content: &str) -> bool {
    content.len() > HIGHLIGHT_SIZE_LIMIT
}
```

- [ ] **Step 7: Update `load_file()` to detect the language**

In the `Ok(content)` arm of `load_file()`, replace:

```rust
                self.current_path = Some(path);
                self.status_message = Some(("File berhasil dibuka.".to_string(), false));
```

with:

```rust
                self.active_language = Language::from_path(&path);
                self.current_path = Some(path);
                self.status_message = Some((
                    if text_content_exceeds_highlight_limit(&content) {
                        "File terlalu besar untuk syntax highlighting.".to_string()
                    } else {
                        "File berhasil dibuka.".to_string()
                    },
                    false,
                ));
```

- [ ] **Step 8: Update `save_as()` to detect the language**

In the successful write arm of `save_as()`, replace:

```rust
                    self.saved_content = self.text_content.clone();
                    self.current_path = Some(path);
```

with:

```rust
                    self.saved_content = self.text_content.clone();
                    self.active_language = Language::from_path(&path);
                    self.current_path = Some(path);
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test --bin rred_text_editor app::tests`

Expected: PASS — 14 tests, 0 failures.

- [ ] **Step 10: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): track detected and overridden language"
```

---

### Task 4: Render the Highlighted Editor and Expose the Selector

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `SyntaxHighlighter`, `HighlighterLayouter` from Tasks 1–2; `Language` state from Task 3.
- Produces:
  - `TextEditorApp` field `pub highlighter: SyntaxHighlighter`
  - free function `fn render_editor(ui: &mut egui::Ui, text: &mut String, layouter: Option<&mut HighlighterLayouter<'_>>)`

- [ ] **Step 1: Add the highlighter field**

Extend the import from Task 3:

```rust
use crate::highlighter::{HighlighterLayouter, Language, SyntaxHighlighter};
```

Add to the struct:

```rust
    pub language_override: Option<Language>,
    pub highlighter: SyntaxHighlighter,
```

Add to the `Default` impl:

```rust
            language_override: None,
            highlighter: SyntaxHighlighter::new(),
```

- [ ] **Step 2: Add the `render_editor` helper**

Add this after `text_content_exceeds_highlight_limit`:

```rust
/// Renders the multiline editor, optionally through a syntax highlighter.
///
/// When `layouter` is `None` the text renders plain, which is what oversized
/// documents get. Lines are joined back together with `\n` separators so the
/// buffer round-trips exactly, keeping the caret and selection aligned.
fn render_editor(ui: &mut egui::Ui, text: &mut String, mut layouter: Option<&mut HighlighterLayouter<'_>>) {
    let text_edit = egui::TextEdit::multiline(text)
        .font(egui::TextStyle::Monospace)
        .desired_width(f32::INFINITY);

    match layouter.as_deref_mut() {
        Some(highlighter) => {
            let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
                let mut job = egui::text::LayoutJob::default();
                for (index, line) in buf.as_str().split('\n').enumerate() {
                    if index > 0 {
                        job.append("\n", 0.0, highlighter.default_format().clone());
                    }
                    let line_job = highlighter.highlight_line(line);
                    let offset = job.text.len();
                    job.text.push_str(&line_job.text);
                    for mut section in line_job.sections {
                        section.byte_range.start += offset;
                        section.byte_range.end += offset;
                        job.sections.push(section);
                    }
                }
                job.wrap.max_width = f32::INFINITY;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            ui.add_sized(ui.available_size(), text_edit.layouter(&mut lay));
        }
        None => {
            ui.add_sized(ui.available_size(), text_edit);
        }
    }
}
```

- [ ] **Step 3: Replace the central panel**

Replace the entire central panel block:

```rust
        // 4. Central Panel (Main Text Editor Area)
        egui::CentralPanel::default().show(ui, |ui| {
            let text_style = egui::TextStyle::Monospace;
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.text_content)
                        .font(text_style)
                        .lock_focus(true)
                        .desired_width(f32::INFINITY),
                );
            });
        });
```

with:

```rust
        // 4. Central Panel (Main Text Editor Area)
        egui::CentralPanel::default().show(ui, |ui| {
            let font_id = ui.fonts(|f| f.resolve_text_style(egui::TextStyle::Monospace));
            let too_large = text_content_exceeds_highlight_limit(&self.text_content);

            // Split the borrows so the text and the highlighter can be handed to
            // `render_editor` together.
            let Self {
                text_content,
                highlighter,
                active_language,
                language_override,
                ..
            } = self;
            let language = language_override.unwrap_or(*active_language);

            if too_large {
                render_editor(ui, text_content, None);
            } else {
                let mut layouter = highlighter.layouter(language, font_id);
                render_editor(ui, text_content, Some(&mut layouter));
            }
        });
```

- [ ] **Step 4: Add the language selector to the File menu**

In the `ui.menu_button("File", ...)` closure, replace:

```rust
                    ui.separator();
                    if ui.button("Keluar").clicked() {
```

with:

```rust
                    ui.separator();
                    self.show_language_menu(ui);
                    ui.separator();
                    if ui.button("Keluar").clicked() {
```

- [ ] **Step 5: Add the language indicator to the status bar**

In the `egui::Panel::bottom("status_bar")` closure, replace:

```rust
                ui.label(format!("Dokumen: {doc_label}{dirty_str}"));
```

with:

```rust
                ui.label(format!("Dokumen: {doc_label}{dirty_str}"));
                ui.separator();
                self.show_language_menu(ui);
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`

Expected: compiles cleanly. Note that `render_editor` no longer wraps the editor in a `ScrollArea` — `TextEdit` scrolls itself when given `desired_width(f32::INFINITY)`, so the outer scroll area was redundant.

- [ ] **Step 7: Run the full test suite**

Run: `cargo test`

Expected: PASS — 14 tests, 0 failures.

- [ ] **Step 8: Run clippy**

Run: `cargo clippy -- -D warnings`

Expected: exit code 0.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): render editor with syntax highlighting and language selector"
```

---

### Task 5: End-to-End Verification

**Files:**
- None modified.

- [ ] **Step 1: Run the complete test suite**

Run: `cargo test`

Expected: PASS — all 14 tests across `app`, `file_browser`, `highlighter`, and `modals`, 0 failures.

- [ ] **Step 2: Run clippy with warnings denied on all targets**

Run: `cargo clippy --all-targets -- -D warnings`

Expected: exit code 0, no output.

- [ ] **Step 3: Run a release build**

Run: `cargo build --release`

Expected: exit code 0, clean build.

- [ ] **Step 4: Confirm no C regex library is linked**

Run: `cargo tree -i onig 2>&1 | head -5`

Expected: an error saying the package `onig` was not found in the dependency tree. This confirms the pure-Rust `regex-fancy` backend is in use. If `onig` appears, the `default-features = false` constraint was not applied correctly in `Cargo.toml`.