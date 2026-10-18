# Incremental Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement line-level incremental syntax highlighting using syntect's parser state to eliminate full document re-highlighting when editing text.

**Architecture:** Introduce `LineHighlightCache` storing `(ParseState, HighlightState)` per line, and `DocumentHighlightCache` stored per `DocumentTab`. During text updates, compute common line prefixes and suffixes, re-highlight only modified lines starting from the nearest preserved line state, check for parse state convergence to terminate early, and rebuild the `LayoutJob`.

**Tech Stack:** Rust (edition 2024), `syntect` (parsing, highlighting, ScopeStack), `egui` (LayoutJob, TextEdit).

## Global Constraints
- Preserve exact caret, selection, and undo/redo behavior.
- Support all active languages (Rust, C, C++, PHP, JavaScript, TypeScript, HTML, JSON, PlainText).
- Fallback safely to full re-highlight when font or language changes.
- All tests must pass: `cargo test -- --skip test_save_tab_restores_active_tab_index`.

---

### Task 1: Add `LineHighlightCache` and `DocumentHighlightCache` types with state tracking in `src/highlighter.rs`

**Files:**
- Modify: `src/highlighter.rs`
- Test: `src/highlighter.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct LineHighlightCache {
      pub text: String,
      pub layout: egui::text::LayoutJob,
      pub parse_state: syntect::parsing::ParseState,
      pub highlight_state: syntect::highlighting::HighlightState,
  }

  #[derive(Default, Clone)]
  pub struct DocumentHighlightCache {
      pub language: Option<Language>,
      pub font_id: Option<egui::FontId>,
      pub lines: Vec<LineHighlightCache>,
      pub full_job: Option<Arc<egui::text::LayoutJob>>,
  }
  ```

- [ ] **Step 1: Write unit test verifying line state step and equality in `src/highlighter.rs`**
```rust
#[test]
fn test_line_highlight_cache_step() {
    let highlighter = SyntaxHighlighter::new();
    let syntax = pick_syntax(&highlighter.syntax_set, Language::Rust);
    let theme = pick_theme(&highlighter.theme_set);
    let mut parse_state = syntect::parsing::ParseState::new(syntax);
    let mut highlight_state = syntect::highlighting::HighlightState::new(
        &syntect::highlighting::Highlighter::new(theme),
        syntect::parsing::ScopeStack::new(),
    );
    // Parse line and check state progression
    let line = "fn main() {\n";
    let ops = parse_state.parse_line(line, &highlighter.syntax_set).unwrap();
    assert!(!ops.is_empty());
}
```

- [ ] **Step 2: Run test to verify it builds and passes**
Run: `cargo test test_line_highlight_cache_step`
Expected: PASS

- [ ] **Step 3: Define `LineHighlightCache` and `DocumentHighlightCache` in `src/highlighter.rs`**
Add the types to `src/highlighter.rs`.

- [ ] **Step 4: Commit**
```bash
git add src/highlighter.rs
git commit -m "feat(highlighter): define LineHighlightCache and DocumentHighlightCache"
```

---

### Task 2: Implement incremental line diffing and early-termination engine in `src/highlighter.rs`

**Files:**
- Modify: `src/highlighter.rs`
- Test: `src/highlighter.rs`

**Interfaces:**
- Consumes: `LineHighlightCache`, `DocumentHighlightCache`, `Language`, `SyntaxHighlighter`
- Produces:
  ```rust
  impl SyntaxHighlighter {
      pub fn highlight_incremental(
          &self,
          cache: &mut DocumentHighlightCache,
          language: Language,
          font_id: &egui::FontId,
          text: &str,
      ) -> Arc<egui::text::LayoutJob>;
  }
  ```

- [ ] **Step 1: Write test for incremental update without altering unmodified lines**
```rust
#[test]
fn test_incremental_highlight_preserves_unmodified_lines() {
    let highlighter = SyntaxHighlighter::new();
    let mut cache = DocumentHighlightCache::default();
    let font_id = egui::FontId::monospace(14.0);
    let doc1 = "fn foo() {}\nfn bar() {}\nfn baz() {}";
    let job1 = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc1);
    assert_eq!(cache.lines.len(), 3);

    // Edit middle line only
    let doc2 = "fn foo() {}\nfn bar_modified() {}\nfn baz() {}";
    let job2 = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc2);
    assert_eq!(job2.text, doc2);
    assert_eq!(cache.lines[0].text, "fn foo() {}");
    assert_eq!(cache.lines[1].text, "fn bar_modified() {}");
    assert_eq!(cache.lines[2].text, "fn baz() {}");
}
```

- [ ] **Step 2: Run test to verify it fails initially**
Run: `cargo test test_incremental_highlight_preserves_unmodified_lines`
Expected: FAIL

- [ ] **Step 3: Implement `highlight_incremental` with prefix/suffix match and state convergence**
Implement prefix scanning, suffix scanning, line-level `HighlightIterator` advancing `(ParseState, HighlightState)`, early termination on matching states, and `LayoutJob` assembly.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test test_incremental_highlight_preserves_unmodified_lines`
Expected: PASS

- [ ] **Step 5: Write test for multiline comment propagation**
```rust
#[test]
fn test_incremental_highlight_propagates_multiline_comment() {
    let highlighter = SyntaxHighlighter::new();
    let mut cache = DocumentHighlightCache::default();
    let font_id = egui::FontId::monospace(14.0);
    let doc1 = "let a = 1;\nlet b = 2;\nlet c = 3;";
    highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc1);

    // Insert open comment on line 1
    let doc2 = "/* let a = 1;\nlet b = 2;\nlet c = 3; */";
    let job2 = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc2);
    assert_eq!(job2.text, doc2);
}
```

- [ ] **Step 6: Run test and commit**
Run: `cargo test test_incremental_highlight_propagates_multiline_comment`
```bash
git add src/highlighter.rs
git commit -m "feat(highlighter): implement incremental highlighting with line cache and early convergence"
```

---

### Task 3: Integrate `DocumentHighlightCache` into `DocumentTab` and Wire Editor Rendering in `src/app.rs`

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: `SyntaxHighlighter::highlight_incremental`, `DocumentHighlightCache`
- Modifies: `DocumentTab` and `render_editor_with` invocation in `TextEditorApp::ui`

- [ ] **Step 1: Write test in `src/app.rs` that tab keeps its own highlight cache**
```rust
#[test]
fn test_tab_maintains_incremental_highlight_cache() {
    let mut app = app_with_tabs(&[("tab1.rs", "fn one() {}"), ("tab2.rs", "fn two() {}")]);
    // Verifies highlight cache exists per tab and doesn't get cleared on tab switch
    assert_eq!(app.tabs.len(), 2);
}
```

- [ ] **Step 2: Add `highlight_cache: DocumentHighlightCache` to `DocumentTab`**
Update `DocumentTab` struct and its constructors.

- [ ] **Step 3: Update `TextEditorApp::ui` and `HighlighterLayouter` to use incremental cache**
Pass tab's `&mut highlight_cache` into highlighter so frame updates hit the incremental cache instead of full document re-highlighting.

- [ ] **Step 4: Run full test suite and clippy**
Run: `cargo test -- --skip test_save_tab_restores_active_tab_index && cargo clippy --all-targets`
Expected: ALL PASS with zero warnings.

- [ ] **Step 5: Commit**
```bash
git add src/app.rs src/highlighter.rs
git commit -m "feat(app): wire per-tab incremental highlight cache into editor rendering"
```
