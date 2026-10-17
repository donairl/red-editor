# Web Language Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Highlight JavaScript, TypeScript and HTML files, detected from the file extension and selectable from the "Bahasa" menu.

**Architecture:** Add three variants to the existing `Language` enum in `src/highlighter.rs`. TypeScript reuses syntect's bundled `JavaScript` grammar because syntect ships no TypeScript grammar. Everything downstream (layout cache, background pass, menu built from `Language::all()`) is unchanged.

**Tech Stack:** Rust (edition 2024), `eframe`/`egui` 0.37, `syntect` 5.3 (`regex-fancy`).

## Global Constraints

- `syntect` stays at `5.3` with `default-features = false` and the `regex-fancy` backend. No new dependencies and no vendored grammar files.
- `TypeScript::syntax_name()` returns `"JavaScript"`; its `display_name()` stays `"TypeScript"`.
- Extension matching is case-insensitive. Unknown extensions map to `PlainText`.
- `.vue`, `.svelte`, `.json` and `.css` are not mapped.
- `Language::all()` lists the new variants after `Php`, giving 8 entries.
- Only `src/highlighter.rs` changes. `src/app.rs` is not edited.
- `cargo test`, `cargo clippy --all-targets` and `cargo build --release` stay clean. The 37 existing tests keep passing.

---

### Task 1: Add JavaScript, TypeScript and HTML to `Language`

**Files:**
- Modify: `src/highlighter.rs:20-86` (enum, `all`, `from_extension`, `display_name`, `syntax_name`)
- Modify: `src/highlighter.rs:498-568` (existing tests that enumerate languages)
- Test: `src/highlighter.rs` (`mod tests`, appended before the closing `}` at the end of the file)

**Interfaces:**
- Consumes (existing, unchanged): `Language::{all, from_extension, from_path, display_name, syntax_name}`, `SyntaxHighlighter::{new, layouter, syntax_set}`, `HighlighterLayouter::highlight_line(&mut self, &str) -> egui::text::LayoutJob`.
- Produces: `Language::JavaScript`, `Language::TypeScript`, `Language::Html`.

- [ ] **Step 1: Update the existing tests that enumerate languages**

In `test_from_extension_maps_supported_extensions`, add after the `php` assertion:

```rust
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
```

In `test_from_extension_is_case_insensitive`, add:

```rust
        assert_eq!(Language::from_extension("JS"), Language::JavaScript);
        assert_eq!(Language::from_extension("Ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("HTML"), Language::Html);
```

In `test_from_path_handles_full_paths_and_missing_extension`, add before the closing brace:

```rust
        assert_eq!(
            Language::from_path(Path::new("/srv/app.test.ts")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("/srv/site/index.html")),
            Language::Html
        );
```

In `test_display_name_labels_each_language`, add:

```rust
        assert_eq!(Language::JavaScript.display_name(), "JavaScript");
        assert_eq!(Language::TypeScript.display_name(), "TypeScript");
        assert_eq!(Language::Html.display_name(), "HTML");
```

In `test_all_lists_every_variant`, change `5` to `8`.

- [ ] **Step 2: Add the new tests**

Append inside `mod tests`, after `test_block_comment_keeps_comment_color_across_lines`:

```rust
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test 2>&1 | grep -E "^error" -A4 | head -20`
Expected: compile errors `no variant or associated item named 'JavaScript' found for enum 'Language'` (and the same for `TypeScript` and `Html`).

- [ ] **Step 4: Add the enum variants**

In `src/highlighter.rs`, replace

```rust
    Cpp,
    Php,
}
```

with

```rust
    Cpp,
    Php,
    JavaScript,
    /// Shares the `JavaScript` grammar: syntect bundles no TypeScript grammar.
    TypeScript,
    Html,
}
```

- [ ] **Step 5: Extend `all`, `from_extension`, `display_name` and `syntax_name`**

In `all()`, replace

```rust
            Language::Php,
        ]
```

with

```rust
            Language::Php,
            Language::JavaScript,
            Language::TypeScript,
            Language::Html,
        ]
```

In `from_extension`, replace

```rust
            "php" => Language::Php,
```

with

```rust
            "php" => Language::Php,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
            "html" | "htm" => Language::Html,
```

In `display_name`, replace the `Language::Php => "PHP",` arm (the one directly above the closing of that `match`, before the `syntax_name` doc comment) with:

```rust
            Language::Php => "PHP",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Html => "HTML",
```

In `syntax_name`, replace its `Language::Php => "PHP",` arm with:

```rust
            Language::Php => "PHP",
            Language::JavaScript | Language::TypeScript => "JavaScript",
            Language::Html => "HTML",
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test 2>&1 | grep -E "test result|FAILED|panicked|^error" -A4`
Expected: `test result: ok. 40 passed; 0 failed` (37 existing plus 3 new).

- [ ] **Step 7: Run clippy and the release build**

Run: `cargo clippy --all-targets 2>&1 | tail -5 && cargo build --release 2>&1 | tail -2`
Expected: no warnings or errors; both finish with `Finished`.

- [ ] **Step 8: Check the diff is only what is intended, then commit**

Run: `git status --short`
Expected: only ` M src/highlighter.rs`.

```bash
git add src/highlighter.rs
git commit -m "feat(highlighter): highlight JavaScript, TypeScript and HTML"
```

---

## Self-Review

- **Spec coverage:** variants and names (Steps 4-5), extension table (Steps 1 and 5), TypeScript sharing the JavaScript grammar (Steps 2 and 5), `all()` of 8 (Steps 1 and 5), embedded script/style colouring (Step 2), multi-dot path (Step 1), unchanged `app.rs` (Global Constraints). No gaps.
- **Placeholders:** none; every step carries its code or command.
- **Type consistency:** `JavaScript`, `TypeScript` and `Html` are used with the same spelling in every step. `section_counts` is defined once in Step 2 and used by both new tests that need it.
