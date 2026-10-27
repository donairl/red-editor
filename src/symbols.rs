//! Code navigation for Rust, PHP, Go and Python: the definitions in a file,
//! the word under the caret, and the Go to Symbol bar.
//!
//! Definitions are found by a light line scanner, not a parser: a line that
//! starts with `fn`, `def`, `func`, `class`, … after its modifiers defines
//! something. Nesting comes from indentation, so code is expected to be
//! formatted. Block comments and multi-line strings are skipped.

use eframe::egui::{self, Key};

use crate::highlighter::Language;
use crate::i18n::I18n;

/// Whether code navigation knows `language`.
pub fn supports(language: Language) -> bool {
    matches!(
        language,
        Language::Rust | Language::Php | Language::Go | Language::Python
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Module,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    /// A Rust `impl` block: listed in the outline, never a definition.
    Impl,
    Type,
    Function,
    Method,
    Macro,
    Constant,
    Variable,
}

impl SymbolKind {
    /// Short tag shown beside the name.
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Module => "mod",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Impl => "impl",
            SymbolKind::Type => "type",
            SymbolKind::Function => "fn",
            SymbolKind::Method => "method",
            SymbolKind::Macro => "macro",
            SymbolKind::Constant => "const",
            SymbolKind::Variable => "var",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based.
    pub line: usize,
    /// How many symbols enclose it.
    pub depth: usize,
}

impl Symbol {
    /// Whether the symbol defines `name`, for Go to Definition.
    pub fn defines(&self, name: &str) -> bool {
        self.kind != SymbolKind::Impl && self.name == name
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier `s` starts with, after any spaces, and what follows it.
fn ident(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let end = s.find(|c: char| !is_ident(c)).unwrap_or(s.len());
    let name = &s[..end];
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then(|| (name, &s[end..]))
}

/// What follows `keyword` when `s` starts with it as a whole word.
fn keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(keyword)?;
    (!rest.starts_with(is_ident)).then_some(rest)
}

/// Columns of leading whitespace, a tab counting four.
fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Hides lines inside block comments and multi-line strings.
#[derive(Default)]
struct Skipper {
    /// The delimiter that ends what the scanner is inside of.
    inside: Option<&'static str>,
}

impl Skipper {
    /// The part of `line` that is code, or `None` for none of it.
    fn code<'a>(&mut self, line: &'a str, language: Language) -> Option<&'a str> {
        if let Some(end) = self.inside {
            let (_, after) = line.split_once(end)?;
            self.inside = None;
            return Some(after);
        }
        let trimmed = line.trim_start();
        match language {
            Language::Python => {
                for quote in ["\"\"\"", "'''"] {
                    if line.matches(quote).count() % 2 == 1 {
                        self.inside = Some(quote);
                        // A docstring's first line defines nothing.
                        return Some(if trimmed.starts_with(quote) { "" } else { line });
                    }
                }
            }
            _ => {
                if let Some(rest) = trimmed.strip_prefix("/*") {
                    return match rest.split_once("*/") {
                        Some((_, after)) => Some(after),
                        None => {
                            self.inside = Some("*/");
                            None
                        }
                    };
                }
                if language == Language::Go && line.matches('`').count() % 2 == 1 {
                    self.inside = Some("`");
                    return Some(line);
                }
            }
        }
        Some(line)
    }
}

/// Everything defined in `text`, in source order.
pub fn symbols(text: &str, language: Language) -> Vec<Symbol> {
    let mut found = Vec::new();
    if !supports(language) {
        return found;
    }
    let mut skipper = Skipper::default();
    // Indent and kind of each symbol whose body may follow, innermost last.
    let mut scopes: Vec<(usize, SymbolKind)> = Vec::new();
    let mut go_group: Option<GoGroup> = None;
    for (index, line) in text.lines().enumerate() {
        let Some(code) = skipper.code(line, language) else {
            continue;
        };
        let code = code.trim();
        // A brace on its own line opens the body above it, at its indent.
        if code.is_empty() || code == "{" {
            continue;
        }
        let indent = indent_width(line);
        while scopes.last().is_some_and(|&(open, _)| open >= indent) {
            scopes.pop();
        }
        let parent = scopes.last().map(|&(_, kind)| kind);
        let symbol = match language {
            Language::Rust => rust(code, parent),
            Language::Go => go(code, indent, &mut go_group),
            Language::Python => python(code, parent),
            Language::Php => php(code, parent),
            _ => None,
        };
        if let Some((kind, name)) = symbol {
            found.push(Symbol {
                name,
                kind,
                line: index + 1,
                depth: scopes.len(),
            });
            scopes.push((indent, kind));
        }
    }
    found
}

fn method_or_function(parent: Option<SymbolKind>, methods_in: &[SymbolKind]) -> SymbolKind {
    match parent {
        Some(kind) if methods_in.contains(&kind) => SymbolKind::Method,
        _ => SymbolKind::Function,
    }
}

fn rust(line: &str, parent: Option<SymbolKind>) -> Option<(SymbolKind, String)> {
    let mut rest = line;
    loop {
        let (word, after) = ident(rest)?;
        rest = after;
        let kind = match word {
            "pub" => {
                // `pub(crate)`, `pub(in path)`
                if let Some(inner) = rest.trim_start().strip_prefix('(') {
                    rest = inner.split_once(')')?.1;
                }
                continue;
            }
            "async" | "unsafe" | "default" => continue,
            "extern" => {
                // `extern "C" fn`
                let after = rest.trim_start();
                if let Some(abi) = after.strip_prefix('"') {
                    rest = abi.split_once('"')?.1;
                }
                continue;
            }
            "const" => {
                let (next, _) = ident(rest)?;
                if matches!(next, "fn" | "async" | "unsafe" | "extern") {
                    continue;
                }
                SymbolKind::Constant
            }
            "static" => {
                if let Some(after) = keyword(rest.trim_start(), "mut") {
                    rest = after;
                }
                SymbolKind::Constant
            }
            "fn" => method_or_function(parent, &[SymbolKind::Impl, SymbolKind::Trait]),
            "struct" | "union" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "type" => SymbolKind::Type,
            "mod" => SymbolKind::Module,
            "macro_rules" => {
                rest = rest.trim_start().strip_prefix('!')?;
                SymbolKind::Macro
            }
            "impl" => {
                let head = rest.split(['{', ';']).next().unwrap_or(rest);
                let head = head.split(" where").next().unwrap_or(head).trim();
                let gap = if head.starts_with('<') { "" } else { " " };
                return Some((SymbolKind::Impl, format!("impl{gap}{head}")));
            }
            _ => return None,
        };
        let (name, _) = ident(rest)?;
        return (name != "_").then(|| (kind, name.to_owned()));
    }
}

/// A Go `const (`, `var (` or `type (` block being read.
struct GoGroup {
    kind: SymbolKind,
    /// Indent of its members, once the first one is seen.
    indent: Option<usize>,
}

fn go(line: &str, indent: usize, group: &mut Option<GoGroup>) -> Option<(SymbolKind, String)> {
    if let Some(open) = group {
        if line.starts_with(')') {
            *group = None;
            return None;
        }
        // Only members, not the fields of a struct declared in the block.
        if *open.indent.get_or_insert(indent) != indent {
            return None;
        }
        let (name, rest) = ident(line)?;
        let kind = match (open.kind, ident(rest)) {
            (SymbolKind::Type, Some(("struct", _))) => SymbolKind::Struct,
            (SymbolKind::Type, Some(("interface", _))) => SymbolKind::Interface,
            (kind, _) => kind,
        };
        return (name != "_").then(|| (kind, name.to_owned()));
    }
    // Everything else is declared at the top level.
    if indent > 0 {
        return None;
    }
    let (word, rest) = ident(line)?;
    let kind = match word {
        "func" => {
            let rest = rest.trim_start();
            return match rest.strip_prefix('(') {
                // A receiver: `func (s *Server) Name(`
                Some(receiver) => {
                    let (name, _) = ident(receiver.split_once(')')?.1)?;
                    Some((SymbolKind::Method, name.to_owned()))
                }
                None => ident(rest).map(|(name, _)| (SymbolKind::Function, name.to_owned())),
            };
        }
        "type" => SymbolKind::Type,
        "const" => SymbolKind::Constant,
        "var" => SymbolKind::Variable,
        "package" => SymbolKind::Module,
        _ => return None,
    };
    if rest.trim_start().starts_with('(') {
        *group = Some(GoGroup { kind, indent: None });
        return None;
    }
    let (name, after) = ident(rest)?;
    let kind = match (kind, ident(after)) {
        (SymbolKind::Type, Some(("struct", _))) => SymbolKind::Struct,
        (SymbolKind::Type, Some(("interface", _))) => SymbolKind::Interface,
        (kind, _) => kind,
    };
    (name != "_").then(|| (kind, name.to_owned()))
}

fn python(line: &str, parent: Option<SymbolKind>) -> Option<(SymbolKind, String)> {
    let line = keyword(line, "async").map_or(line, str::trim_start);
    if let Some(rest) = keyword(line, "def") {
        let kind = method_or_function(parent, &[SymbolKind::Class]);
        return ident(rest).map(|(name, _)| (kind, name.to_owned()));
    }
    let rest = keyword(line, "class")?;
    ident(rest).map(|(name, _)| (SymbolKind::Class, name.to_owned()))
}

fn php(line: &str, parent: Option<SymbolKind>) -> Option<(SymbolKind, String)> {
    let mut rest = line;
    loop {
        let (word, after) = ident(rest)?;
        rest = after;
        let kind = match word {
            "abstract" | "final" | "public" | "protected" | "private" | "static" | "readonly" => {
                continue;
            }
            "function" => {
                rest = rest.trim_start().trim_start_matches('&');
                method_or_function(
                    parent,
                    &[
                        SymbolKind::Class,
                        SymbolKind::Trait,
                        SymbolKind::Interface,
                        SymbolKind::Enum,
                    ],
                )
            }
            "class" => SymbolKind::Class,
            "interface" => SymbolKind::Interface,
            "trait" => SymbolKind::Trait,
            "enum" => SymbolKind::Enum,
            "namespace" => {
                let name = rest.trim().trim_end_matches([';', '{']).trim();
                return (!name.is_empty()).then(|| (SymbolKind::Module, name.to_owned()));
            }
            "const" => {
                // A typed constant names its type first: `const string NAME = `.
                let (first, after) = ident(rest)?;
                let name = match ident(after) {
                    Some((second, _)) => second,
                    None => first,
                };
                return Some((SymbolKind::Constant, name.to_owned()));
            }
            _ => return None,
        };
        let (name, _) = ident(rest)?;
        return Some((kind, name.to_owned()));
    }
}

/// The identifier at or just before the `char` index `at`.
pub fn word_at(text: &str, at: usize) -> Option<String> {
    let start = text.char_indices().nth(at).map_or(text.len(), |(i, _)| i);
    let before = text[..start]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident(c))
        .last()
        .map_or(start, |(i, _)| i);
    let after = text[start..]
        .find(|c: char| !is_ident(c))
        .map_or(text.len(), |i| start + i);
    let word = &text[before..after];
    (!word.is_empty() && !word.starts_with(|c: char| c.is_ascii_digit())).then(|| word.to_owned())
}

/// State of the Go to Symbol bar.
#[derive(Default)]
pub struct SymbolPicker {
    pub visible: bool,
    pub filter: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// The document's symbols when the bar opened.
    pub symbols: Vec<Symbol>,
    /// Index into the filtered list.
    selected: usize,
}

impl SymbolPicker {
    pub fn open(&mut self, symbols: Vec<Symbol>) {
        *self = Self {
            visible: true,
            focus: true,
            symbols,
            ..Self::default()
        };
    }

    /// The symbols whose name holds the filter, ignoring case.
    fn matches(&self) -> Vec<&Symbol> {
        let filter = self.filter.to_lowercase();
        self.symbols
            .iter()
            .filter(|s| s.name.to_lowercase().contains(&filter))
            .collect()
    }
}

/// What the user asked of the Go to Symbol bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerAction {
    /// Jump to this 1-based line.
    Go(usize),
    Close,
}

/// Rows of the list shown at once.
const VISIBLE_ROWS: f32 = 12.0;

/// Draws the bar: a filter field over the matching symbols. Up and Down
/// move the selection, Enter jumps to it, Escape closes.
pub fn show_picker(
    ui: &mut egui::Ui,
    state: &mut SymbolPicker,
    i18n: &I18n,
) -> Option<PickerAction> {
    let mut action = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let id = egui::Id::unique("goto_symbol");
                // Focused before it draws, so its key filter holds from the
                // first frame.
                if std::mem::take(&mut state.focus) {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                // Taken before the field draws, which would move its caret.
                if ui.memory(|m| m.has_focus(id)) {
                    let (up, down) = ui.input_mut(|i| {
                        (
                            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                        )
                    });
                    let count = state.matches().len();
                    if up {
                        state.selected = state.selected.saturating_sub(1);
                    }
                    if down && state.selected + 1 < count {
                        state.selected += 1;
                    }
                }
                let field = ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .id(id)
                        // Up and Down pick a symbol instead of moving focus.
                        .event_filter(egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            ..Default::default()
                        })
                        .hint_text(i18n.t("symbol.hint"))
                        .desired_width(260.0),
                );
                if field.changed() {
                    state.selected = 0;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    match state.matches().get(state.selected) {
                        Some(symbol) => action = Some(PickerAction::Go(symbol.line)),
                        None => field.request_focus(),
                    }
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) || ui.button("\u{d7}").clicked() {
                    action = Some(PickerAction::Close);
                }
            });

            let matches = state.matches();
            if matches.is_empty() {
                ui.weak(i18n.t("symbol.none"));
                return;
            }
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
            egui::ScrollArea::vertical()
                .max_height(row_height * VISIBLE_ROWS)
                .auto_shrink([false, true])
                .show_rows(ui, row_height, matches.len(), |ui, rows| {
                    for index in rows {
                        let symbol = matches[index];
                        let label = format!(
                            "{}{} {}  :{}",
                            "  ".repeat(symbol.depth),
                            symbol.name,
                            symbol.kind.label(),
                            symbol.line
                        );
                        let selected = index == state.selected;
                        let row =
                            ui.selectable_label(selected, egui::RichText::new(label).monospace());
                        if selected {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            action = Some(PickerAction::Go(symbol.line));
                        }
                    }
                });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(text: &str, language: Language) -> Vec<(usize, &'static str, String, usize)> {
        symbols(text, language)
            .into_iter()
            .map(|s| (s.depth, s.kind.label(), s.name, s.line))
            .collect()
    }

    fn row(
        depth: usize,
        kind: &'static str,
        name: &str,
        line: usize,
    ) -> (usize, &'static str, String, usize) {
        (depth, kind, name.to_owned(), line)
    }

    #[test]
    fn test_supports_only_the_four_languages() {
        for language in [
            Language::Rust,
            Language::Php,
            Language::Go,
            Language::Python,
        ] {
            assert!(supports(language), "{language:?}");
        }
        assert!(!supports(Language::JavaScript));
        assert!(symbols("function f() {}", Language::JavaScript).is_empty());
    }

    #[test]
    fn test_rust_symbols() {
        let text = "\
//! fn not_this() {}
use std::fmt;

/// Docs.
pub(crate) struct Point { x: i32 }
pub enum Shape {}
const LIMIT: usize = 3;
static mut COUNT: u32 = 0;
type Alias = u8;
macro_rules! make { () => {} }

impl<T> fmt::Display for Point<T> where T: Copy {
    pub const fn new() -> Self {}

    async unsafe fn run(&self) {
        fn helper() {}
    }
}

pub trait Draw {
    fn draw(&self);
}

/*
fn commented() {}
*/
extern \"C\" fn callback() {}
mod tests {
    fn test_it() {}
}
";
        assert_eq!(
            outline(text, Language::Rust),
            [
                row(0, "struct", "Point", 5),
                row(0, "enum", "Shape", 6),
                row(0, "const", "LIMIT", 7),
                row(0, "const", "COUNT", 8),
                row(0, "type", "Alias", 9),
                row(0, "macro", "make", 10),
                row(0, "impl", "impl<T> fmt::Display for Point<T>", 12),
                row(1, "method", "new", 13),
                row(1, "method", "run", 15),
                row(2, "fn", "helper", 16),
                row(0, "trait", "Draw", 20),
                row(1, "method", "draw", 21),
                row(0, "fn", "callback", 27),
                row(0, "mod", "tests", 28),
                row(1, "fn", "test_it", 29),
            ]
        );
    }

    #[test]
    fn test_go_symbols() {
        let text = "\
package main

type Server struct {
\tName string
}

type (
\tID int
\tConfig struct {
\t\tPort int
\t}
\tReader interface{}
)

const (
\tA = iota
\t_ = 1
)

var debug = false

func (s *Server) Start() error {
\tvar local = 1
}

func main() {}
";
        assert_eq!(
            outline(text, Language::Go),
            [
                row(0, "mod", "main", 1),
                row(0, "struct", "Server", 3),
                row(0, "type", "ID", 8),
                row(0, "struct", "Config", 9),
                row(0, "interface", "Reader", 12),
                row(0, "const", "A", 16),
                row(0, "var", "debug", 20),
                row(0, "method", "Start", 22),
                row(0, "fn", "main", 26),
            ]
        );
    }

    #[test]
    fn test_python_symbols() {
        let text = "\
import os

def top():
    \"\"\"Docs.

def not_this():
    \"\"\"
    def inner():
        pass

class Shape(Base):
    '''One line.'''
    async def area(self):
        pass

    class Meta:
        def method(self): pass

def after(): pass
";
        assert_eq!(
            outline(text, Language::Python),
            [
                row(0, "fn", "top", 3),
                row(1, "fn", "inner", 8),
                row(0, "class", "Shape", 11),
                row(1, "method", "area", 13),
                row(1, "class", "Meta", 16),
                row(2, "method", "method", 17),
                row(0, "fn", "after", 19),
            ]
        );
    }

    #[test]
    fn test_php_symbols() {
        let text = "\
<?php
namespace App\\Http;

/**
 * function not_this()
 */
final class Controller extends Base
{
    public const VERSION = 2;
    const string NAME = 'x';

    public static function &make(): static
    {
        $f = function () {};
    }

    abstract protected function handle();
}

interface Shape {
    public function area(): float;
}

enum Suit: string {
    case Hearts = 'H';
}

function helper() {}
";
        assert_eq!(
            outline(text, Language::Php),
            [
                row(0, "mod", "App\\Http", 2),
                row(0, "class", "Controller", 7),
                row(1, "const", "VERSION", 9),
                row(1, "const", "NAME", 10),
                row(1, "method", "make", 12),
                row(1, "method", "handle", 17),
                row(0, "interface", "Shape", 20),
                row(1, "method", "area", 21),
                row(0, "enum", "Suit", 24),
                row(0, "fn", "helper", 28),
            ]
        );
    }

    #[test]
    fn test_defines_skips_impl_blocks() {
        let found = symbols("struct A;\nimpl A {}\n", Language::Rust);
        let defining: Vec<_> = found.iter().filter(|s| s.defines("A")).collect();
        assert_eq!(defining.len(), 1);
        assert_eq!(defining[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn test_word_at_the_caret() {
        let text = "let total = sum_of(a, b2);";
        assert_eq!(word_at(text, 4).as_deref(), Some("total"));
        assert_eq!(
            word_at(text, 9).as_deref(),
            Some("total"),
            "just after the word"
        );
        assert_eq!(word_at(text, 15).as_deref(), Some("sum_of"));
        assert_eq!(word_at(text, 10), None, "between spaces");
        assert_eq!(word_at("$this->name", 8).as_deref(), Some("name"));
        assert_eq!(word_at("x = 42", 5), None, "a number");
        assert_eq!(word_at("käse()", 2).as_deref(), Some("käse"));
        assert_eq!(word_at("end", 3).as_deref(), Some("end"));
    }

    #[test]
    fn test_picker_filters_ignoring_case() {
        let mut picker = SymbolPicker::default();
        picker.open(symbols(
            "fn alpha() {}\nfn Beta() {}\nfn alphabet() {}\n",
            Language::Rust,
        ));
        picker.filter = "ALPHA".into();
        let names: Vec<_> = picker.matches().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha", "alphabet"]);
    }

    #[test]
    fn test_picker_keys_move_and_pick() {
        let ctx = egui::Context::default();
        let mut picker = SymbolPicker::default();
        picker.open(symbols("fn a() {}\nfn b() {}\nfn c() {}\n", Language::Rust));
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut picked = None;
        // egui applies the field's key filter from its second focused frame.
        let frames = [
            vec![],
            vec![],
            vec![key(Key::ArrowDown)],
            vec![key(Key::ArrowDown)],
            vec![key(Key::ArrowDown)],
            vec![key(Key::ArrowUp)],
            vec![key(Key::Enter)],
        ];
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if let Some(action) = show_picker(ui, &mut picker, &I18n::builtin()) {
                    picked = Some(action);
                }
            });
            output.textures_delta.clear();
        }
        assert_eq!(
            picked,
            Some(PickerAction::Go(2)),
            "down past the end stays on the last"
        );
    }
}
