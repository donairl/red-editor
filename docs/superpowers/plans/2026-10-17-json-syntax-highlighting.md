# JSON Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add syntax highlighting for `.json` and `.jsonc` files using `syntect`'s bundled `"JSON"` grammar, with a VS Code gold `{}` badge on tabs and the file explorer sidebar.

**Architecture:** Extend the `Language` enum in `src/highlighter.rs` with `Language::Json` and map `.json` / `.jsonc` extensions. Update `src/tab_bar.rs` badge renderer with the `{}` badge and `#CBB041` color. Menus and sidebar inherit changes automatically.

**Tech Stack:** Rust 2024, `syntect 5.3`, `egui 0.37` (`eframe`).

## Global Constraints

- Follow established `Language` enum patterns in `src/highlighter.rs`.
- No new dependencies or external grammar files; use syntect's bundled `"JSON"` grammar.
- Case-insensitive extension mapping for `.json` and `.jsonc`.
- Maintain test coverage with all existing tests passing.

---

### Task 1: Add `Language::Json` and Highlighting Support in `src/highlighter.rs`

**Files:**
- Modify: `src/highlighter.rs`
- Test: `src/highlighter.rs`

**Interfaces:**
- Produces: `Language::Json` variant, `Language::from_extension("json") -> Language::Json`, `Language::from_extension("jsonc") -> Language::Json`, `Language::display_name(&self) -> "JSON"`, `Language::syntax_name(&self) -> "JSON"`.

- [ ] **Step 1: Write failing unit tests in `src/highlighter.rs`**

Add tests for JSON extension mapping and highlighting in `src/highlighter.rs` test module:

```rust
    #[test]
    fn test_json_is_coloured() {
        let mut highlighter = SyntaxHighlighter::new();
        let font_id = egui::FontId::monospace(14.0);
        let mut layouter = highlighter.layouter(Language::Json, font_id);
        let text = "{\n  \"key\": 42,\n  \"flag\": true\n}";
        let job = layouter.document_job(text);
        job.debug_sanity_check();
        assert_eq!(job.text, text);
        assert!(
            job.sections.len() > 1,
            "expected multiple formatted sections for JSON, got {}",
            job.sections.len()
        );
    }
```

And update:
- `test_all_lists_every_variant`: update expected length from 8 to 9.
- `test_from_extension_maps_supported_extensions`: add `assert_eq!(Language::from_extension("json"), Language::Json);` and `assert_eq!(Language::from_extension("jsonc"), Language::Json);`.
- `test_from_extension_is_case_insensitive`: add `assert_eq!(Language::from_extension("JSON"), Language::Json);` and `assert_eq!(Language::from_extension("JSONC"), Language::Json);`.
- `test_display_name_labels_each_language`: add `assert_eq!(Language::Json.display_name(), "JSON");`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -- highlighter::tests --skip test_save_tab_restores_active_tab_index`
Expected: Compilation failure because `Language::Json` variant does not exist yet.

- [ ] **Step 3: Implement `Language::Json` in `src/highlighter.rs`**

In `src/highlighter.rs`:
1. Add `Json` to `pub enum Language`:
   ```rust
   pub enum Language {
       PlainText,
       Rust,
       C,
       Cpp,
       Php,
       JavaScript,
       TypeScript,
       Html,
       Json,
   }
   ```
2. Update `Language::all()`:
   ```rust
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
   ```
3. Update `Language::from_extension`:
   ```rust
   "json" | "jsonc" => Language::Json,
   ```
4. Update `Language::display_name`:
   ```rust
   Language::Json => "JSON",
   ```
5. Update `Language::syntax_name`:
   ```rust
   Language::Json => "JSON",
   ```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -- highlighter::tests --skip test_save_tab_restores_active_tab_index`
Expected: PASS (all highlighter unit tests pass).

---

### Task 2: Add JSON Tab and Explorer Badge in `src/tab_bar.rs`

**Files:**
- Modify: `src/tab_bar.rs`
- Test: `src/tab_bar.rs`

**Interfaces:**
- Consumes: `Language::Json` from `src/highlighter.rs`
- Produces: `tab_bar::badge(Language::Json) -> ("{}", Some(Color32::from_rgb(0xCB, 0xB0, 0x41)))`

- [ ] **Step 1: Write failing test in `src/tab_bar.rs`**

In `src/tab_bar.rs` under `mod tests`:
Update `test_every_language_has_a_badge`:
```rust
        assert_eq!(
            badge(Language::Json),
            ("{}", Some(Color32::from_rgb(0xCB, 0xB0, 0x41)))
        );
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -- tab_bar::tests --skip test_save_tab_restores_active_tab_index`
Expected: Non-exhaustive pattern or badge test failure if not yet handled in `badge()`.

- [ ] **Step 3: Implement `badge` branch for `Language::Json`**

In `src/tab_bar.rs`:
```rust
pub fn badge(language: Language) -> (&'static str, Option<Color32>) {
    match language {
        Language::PlainText => ("TXT", None),
        Language::Rust => ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84))),
        Language::C => ("C", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Cpp => ("C++", Some(Color32::from_rgb(0xF3, 0x4B, 0x7D))),
        Language::Php => ("PHP", Some(Color32::from_rgb(0xA0, 0x74, 0xC4))),
        Language::JavaScript => ("JS", Some(Color32::from_rgb(0xCB, 0xCB, 0x41))),
        Language::TypeScript => ("TS", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Html => ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26))),
        Language::Json => ("{}", Some(Color32::from_rgb(0xCB, 0xB0, 0x41))),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -- tab_bar::tests --skip test_save_tab_restores_active_tab_index`
Expected: PASS.

---

### Task 3: Update Documentation and Run Full Verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update `README.md`**

In `README.md`, update line 8:
Change:
```markdown
- **Syntax highlighting** untuk Rust, C, C++, PHP, JavaScript, TypeScript dan HTML, dikenali dari ekstensi file.
```
To:
```markdown
- **Syntax highlighting** untuk Rust, C, C++, PHP, JavaScript, TypeScript, HTML dan JSON, dikenali dari ekstensi file.
```

- [ ] **Step 2: Run full test suite and clippy**

Run:
```bash
cargo test -- --skip test_save_tab_restores_active_tab_index
cargo clippy --all-targets -- -D warnings
```
Expected: PASS with 0 failures, 0 clippy warnings.
