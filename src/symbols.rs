//! Code navigation for Rust, PHP, Go, Python, JavaScript / TypeScript and
//! C / C++: the definitions in a file, the word under the caret, and the Go
//! to Symbol bar. Markdown gets its headings, for Go to Symbol only.
//!
//! Definitions are found by a light line scanner, not a parser: a line that
//! starts with `fn`, `def`, `func`, `class`, … after its modifiers defines
//! something; in C and C++, a line naming something just before its `(`.
//! Nesting comes from indentation, so code is expected to be formatted.
//! Block comments and multi-line strings are skipped.

use eframe::egui::{self, Key};

use crate::highlighter::Language;
use crate::i18n::I18n;

/// Whether Go to Symbol knows `language`.
pub fn supports(language: Language) -> bool {
    matches!(
        language,
        Language::Rust
            | Language::Php
            | Language::Go
            | Language::Python
            | Language::JavaScript
            | Language::TypeScript
            | Language::C
            | Language::Cpp
            | Language::Markdown
    )
}

/// Whether Go to Definition and Find References know `language`: every
/// language Go to Symbol knows but Markdown, whose headings are no
/// identifiers.
pub fn navigates(language: Language) -> bool {
    supports(language) && language != Language::Markdown
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
    /// A Markdown heading of this level, 1 to 6.
    Heading(u8),
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
            SymbolKind::Heading(1) => "h1",
            SymbolKind::Heading(2) => "h2",
            SymbolKind::Heading(3) => "h3",
            SymbolKind::Heading(4) => "h4",
            SymbolKind::Heading(5) => "h5",
            SymbolKind::Heading(_) => "h6",
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
        !matches!(self.kind, SymbolKind::Impl | SymbolKind::Heading(_)) && self.name == name
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

/// The identifier `s` ends with, after trailing spaces.
fn last_ident(s: &str) -> Option<&str> {
    let s = s.trim_end();
    let start = s
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident(c))
        .last()
        .map_or(s.len(), |(i, _)| i);
    let name = &s[start..];
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then_some(name)
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
                let backticks = matches!(
                    language,
                    Language::Go | Language::JavaScript | Language::TypeScript
                );
                if backticks && line.matches('`').count() % 2 == 1 {
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
    if language == Language::Markdown {
        return markdown(text);
    }
    let mut skipper = Skipper::default();
    let mut c_state = CState::default();
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
        // C++ access labels sit at their class's indent without closing it.
        if matches!(language, Language::Cpp)
            && matches!(code, "public:" | "private:" | "protected:")
        {
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
            Language::JavaScript | Language::TypeScript => javascript(code, parent),
            Language::C | Language::Cpp => c_family(code, indent, parent, &mut c_state),
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

/// The modifiers that may come before a JavaScript or TypeScript definition.
const JS_MODIFIERS: &[&str] = &[
    "export", "default", "declare", "abstract", "async", "public", "private", "protected",
    "static", "readonly", "override", "accessor",
];

fn javascript(line: &str, parent: Option<SymbolKind>) -> Option<(SymbolKind, String)> {
    if line.starts_with("//") || line.starts_with('*') || line.starts_with('@') {
        return None;
    }
    let member_of = matches!(parent, Some(SymbolKind::Class | SymbolKind::Interface));
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        let Some((word, after)) = ident(trimmed) else {
            // A `#private` method, or a `*generator` one.
            let name = trimmed.strip_prefix('#').or_else(|| trimmed.strip_prefix('*'))?;
            let (name, after) = ident(name).filter(|_| member_of)?;
            return js_member(name, after);
        };
        let kind = match word {
            _ if JS_MODIFIERS.contains(&word) => {
                rest = after;
                continue;
            }
            "class" | "interface" | "enum" | "namespace" | "module" => {
                rest = after;
                match word {
                    "class" => SymbolKind::Class,
                    "interface" => SymbolKind::Interface,
                    "enum" => SymbolKind::Enum,
                    _ => SymbolKind::Module,
                }
            }
            // `get name()`, `set name(v)`; a member called `get` stays one.
            "get" | "set" if member_of && ident(after).is_some() => {
                rest = after;
                continue;
            }
            "function" => {
                rest = after.trim_start().trim_start_matches('*');
                SymbolKind::Function
            }
            "type" => {
                // `type` is also a fine name for a variable: a type alias
                // goes on with `=` or its parameters.
                let (name, after) = ident(after)?;
                let next = after.trim_start();
                return (next.starts_with('=') || next.starts_with('<'))
                    .then(|| (SymbolKind::Type, name.to_owned()));
            }
            "const" if keyword(after.trim_start(), "enum").is_some() => {
                rest = keyword(after.trim_start(), "enum")?;
                SymbolKind::Enum
            }
            "const" | "let" | "var" => {
                // Only the module's own: a body's locals are no outline.
                if !matches!(parent, None | Some(SymbolKind::Module)) {
                    return None;
                }
                let (name, after) = ident(after)?;
                let value = after.split_once('=').map_or("", |(_, value)| value.trim_start());
                let kind = if js_function_value(value) {
                    SymbolKind::Function
                } else if word == "const" {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };
                return Some((kind, name.to_owned()));
            }
            _ if member_of => return js_member(word, after),
            _ => return None,
        };
        let (name, _) = ident(rest)?;
        return Some((kind, name.to_owned()));
    }
}

/// Whether a value assigned to a name is a function: `function`, `async`,
/// or an arrow.
fn js_function_value(value: &str) -> bool {
    keyword(value, "function").is_some()
        || keyword(value, "async").is_some()
        || (value.starts_with('(') && value.contains("=>"))
        || ident(value).is_some_and(|(_, rest)| rest.trim_start().starts_with("=>"))
}

/// A class or interface member called `name`, followed by `after`: a method
/// when its parameters come next, or an arrow function assigned to it.
/// Fields are no outline.
fn js_member(name: &str, after: &str) -> Option<(SymbolKind, String)> {
    // TypeScript's optional and definite members: `name?(`, `name!:`.
    let next = after.trim_start().trim_start_matches(['?', '!']).trim_start();
    let method = next.starts_with('(')
        || next.starts_with('<')
        || next
            .strip_prefix('=')
            .is_some_and(|value| js_function_value(value.trim_start()));
    method.then(|| (SymbolKind::Method, name.to_owned()))
}

/// Words that come before a `(` without a function being defined: control
/// flow, operators that look like calls, and the types a declaration such as
/// `void (*callback)(int)` starts with.
const C_NOT_NAMES: &[&str] = &[
    "if", "for", "while", "switch", "return", "sizeof", "else", "do", "catch", "case", "new",
    "delete", "throw", "static_assert", "decltype", "alignof", "alignas", "typeid", "defined",
    "__attribute__", "__declspec", "noexcept", "requires", "operator", "void", "int", "char",
    "short", "long", "float", "double", "bool", "unsigned", "signed", "auto", "const",
    "struct", "union", "enum", "class",
];

/// The words that only qualify what follows them in C and C++.
const C_MODIFIERS: &[&str] = &["export", "extern", "static", "inline", "constexpr"];

/// What the C scanner carries from one line to the next.
#[derive(Default)]
struct CState {
    /// The indent of a `typedef` whose body is open: its name comes with the
    /// `}` that closes it, `} Name;`.
    typedef: Option<usize>,
}

fn c_family(
    line: &str,
    indent: usize,
    parent: Option<SymbolKind>,
    state: &mut CState,
) -> Option<(SymbolKind, String)> {
    if line.starts_with("//") || line.starts_with('*') {
        return None;
    }
    if let Some(open) = state.typedef {
        // The members of the typedef'd struct are fields, not symbols.
        if indent > open || !line.starts_with('}') {
            return None;
        }
        state.typedef = None;
        let name = last_ident(line.trim_start_matches('}').trim().trim_end_matches(';'))?;
        return Some((SymbolKind::Type, name.to_owned()));
    }
    if let Some(directive) = line.strip_prefix('#') {
        let rest = keyword(directive.trim_start(), "define")?;
        return ident(rest).map(|(name, _)| (SymbolKind::Macro, name.to_owned()));
    }
    let end = line.trim_end();
    if let Some(rest) = keyword(line, "typedef") {
        if let Some(head) = end.strip_suffix(';') {
            // `typedef void (*handler)(int);`
            if let Some((_, pointer)) = head.split_once("(*") {
                return ident(pointer).map(|(name, _)| (SymbolKind::Type, name.to_owned()));
            }
            return last_ident(head).map(|name| (SymbolKind::Type, name.to_owned()));
        }
        // `typedef struct Name {` or `typedef struct`: the alias comes with
        // the closing `}`.
        state.typedef = Some(indent);
        return c_type_head(rest);
    }
    let mut rest = line;
    while let Some((word, after)) = ident(rest).filter(|(word, _)| C_MODIFIERS.contains(word)) {
        let _ = word;
        rest = after;
    }
    // A type, unless the line declares a function returning one:
    // `struct node *next(struct node *n)`.
    if let Some((word, after)) = ident(rest)
        && (word == "namespace" || !line.contains('('))
    {
        if word == "using" {
            // `using Name = Type;`; `using namespace std;` names nothing.
            let (name, after) = ident(after)?;
            return after
                .trim_start()
                .starts_with('=')
                .then(|| (SymbolKind::Type, name.to_owned()));
        }
        if let Some(found) = c_type_head(rest) {
            // A forward declaration, or a variable of the type, defines
            // nothing here.
            let body = line.find('{');
            let assigned = line.find('=').is_some_and(|at| body.is_none_or(|open| at < open));
            if (end.ends_with(';') && body.is_none()) || assigned {
                return None;
            }
            return Some(found);
        }
    }
    // Statements in a body, a struct's fields: nothing more to find.
    let in_type = matches!(parent, Some(SymbolKind::Class | SymbolKind::Struct));
    if !matches!(parent, None | Some(SymbolKind::Module)) && !in_type {
        return None;
    }
    if parent.is_none() && indent > 0 {
        return None;
    }
    let head = &line[..line.find('(')?];
    if head.contains(['=', '"']) || head.matches('<').count() != head.matches('>').count() {
        return None;
    }
    let name = last_ident(head)?;
    if C_NOT_NAMES.contains(&name) {
        return None;
    }
    // A prototype or a call ends in `;`; a declaration in a class is one
    // of its methods.
    if !in_type && (end.ends_with(';') || end.ends_with(',')) {
        return None;
    }
    let before = head.trim_end();
    let before = &before[..before.len() - name.len()];
    let destructor = before.ends_with('~');
    let qualified = before.trim_end_matches('~').ends_with("::");
    let name = if destructor { format!("~{name}") } else { name.to_owned() };
    let kind = if in_type || qualified {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };
    Some((kind, name))
}

/// `struct Name`, `class Name`, `enum class Name`, `namespace Name`: the kind
/// and the name, if `line` starts with one. A macro between the keyword and
/// the name, `class API Name`, is passed over.
fn c_type_head(line: &str) -> Option<(SymbolKind, String)> {
    let (word, after) = ident(line)?;
    let kind = match word {
        "struct" | "union" => SymbolKind::Struct,
        "class" => SymbolKind::Class,
        "enum" => SymbolKind::Enum,
        "namespace" => SymbolKind::Module,
        _ => return None,
    };
    let mut after = after;
    if kind == SymbolKind::Enum
        && let Some((_, rest)) = ident(after).filter(|(w, _)| matches!(*w, "class" | "struct"))
    {
        after = rest;
    }
    let (mut name, rest) = ident(after)?;
    if let Some((second, tail)) = ident(rest)
        && second != "final"
    {
        let tail = tail.trim_start();
        if tail.is_empty() || tail.starts_with(['{', ':']) {
            name = second;
        }
    }
    Some((kind, name.to_owned()))
}

/// The headings of a Markdown document, nested by level: `#` headings and
/// underlined ones, outside code blocks, HTML comments and front matter.
fn markdown(text: &str) -> Vec<Symbol> {
    let mut found = Vec::new();
    // The levels of the headings that enclose the next one.
    let mut open: Vec<u8> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut comment = false;
    let mut previous: Option<&str> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    // YAML front matter is no heading, whatever its `---` say.
    if lines.first().is_some_and(|line| line.trim_end() == "---")
        && let Some(close) = lines
            .iter()
            .skip(1)
            .position(|line| matches!(line.trim_end(), "---" | "..."))
    {
        index = close + 2;
    }
    let mut push = |found: &mut Vec<Symbol>, level: u8, title: &str, line: usize| {
        while open.last().is_some_and(|&above| above >= level) {
            open.pop();
        }
        found.push(Symbol {
            name: title.to_owned(),
            kind: SymbolKind::Heading(level),
            line,
            depth: open.len(),
        });
        open.push(level);
    };
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = line.trim();
        if let Some((mark, length)) = fence {
            let closes = indent < 4
                && trimmed.chars().all(|c| c == mark)
                && trimmed.chars().count() >= length;
            if closes {
                fence = None;
            }
            previous = None;
            continue;
        }
        if comment {
            comment = !trimmed.contains("-->");
            previous = None;
            continue;
        }
        if indent < 4 {
            let mark = trimmed.chars().next();
            if let Some(mark @ ('`' | '~')) = mark {
                let length = trimmed.chars().take_while(|&c| c == mark).count();
                if length >= 3 {
                    fence = Some((mark, length));
                    previous = None;
                    continue;
                }
            }
            if trimmed.starts_with("<!--") && !trimmed.contains("-->") {
                comment = true;
                previous = None;
                continue;
            }
            // `# Title`, with an optional closing run of `#`.
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&level) {
                let rest = &trimmed[level..];
                if rest.is_empty() || rest.starts_with([' ', '\t']) {
                    let title = rest.trim().trim_end_matches('#').trim_end();
                    if !title.is_empty() {
                        push(&mut found, level as u8, title, index);
                    }
                    previous = None;
                    continue;
                }
            }
            // A paragraph line underlined with `=` or `-`.
            if let Some(title) = previous
                && !trimmed.is_empty()
            {
                let underline = |c| trimmed.chars().all(|x| x == c);
                let level = if underline('=') {
                    Some(1)
                } else if underline('-') {
                    Some(2)
                } else {
                    None
                };
                if let Some(level) = level {
                    push(&mut found, level, title, index - 1);
                    previous = None;
                    continue;
                }
            }
        }
        // Only a paragraph line can be underlined into a heading: not a list
        // item, a quote, a table or code.
        let paragraph = indent < 4
            && !trimmed.is_empty()
            && !trimmed.starts_with(['-', '*', '+', '>', '|'])
            && !trimmed.starts_with(|c: char| c.is_ascii_digit());
        previous = paragraph.then_some(trimmed);
    }
    found
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
    fn test_which_languages_are_supported() {
        for language in [
            Language::Rust,
            Language::Php,
            Language::Go,
            Language::Python,
            Language::JavaScript,
            Language::TypeScript,
            Language::C,
            Language::Cpp,
        ] {
            assert!(supports(language), "{language:?}");
            assert!(navigates(language), "{language:?}");
        }
        // Markdown has an outline, but its headings are no identifiers.
        assert!(supports(Language::Markdown));
        assert!(!navigates(Language::Markdown));
        for language in [Language::PlainText, Language::Html, Language::Css, Language::Json] {
            assert!(!supports(language), "{language:?}");
        }
        assert!(symbols("body { color: red }", Language::Css).is_empty());
    }

    #[test]
    fn test_javascript_symbols() {
        let text = "\
// function not_this() {}
import { x } from './x.js';

export default class Shape extends Base {
    #count = 0;
    static origin = null;

    constructor(name) {
        const local = 1;
    }

    get area() {
        return 1;
    }

    async *walk() {}
    #secret() {}
    handler = (event) => {};
}

export async function load(url) {
    function inner() {}
}

const LIMIT = 10;
let counter = 0;
export const run = async () => {};
const double = x => x * 2;
const items = list.map(a => a);
/*
function commented() {}
*/
const html = `
function inTemplate() {}
`;
";
        assert_eq!(
            outline(text, Language::JavaScript),
            [
                row(0, "class", "Shape", 4),
                row(1, "method", "constructor", 8),
                row(1, "method", "area", 12),
                row(1, "method", "walk", 16),
                row(1, "method", "secret", 17),
                row(1, "method", "handler", 18),
                row(0, "fn", "load", 21),
                row(1, "fn", "inner", 22),
                row(0, "const", "LIMIT", 25),
                row(0, "var", "counter", 26),
                row(0, "fn", "run", 27),
                row(0, "fn", "double", 28),
                row(0, "const", "items", 29),
                row(0, "const", "html", 33),
            ]
        );
    }

    #[test]
    fn test_typescript_symbols() {
        let text = "\
export interface Shape {
    area(): number;
    name: string;
    resize?(factor: number): void;
}

export type Id = string;
type Pair<T> = [T, T];
let type = 1;

export enum Color { Red, Green }
const enum Flags { A = 1 }

declare namespace Api {
    function call(): void;
}

export abstract class Base<T> implements Shape {
    private readonly id: Id;
    public abstract area(): number;
    protected static make<U>(value: U): Base<U> {}
}
";
        assert_eq!(
            outline(text, Language::TypeScript),
            [
                row(0, "interface", "Shape", 1),
                row(1, "method", "area", 2),
                row(1, "method", "resize", 4),
                row(0, "type", "Id", 7),
                row(0, "type", "Pair", 8),
                row(0, "var", "type", 9),
                row(0, "enum", "Color", 11),
                row(0, "enum", "Flags", 12),
                row(0, "mod", "Api", 14),
                row(1, "fn", "call", 15),
                row(0, "class", "Base", 18),
                row(1, "method", "area", 20),
                row(1, "method", "make", 21),
            ]
        );
    }

    #[test]
    fn test_c_symbols() {
        let text = "\
#include <stdio.h>
#define MAX_LEN 64

/* int not_this(void) { } */
struct point {
    int x;
    void (*draw)(struct point *p);
};

struct point origin = { 0, 0 };
struct node;
typedef unsigned int uint;
typedef void (*handler)(int code);

typedef struct {
    int width;
} size_t2;

enum color { RED, GREEN };

static int helper(int a);

static inline int add(int a, int b)
{
    int sum = add_more(a, b);
    if (sum > 0) {
        return sum;
    }
    return 0;
}

struct node *next(struct node *n) {
    return n;
}

int
main(void)
{
    printf(\"hi\");
}
";
        assert_eq!(
            outline(text, Language::C),
            [
                row(0, "macro", "MAX_LEN", 2),
                row(0, "struct", "point", 5),
                row(0, "type", "uint", 12),
                row(0, "type", "handler", 13),
                row(0, "type", "size_t2", 17),
                row(0, "enum", "color", 19),
                row(0, "fn", "add", 23),
                row(0, "fn", "next", 32),
                row(0, "fn", "main", 37),
            ]
        );
    }

    #[test]
    fn test_cpp_symbols() {
        let text = "\
namespace geometry {

class EXPORT Shape : public Base {
public:
    Shape(int sides);
    virtual ~Shape();
    double area() const;
    std::function<void()> callback;
    int count = compute(3);
};

enum class Kind { Circle, Square };
using Id = std::string;
using namespace std;

template <typename T>
T largest(T a, T b) {
    return a > b ? a : b;
}

Shape::Shape(int sides) {}
Shape::~Shape() {}
double Shape::area() const {
    return 0.0;
}

}
";
        assert_eq!(
            outline(text, Language::Cpp),
            [
                row(0, "mod", "geometry", 1),
                row(0, "class", "Shape", 3),
                row(1, "method", "Shape", 5),
                row(1, "method", "~Shape", 6),
                row(1, "method", "area", 7),
                row(0, "enum", "Kind", 12),
                row(0, "type", "Id", 13),
                row(0, "fn", "largest", 17),
                row(0, "method", "Shape", 21),
                row(0, "method", "~Shape", 22),
                row(0, "method", "area", 23),
            ]
        );
    }

    #[test]
    fn test_markdown_headings() {
        let text = "\
---
title: Front matter
---

# Guide

Intro text.

## Install ##

```sh
# not a heading
```

### Linux
Setup
=====
Details
-------

- item
---

<!--
# commented
-->
    # indented code
#not-a-heading
####### seven
## Usage
";
        assert_eq!(
            outline(text, Language::Markdown),
            [
                row(0, "h1", "Guide", 5),
                row(1, "h2", "Install", 9),
                row(2, "h3", "Linux", 15),
                row(0, "h1", "Setup", 16),
                row(1, "h2", "Details", 18),
                row(1, "h2", "Usage", 30),
            ]
        );
        // Headings are an outline, never a definition.
        let found = symbols("# Guide\n", Language::Markdown);
        assert!(!found[0].defines("Guide"));
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
