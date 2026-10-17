# Horizontal Scrollbar and Word-Wrap Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let lines wider than the window scroll sideways, with a "Tampilan > Bungkus Baris" menu checkbox that turns word wrap on or off (default off).

**Architecture:** `render_editor` in `src/app.rs` takes a `word_wrap: bool`. Wrap off uses `ScrollArea::both()` and a layouter whose job has `wrap.max_width = f32::INFINITY`; wrap on keeps `ScrollArea::vertical()` and wraps to the width egui passes the layouter. The stock `TextEdit` always wraps to the viewport, so the plain path needs its own layouter when wrap is off. `src/highlighter.rs` is untouched.

**Tech Stack:** Rust (edition 2024), `eframe`/`egui` 0.37.

## Global Constraints

- Only `src/app.rs` changes. `src/highlighter.rs`, the layout cache and the background highlighting pass are not edited.
- `TextEditorApp` gains `pub word_wrap: bool`, `false` in `Default`. The setting is not persisted.
- The menu is a new `ui.menu_button("Tampilan", ...)` between "File" and "Help", holding one checkbox labelled exactly `Bungkus Baris`.
- Wrap off: `egui::ScrollArea::both()`. Wrap on: `egui::ScrollArea::vertical()` (the current behaviour).
- Wrap on must leave the plain path on the stock `TextEdit` layout (no custom layouter), so its behaviour is unchanged.
- No new dependencies. `cargo test`, `cargo clippy --all-targets` and `cargo build --release` stay clean. The 40 existing tests keep passing.

## Verified egui 0.37 facts (from a probe, 800x600 viewport, 3000-char line)

- `ScrollArea::both()` + `add_sized(ui.available_size(), text_edit.layouter(..))` with a job whose `wrap.max_width` is `f32::INFINITY`: content width becomes the longest line (about 23581 px), inner viewport stays 784 px, and a horizontal wheel event moves the offset (`offset.x == 300`). True for the highlighted path and for a plain `LayoutJob::simple` layouter.
- A stock `TextEdit` (no layouter) stays 784 px wide whatever `desired_width` is, because multiline text edits shrink to the available width. That is why wrap-off plain text needs a layouter.
- With wrap on (`ScrollArea::vertical()`, `max_width = wrap_width`), content width stays 784 px and a horizontal wheel event leaves `offset.x == 0`.
- The stock text-edit layouter (egui `text_edit/builder.rs:574-590`) colours text with `override_text_color`, else `widgets.inactive.text_color()` (`theme/default_style.rs:144-148`), sets `keep_trailing_whitespace = true`, and sets `line_height = row_height + extra_text_line_spacing` on every section. The wrap-off plain layouter copies all three.
- `State::clamped_offset()` is the public way to read the scroll offset. The content size is not public on `State`, so tests assert on the offset.

---

### Task 1: Word-wrap toggle and horizontal scrolling

**Files:**
- Modify: `src/app.rs` (struct `TextEditorApp` and its `Default`, `render_editor`, the File/Help menu area in `ui`, the central-panel call sites, and `mod tests`)

**Interfaces:**
- Consumes (existing, unchanged): `HighlighterLayouter::frame_job(&mut self, &str, &egui::Context) -> Arc<egui::text::LayoutJob>`, `SyntaxHighlighter::{new, layouter}`, `egui::containers::scroll_area::State::clamped_offset`.
- Produces: `TextEditorApp::word_wrap: bool` and `fn render_editor(ui: &mut egui::Ui, text: &mut String, layouter: Option<&mut HighlighterLayouter<'_>>, word_wrap: bool)`.

- [ ] **Step 1: Update the existing tests and write the new failing tests**

In `mod tests` of `src/app.rs`, replace the helper `scroll_editor_with_wheel` with a delta-taking version, keeping the old name as a thin wrapper:

```rust
    /// Scrolls the editor with a real wheel event: the pointer hovers over the
    /// editor and the wheel moves the content by `delta` points (negative moves
    /// the content up or left). Runs enough frames for the animated scroll to
    /// settle.
    fn scroll_editor_with_wheel_delta(
        ctx: &egui::Context,
        delta: egui::Vec2,
        mut body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        let pointer = egui::Event::PointerMoved(egui::pos2(100.0, 100.0));
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame_with_events(ctx, vec![pointer, wheel], &mut body);
        for _ in 0..30 {
            run_frame_with_events(ctx, Vec::new(), &mut body);
        }
        editor_scroll_area(ctx).expect("ScrollArea must survive")
    }

    /// Scrolls the editor down with the mouse wheel.
    fn scroll_editor_with_wheel(
        ctx: &egui::Context,
        body: impl FnMut(&mut egui::Ui),
    ) -> egui::containers::scroll_area::State {
        scroll_editor_with_wheel_delta(ctx, egui::vec2(0.0, -200.0), body)
    }
```

Add `, false` as a fourth argument to every existing `super::render_editor(...)` call in `mod tests`: the two calls in `test_render_editor_plain_path_is_scrollable`, the call in `test_render_editor_highlighted_path_is_scrollable`, and the two calls in `test_render_editor_round_trips_text_on_both_paths`. For example `super::render_editor(ui, &mut text, None)` becomes `super::render_editor(ui, &mut text, None, false)` and `super::render_editor(ui, &mut text, Some(&mut layouter))` becomes `super::render_editor(ui, &mut text, Some(&mut layouter), false)`.

Then append these tests before the closing `}` of `mod tests`:

```rust
    /// One long line, far wider than the 800 point test viewport.
    fn wide_document() -> String {
        format!("let a = \"{}\";\nshort\nline3", "x".repeat(3000))
    }

    /// Renders `text` through `render_editor` on the plain or highlighted path
    /// and returns the scroll state after a horizontal wheel scroll of 300
    /// points. The buffer is asserted unchanged.
    fn scroll_sideways(highlighted: bool, word_wrap: bool) -> egui::containers::scroll_area::State {
        let ctx = egui::Context::default();
        let original = wide_document();
        let mut text = original.clone();
        let mut highlighter = SyntaxHighlighter::new();

        let area = scroll_editor_with_wheel_delta(&ctx, egui::vec2(-300.0, 0.0), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if highlighted {
                    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                    let mut layouter = highlighter.layouter(Language::Rust, font_id);
                    super::render_editor(ui, &mut text, Some(&mut layouter), word_wrap);
                } else {
                    super::render_editor(ui, &mut text, None, word_wrap);
                }
            });
        });
        assert_eq!(text, original, "scrolling must not alter the buffer");
        area
    }

    #[test]
    fn test_word_wrap_defaults_to_off() {
        assert!(!TextEditorApp::default().word_wrap);
    }

    /// Wrap off: a line wider than the window scrolls sideways on the plain path.
    #[test]
    fn test_plain_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(false, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap off: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_scrolls_horizontally_without_wrap() {
        let area = scroll_sideways(true, false);
        assert!(
            area.clamped_offset().x > 0.0,
            "a long line must scroll sideways, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the text wraps to the window, so there is nothing to scroll
    /// sideways, on the plain path.
    #[test]
    fn test_plain_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(false, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }

    /// Wrap on: the same on the highlighted path.
    #[test]
    fn test_highlighted_path_does_not_scroll_horizontally_with_wrap() {
        let area = scroll_sideways(true, true);
        assert_eq!(
            area.clamped_offset().x,
            0.0,
            "wrapped text must fit the window width, offset {:?}",
            area.clamped_offset()
        );
    }
```

Also extend `test_render_editor_round_trips_text_on_both_paths` so it covers both wrap settings: wrap its two blocks in `for word_wrap in [false, true] { ... }` and pass `word_wrap` instead of `false` to `render_editor`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin rred_text_editor app::tests 2>&1 | tail -20`
Expected: compile errors `this function takes 3 arguments but 4 arguments were supplied` for `render_editor`, and `no field named word_wrap` on `TextEditorApp`.

- [ ] **Step 3: Add the `word_wrap` field**

In `struct TextEditorApp`, after `pub highlighter: SyntaxHighlighter,` add:

```rust
    /// Wrap long lines to the window width. Off by default: lines run on and
    /// the editor scrolls sideways instead.
    pub word_wrap: bool,
```

In `impl Default for TextEditorApp`, after `highlighter: SyntaxHighlighter::new(),` add:

```rust
            word_wrap: false,
```

- [ ] **Step 4: Rewrite `render_editor`**

Replace the doc comment and body of `render_editor` (from `/// Renders the multiline editor` through its closing `}`) with:

```rust
/// Renders the multiline editor, optionally through a syntax highlighter.
///
/// When `layouter` is `None` the text renders plain, which is what oversized
/// documents get. Lines are joined back together with `\n` separators so the
/// buffer round-trips exactly, keeping the caret and selection aligned.
///
/// With `word_wrap` on, long lines wrap to the window width and only vertical
/// scrolling is needed. With it off, lines never wrap and the editor scrolls
/// sideways as well. Wrap off needs a layouter on the plain path too: a stock
/// multiline `TextEdit` always shrinks to the available width, so it wraps
/// whatever `desired_width` says.
///
/// The editor is wrapped in a `ScrollArea`: `TextEdit` has no scrolling
/// of its own for multiline input (`clip_text` is `false`, so its `text_offset`
/// handling never runs) and its only `ui.scroll_to_rect` call writes to
/// `PassState::scroll_target`, which only `ScrollArea::show` drains. Without this
/// wrapper a document taller than the viewport cannot be scrolled and, with the
/// default `cursor_at_end`, the caret lands off-screen and stays unreachable.
fn render_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    mut layouter: Option<&mut HighlighterLayouter<'_>>,
    word_wrap: bool,
) {
    let scroll_area = if word_wrap {
        egui::ScrollArea::vertical()
    } else {
        egui::ScrollArea::both()
    };
    scroll_area.show(ui, |ui| {
        let text_edit = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace)
            .lock_focus(true)
            .desired_width(f32::INFINITY);

        match layouter.as_mut() {
            Some(highlighter) => {
                // `TextEdit` calls this every frame. `frame_job` reuses the
                // cached result, and highlights large documents on a background
                // thread (plain text meanwhile) so the window never freezes.
                // Wrapping is a layout concern, so it is applied to the clone and
                // never reaches the cache.
                let mut lay = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = (*highlighter.frame_job(buf.as_str(), ui.ctx())).clone();
                    job.wrap.max_width = if word_wrap { wrap_width } else { f32::INFINITY };
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                ui.add_sized(ui.available_size(), text_edit.layouter(&mut lay));
            }
            None if word_wrap => {
                ui.add_sized(ui.available_size(), text_edit);
            }
            None => {
                // Mirror egui's stock `TextEdit` layouter so wrap-off plain text
                // looks identical to the wrapped text: the colour is the
                // `inactive` widget colour (a text edit does not brighten on
                // hover), unless the theme overrides it, and every section gets
                // the same line height.
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let color = ui
                    .visuals()
                    .override_text_color
                    .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
                let line_height = ui.fonts_mut(|f| f.row_height(&font_id))
                    + ui.spacing().extra_text_line_spacing;
                let mut lay = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
                    let mut job = egui::text::LayoutJob::simple(
                        buf.as_str().to_owned(),
                        font_id.clone(),
                        color,
                        f32::INFINITY,
                    );
                    // Typing a trailing space must not make the caret jump.
                    job.keep_trailing_whitespace = true;
                    for section in &mut job.sections {
                        section.format.line_height = Some(line_height);
                    }
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                ui.add_sized(ui.available_size(), text_edit.layouter(&mut lay));
            }
        }
    });
}
```

- [ ] **Step 5: Pass `word_wrap` from the central panel**

In `fn ui`, section "4. Central Panel", add `word_wrap` to the destructuring and to both calls:

```rust
            let Self {
                text_content,
                highlighter,
                active_language,
                language_override,
                word_wrap,
                ..
            } = self;
            let language = language_override.unwrap_or(*active_language);

            if too_large {
                render_editor(ui, text_content, None, *word_wrap);
            } else {
                let mut layouter = highlighter.layouter(language, font_id);
                render_editor(ui, text_content, Some(&mut layouter), *word_wrap);
            }
```

- [ ] **Step 6: Add the "Tampilan" menu**

In the top panel, between the end of `ui.menu_button("File", ...)` and `// Menu Help`, insert:

```rust
                // Menu Tampilan
                ui.menu_button("Tampilan", |ui| {
                    ui.checkbox(&mut self.word_wrap, "Bungkus Baris");
                });

```

- [ ] **Step 7: Run the new tests**

Run: `cargo test --bin rred_text_editor app::tests 2>&1 | tail -25`
Expected: all `app::tests` pass, including the five new tests and the updated scroll and round-trip tests.

- [ ] **Step 8: Prove the horizontal tests can fail**

Temporarily change `let scroll_area = if word_wrap {` to `let scroll_area = if true {` in `render_editor`, run `cargo test --bin rred_text_editor scrolls_horizontally_without_wrap 2>&1 | tail -15`, and confirm both tests FAIL with "a long line must scroll sideways". Then revert the change and re-run to confirm they pass again.

- [ ] **Step 9: Full verification**

Run, in order:
- `cargo test 2>&1 | grep -E "test result|FAILED|panicked"` — expect `test result: ok. 45 passed; 0 failed` (40 existing + 5 new).
- `cargo clippy --all-targets 2>&1 | tail -5` — expect no warnings.
- `cargo build --release 2>&1 | tail -3` — expect `Finished`.
- `git status --short` — expect only `src/app.rs` modified.

- [ ] **Step 10: Commit**

```bash
git add src/app.rs
git commit -m "feat(editor): add horizontal scrollbar and word-wrap toggle"
```
