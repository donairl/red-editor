# Color Themes Design

**Date:** 2026-10-21
**Status:** Approved by user
**Project:** `rred_text_editor`

## Objective

Let the user pick a color theme from **View > Theme**. The theme colors syntax highlighting and every custom-painted part of the UI (tab strip, Activity Bar, Explorer, gutter, console), plus egui's own widgets (menus, modals, Markdown preview). Switching applies on the next frame, with no restart, and the choice is remembered.

## Motivation

The UI already has VS Code "Dark Modern" and "Light Modern" palettes, chosen per module from `ui.visuals().dark_mode`, so it follows the OS. Syntax highlighting is fixed to syntect's `InspiredGitHub`, a light theme, which reads poorly on the dark editor. There is no way to choose a theme and nothing is saved.

## Requirements & Key Decisions

1. **Theme list (chosen: Light/Dark + extra presets).**

   | `ThemeId`        | Menu label      | Slug              | Dark | syntect theme          |
   |------------------|-----------------|-------------------|------|------------------------|
   | `System`         | System (translated) | `system`      | (OS) | (resolved theme's)     |
   | `LightModern`    | Light Modern    | `light-modern`    | no   | `InspiredGitHub`       |
   | `DarkModern`     | Dark Modern     | `dark-modern`     | yes  | `base16-ocean.dark`    |
   | `SolarizedLight` | Solarized Light | `solarized-light` | no   | `Solarized (light)`    |
   | `SolarizedDark`  | Solarized Dark  | `solarized-dark`  | yes  | `Solarized (dark)`     |
   | `OceanDark`      | Ocean Dark      | `ocean-dark`      | yes  | `base16-ocean.dark`    |
   | `MochaDark`      | Mocha Dark      | `mocha-dark`      | yes  | `base16-mocha.dark`    |
   | `EightiesDark`   | Eighties Dark   | `eighties-dark`   | yes  | `base16-eighties.dark` |

   `System` resolves every frame to `LightModern` or `DarkModern` from `ctx.system_theme()`, falling back to `DarkModern` when the OS does not say. Preset names are proper names and are not translated.

2. **Structure (chosen: base colors + derived palettes).** A new module `src/theme.rs` holds:

   ```rust
   pub struct Theme {
       pub dark: bool,
       pub syntax: &'static str,      // syntect theme name
       pub tab: TabPalette,
       pub sidebar: SidebarPalette,
       pub gutter: GutterPalette,
       pub terminal: TerminalPalette,
       /// `None` keeps egui's stock visuals (the Modern themes).
       base: Option<Base>,
   }

   pub struct Base {
       pub bg: Color32,        // editor, active tab, gutter, console
       pub chrome: Color32,    // tab strip, Activity Bar, sidebar, panels
       pub border: Color32,
       pub fg: Color32,        // body text
       pub fg_strong: Color32, // active tab title, active line number, lit icons
       pub fg_dim: Color32,    // inactive tab titles, line numbers, dim icons
       pub accent: Color32,
       pub hover: Color32,
       pub selection: Color32,
   }
   ```

   Every theme is a `const`. `LightModern` and `DarkModern` reuse the existing `TabPalette::LIGHT/DARK`, `SidebarPalette::LIGHT/DARK`, `GutterPalette::LIGHT/DARK` and `TerminalPalette::LIGHT/DARK` unchanged, so today's look and its tests stay. Presets go through `const fn Theme::from_base(base, dark, syntax)`:

   | Palette field                       | From base   |
   |-------------------------------------|-------------|
   | tab `strip`, `tab_inactive`         | `chrome`    |
   | tab `tab_active`, `tab_hover`       | `bg`        |
   | tab `border`                        | `border`    |
   | tab `accent`                        | `accent`    |
   | tab `fg_active` / `fg_inactive`     | `fg_strong` / `fg_dim` |
   | tab `close_hover`                   | `hover`     |
   | sidebar `background`                | `chrome`    |
   | sidebar `border`, `indent_guide`    | `border`    |
   | sidebar `icon_active` / `icon_inactive` | `fg_strong` / `fg_dim` |
   | sidebar `accent`                    | `accent`    |
   | sidebar `foreground`                | `fg`        |
   | sidebar `row_hover` / `row_selected`| `hover` / `selection` |
   | gutter `number` / `number_active`   | `fg_dim` / `fg_strong` |
   | gutter `bookmark`                   | `accent`    |
   | gutter `background`                 | `bg`        |
   | terminal `background` / `foreground`| `bg` / `fg` |
   | terminal `ansi`                     | `TerminalPalette::DARK/LIGHT.ansi` by `dark` |

3. **Preset base colors**, from each scheme's published palette (Solarized, base16); `chrome` and `hover` are hand-picked steps between them.

   | Field       | Solarized Light | Solarized Dark | Ocean Dark | Mocha Dark | Eighties Dark |
   |-------------|-----------------|----------------|------------|------------|---------------|
   | `bg`        | `#FDF6E3`       | `#002B36`      | `#2B303B`  | `#3B3228`  | `#2D2D2D`     |
   | `chrome`    | `#EEE8D5`       | `#00212B`      | `#232831`  | `#322A21`  | `#252525`     |
   | `border`    | `#DDD6C1`       | `#073642`      | `#343D46`  | `#534636`  | `#393939`     |
   | `fg`        | `#657B83`       | `#839496`      | `#C0C5CE`  | `#D0C8C6`  | `#D3D0C8`     |
   | `fg_strong` | `#073642`       | `#93A1A1`      | `#EFF1F5`  | `#F5EEEB`  | `#F2F0EC`     |
   | `fg_dim`    | `#93A1A1`       | `#586E75`      | `#65737E`  | `#7E705A`  | `#747369`     |
   | `accent`    | `#268BD2`       | `#268BD2`      | `#8FA1B3`  | `#8AB3B5`  | `#6699CC`     |
   | `hover`     | `#E6DEC6`       | `#004454`      | `#3B4450`  | `#4A3F31`  | `#353535`     |
   | `selection` | `#DFCA88`       | `#005A6F`      | `#4F5B66`  | `#645240`  | `#515151`     |

4. **Getting the theme to the modules.** The active, resolved `ThemeId` is stored in egui's context memory. `theme::active(ctx) -> &'static Theme` reads it; when nothing is stored (headless tests) it falls back to `DarkModern` or `LightModern` from `ctx`'s `dark_mode`, matching today's behaviour. Every `X::for_visuals(ui.visuals())` call in `tab_bar`, `activity_bar`, `file_browser`, `gutter`, `terminal` and `app` becomes `theme::active(ui.ctx()).x`, and the `for_visuals` functions are removed. No `show()` signature changes.

5. **egui visuals.** Applied whenever the resolved theme changes (not every frame): start from `Visuals::dark()` or `Visuals::light()` by `dark`. For a theme with a `Base`, also set `panel_fill` and `window_fill` to `chrome`, `extreme_bg_color` to `bg`, `faint_bg_color` to `hover`, the `noninteractive` and `inactive` widget text strokes to `fg`, `selection.bg_fill` to `selection`, `selection.stroke` to `fg_strong`, and `hyperlink_color` to `accent`. Modals, menus and the Markdown preview follow because they draw with egui's visuals. Plain text in the editor takes egui's text color, so it follows `fg` too.

6. **Syntax highlighting without restart.** `SyntaxHighlighter` picks the syntect theme by name instead of the fixed `THEME_NAME`. The syntax theme name joins language and font as a cache key in `DocumentHighlightCache`, `DocumentCache` and `PendingHighlight`; a theme change misses the cache and the next frame recolors the document. `layouter()` takes the theme name from the caller. An unknown name falls back to the first bundled theme, as today.

7. **Menu.** **View > Theme** submenu after **Console**: one selectable label per `ThemeId`, the current one selected. New i18n keys `menu.theme` and `theme.system` in all five language files.

8. **Persistence.** The chosen `ThemeId` (not the resolved one) is saved under storage key `"theme"` as its slug. Default and fallback for a missing or unknown slug: `System`.

9. **Unchanged.** Status-bar message colors (red/green), the Find match highlight (translucent yellow) and the tab language badge colors stay as they are; they read on every theme.

## Out of Scope

Theme editor, importing or exporting custom themes, per-theme ANSI colors, keyboard shortcut for switching.

## Testing

- Every `ThemeId` maps to a theme; slugs round-trip; an unknown slug parses as `System`.
- Every theme's `syntax` name exists in `ThemeSet::load_defaults()`.
- `LightModern`/`DarkModern` palettes equal the existing `LIGHT`/`DARK` constants.
- For every theme: `fg != bg`, `fg_strong != bg`, `accent != bg` on the derived palettes.
- `theme::active` falls back to the Modern theme matching `dark_mode` when nothing is stored, and returns the stored theme otherwise.
- Highlighting the same text with two syntax themes through one `DocumentHighlightCache` yields different token colors (the cache is invalidated).
- App: picking a theme stores it; `save()` writes its slug; a new app built from that storage starts with it.
- `cargo build` and `cargo clippy --all-targets` add no warnings; `cargo test` passes.
