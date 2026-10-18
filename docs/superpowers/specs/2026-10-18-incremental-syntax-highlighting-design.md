# Incremental Syntax Highlighting Design

## Problem
In `red-text-editor`, whenever the user types a character in an open file, the entire buffer is split and highlighted from line 0 through the end of the file. For files with hundreds or thousands of lines, this causes noticeable latency, high CPU usage, and frame drops.

## Solution Overview
Implement line-level incremental syntax highlighting with `syntect::parsing::ParseState` and `syntect::highlighting::HighlightState` caching per line. When text is modified:
1. Identify the prefix lines that have not changed (`prefix_idx`).
2. Identify suffix lines that have not changed (`suffix_count`).
3. Resume parsing starting at `prefix_idx` using the cached state from `prefix_idx - 1`.
4. Apply early termination when the parse state matches the previous state at a suffix line.
5. Reconstruct the complete `egui::text::LayoutJob` using the cached lines.
6. Store the line cache per tab (`DocumentTab`), so switching tabs preserves each tab's highlight cache.

## Data Structures

### `src/highlighter.rs`

```rust
use syntect::parsing::ParseState;
use syntect::highlighting::HighlightState;

#[derive(Clone)]
pub struct LineHighlightCache {
    pub text: String,
    pub layout: egui::text::LayoutJob,
    pub parse_state: ParseState,
    pub highlight_state: HighlightState,
}

#[derive(Default, Clone)]
pub struct DocumentHighlightCache {
    pub language: Option<Language>,
    pub font_id: Option<egui::FontId>,
    pub lines: Vec<LineHighlightCache>,
    pub full_job: Option<Arc<egui::text::LayoutJob>>,
}
```

### `src/app.rs`

Extend `DocumentTab`:
```rust
pub struct DocumentTab {
    pub text_content: String,
    pub saved_content: String,
    pub current_path: Option<PathBuf>,
    pub active_language: Language,
    pub language_override: Option<Language>,
    pub highlight_cache: DocumentHighlightCache,
}
```

## Highlighting Algorithm

1. **Validation & Invalidation**:
   - Check if `cache.language != Some(current_language)` or `cache.font_id != Some(current_font_id)`.
   - If changed, clear `cache.lines` and `cache.full_job`.

2. **Diffing**:
   - Split input string into lines: `new_lines: Vec<&str>`.
   - Compute `prefix_match_count`: number of lines from the start matching `cache.lines[i].text`.
   - Compute `suffix_match_count`: number of lines from the end matching `cache.lines[cache.lines.len() - 1 - i].text`, bounded by `prefix_match_count`.

3. **Incremental Processing**:
   - Retrieve initial `(ParseState, HighlightState)`:
     - If `prefix_match_count == 0`: initialize new states from `syntax` and `theme`.
     - Else: clone states from `cache.lines[prefix_match_count - 1]`.
   - Process changed lines starting at `prefix_match_count`:
     - Highlight line using `HighlightLines` or manual state advance (`ParseState::parse_line`, `RangedHighlightIterator`).
     - Check for early termination: if current index is in the suffix region and `(new_parse_state == old_parse_state)`, stop re-highlighting and keep the rest of the suffix lines.

4. **Reassembly**:
   - Build a merged `LayoutJob` from all cached line jobs.
   - Cache `full_job = Arc::new(merged_job)`.

## Testing & Verification
- Unit test: typing a character on line 5 of a 100-line file only recalculates line 5 (and doesn't reparse lines 6..100 if state hasn't changed).
- Unit test: typing `/*` (opening a multi-line comment) continues recalculating subsequent lines until comment close `*/`.
- Unit test: switching tabs preserves tab-specific line cache.
- Regression test: ensure all existing tests pass (`cargo test -- --skip test_save_tab_restores_active_tab_index`).
