//! Syntax highlighting for the central editor.
//!
//! Owns language detection and the `syntect`-backed highlighter. `app.rs` never
//! touches `syntect` types directly — it holds a `Language` value and passes it here.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use syntect::easy::HighlightLines;
use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
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
    JavaScript,
    /// Shares the `JavaScript` grammar: syntect bundles no TypeScript grammar.
    TypeScript,
    Html,
    Json,
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
            Language::JavaScript,
            Language::TypeScript,
            Language::Html,
            Language::Json,
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
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
            "html" | "htm" => Language::Html,
            "json" | "jsonc" => Language::Json,
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
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Html => "HTML",
            Language::Json => "JSON",
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
            Language::JavaScript | Language::TypeScript => "JavaScript",
            Language::Html => "HTML",
            Language::Json => "JSON",
        }
    }
}

/// Holds syntect's grammars and themes for the lifetime of the application.
///
/// Construct once and reuse: loading the bundled sets is expensive.
pub struct SyntaxHighlighter {
    // `Arc` so a background pass can share the grammars without copying them.
    syntax_set: Arc<SyntaxSet>,
    theme_set: Arc<ThemeSet>,
    /// The most recently highlighted document. `TextEdit` calls its layouter
    /// every frame, and highlighting a whole document is far too slow for that,
    /// so the result is reused until the text, language or font changes.
    cache: Option<DocumentCache>,
    /// A background pass for a document too large to highlight inside a frame.
    pending: Option<PendingHighlight>,
}

/// Documents up to this size are highlighted synchronously inside the frame.
/// Larger ones are highlighted on a background thread and shown as plain text
/// until the result arrives, so the window never freezes.
const SYNC_HIGHLIGHT_LIMIT: usize = 8 * 1024;

/// One highlighted line and the parser state *after* it.
///
/// The end state is the start state of the next line, so a later edit can
/// resume highlighting from any preserved line instead of the document start.
#[derive(Clone)]
pub struct LineHighlightCache {
    pub text: String,
    pub layout: egui::text::LayoutJob,
    pub parse_state: ParseState,
    pub highlight_state: HighlightState,
}

/// Per-document line cache used by [`SyntaxHighlighter::highlight_incremental`].
///
/// Keep one per open document: it is only valid for the language and font it
/// was built with, and is rebuilt from scratch when either changes.
#[derive(Default, Clone)]
pub struct DocumentHighlightCache {
    pub language: Option<Language>,
    pub font_id: Option<egui::FontId>,
    pub lines: Vec<LineHighlightCache>,
    pub full_job: Option<Arc<egui::text::LayoutJob>>,
}

/// A cache never takes part in comparing the tab that owns it.
impl PartialEq for DocumentHighlightCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for DocumentHighlightCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentHighlightCache")
            .field("language", &self.language)
            .field("lines", &self.lines.len())
            .finish()
    }
}

/// A fully highlighted document together with the inputs that produced it.
struct DocumentCache {
    language: Language,
    font_id: egui::FontId,
    text: String,
    job: Arc<egui::text::LayoutJob>,
}

/// A highlighting pass running on a background thread.
struct PendingHighlight {
    language: Language,
    font_id: egui::FontId,
    text: String,
    /// Plain-text stand-in shown until `result` arrives.
    placeholder: Arc<egui::text::LayoutJob>,
    result: mpsc::Receiver<egui::text::LayoutJob>,
    cancel: Arc<AtomicBool>,
}

impl Drop for PendingHighlight {
    /// A pass that is replaced (the text changed) or dropped stops early
    /// instead of burning CPU on a result nobody will read.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The configured theme, falling back to the first bundled one.
fn pick_theme(theme_set: &ThemeSet) -> &Theme {
    theme_set
        .themes
        .get(THEME_NAME)
        .or_else(|| theme_set.themes.values().next())
        .expect("syntect always bundles at least one theme")
}

/// The grammar for `lang`, falling back to plain text if it is missing.
fn pick_syntax(syntax_set: &SyntaxSet, lang: Language) -> &SyntaxReference {
    syntax_set
        .find_syntax_by_name(lang.syntax_name())
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

impl SyntaxHighlighter {
    /// Loads syntect's bundled grammars and themes. Infallible.
    pub fn new() -> Self {
        Self {
            // The layouter feeds this set lines split on '\n', so it needs the
            // newline-terminated grammar variant.
            syntax_set: Arc::new(SyntaxSet::load_defaults_newlines()),
            theme_set: Arc::new(ThemeSet::load_defaults()),
            cache: None,
            pending: None,
        }
    }

    /// The bundled grammar set.
    // Only the tests inspect the raw grammar set; the layouter borrows the
    // field directly, so this accessor is not part of the runtime surface.
    #[cfg(test)]
    pub fn syntax_set(&self) -> &SyntaxSet {
        &self.syntax_set
    }

    /// The active theme, falling back to the first bundled theme if the
    /// configured one is ever missing.
    // Only the tests read the theme directly; `layouter` picks it itself.
    #[cfg(test)]
    pub fn theme(&self) -> &Theme {
        pick_theme(&self.theme_set)
    }
}

impl SyntaxHighlighter {
    /// Highlights `text`, re-parsing only the lines an edit touched.
    ///
    /// `cache` holds the previous result with the parser state after every
    /// line. Unchanged leading lines are kept as they are. Re-highlighting
    /// resumes from the last preserved line and stops as soon as the parser
    /// state after a re-highlighted line matches the cached one for the same
    /// unchanged trailing line, since everything after it would come out
    /// identical. A change of language or font rebuilds the whole cache.
    // The runtime path goes through `HighlighterLayouter`, which borrows the
    // grammars directly; only the tests call this method.
    #[cfg(test)]
    pub fn highlight_incremental(
        &self,
        cache: &mut DocumentHighlightCache,
        language: Language,
        font_id: &egui::FontId,
        text: &str,
    ) -> Arc<egui::text::LayoutJob> {
        highlight_incremental_with(&self.syntax_set, &self.theme_set, cache, language, font_id, text)
    }
}

/// [`SyntaxHighlighter::highlight_incremental`] over borrowed grammars, so the
/// layouter can call it without holding the whole highlighter.
fn highlight_incremental_with(
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
    cache: &mut DocumentHighlightCache,
    language: Language,
    font_id: &egui::FontId,
    text: &str,
) -> Arc<egui::text::LayoutJob> {
    {
        if cache.language != Some(language) || cache.font_id.as_ref() != Some(font_id) {
            cache.lines.clear();
            cache.full_job = None;
            cache.language = Some(language);
            cache.font_id = Some(font_id.clone());
        }
        if let Some(job) = &cache.full_job
            && job.text == text
        {
            return Arc::clone(job);
        }

        let theme = pick_theme(theme_set);
        let syntax = pick_syntax(syntax_set, language);
        let highlighter = Highlighter::new(theme);
        let default_foreground = theme.settings.foreground;
        let default_format = egui::text::TextFormat {
            font_id: font_id.clone(),
            ..Default::default()
        };
        let initial = (
            ParseState::new(syntax),
            HighlightState::new(&highlighter, ScopeStack::new()),
        );

        let new_lines: Vec<&str> = text.split('\n').collect();
        let old = &cache.lines;
        let max_common = new_lines.len().min(old.len());
        let prefix = (0..max_common)
            .take_while(|&i| old[i].text == new_lines[i])
            .count();
        let suffix = (0..max_common - prefix)
            .take_while(|&k| old[old.len() - 1 - k].text == new_lines[new_lines.len() - 1 - k])
            .count();
        let suffix_start_new = new_lines.len() - suffix;
        let suffix_start_old = old.len() - suffix;

        // The state before old line `index`, i.e. after the line preceding it.
        let state_before = |index: usize| match index.checked_sub(1) {
            Some(prev) => (old[prev].parse_state.clone(), old[prev].highlight_state.clone()),
            None => initial.clone(),
        };

        let (mut parse_state, mut highlight_state) = state_before(prefix);
        let mut rebuilt = Vec::new();
        let mut converged_at = None;
        for (j, line) in new_lines.iter().enumerate().skip(prefix) {
            if j >= suffix_start_new {
                let o = suffix_start_old + (j - suffix_start_new);
                let (old_parse, old_highlight) = state_before(o);
                if parse_state == old_parse && highlight_state == old_highlight {
                    converged_at = Some(o);
                    break;
                }
            }
            let layout = match parse_state.parse_line(line, syntax_set) {
                Ok(ops) => ranges_to_job(
                    HighlightIterator::new(&mut highlight_state, &ops, line, &highlighter),
                    default_foreground,
                    &default_format,
                ),
                Err(_) => plain_line_job(line, &default_format),
            };
            rebuilt.push(LineHighlightCache {
                text: (*line).to_owned(),
                layout,
                parse_state: parse_state.clone(),
                highlight_state: highlight_state.clone(),
            });
        }

        let tail = match converged_at {
            Some(o) => cache.lines.split_off(o),
            None => Vec::new(),
        };
        cache.lines.truncate(prefix);
        cache.lines.extend(rebuilt);
        cache.lines.extend(tail);

        let mut job = egui::text::LayoutJob::default();
        for (index, line) in cache.lines.iter().enumerate() {
            if index > 0 {
                job.append("\n", 0.0, default_format.clone());
            }
            let offset = job.text.len();
            job.text.push_str(&line.layout.text);
            for section in &line.layout.sections {
                let mut section = section.clone();
                section.byte_range.start += offset;
                section.byte_range.end += offset;
                job.sections.push(section);
            }
        }
        job.wrap.max_width = f32::INFINITY;

        let job = Arc::new(job);
        cache.full_job = Some(Arc::clone(&job));
        job
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
    syntax_set: &'a Arc<SyntaxSet>,
    theme_set: &'a Arc<ThemeSet>,
    theme: &'a Theme,
    syntax: &'a SyntaxReference,
    language: Language,
    default_foreground: Option<syntect::highlighting::Color>,
    default_format: egui::text::TextFormat,
    lines: HighlightLines<'a>,
    cache: &'a mut Option<DocumentCache>,
    pending: &'a mut Option<PendingHighlight>,
    /// The open document's line cache. When set, small documents are
    /// highlighted incrementally instead of from scratch after every edit.
    document: Option<&'a mut DocumentHighlightCache>,
}

/// Builds a layout job from highlighted `(style, piece)` runs, leaving runs in
/// the theme's default color unformatted.
fn ranges_to_job<'s>(
    ranges: impl IntoIterator<Item = (syntect::highlighting::Style, &'s str)>,
    default_foreground: Option<syntect::highlighting::Color>,
    default_format: &egui::text::TextFormat,
) -> egui::text::LayoutJob {
    let is_default = |color: syntect::highlighting::Color| {
        default_foreground.is_some_and(|fg| fg.r == color.r && fg.g == color.g && fg.b == color.b)
    };
    let mut job = egui::text::LayoutJob::default();
    for (style, piece) in ranges {
        if is_default(style.foreground) {
            job.append(piece, 0.0, default_format.clone());
        } else {
            job.append(
                piece,
                0.0,
                egui::text::TextFormat {
                    // egui::FontId is not Copy.
                    font_id: default_format.font_id.clone(),
                    color: color_to_egui(style.foreground),
                    ..default_format.clone()
                },
            );
        }
    }
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Highlights one line with an already-running parser state.
///
/// A line the grammar cannot parse falls back to unformatted text rather than
/// failing.
fn highlight_line_with(
    lines: &mut HighlightLines<'_>,
    syntax_set: &SyntaxSet,
    default_foreground: Option<syntect::highlighting::Color>,
    default_format: &egui::text::TextFormat,
    line: &str,
) -> egui::text::LayoutJob {
    match lines.highlight_line(line, syntax_set) {
        Ok(ranges) => ranges_to_job(ranges, default_foreground, default_format),
        Err(_) => plain_line_job(line, default_format),
    }
}

/// An unformatted single-line job.
fn plain_line_job(line: &str, default_format: &egui::text::TextFormat) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(line, 0.0, default_format.clone());
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Joins per-line jobs into one document job whose text matches `text` exactly
/// (lines separated by `\n`), keeping the caret and selection aligned.
///
/// Returns `None` if `cancel` is raised part-way through.
fn join_lines(
    text: &str,
    default_format: &egui::text::TextFormat,
    cancel: &AtomicBool,
    mut line_job: impl FnMut(&str) -> egui::text::LayoutJob,
) -> Option<egui::text::LayoutJob> {
    let mut job = egui::text::LayoutJob::default();
    for (index, line) in text.split('\n').enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if index > 0 {
            job.append("\n", 0.0, default_format.clone());
        }
        let line_job = line_job(line);
        let offset = job.text.len();
        job.text.push_str(&line_job.text);
        for mut section in line_job.sections {
            section.byte_range.start += offset;
            section.byte_range.end += offset;
            job.sections.push(section);
        }
    }
    job.wrap.max_width = f32::INFINITY;
    Some(job)
}

impl<'a> HighlighterLayouter<'a> {
    /// Highlights small documents incrementally through `cache`, the line
    /// cache of the document being edited.
    pub fn with_document_cache(mut self, cache: &'a mut DocumentHighlightCache) -> Self {
        self.document = Some(cache);
        self
    }

    /// The layout for one frame of the editor. Never blocks for long.
    ///
    /// Small documents are highlighted inline. Larger ones are highlighted on a
    /// background thread: until it finishes this returns the document as plain
    /// text (same bytes, so the caret stays valid) and asks `ctx` to repaint
    /// when the colored result is ready. An edit abandons the stale pass.
    pub fn frame_job(&mut self, text: &str, ctx: &egui::Context) -> Arc<egui::text::LayoutJob> {
        if text.len() <= SYNC_HIGHLIGHT_LIMIT {
            *self.pending = None;
            if let Some(document) = self.document.as_deref_mut() {
                return highlight_incremental_with(
                    self.syntax_set,
                    self.theme_set,
                    document,
                    self.language,
                    &self.default_format.font_id,
                    text,
                );
            }
            return self.document_job(text);
        }
        if let Some(job) = self.cached(text) {
            return job;
        }

        if let Some(pending) = self.pending.as_ref()
            && pending.language == self.language
            && pending.font_id == self.default_format.font_id
            && pending.text == text
        {
            match pending.result.try_recv() {
                Ok(job) => {
                    let job = Arc::new(job);
                    self.store(text, &job);
                    *self.pending = None;
                    return job;
                }
                Err(mpsc::TryRecvError::Empty) => return Arc::clone(&pending.placeholder),
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The worker died without a result. Keep the plain text
                    // rather than respawning it every frame.
                    let placeholder = Arc::clone(&pending.placeholder);
                    self.store(text, &placeholder);
                    *self.pending = None;
                    return placeholder;
                }
            }
        }

        // Nothing usable in flight: replace (and thereby cancel) any old pass.
        let mut plain = egui::text::LayoutJob::single_section(
            text.to_owned(),
            self.default_format.clone(),
        );
        plain.wrap.max_width = f32::INFINITY;
        let placeholder = Arc::new(plain);

        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let worker = Worker {
            syntax_set: Arc::clone(self.syntax_set),
            theme_set: Arc::clone(self.theme_set),
            language: self.language,
            default_format: self.default_format.clone(),
            text: text.to_owned(),
            cancel: Arc::clone(&cancel),
            ctx: ctx.clone(),
        };
        std::thread::spawn(move || {
            if let Some(job) = worker.run() {
                // The receiver is gone if the pass was superseded.
                let _ = sender.send(job);
                worker.ctx.request_repaint();
            }
        });

        *self.pending = Some(PendingHighlight {
            language: self.language,
            font_id: self.default_format.font_id.clone(),
            text: text.to_owned(),
            placeholder: Arc::clone(&placeholder),
            result: receiver,
            cancel,
        });
        placeholder
    }

    /// The cached job, if it was built from exactly this text, language and font.
    fn cached(&self, text: &str) -> Option<Arc<egui::text::LayoutJob>> {
        self.cache
            .as_ref()
            .filter(|cached| {
                cached.language == self.language
                    && cached.font_id == self.default_format.font_id
                    && cached.text == text
            })
            .map(|cached| Arc::clone(&cached.job))
    }

    fn store(&mut self, text: &str, job: &Arc<egui::text::LayoutJob>) {
        *self.cache = Some(DocumentCache {
            language: self.language,
            font_id: self.default_format.font_id.clone(),
            text: text.to_owned(),
            job: Arc::clone(job),
        });
    }

    /// Highlights a whole document synchronously into one layout job, reusing
    /// the previous result when the text, language and font are unchanged.
    ///
    /// `TextEdit` asks for a layout every frame; without this cache each frame
    /// re-ran the grammar over the entire document and froze the UI. Every
    /// rebuild starts from a fresh parser state, so nothing leaks from an
    /// earlier pass.
    pub fn document_job(&mut self, text: &str) -> Arc<egui::text::LayoutJob> {
        if let Some(job) = self.cached(text) {
            return job;
        }

        self.lines = HighlightLines::new(self.syntax, self.theme);
        let never = AtomicBool::new(false);
        let default_format = self.default_format.clone();
        let job = join_lines(text, &default_format, &never, |line| self.highlight_line(line))
            .expect("a pass that is never cancelled always finishes");

        let job = Arc::new(job);
        self.store(text, &job);
        job
    }

    /// Highlights one line and returns its layout job.
    ///
    /// Lines must be supplied in document order: state carried between calls is
    /// what keeps multi-line constructs colored.
    pub fn highlight_line(&mut self, line: &str) -> egui::text::LayoutJob {
        highlight_line_with(
            &mut self.lines,
            self.syntax_set,
            self.default_foreground,
            &self.default_format,
            line,
        )
    }

    /// The format used for uncolored runs.
    // Only the tests inspect it; the document builders use their own copy.
    #[cfg(test)]
    pub fn default_format(&self) -> &egui::text::TextFormat {
        &self.default_format
    }
}

/// Everything a background pass needs, owned so it can outlive the frame.
struct Worker {
    syntax_set: Arc<SyntaxSet>,
    theme_set: Arc<ThemeSet>,
    language: Language,
    default_format: egui::text::TextFormat,
    text: String,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
}

impl Worker {
    /// Highlights the whole document, or returns `None` if it was cancelled.
    fn run(&self) -> Option<egui::text::LayoutJob> {
        let theme = pick_theme(&self.theme_set);
        let syntax = pick_syntax(&self.syntax_set, self.language);
        let default_foreground = theme.settings.foreground;
        let mut lines = HighlightLines::new(syntax, theme);
        join_lines(&self.text, &self.default_format, &self.cancel, |line| {
            highlight_line_with(
                &mut lines,
                &self.syntax_set,
                default_foreground,
                &self.default_format,
                line,
            )
        })
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
        // Borrow the fields separately: the cache is mutable, the grammar and
        // theme are shared with the layouter.
        let theme = pick_theme(&self.theme_set);
        let syntax = pick_syntax(&self.syntax_set, lang);
        HighlighterLayouter {
            syntax_set: &self.syntax_set,
            theme_set: &self.theme_set,
            theme,
            syntax,
            language: lang,
            default_foreground: theme.settings.foreground,
            default_format: egui::text::TextFormat {
                font_id,
                ..Default::default()
            },
            lines: HighlightLines::new(syntax, theme),
            cache: &mut self.cache,
            pending: &mut self.pending,
            document: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use super::*;
    use std::path::Path;

    #[test]
    fn test_incremental_highlight_preserves_unmodified_lines() {
        let highlighter = SyntaxHighlighter::new();
        let mut cache = DocumentHighlightCache::default();
        let font_id = egui::FontId::monospace(14.0);
        let doc1 = "fn foo() {}\nfn bar() {}\nfn baz() {}";
        highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc1);
        assert_eq!(cache.lines.len(), 3);

        let doc2 = "fn foo() {}\nfn bar_modified() {}\nfn baz() {}";
        let job2 = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc2);
        assert_eq!(job2.text, doc2);
        assert_eq!(cache.lines[0].text, "fn foo() {}");
        assert_eq!(cache.lines[1].text, "fn bar_modified() {}");
        assert_eq!(cache.lines[2].text, "fn baz() {}");
    }

    #[test]
    fn test_incremental_highlight_propagates_multiline_comment() {
        let highlighter = SyntaxHighlighter::new();
        let mut cache = DocumentHighlightCache::default();
        let font_id = egui::FontId::monospace(14.0);
        let doc1 = "let a = 1;\nlet b = 2;\nlet c = 3;";
        highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc1);

        let doc2 = "/* let a = 1;\nlet b = 2;\nlet c = 3; */";
        let job2 = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc2);
        assert_eq!(job2.text, doc2);
        assert_matches_full(&job2, doc2);
    }

    /// The incremental result must equal a from-scratch highlight.
    fn assert_matches_full(job: &egui::text::LayoutJob, text: &str) {
        let mut fresh = SyntaxHighlighter::new();
        let font_id = egui::FontId::monospace(14.0);
        let full = fresh.layouter(Language::Rust, font_id).document_job(text);
        assert_eq!(job.text, full.text);
        assert_eq!(job.sections.len(), full.sections.len(), "section count differs");
        for (a, b) in job.sections.iter().zip(full.sections.iter()) {
            assert_eq!(a.byte_range, b.byte_range);
            assert_eq!(a.format.color, b.format.color);
        }
    }

    #[test]
    fn test_incremental_highlight_matches_full_across_edits() {
        let highlighter = SyntaxHighlighter::new();
        let mut cache = DocumentHighlightCache::default();
        let font_id = egui::FontId::monospace(14.0);
        let edits = [
            "fn a() {}\nlet x = 1;\nlet y = \"s\";\n// end",
            "fn a() {}\nlet x = 1;\nlet y = \"s\";\n// end\nfn z() {}",
            "fn a() {}\n/* open\nlet x = 1;\nlet y = \"s\";\n// end\nfn z() {}",
            "fn a() {}\n/* open */\nlet x = 1;\nlet y = \"s\";\n// end\nfn z() {}",
            "let x = 1;\nlet y = \"s\";\n// end\nfn z() {}",
            "",
            "fn only() {}",
        ];
        for doc in edits {
            let job = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc);
            assert_matches_full(&job, doc);
            assert_eq!(cache.lines.len(), doc.split('\n').count());
        }
    }

    #[test]
    fn test_incremental_highlight_converges_without_touching_tail() {
        let highlighter = SyntaxHighlighter::new();
        let mut cache = DocumentHighlightCache::default();
        let font_id = egui::FontId::monospace(14.0);
        let doc1 = "let a = 1;\nlet b = 2;\nlet c = 3;";
        highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc1);
        // Tag the tail line's cached layout; a convergent edit must carry it over.
        cache.lines[2].layout.wrap.max_rows = 7;

        let doc2 = "let a = 1;\nlet bb = 22;\nlet c = 3;";
        highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc2);
        assert_eq!(cache.lines[2].layout.wrap.max_rows, 7, "tail line was re-highlighted");
    }

    #[test]
    fn test_incremental_highlight_rebuilds_on_language_or_font_change() {
        let highlighter = SyntaxHighlighter::new();
        let mut cache = DocumentHighlightCache::default();
        let font_id = egui::FontId::monospace(14.0);
        let doc = "fn a() {}";
        let rust = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc);
        let plain = highlighter.highlight_incremental(&mut cache, Language::PlainText, &font_id, doc);
        assert!(rust.sections.len() > plain.sections.len());
        let bigger = egui::FontId::monospace(20.0);
        let job = highlighter.highlight_incremental(&mut cache, Language::PlainText, &bigger, doc);
        assert_eq!(job.sections[0].format.font_id, bigger);
    }

    #[test]
    fn test_line_highlight_cache_step() {
        let highlighter = SyntaxHighlighter::new();
        let syntax = pick_syntax(&highlighter.syntax_set, Language::Rust);
        let theme = pick_theme(&highlighter.theme_set);
        let mut parse_state = ParseState::new(syntax);
        let mut highlight_state =
            HighlightState::new(&Highlighter::new(theme), ScopeStack::new());
        let start_parse = parse_state.clone();
        let start_highlight = highlight_state.clone();
        assert!(start_parse == parse_state && start_highlight == highlight_state);

        let ops = parse_state
            .parse_line("fn main() {\n", &highlighter.syntax_set)
            .unwrap();
        assert!(!ops.is_empty());
        let hl = Highlighter::new(theme);
        let _ = HighlightIterator::new(&mut highlight_state, &ops, "fn main() {\n", &hl).count();
        // An open block changes both states, so they no longer equal the start.
        assert!(start_parse != parse_state);
        assert!(start_highlight != highlight_state);
    }

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
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("mjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("cjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("jsx"), Language::JavaScript);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("mts"), Language::TypeScript);
        assert_eq!(Language::from_extension("cts"), Language::TypeScript);
        assert_eq!(Language::from_extension("tsx"), Language::TypeScript);
        assert_eq!(Language::from_extension("html"), Language::Html);
        assert_eq!(Language::from_extension("htm"), Language::Html);
        assert_eq!(Language::from_extension("json"), Language::Json);
        assert_eq!(Language::from_extension("jsonc"), Language::Json);
    }

    #[test]
    fn test_from_extension_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Language::Rust);
        assert_eq!(Language::from_extension("PHP"), Language::Php);
        assert_eq!(Language::from_extension("Cpp"), Language::Cpp);
        assert_eq!(Language::from_extension("JS"), Language::JavaScript);
        assert_eq!(Language::from_extension("Ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("HTML"), Language::Html);
        assert_eq!(Language::from_extension("JSON"), Language::Json);
        assert_eq!(Language::from_extension("JSONC"), Language::Json);
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
        assert_eq!(
            Language::from_path(Path::new("/srv/app.test.ts")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("/srv/site/index.html")),
            Language::Html
        );
    }

    #[test]
    fn test_display_name_labels_each_language() {
        assert_eq!(Language::PlainText.display_name(), "Teks Biasa");
        assert_eq!(Language::Rust.display_name(), "Rust");
        assert_eq!(Language::C.display_name(), "C");
        assert_eq!(Language::Cpp.display_name(), "C++");
        assert_eq!(Language::Php.display_name(), "PHP");
        assert_eq!(Language::JavaScript.display_name(), "JavaScript");
        assert_eq!(Language::TypeScript.display_name(), "TypeScript");
        assert_eq!(Language::Html.display_name(), "HTML");
        assert_eq!(Language::Json.display_name(), "JSON");
    }

    #[test]
    fn test_all_lists_every_variant() {
        assert_eq!(Language::all().len(), 9);
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

    /// Regression: the editor re-ran syntect over the whole document on every
    /// frame, freezing the UI for seconds on mid-sized files. An unchanged
    /// document must reuse the previously built job.
    #[test]
    fn test_document_job_is_cached_across_frames() {
        let mut highlighter = SyntaxHighlighter::new();
        let font = egui::FontId::monospace(14.0);
        let text = "fn main() {\n    let x = 1;\n}\n";

        let first = highlighter
            .layouter(Language::Rust, font.clone())
            .document_job(text);
        // A new layouter per frame, as `render_editor` builds one each frame.
        let second = highlighter
            .layouter(Language::Rust, font.clone())
            .document_job(text);

        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "unchanged text/language/font must not be re-highlighted"
        );
    }

    #[test]
    fn test_document_job_rebuilds_when_input_changes() {
        let mut highlighter = SyntaxHighlighter::new();
        let font = egui::FontId::monospace(14.0);
        let base = highlighter
            .layouter(Language::Rust, font.clone())
            .document_job("let a = 1;");

        let edited = highlighter
            .layouter(Language::Rust, font.clone())
            .document_job("let a = 2;");
        assert!(!std::sync::Arc::ptr_eq(&base, &edited));
        assert_eq!(edited.text, "let a = 2;");

        let other_language = highlighter
            .layouter(Language::Php, font.clone())
            .document_job("let a = 2;");
        assert!(!std::sync::Arc::ptr_eq(&edited, &other_language));

        let other_font = highlighter
            .layouter(Language::Php, egui::FontId::monospace(20.0))
            .document_job("let a = 2;");
        assert!(!std::sync::Arc::ptr_eq(&other_language, &other_font));
    }

    /// Lines must be highlighted from a fresh grammar state each rebuild, and the
    /// joined job must reproduce the buffer byte-for-byte.
    #[test]
    fn test_document_job_text_matches_buffer_and_keeps_comment_state() {
        let comment = egui::Color32::from_rgb(150, 152, 150);
        let mut highlighter = SyntaxHighlighter::new();
        let text = "/* start\n   still inside */\nint x;";
        let mut layouter = highlighter.layouter(Language::C, egui::FontId::monospace(14.0));

        let job = layouter.document_job(text);
        job.debug_sanity_check();
        assert_eq!(job.text, text);

        let commented: String = job
            .sections
            .iter()
            .filter(|s| s.format.color == comment)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(commented, "/* start   still inside */");

        // Rebuilding on the same layouter must not inherit the previous pass's
        // parser state (the comment is closed by now).
        let again = layouter.document_job("int y;");
        let leaked = again.sections.iter().any(|s| s.format.color == comment);
        assert!(!leaked, "grammar state leaked between rebuilds");
    }

    /// Polls `frame_job` the way the editor does (once per frame) until the
    /// highlighted job arrives or the deadline passes.
    fn wait_for_highlighted(
        highlighter: &mut SyntaxHighlighter,
        ctx: &egui::Context,
        text: &str,
    ) -> Arc<egui::text::LayoutJob> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let job = highlighter
                .layouter(Language::Rust, egui::FontId::monospace(14.0))
                .frame_job(text, ctx);
            if job.sections.len() > 1 {
                return job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background highlighting never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn big_rust_document() -> String {
        "    let value = compute(a, b) + \"string\"; // trailing comment\n"
            .repeat(SYNC_HIGHLIGHT_LIMIT / 20)
    }

    /// Regression: a large document used to be highlighted synchronously inside
    /// the layouter, freezing the window. Above `SYNC_HIGHLIGHT_LIMIT` the first
    /// call must return immediately with the plain text and finish in the
    /// background.
    #[test]
    fn test_large_document_highlights_in_background_without_blocking() {
        let text = big_rust_document();
        assert!(text.len() > SYNC_HIGHLIGHT_LIMIT);
        let ctx = egui::Context::default();
        let mut highlighter = SyntaxHighlighter::new();

        let started = std::time::Instant::now();
        let first = highlighter
            .layouter(Language::Rust, egui::FontId::monospace(14.0))
            .frame_job(&text, &ctx);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "first frame blocked for {:?}",
            started.elapsed()
        );
        assert_eq!(first.text, text, "placeholder must match the buffer");
        assert_eq!(first.sections.len(), 1, "placeholder is plain text");

        let done = wait_for_highlighted(&mut highlighter, &ctx, &text);
        done.debug_sanity_check();
        assert_eq!(done.text, text, "highlighted job must match the buffer");

        let again = highlighter
            .layouter(Language::Rust, egui::FontId::monospace(14.0))
            .frame_job(&text, &ctx);
        assert!(Arc::ptr_eq(&done, &again), "finished job must be cached");
    }

    /// An edit made while a pass is running must never be answered with the
    /// older text's highlighting.
    #[test]
    fn test_background_highlight_discards_stale_result() {
        let old = big_rust_document();
        let new = format!("{old}// edited\n");
        let ctx = egui::Context::default();
        let mut highlighter = SyntaxHighlighter::new();

        let _ = highlighter
            .layouter(Language::Rust, egui::FontId::monospace(14.0))
            .frame_job(&old, &ctx);
        let placeholder = highlighter
            .layouter(Language::Rust, egui::FontId::monospace(14.0))
            .frame_job(&new, &ctx);
        assert_eq!(placeholder.text, new);

        let done = wait_for_highlighted(&mut highlighter, &ctx, &new);
        assert_eq!(done.text, new);
    }

    /// Small documents keep the synchronous path: no flash of plain text.
    #[test]
    fn test_small_document_is_highlighted_immediately() {
        let ctx = egui::Context::default();
        let mut highlighter = SyntaxHighlighter::new();
        let job = highlighter
            .layouter(Language::Rust, egui::FontId::monospace(14.0))
            .frame_job("fn main() {}", &ctx);
        assert!(job.sections.len() > 1);
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

    #[test]
    fn test_typescript_shares_the_javascript_grammar() {
        assert_eq!(Language::TypeScript.syntax_name(), "JavaScript");
        assert_eq!(Language::JavaScript.syntax_name(), "JavaScript");
        assert_eq!(Language::Html.syntax_name(), "HTML");
    }

    /// Number of formatted sections `highlight_line` emits for each line, with
    /// the same layouter (so multi-line state carries) across the lines.
    fn section_counts(lang: Language, lines: &[&str]) -> Vec<usize> {
        let mut highlighter = SyntaxHighlighter::new();
        let mut layouter = highlighter.layouter(lang, egui::FontId::monospace(14.0));
        lines
            .iter()
            .map(|line| {
                let job = layouter.highlight_line(line);
                job.debug_sanity_check();
                assert_eq!(job.text, *line, "text mangled for {lang:?}");
                job.sections.len()
            })
            .collect()
    }

    #[test]
    fn test_web_languages_are_coloured() {
        let samples = [
            (Language::JavaScript, "const x = 1; // note"),
            (Language::TypeScript, "let y: number = 2;"),
            (Language::Html, "<div class=\"a\">hi</div>"),
        ];
        for (lang, line) in samples {
            let counts = section_counts(lang, &[line]);
            assert!(
                counts[0] > 1,
                "{lang:?} produced a single plain section for {line:?}"
            );
        }
    }

    #[test]
    fn test_html_colours_embedded_script_and_style() {
        // The same line is plain inside a paragraph and coloured inside
        // <script>/<style>, proving the HTML grammar delegates to JS and CSS.
        let plain = section_counts(Language::Html, &["<p>", "var x = 1;", "</p>"]);
        let script = section_counts(Language::Html, &["<script>", "var x = 1;", "</script>"]);
        let style = section_counts(Language::Html, &["<style>", "p { color: red; }", "</style>"]);

        assert_eq!(plain[1], 1, "text inside <p> should be a single section");
        assert!(script[1] > 1, "script body should be coloured as JavaScript");
        assert!(style[1] > 1, "style body should be coloured as CSS");
    }

    #[test]
    fn test_json_is_coloured() {
        let counts = section_counts(Language::Json, &["{\"key\": 42, \"flag\": true}"]);
        assert!(
            counts[0] > 1,
            "JSON produced a single plain section for sample text"
        );
    }
}