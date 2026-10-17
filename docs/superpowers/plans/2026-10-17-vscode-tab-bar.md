# VS Code-Style Tab Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the boxed tab bar with a flat VS Code "Dark Modern" / "Light Modern" tab strip that joins the editor below it.

**Architecture:** A new `src/tab_bar.rs` paints the strip with egui's `Painter` and reports clicks as a `TabBarAction`; it only sees `TabView` values, never `DocumentTab`. `app.rs` builds the views, applies the action, and draws the central panel and editor without frames so the active tab and the editor share one background.

**Tech Stack:** Rust 2024, eframe/egui 0.37.

Spec: `docs/superpowers/specs/2026-10-17-vscode-tab-bar-design.md`.

---

## File Structure

- Create `src/tab_bar.rs` — palette, language badge, close-slot rule, `show()` and its tests.
- Modify `src/main.rs` — register `mod tab_bar;`.
- Modify `src/app.rs` — `DocumentTab::tab_view`, `render_tab_bar` delegates to `tab_bar::show` and returns its response, frameless central panel and editor, rewritten tab bar test.

---

### Task 1: Palette, badges and the close-slot rule

**Files:**
- Create: `src/tab_bar.rs`
- Modify: `src/main.rs:1-4`

- [ ] **Step 1: Register the module and write the failing tests**

In `src/main.rs`, add `mod tab_bar;` after `mod modals;`.

Create `src/tab_bar.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dark_palette_matches_vscode_dark_modern() {
        let p = TabPalette::DARK;
        assert_eq!(p.strip, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_active, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.tab_hover, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.border, Color32::from_rgb(0x2B, 0x2B, 0x2B));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x78, 0xD4));
        assert_eq!(p.fg_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x9D, 0x9D, 0x9D));
    }

    #[test]
    fn test_light_palette_matches_vscode_light_modern() {
        let p = TabPalette::LIGHT;
        assert_eq!(p.strip, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.tab_hover, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.border, Color32::from_rgb(0xE5, 0xE5, 0xE5));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x5F, 0xB8));
        assert_eq!(p.fg_active, Color32::from_rgb(0x3B, 0x3B, 0x3B));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x86, 0x86, 0x86));
    }

    #[test]
    fn test_palette_follows_egui_theme() {
        assert_eq!(TabPalette::for_visuals(&egui::Visuals::dark()), TabPalette::DARK);
        assert_eq!(TabPalette::for_visuals(&egui::Visuals::light()), TabPalette::LIGHT);
    }

    #[test]
    fn test_every_language_has_a_badge() {
        for lang in Language::all() {
            assert!(!badge(*lang).0.is_empty(), "no badge for {lang:?}");
        }
        assert_eq!(badge(Language::Rust), ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84))));
        assert_eq!(badge(Language::Html), ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26))));
        assert_eq!(badge(Language::PlainText), ("TXT", None));
    }

    /// The slot under the pointer always offers `×`; otherwise unsaved
    /// changes win over "active or hovered", so an active dirty tab keeps
    /// its dot.
    #[test]
    fn test_close_slot_rule() {
        use CloseSlot::*;
        // (slot hovered, dirty, active, tab hovered) -> slot
        assert_eq!(close_slot(true, true, false, true), CrossHovered);
        assert_eq!(close_slot(true, false, false, true), CrossHovered);
        assert_eq!(close_slot(false, true, true, false), Dot);
        assert_eq!(close_slot(false, true, false, true), Dot);
        assert_eq!(close_slot(false, false, true, false), Cross);
        assert_eq!(close_slot(false, false, false, true), Cross);
        assert_eq!(close_slot(false, false, false, false), Empty);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --quiet tab_bar`
Expected: compile errors — `TabPalette`, `badge`, `close_slot`, `CloseSlot` not found.

- [ ] **Step 3: Implement**

Put this above the test module in `src/tab_bar.rs`:

```rust
//! VS Code-style tab strip drawn above the editor.
//!
//! Knows nothing about documents: `app.rs` describes each tab as a
//! [`TabView`] and applies the returned [`TabBarAction`].

use eframe::egui::{self, Color32};

use crate::highlighter::Language;

/// What the strip needs to know about one open document.
#[derive(Clone, Debug, PartialEq)]
pub struct TabView {
    /// File name, or the placeholder for a new document. No dirty marker:
    /// the close slot shows that.
    pub title: String,
    /// Full path shown on hover; `None` for a document never saved.
    pub tooltip: Option<String>,
    pub language: Language,
    pub dirty: bool,
}

/// Colours of the strip, after VS Code's "Dark Modern" and "Light Modern".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabPalette {
    pub strip: Color32,
    pub tab_inactive: Color32,
    /// Also the editor background, so the active tab joins the editor.
    pub tab_active: Color32,
    pub tab_hover: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub fg_active: Color32,
    pub fg_inactive: Color32,
    /// Rounded box behind the `×` under the pointer.
    pub close_hover: Color32,
}

impl TabPalette {
    pub const DARK: TabPalette = TabPalette {
        strip: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_inactive: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_active: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        tab_hover: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        border: Color32::from_rgb(0x2B, 0x2B, 0x2B),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        fg_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        fg_inactive: Color32::from_rgb(0x9D, 0x9D, 0x9D),
        close_hover: Color32::from_rgb(0x31, 0x31, 0x31),
    };

    pub const LIGHT: TabPalette = TabPalette {
        strip: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_inactive: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        tab_hover: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
        fg_active: Color32::from_rgb(0x3B, 0x3B, 0x3B),
        fg_inactive: Color32::from_rgb(0x86, 0x86, 0x86),
        close_hover: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    };

    /// The palette matching egui's current light or dark mode.
    pub fn for_visuals(visuals: &egui::Visuals) -> TabPalette {
        if visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Short coloured label shown before a tab's name. A `None` colour means
/// "use the inactive foreground".
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
    }
}

/// What the close slot at the right of a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseSlot {
    Empty,
    Dot,
    Cross,
    /// `×` on a rounded hover box.
    CrossHovered,
}

/// VS Code's rule for the close slot.
fn close_slot(slot_hovered: bool, dirty: bool, active: bool, tab_hovered: bool) -> CloseSlot {
    if slot_hovered {
        CloseSlot::CrossHovered
    } else if dirty {
        CloseSlot::Dot
    } else if active || tab_hovered {
        CloseSlot::Cross
    } else {
        CloseSlot::Empty
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --quiet tab_bar`
Expected: 5 passed. (`dead_code` warnings for `TabView` and `close_slot` are expected until Task 2.)

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/tab_bar.rs
git commit -m "feat(tab_bar): add VS Code palette, language badges and close-slot rule"
```

---

### Task 2: Paint the strip and report clicks

**Files:**
- Modify: `src/tab_bar.rs`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `src/tab_bar.rs`:

```rust
    fn view(title: &str, dirty: bool) -> TabView {
        TabView {
            title: title.to_owned(),
            tooltip: None,
            language: Language::Rust,
            dirty,
        }
    }

    /// One headless frame of the strip; returns its response and the shapes.
    fn run(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        tabs: &[TabView],
        active: usize,
    ) -> (TabBarResponse, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut response = None;
        let mut output = ctx.run_ui(input, |ui| response = Some(show(ui, tabs, active)));
        output.textures_delta.clear();
        (response.expect("show ran"), output.shapes)
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    /// A filled circle drawn inside `rect`: the dirty dot.
    fn dot_in(shapes: &[egui::epaint::ClippedShape], rect: egui::Rect) -> bool {
        shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Circle(circle)
                if rect.contains(circle.center) && circle.fill != Color32::TRANSPARENT)
        })
    }

    #[test]
    fn test_tabs_are_flush_and_full_height() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        assert_eq!(bar.tab_rects.len(), 2);
        assert_eq!(bar.close_rects.len(), 2);
        assert_eq!(bar.tab_rects[0].height(), TAB_HEIGHT);
        assert_eq!(bar.tab_rects[0].right(), bar.tab_rects[1].left(), "tabs must touch");
        assert_eq!(bar.tab_rects[1].right(), bar.new_tab_rect.left());
        assert!(bar.tab_rects[1].contains_rect(bar.close_rects[1]));
    }

    #[test]
    fn test_clicking_a_tab_selects_it() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 1);
        let (bar, _) = run(&ctx, click(bar.tab_rects[0].center()), &tabs, 1);
        assert_eq!(bar.action, Some(TabBarAction::Select(0)));
    }

    #[test]
    fn test_clicking_a_close_slot_closes_that_tab() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        let (bar, _) = run(&ctx, click(bar.close_rects[1].center()), &tabs, 0);
        assert_eq!(bar.action, Some(TabBarAction::Close(1)));
    }

    #[test]
    fn test_clicking_plus_asks_for_a_new_tab() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false)];
        let (bar, _) = run(&ctx, Vec::new(), &tabs, 0);
        let (bar, _) = run(&ctx, click(bar.new_tab_rect.center()), &tabs, 0);
        assert_eq!(bar.action, Some(TabBarAction::New));
    }

    #[test]
    fn test_dirty_tab_shows_a_dot_until_its_slot_is_hovered() {
        let ctx = egui::Context::default();
        let tabs = [view("a.rs", false), view("b.rs", true)];
        let (bar, shapes) = run(&ctx, Vec::new(), &tabs, 0);
        assert!(dot_in(&shapes, bar.close_rects[1]), "dirty tab must show a dot");
        assert!(!dot_in(&shapes, bar.close_rects[0]), "clean tab has no dot");

        let hover = vec![egui::Event::PointerMoved(bar.close_rects[1].center())];
        let (bar, shapes) = run(&ctx, hover, &tabs, 0);
        assert!(!dot_in(&shapes, bar.close_rects[1]), "hovered slot shows × instead");
        assert_eq!(bar.action, None, "hovering alone does nothing");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --quiet tab_bar`
Expected: compile errors — `show`, `TabBarResponse`, `TabBarAction`, `TAB_HEIGHT` not found.

- [ ] **Step 3: Implement**

Change the `use` line at the top of `src/tab_bar.rs` to:

```rust
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2, pos2, vec2};
```

Add below `close_slot`:

```rust
/// Height of the strip and of every tab in it.
pub const TAB_HEIGHT: f32 = 35.0;
const PADDING_X: f32 = 10.0;
const BADGE_GAP: f32 = 6.0;
const CLOSE_GAP: f32 = 8.0;
const CLOSE_SIZE: f32 = 16.0;

/// What the user asked the strip to do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    New,
}

/// One frame of the strip: what was clicked, and where each part was drawn.
#[derive(Clone, Debug)]
pub struct TabBarResponse {
    pub action: Option<TabBarAction>,
    // Only the tests read where things were drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub tab_rects: Vec<Rect>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub close_rects: Vec<Rect>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub new_tab_rect: Rect,
}

/// Draws the strip across the available width and reports what was clicked.
pub fn show(ui: &mut egui::Ui, tabs: &[TabView], active: usize) -> TabBarResponse {
    let palette = TabPalette::for_visuals(ui.visuals());

    // Background and bottom border for the whole width, including the empty
    // part after the last tab. The active tab paints over the border.
    let top = ui.available_rect_before_wrap();
    let strip = Rect::from_min_size(top.min, vec2(top.width(), TAB_HEIGHT));
    ui.painter().rect_filled(strip, CornerRadius::ZERO, palette.strip);
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, palette.border));

    egui::ScrollArea::horizontal()
        .id_salt("tab_bar_scroll")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let mut response = TabBarResponse {
                    action: None,
                    tab_rects: Vec::with_capacity(tabs.len()),
                    close_rects: Vec::with_capacity(tabs.len()),
                    new_tab_rect: Rect::NOTHING,
                };
                for (index, tab) in tabs.iter().enumerate() {
                    let (rect, close_rect, action) =
                        show_tab(ui, &palette, index, tab, index == active);
                    response.tab_rects.push(rect);
                    response.close_rects.push(close_rect);
                    response.action = response.action.or(action);
                }

                let (rect, new_tab) = ui.allocate_exact_size(Vec2::splat(TAB_HEIGHT), Sense::click());
                if new_tab.hovered() {
                    // Leave the strip's bottom border visible.
                    let fill = rect.with_max_y(rect.bottom() - 1.0);
                    ui.painter().rect_filled(fill, CornerRadius::ZERO, palette.tab_hover);
                }
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(16.0),
                    palette.fg_inactive,
                );
                if new_tab.clicked() {
                    response.action = Some(TabBarAction::New);
                }
                response.new_tab_rect = rect;
                response
            })
            .inner
        })
        .inner
}

/// Draws one tab; returns its rect, its close slot's rect and any click.
fn show_tab(
    ui: &mut egui::Ui,
    palette: &TabPalette,
    index: usize,
    tab: &TabView,
    active: bool,
) -> (Rect, Rect, Option<TabBarAction>) {
    let fg = if active { palette.fg_active } else { palette.fg_inactive };
    let (badge_text, badge_color) = badge(tab.language);
    let badge_galley = ui.painter().layout_no_wrap(
        badge_text.to_owned(),
        FontId::monospace(10.0),
        badge_color.unwrap_or(palette.fg_inactive),
    );
    let name_galley = ui
        .painter()
        .layout_no_wrap(tab.title.clone(), FontId::proportional(13.0), fg);

    let width = PADDING_X
        + badge_galley.size().x
        + BADGE_GAP
        + name_galley.size().x
        + CLOSE_GAP
        + CLOSE_SIZE
        + PADDING_X;
    let (rect, body) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
    let close_rect = Rect::from_center_size(
        pos2(rect.right() - PADDING_X - CLOSE_SIZE / 2.0, rect.center().y),
        Vec2::splat(CLOSE_SIZE),
    );
    // Registered after the body, so it wins clicks inside the slot.
    let close = ui.interact(close_rect, body.id.with("close"), Sense::click());
    let tab_hovered = body.hovered() || close.hovered();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if active {
            palette.tab_active
        } else if tab_hovered {
            palette.tab_hover
        } else {
            palette.tab_inactive
        };
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        let border = Stroke::new(1.0, palette.border);
        painter.vline(rect.right() - 0.5, rect.y_range(), border);
        if active {
            painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, palette.accent));
        } else {
            painter.hline(rect.x_range(), rect.bottom() - 0.5, border);
        }

        let badge_pos = pos2(
            rect.left() + PADDING_X,
            rect.center().y - badge_galley.size().y / 2.0,
        );
        let name_pos = pos2(
            badge_pos.x + badge_galley.size().x + BADGE_GAP,
            rect.center().y - name_galley.size().y / 2.0,
        );
        painter.galley(badge_pos, badge_galley, fg);
        painter.galley(name_pos, name_galley, fg);

        let slot = close_slot(close.hovered(), tab.dirty, active, tab_hovered);
        paint_close_slot(painter, palette, close_rect, slot, fg);
    }

    let body = match &tab.tooltip {
        Some(path) => body.on_hover_text(path),
        None => body,
    };
    let action = if close.clicked() {
        Some(TabBarAction::Close(index))
    } else if body.clicked() {
        Some(TabBarAction::Select(index))
    } else {
        None
    };
    (rect, close_rect, action)
}

/// The dot and cross are drawn as shapes, not glyphs, so they never depend on
/// what the bundled fonts cover.
fn paint_close_slot(
    painter: &egui::Painter,
    palette: &TabPalette,
    rect: Rect,
    slot: CloseSlot,
    color: Color32,
) {
    let center = rect.center();
    match slot {
        CloseSlot::Empty => {}
        CloseSlot::Dot => {
            painter.circle_filled(center, 4.0, color);
        }
        CloseSlot::Cross | CloseSlot::CrossHovered => {
            if slot == CloseSlot::CrossHovered {
                painter.rect_filled(rect, CornerRadius::same(4), palette.close_hover);
            }
            let d = 3.5;
            let stroke = Stroke::new(1.3, color);
            painter.line_segment([center + vec2(-d, -d), center + vec2(d, d)], stroke);
            painter.line_segment([center + vec2(-d, d), center + vec2(d, -d)], stroke);
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --quiet tab_bar`
Expected: 10 passed.

- [ ] **Step 5: Commit**

```bash
git add src/tab_bar.rs
git commit -m "feat(tab_bar): paint VS Code-style tab strip and report clicks"
```

---

### Task 3: Use the strip in the app and join it to the editor

**Files:**
- Modify: `src/app.rs` (`DocumentTab`, `render_tab_bar` at ~433, `render_editor` at ~511, central panel at ~678, tab bar test at ~1290)

- [ ] **Step 1: Write the failing tests**

In `src/app.rs` tests, add a click helper next to `run_frame_with_events`:

```rust
    /// Moves the pointer to `pos` and clicks the primary button there.
    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }
```

Add:

```rust
    #[test]
    fn test_tab_view_carries_dirty_state_instead_of_a_title_marker() {
        let mut tab = DocumentTab::default_new();
        tab.text_content.push('!');
        tab.active_language = Language::Php;
        let view = tab.tab_view();
        assert_eq!(view.title, "[Dokumen Baru]");
        assert!(view.dirty);
        assert_eq!(view.language, Language::Php);
        assert_eq!(view.tooltip, None);

        tab.current_path = Some(PathBuf::from("/tmp/x/index.php"));
        assert_eq!(tab.tab_view().tooltip.as_deref(), Some("/tmp/x/index.php"));
    }
```

Replace the whole `test_tab_bar_ui_rendering_and_interaction` with:

```rust
    #[test]
    fn test_tab_bar_ui_rendering_and_interaction() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.open_new_tab(); // Tab 0 and Tab 1, both blank
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_index, 1);

        let mut bar = None;
        run_frame_with_events(&ctx, Vec::new(), |ui| bar = Some(app.render_tab_bar(ui)));
        let tab_0 = bar.take().unwrap().tab_rects[0].center();

        run_frame_with_events(&ctx, click_events(tab_0), |ui| bar = Some(app.render_tab_bar(ui)));
        assert_eq!(app.active_tab_index, 0, "clicking tab 0 must activate it");
        let close_1 = bar.take().unwrap().close_rects[1].center();

        run_frame_with_events(&ctx, click_events(close_1), |ui| bar = Some(app.render_tab_bar(ui)));
        assert_eq!(app.tabs.len(), 1, "clicking tab 1's close slot must close it");
        assert_eq!(app.active_tab_index, 0);

        run_frame_with_events(&ctx, Vec::new(), |ui| bar = Some(app.render_tab_bar(ui)));
        let plus = bar.take().unwrap().new_tab_rect.center();

        run_frame_with_events(&ctx, click_events(plus), |ui| {
            app.render_tab_bar(ui);
        });
        assert_eq!(app.tabs.len(), 2, "clicking + must open a new tab");
        assert_eq!(app.active_tab_index, 1);
    }
```

(Keep any assertions that followed the `+` click in the old test if they check further behaviour; drop only the hard-coded coordinates.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --quiet test_tab_`
Expected: compile errors — no method `tab_view`, `render_tab_bar` returns `()`.

- [ ] **Step 3: Implement**

In `src/app.rs` add `use crate::tab_bar;` to the imports.

Add to `impl DocumentTab`:

```rust
    /// How this document appears in the tab strip.
    pub fn tab_view(&self) -> tab_bar::TabView {
        tab_bar::TabView {
            title: self.file_display_name(),
            tooltip: self.current_path.as_ref().map(|p| p.display().to_string()),
            language: self.effective_language(),
            dirty: self.is_dirty(),
        }
    }
```

Replace `render_tab_bar` with:

```rust
    /// Draws the tab strip and applies what the user clicked.
    pub fn render_tab_bar(&mut self, ui: &mut egui::Ui) -> tab_bar::TabBarResponse {
        let views: Vec<tab_bar::TabView> = self.tabs.iter().map(DocumentTab::tab_view).collect();
        let response = tab_bar::show(ui, &views, self.active_tab_index);
        match response.action {
            Some(tab_bar::TabBarAction::Select(index)) => self.active_tab_index = index,
            Some(tab_bar::TabBarAction::Close(index)) => self.trigger_close_tab(index),
            Some(tab_bar::TabBarAction::New) => self.open_new_tab(),
            None => {}
        }
        response
    }
```

In `render_editor`, add a frame to the `TextEdit` builder (after `.min_size(...)`):

```rust
            // No outline or rounded fill: the editor sits on the central
            // panel's background, the same colour as the active tab.
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 6)))
```

In `eframe::App::ui`, replace the central panel opening and drop the separator:

```rust
        // 4. Central Panel (Main Text Editor Area)
        // No margin and the active tab's colour, so the active tab and the
        // editor below it read as one surface, as in VS Code.
        let editor_fill = tab_bar::TabPalette::for_visuals(ui.visuals()).tab_active;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(editor_fill))
            .show(ui, |ui| {
                self.render_tab_bar(ui);
```

(remove the `ui.separator();` that followed `self.render_tab_bar(ui);`, and close the extra `.show(` chain formatting accordingly; the body is unchanged).

- [ ] **Step 4: Run the full suite and clippy**

Run: `cargo test --quiet && cargo clippy --all-targets --quiet`
Expected: all tests pass, no clippy warnings. Note: `test_save_tab_restores_active_tab_index` opens a real Save-As dialog on a desktop session; cancel it if it appears.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(editor): use VS Code-style tab strip and join it to the editor"
```

---

### Task 4: Visual check

- [ ] **Step 1:** Temporarily make `main.rs` load `$RRED_DEBUG_OPEN` into the first tab, open a Rust file, add a second (dirty) tab, screenshot with `spectacle -b -n -a -o <file>`, then restore `main.rs` with `git checkout -- src/main.rs`.
- [ ] **Step 2:** Confirm: flat strip, blue top line on the active tab, active tab joins the editor, coloured badge, dot on the dirty tab, `+` after the last tab.
