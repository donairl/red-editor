# Color Themes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** View > Theme offers System plus seven themes that recolor syntax highlighting, the custom-painted UI and egui's widgets live, and the choice is remembered.

**Architecture:** A new `src/theme.rs` owns `ThemeId`, `Base` and `const Theme` values built from the existing Modern palettes or derived from a few base colors. `app.rs` calls `theme::apply(ctx, choice)` at the start of every frame; it resolves `System`, stores the resolved id in egui's memory and swaps egui's visuals when it changes. Painting modules read `theme::active(ctx)`. The syntect theme name becomes part of the highlighter's cache keys, so a switch recolors on the next frame.

**Tech Stack:** Rust 2024, eframe/egui 0.37, syntect 5.3 (bundled themes).

Spec: `docs/superpowers/specs/2026-10-21-color-themes-design.md`.

One deviation from the spec, recorded in Task 6: instead of `layouter()` taking the theme name, `SyntaxHighlighter::set_theme(name)` stores it. Same behaviour, and the ~40 existing `layouter(lang, font)` calls stay as they are.

---

## File Structure

- Create `src/theme.rs`: theme ids, base colors, theme constants, `apply`/`active`, egui visuals.
- Modify `src/main.rs`: `mod theme;`.
- Modify `src/highlighter.rs`: theme name instead of `THEME_NAME`; theme in cache keys; `set_theme`.
- Modify `src/tab_bar.rs`, `src/activity_bar.rs`, `src/file_browser.rs`, `src/gutter.rs`, `src/terminal.rs`: read palettes from `theme::active`; drop `for_visuals`.
- Modify `src/app.rs`: `theme` field, apply per frame, View > Theme menu, save/restore.
- Modify `assets/lang/{en,id,de,es,zh}.json`: `menu.theme`, `theme.system`.
- Modify `README.md`, `README.id.md`: View menu row.

---

### Task 1: Theme data (`src/theme.rs`)

**Files:** Create `src/theme.rs`; modify `src/main.rs`.

- [ ] **Step 1: Write the module with its tests**

```rust
//! Color themes: the palettes of the custom-painted UI, egui's own visuals
//! and the syntect theme, switched together from View > Theme.

use eframe::egui::{self, Color32};

use crate::file_browser::SidebarPalette;
use crate::gutter::GutterPalette;
use crate::tab_bar::TabPalette;
use crate::terminal::TerminalPalette;

/// A theme the user can pick. `System` follows the OS between the two
/// Modern themes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeId {
    #[default]
    System,
    LightModern,
    DarkModern,
    SolarizedLight,
    SolarizedDark,
    OceanDark,
    MochaDark,
    EightiesDark,
}

impl ThemeId {
    /// Every theme, in menu order.
    pub const ALL: [ThemeId; 8] = [
        ThemeId::System,
        ThemeId::LightModern,
        ThemeId::DarkModern,
        ThemeId::SolarizedLight,
        ThemeId::SolarizedDark,
        ThemeId::OceanDark,
        ThemeId::MochaDark,
        ThemeId::EightiesDark,
    ];

    /// The name the choice is saved under.
    pub fn slug(self) -> &'static str {
        match self {
            ThemeId::System => "system",
            ThemeId::LightModern => "light-modern",
            ThemeId::DarkModern => "dark-modern",
            ThemeId::SolarizedLight => "solarized-light",
            ThemeId::SolarizedDark => "solarized-dark",
            ThemeId::OceanDark => "ocean-dark",
            ThemeId::MochaDark => "mocha-dark",
            ThemeId::EightiesDark => "eighties-dark",
        }
    }

    /// The theme saved as `slug`; `System` for anything else.
    pub fn from_slug(slug: &str) -> ThemeId {
        Self::ALL
            .into_iter()
            .find(|id| id.slug() == slug)
            .unwrap_or_default()
    }

    /// The menu label of a named theme. `None` for `System`, whose label is
    /// translated.
    pub fn name(self) -> Option<&'static str> {
        match self {
            ThemeId::System => None,
            ThemeId::LightModern => Some("Light Modern"),
            ThemeId::DarkModern => Some("Dark Modern"),
            ThemeId::SolarizedLight => Some("Solarized Light"),
            ThemeId::SolarizedDark => Some("Solarized Dark"),
            ThemeId::OceanDark => Some("Ocean Dark"),
            ThemeId::MochaDark => Some("Mocha Dark"),
            ThemeId::EightiesDark => Some("Eighties Dark"),
        }
    }

    /// The theme to paint with: `System` becomes the Modern theme matching
    /// the OS, dark when the OS does not say.
    pub fn resolve(self, system: Option<egui::Theme>) -> ThemeId {
        match (self, system) {
            (ThemeId::System, Some(egui::Theme::Light)) => ThemeId::LightModern,
            (ThemeId::System, _) => ThemeId::DarkModern,
            (id, _) => id,
        }
    }

    /// The colors of this theme; `System` gives Dark Modern, so resolve it
    /// first.
    pub fn theme(self) -> &'static Theme {
        match self {
            ThemeId::System | ThemeId::DarkModern => &DARK_MODERN,
            ThemeId::LightModern => &LIGHT_MODERN,
            ThemeId::SolarizedLight => &SOLARIZED_LIGHT,
            ThemeId::SolarizedDark => &SOLARIZED_DARK,
            ThemeId::OceanDark => &OCEAN_DARK,
            ThemeId::MochaDark => &MOCHA_DARK,
            ThemeId::EightiesDark => &EIGHTIES_DARK,
        }
    }
}

/// The few colors a derived theme is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    /// Editor, active tab, gutter and console.
    pub bg: Color32,
    /// Tab strip, Activity Bar, sidebar and egui's panels.
    pub chrome: Color32,
    pub border: Color32,
    /// Body text.
    pub fg: Color32,
    /// Active tab title, active line number, lit icons.
    pub fg_strong: Color32,
    /// Inactive tab titles, line numbers, unlit icons.
    pub fg_dim: Color32,
    pub accent: Color32,
    pub hover: Color32,
    pub selection: Color32,
}

/// Everything one theme colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Name of the bundled syntect theme for the editor.
    pub syntax: &'static str,
    pub tab: TabPalette,
    pub sidebar: SidebarPalette,
    pub gutter: GutterPalette,
    pub terminal: TerminalPalette,
    /// `None` keeps egui's stock visuals.
    base: Option<Base>,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Theme {
    /// A theme whose palettes all come from `base`.
    const fn from_base(base: Base, dark: bool, syntax: &'static str) -> Theme {
        let ansi = if dark {
            TerminalPalette::DARK.ansi
        } else {
            TerminalPalette::LIGHT.ansi
        };
        Theme {
            dark,
            syntax,
            tab: TabPalette {
                strip: base.chrome,
                tab_inactive: base.chrome,
                tab_active: base.bg,
                tab_hover: base.bg,
                border: base.border,
                accent: base.accent,
                fg_active: base.fg_strong,
                fg_inactive: base.fg_dim,
                close_hover: base.hover,
            },
            sidebar: SidebarPalette {
                background: base.chrome,
                border: base.border,
                icon_active: base.fg_strong,
                icon_inactive: base.fg_dim,
                accent: base.accent,
                foreground: base.fg,
                row_hover: base.hover,
                row_selected: base.selection,
                indent_guide: base.border,
            },
            gutter: GutterPalette {
                number: base.fg_dim,
                number_active: base.fg_strong,
                bookmark: base.accent,
                background: base.bg,
            },
            terminal: TerminalPalette {
                background: base.bg,
                foreground: base.fg,
                ansi,
            },
            base: Some(base),
        }
    }

    /// egui's visuals for this theme: the stock dark or light ones,
    /// recolored from the base colors when there are any.
    pub fn visuals(&self) -> egui::Visuals {
        let mut visuals = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        if let Some(base) = self.base {
            visuals.panel_fill = base.chrome;
            visuals.window_fill = base.chrome;
            visuals.extreme_bg_color = base.bg;
            visuals.faint_bg_color = base.hover;
            visuals.widgets.noninteractive.fg_stroke.color = base.fg;
            visuals.widgets.noninteractive.bg_stroke.color = base.border;
            visuals.widgets.inactive.fg_stroke.color = base.fg;
            visuals.selection.bg_fill = base.selection;
            visuals.selection.stroke.color = base.fg_strong;
            visuals.hyperlink_color = base.accent;
        }
        visuals
    }
}

/// VS Code's "Light Modern".
pub const LIGHT_MODERN: Theme = Theme {
    dark: false,
    syntax: "InspiredGitHub",
    tab: TabPalette::LIGHT,
    sidebar: SidebarPalette::LIGHT,
    gutter: GutterPalette::LIGHT,
    terminal: TerminalPalette::LIGHT,
    base: None,
};

/// VS Code's "Dark Modern".
pub const DARK_MODERN: Theme = Theme {
    dark: true,
    syntax: "base16-ocean.dark",
    tab: TabPalette::DARK,
    sidebar: SidebarPalette::DARK,
    gutter: GutterPalette::DARK,
    terminal: TerminalPalette::DARK,
    base: None,
};

pub const SOLARIZED_LIGHT: Theme = Theme::from_base(
    Base {
        bg: hex(0xFDF6E3),
        chrome: hex(0xEEE8D5),
        border: hex(0xDDD6C1),
        fg: hex(0x657B83),
        fg_strong: hex(0x073642),
        fg_dim: hex(0x93A1A1),
        accent: hex(0x268BD2),
        hover: hex(0xE6DEC6),
        selection: hex(0xDFCA88),
    },
    false,
    "Solarized (light)",
);

pub const SOLARIZED_DARK: Theme = Theme::from_base(
    Base {
        bg: hex(0x002B36),
        chrome: hex(0x00212B),
        border: hex(0x073642),
        fg: hex(0x839496),
        fg_strong: hex(0x93A1A1),
        fg_dim: hex(0x586E75),
        accent: hex(0x268BD2),
        hover: hex(0x004454),
        selection: hex(0x005A6F),
    },
    true,
    "Solarized (dark)",
);

pub const OCEAN_DARK: Theme = Theme::from_base(
    Base {
        bg: hex(0x2B303B),
        chrome: hex(0x232831),
        border: hex(0x343D46),
        fg: hex(0xC0C5CE),
        fg_strong: hex(0xEFF1F5),
        fg_dim: hex(0x65737E),
        accent: hex(0x8FA1B3),
        hover: hex(0x3B4450),
        selection: hex(0x4F5B66),
    },
    true,
    "base16-ocean.dark",
);

pub const MOCHA_DARK: Theme = Theme::from_base(
    Base {
        bg: hex(0x3B3228),
        chrome: hex(0x322A21),
        border: hex(0x534636),
        fg: hex(0xD0C8C6),
        fg_strong: hex(0xF5EEEB),
        fg_dim: hex(0x7E705A),
        accent: hex(0x8AB3B5),
        hover: hex(0x4A3F31),
        selection: hex(0x645240),
    },
    true,
    "base16-mocha.dark",
);

pub const EIGHTIES_DARK: Theme = Theme::from_base(
    Base {
        bg: hex(0x2D2D2D),
        chrome: hex(0x252525),
        border: hex(0x393939),
        fg: hex(0xD3D0C8),
        fg_strong: hex(0xF2F0EC),
        fg_dim: hex(0x747369),
        accent: hex(0x6699CC),
        hover: hex(0x353535),
        selection: hex(0x515151),
    },
    true,
    "base16-eighties.dark",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugs_round_trip() {
        for id in ThemeId::ALL {
            assert_eq!(ThemeId::from_slug(id.slug()), id);
        }
        assert_eq!(ThemeId::from_slug("no-such-theme"), ThemeId::System);
        assert_eq!(ThemeId::from_slug(""), ThemeId::System);
    }

    #[test]
    fn test_only_system_has_a_translated_name() {
        for id in ThemeId::ALL {
            assert_eq!(id.name().is_none(), id == ThemeId::System, "{id:?}");
        }
    }

    #[test]
    fn test_system_resolves_to_a_modern_theme() {
        let light = Some(egui::Theme::Light);
        let dark = Some(egui::Theme::Dark);
        assert_eq!(ThemeId::System.resolve(light), ThemeId::LightModern);
        assert_eq!(ThemeId::System.resolve(dark), ThemeId::DarkModern);
        assert_eq!(ThemeId::System.resolve(None), ThemeId::DarkModern);
        assert_eq!(ThemeId::MochaDark.resolve(light), ThemeId::MochaDark);
    }

    #[test]
    fn test_modern_themes_keep_the_vscode_palettes() {
        let dark = ThemeId::DarkModern.theme();
        assert_eq!(dark.tab, TabPalette::DARK);
        assert_eq!(dark.sidebar, SidebarPalette::DARK);
        assert_eq!(dark.gutter, GutterPalette::DARK);
        assert_eq!(dark.terminal, TerminalPalette::DARK);
        let light = ThemeId::LightModern.theme();
        assert_eq!(light.tab, TabPalette::LIGHT);
        assert_eq!(light.sidebar, SidebarPalette::LIGHT);
        assert_eq!(light.gutter, GutterPalette::LIGHT);
        assert_eq!(light.terminal, TerminalPalette::LIGHT);
        assert_eq!(light.visuals(), egui::Visuals::light());
        assert_eq!(dark.visuals(), egui::Visuals::dark());
    }

    #[test]
    fn test_every_syntax_theme_is_bundled() {
        let bundled = syntect::highlighting::ThemeSet::load_defaults();
        for id in ThemeId::ALL {
            let name = id.theme().syntax;
            assert!(bundled.themes.contains_key(name), "{id:?}: {name}");
        }
    }

    #[test]
    fn test_text_stands_out_from_its_background() {
        for id in ThemeId::ALL {
            let t = id.theme();
            assert_ne!(t.tab.fg_active, t.tab.tab_active, "{id:?}");
            assert_ne!(t.tab.fg_inactive, t.tab.strip, "{id:?}");
            assert_ne!(t.sidebar.foreground, t.sidebar.background, "{id:?}");
            assert_ne!(t.gutter.number, t.gutter.background, "{id:?}");
            assert_ne!(t.gutter.bookmark, t.gutter.background, "{id:?}");
            assert_ne!(t.terminal.foreground, t.terminal.background, "{id:?}");
            assert_eq!(t.visuals().dark_mode, t.dark, "{id:?}");
        }
    }

    #[test]
    fn test_derived_theme_recolors_egui() {
        let visuals = ThemeId::SolarizedDark.theme().visuals();
        assert_eq!(visuals.panel_fill, hex(0x00212B));
        assert_eq!(visuals.extreme_bg_color, hex(0x002B36));
        assert_eq!(visuals.widgets.inactive.fg_stroke.color, hex(0x839496));
    }
}
```

Add `mod theme;` to `src/main.rs` between `mod terminal;` and the `use` lines (alphabetical: after `tab_bar`, `terminal`, then `theme`).

- [ ] **Step 2: Run** `cargo test theme::` — expected: 7 passed. (Dead-code warnings for `theme` until Task 2–5 wire it.)

- [ ] **Step 3: Commit** `feat(theme): theme ids, base colors and palettes`

---

### Task 2: Active theme in egui's memory

**Files:** Modify `src/theme.rs`.

- [ ] **Step 1: Add tests**

```rust
    #[test]
    fn test_active_falls_back_to_egui_dark_mode() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Light);
        assert_eq!(active(&ctx), ThemeId::LightModern.theme());
        ctx.set_theme(egui::Theme::Dark);
        assert_eq!(active(&ctx), ThemeId::DarkModern.theme());
    }

    #[test]
    fn test_apply_stores_the_theme_and_its_visuals() {
        let ctx = egui::Context::default();
        let theme = apply(&ctx, ThemeId::SolarizedLight);
        assert_eq!(theme, ThemeId::SolarizedLight.theme());
        assert_eq!(active(&ctx), theme);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(ctx.global_style().visuals, theme.visuals());

        apply(&ctx, ThemeId::MochaDark);
        assert_eq!(active(&ctx), ThemeId::MochaDark.theme());
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, hex(0x322A21));
    }
```

- [ ] **Step 2: Run** `cargo test theme::` — expected: compile error, `apply`/`active` not found.

- [ ] **Step 3: Implement** (above `#[cfg(test)]`)

```rust
/// Where [`apply`] keeps the resolved theme in egui's memory.
fn memory_id() -> egui::Id {
    egui::Id::new("rred_theme")
}

/// Makes `choice` the theme this frame paints with, and returns it. egui's
/// visuals are only replaced when the resolved theme changes.
pub fn apply(ctx: &egui::Context, choice: ThemeId) -> &'static Theme {
    let resolved = choice.resolve(ctx.system_theme());
    let theme = resolved.theme();
    if ctx.data(|d| d.get_temp::<ThemeId>(memory_id())) != Some(resolved) {
        ctx.data_mut(|d| d.insert_temp(memory_id(), resolved));
        let mode = if theme.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        ctx.set_visuals_of(mode, theme.visuals());
        ctx.set_theme(mode);
    }
    theme
}

/// The theme last [`apply`]-ed; before that, the Modern theme matching
/// egui's dark mode.
pub fn active(ctx: &egui::Context) -> &'static Theme {
    ctx.data(|d| d.get_temp::<ThemeId>(memory_id()))
        .unwrap_or_else(|| {
            if ctx.global_style().visuals.dark_mode {
                ThemeId::DarkModern
            } else {
                ThemeId::LightModern
            }
        })
        .theme()
}
```

- [ ] **Step 4: Run** `cargo test theme::` — expected: 9 passed.
- [ ] **Step 5: Commit** `feat(theme): apply and read the active theme`

---

### Task 3: Syntax theme switchable at runtime

**Files:** Modify `src/highlighter.rs`.

Changes:
1. Replace `const THEME_NAME` with `pub const DEFAULT_THEME: &str = "InspiredGitHub";` (doc: "Theme used until [`SyntaxHighlighter::set_theme`] picks another.").
2. `pick_theme(theme_set, name)` looks up `name`, then falls back to the first bundled theme.
3. `SyntaxHighlighter` gets `theme: &'static str` (init `DEFAULT_THEME`) and:
   ```rust
   /// Highlights with the bundled syntect theme `name` from now on. Cached
   /// layouts made with another theme are rebuilt on their next use.
   pub fn set_theme(&mut self, name: &'static str) {
       self.theme = name;
   }
   ```
   `theme()` (test-only) uses `pick_theme(&self.theme_set, self.theme)`.
4. `DocumentHighlightCache` gains `pub theme: Option<&'static str>`; `DocumentCache` and `PendingHighlight` gain `theme: &'static str`; `HighlighterLayouter` and `Worker` gain `theme: &'static str`.
5. `highlight_incremental_with(syntax_set, theme_set, theme_name, cache, ...)`: the reset condition also checks `cache.theme != Some(theme_name)` and sets it; it picks `pick_theme(theme_set, theme_name)`.
6. `layouter()` passes `self.theme`; `cached()`, `store()`, `spawn_pass()`, the pending checks in `frame_job` and `large_document_job`, and the `warm` check all compare/record the theme too.

- [ ] **Step 1: Write the failing test** (in `highlighter.rs` tests)

```rust
    #[test]
    fn test_switching_theme_recolors_cached_documents() {
        let mut highlighter = SyntaxHighlighter::new();
        let font_id = egui::FontId::monospace(14.0);
        let doc = "fn main() { let x = 1; }";
        let colors = |job: &egui::text::LayoutJob| -> Vec<egui::Color32> {
            job.sections.iter().map(|s| s.format.color).collect()
        };

        let mut cache = DocumentHighlightCache::default();
        let light = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc);
        highlighter.set_theme("base16-ocean.dark");
        let dark = highlighter.highlight_incremental(&mut cache, Language::Rust, &font_id, doc);
        assert_ne!(colors(&light), colors(&dark), "the line cache is rebuilt");

        let first = highlighter.layouter(Language::Rust, font_id.clone()).document_job(doc);
        highlighter.set_theme(DEFAULT_THEME);
        let second = highlighter.layouter(Language::Rust, font_id).document_job(doc);
        assert_ne!(colors(&first), colors(&second), "the document cache is rebuilt");
    }
```

- [ ] **Step 2: Run** `cargo test highlighter::tests::test_switching_theme` — expected: compile error (`set_theme`, `DEFAULT_THEME`).
- [ ] **Step 3: Implement changes 1–6.**
- [ ] **Step 4: Run** `cargo test highlighter::` — expected: all pass.
- [ ] **Step 5: Commit** `feat(highlighter): switch the syntect theme at runtime`

---

### Task 4: Painting modules read the active theme

**Files:** Modify `src/tab_bar.rs`, `src/activity_bar.rs`, `src/file_browser.rs`, `src/gutter.rs`, `src/terminal.rs`, `src/app.rs`.

- [ ] **Step 1: Replace each lookup**

| File:line | Before | After |
|---|---|---|
| `tab_bar.rs:172` | `TabPalette::for_visuals(ui.visuals())` | `theme::active(ui.ctx()).tab` |
| `activity_bar.rs:39` | `SidebarPalette::for_visuals(ui.visuals())` | `theme::active(ui.ctx()).sidebar` |
| `file_browser.rs:313` | `SidebarPalette::for_visuals(ui.visuals())` | `theme::active(ui.ctx()).sidebar` |
| `gutter.rs:152` | `GutterPalette::for_visuals(ui.visuals())` | `theme::active(ui.ctx()).gutter` |
| `terminal.rs:241` | `TerminalPalette::for_visuals(ui.visuals())` | `theme::active(ui.ctx()).terminal` |
| `app.rs:657` | `gutter::GutterPalette::for_visuals(ui.visuals()).bookmark` | `theme::active(ui.ctx()).gutter.bookmark` |
| `app.rs:2163` | `file_browser::SidebarPalette::for_visuals(ui.visuals()).background` | `theme::active(&ctx).sidebar.background` |
| `app.rs:2246` | `tab_bar::TabPalette::for_visuals(ui.visuals()).tab_active` | `theme::active(&ctx).tab.tab_active` |
| `app.rs:3877`, `app.rs:4490` (tests) | `crate::gutter::GutterPalette::for_visuals(&ctx.global_style().visuals)` | `crate::theme::active(&ctx).gutter` |
| `activity_bar.rs:219` (test) | `SidebarPalette::for_visuals(&ctx.global_style().visuals).accent` | `crate::theme::active(&ctx).sidebar.accent` |
| `file_browser.rs:1051` (test) | `SidebarPalette::for_visuals(&ctx.global_style().visuals)` | `crate::theme::active(&ctx).sidebar` |

Add `use crate::theme;` where needed; drop imports left unused (`SidebarPalette` in `activity_bar.rs`).

- [ ] **Step 2: Remove the `for_visuals` functions** from `TabPalette`, `SidebarPalette`, `GutterPalette`, `TerminalPalette`, and their test assertions: `tab_bar.rs` `test_palette_follows_egui_theme` (whole test), the two `for_visuals` asserts at the end of `gutter.rs` `test_palettes_match_vscode_modern_themes` and of `file_browser.rs` `test_palettes_match_vscode_modern_themes`. The same behaviour is now covered by `theme::tests::test_active_falls_back_to_egui_dark_mode`. Update the palette doc comments from "after VS Code's ..." to also say presets derive them in `theme.rs` where it reads better.

- [ ] **Step 3: Run** `cargo test` — expected: all pass.
- [ ] **Step 4: Commit** `refactor(theme): paint the UI from the active theme`

---

### Task 5: App wiring, menu, persistence

**Files:** Modify `src/app.rs`, `assets/lang/*.json`, `README.md`, `README.id.md`.

- [ ] **Step 1: Write failing tests** (in `app.rs` tests, next to the history ones)

```rust
    #[test]
    fn test_theme_is_saved_and_restored() {
        let mut app = TextEditorApp::default();
        assert_eq!(app.theme, ThemeId::System);
        app.theme = ThemeId::SolarizedDark;
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        assert_eq!(
            eframe::Storage::get_string(&storage, super::THEME_KEY).as_deref(),
            Some("solarized-dark")
        );
        assert_eq!(saved_theme(Some(&storage)), ThemeId::SolarizedDark);
        assert_eq!(saved_theme(None), ThemeId::System);
    }

    #[test]
    fn test_a_frame_applies_the_chosen_theme() {
        let ctx = egui::Context::default();
        let mut app = TextEditorApp::default();
        app.theme = ThemeId::EightiesDark;
        run_frame(&ctx, &mut app);
        assert_eq!(crate::theme::active(&ctx), ThemeId::EightiesDark.theme());
        assert_eq!(app.highlighter_theme(), "base16-eighties.dark");
        app.theme = ThemeId::LightModern;
        run_frame(&ctx, &mut app);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(app.highlighter_theme(), "InspiredGitHub");
    }
```

`run_frame`: use the existing headless-frame helper in `app.rs` tests if one exists (search `fn run_app` / `ctx.run_ui(` near line 3877); otherwise add one that calls `eframe::App::ui` — check how existing tests drive a full frame and reuse it. `highlighter_theme()` reads `self.highlighter`'s theme; add `#[cfg(test)] pub fn theme_name(&self) -> &'static str` on `SyntaxHighlighter` and call that instead of a new app method if simpler.

- [ ] **Step 2: Run** — expected: compile errors.

- [ ] **Step 3: Implement**
  - `const THEME_KEY: &str = "theme";` beside the other keys (doc: "Storage key under which eframe remembers the chosen color theme.").
  - Field `pub theme: ThemeId` (doc: "The color theme picked in View > Theme; `System` follows the OS."), default `ThemeId::System`; `new()` sets `theme: saved_theme(cc.storage)`.
  - `fn saved_theme(storage) -> ThemeId` beside `saved_history`.
  - `save()`: `storage.set_string(THEME_KEY, self.theme.slug().to_owned());`
  - Top of `ui()`, after `let ctx = ...`: `let theme = theme::apply(&ctx, self.theme); self.highlighter.set_theme(theme.syntax);`
  - `show_view_menu`: after the Console item, `self.show_theme_menu(ui);` then the existing separator.
  - ```rust
    /// Lists every color theme; picking one switches to it at once.
    pub fn show_theme_menu(&mut self, ui: &mut egui::Ui) {
        let label = self.i18n.t("menu.theme").to_owned();
        ui.menu_button(label, |ui| {
            for id in ThemeId::ALL {
                let name = id.name().unwrap_or_else(|| self.i18n.t("theme.system"));
                if ui.selectable_label(self.theme == id, name).clicked() {
                    self.theme = id;
                    ui.close();
                }
            }
        });
    }
    ```
  - i18n keys after `"menu.ui_language"` in every file:
    en `"Theme"`/`"System"`, id `"Tema"`/`"Sistem"`, de `"Farbschema"`/`"System"`, es `"Tema"`/`"Sistema"`, zh `"主题"`/`"跟随系统"`.
  - README View rows: add `Theme` after `Markdown Preview` (en), `Tema` (id). Add one feature bullet in each README naming the themes and that the choice is remembered.

- [ ] **Step 4: Run** `cargo test` — expected: all pass.
- [ ] **Step 5: Commit** `feat(view): View > Theme switches the color theme`

---

### Task 6: Verify and tidy

- [ ] `cargo build 2>&1 | grep -c warning` → 0 (compare with `git stash` baseline if non-zero).
- [ ] `cargo clippy --all-targets` → no new warnings versus `main` before Task 1.
- [ ] `cargo test` → all pass.
- [ ] Update spec item 6: `set_theme(name)` instead of a `layouter()` parameter.
- [ ] Commit `docs(spec): record set_theme in the themes spec`.
