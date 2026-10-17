//! Syntax highlighting for the central editor.
//!
//! Owns language detection and the `syntect`-backed highlighter. `app.rs` never
//! touches `syntect` types directly — it holds a `Language` value and passes it here.

use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use eframe::egui;

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
    // Only the tests need this accessor; the layouter borrows the set directly.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests inspect the raw grammar set"))]
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

#[cfg(test)]
mod tests {
    use eframe::egui;
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
                // `byte_range` is a range of `ByteIndex`, a newtype over `usize`
                // that `str` cannot slice with, so unwrap it to plain bytes.
                .map(|section| &job.text[section.byte_range.start.0..section.byte_range.end.0])
                .collect::<String>()
        };

        // Both lines keep the comment scope, proving multi-line state survives
        // between calls. Without carried state the second line would be plain.
        assert_eq!(runs_in_comment_color(&first), "/* start");
        assert_eq!(runs_in_comment_color(&second), "   still inside */");
    }
}