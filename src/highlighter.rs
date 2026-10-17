//! Syntax highlighting for the central editor.
//!
//! Owns language detection and the `syntect`-backed highlighter. `app.rs` never
//! touches `syntect` types directly — it holds a `Language` value and passes it here.

// The public API below is completed incrementally by the following tasks in this
// feature; the parts not wired into `main.rs` yet would otherwise fail the
// `dead_code` lint under `cargo clippy -- -D warnings`.
#![allow(dead_code)]

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