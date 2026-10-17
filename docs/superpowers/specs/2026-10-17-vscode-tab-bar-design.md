# VS Code-Style Tab Bar Design

**Date:** 2026-10-17
**Status:** Approved by user
**Project:** `rred_text_editor`

## Objective

Replace the boxed, pill-shaped tab bar with a flat tab strip modelled on VS Code's "Dark Modern" theme (and "Light Modern" when the app runs in light mode).

## Motivation

The current tab bar wraps every tab in a `ui.group` frame, highlights the active tab with a rounded `selectable_label` pill, shows an always-visible `small_button("✕")`, and marks unsaved files with a `" *"` suffix. It looks cluttered and unlike any code editor the user is used to.

## Requirements & Key Decisions

1. **Palette (chosen: "Dark Modern").** Picked from `ui.visuals().dark_mode`:

   | Role         | Dark Modern | Light Modern |
   |--------------|-------------|--------------|
   | strip        | `#181818`   | `#F8F8F8`    |
   | tab inactive | `#181818`   | `#F8F8F8`    |
   | tab active   | `#1F1F1F`   | `#FFFFFF`    |
   | tab hover    | `#1F1F1F`   | `#FFFFFF`    |
   | border       | `#2B2B2B`   | `#E5E5E5`    |
   | accent       | `#0078D4`   | `#005FB8`    |
   | fg active    | `#FFFFFF`   | `#3B3B3B`    |
   | fg inactive  | `#9D9D9D`   | `#868686`    |
   | close hover  | `#313131`   | `#E9E9E9`    |

2. **Strip geometry.**
   - Height 35 px, corner radius 0, tabs flush against each other.
   - Each tab has a 1 px right border; the strip has a 1 px bottom border.
   - The active tab uses the active background, draws a 1 px accent line along its top edge, and has no bottom border so it joins the editor below.

3. **Tab contents**, left to right: 10 px padding, language badge, 6 px gap, file name, 8 px gap, 16×16 px close slot, 10 px padding. The close slot always reserves its space so the tab width never changes on hover.

4. **Language badge.** Short coloured text before the name, from the tab's effective language (respecting a manual override):

   | Language   | Badge | Colour    |
   |------------|-------|-----------|
   | Rust       | `RS`  | `#DEA584` |
   | C          | `C`   | `#519ABA` |
   | C++        | `C++` | `#F34B7D` |
   | PHP        | `PHP` | `#A074C4` |
   | JavaScript | `JS`  | `#CBCB41` |
   | TypeScript | `TS`  | `#519ABA` |
   | HTML       | `<>`  | `#E44D26` |
   | Plain text | `TXT` | fg inactive |

5. **Close slot ("dirty dot" + "close on hover only").**
   - The pointer is over the slot: `×` on a small rounded hover box (radius 4 px).
   - Otherwise, the tab has unsaved changes: `●` (also on the active tab, so its unsaved state stays visible).
   - Otherwise, the tab is active or hovered: `×`.
   - Otherwise: empty.
   - Clicking `×` closes the tab via the existing `trigger_close_tab` (so the unsaved-changes prompt still applies). Clicking the `●` slot also closes, as in VS Code.
   - The `" *"` title suffix is removed; the dot replaces it.

6. **Hover and tooltip.** A hovered inactive tab uses the hover background. Hovering any tab shows the full file path as a tooltip; an unsaved new document (no path) has no tooltip.

7. **New tab button.** A 35×35 px `+` after the last tab, in the inactive foreground, with the hover background on hover. Opens a new blank tab (`open_new_tab`).

8. **Overflow.** The strip stays inside the existing horizontal `ScrollArea`.

9. **Editor joins the active tab.**
   - The separator between the tab strip and the editor is removed.
   - The central panel has no inner margin, so the strip spans the full width.
   - The editor is drawn without a rounded frame or outline, filled with the active-tab background, so the active tab and the editor read as one surface.

10. **Fonts.** egui's bundled fonts stay. Loading Segoe UI / Noto Sans is out of scope.

## Architecture

New module `src/tab_bar.rs`, independent of `app.rs`:

- `TabView { title: String, tooltip: Option<String>, language: Language, dirty: bool }` — everything the strip needs about one tab. The strip never sees `DocumentTab`.
- `TabPalette::for_visuals(&egui::Visuals) -> TabPalette` — the table in requirement 1.
- `badge(Language) -> (&'static str, Option<Color32>)` — `None` colour means "use fg inactive" (plain text).
- `TabBarAction { Select(usize), Close(usize), New }`.
- `show(ui, tabs: &[TabView], active: usize) -> TabBarResponse`, where `TabBarResponse { action: Option<TabBarAction>, tab_rects: Vec<Rect>, close_rects: Vec<Rect>, new_tab_rect: Rect }`. The rects say where each part was drawn; tests click them instead of hard-coded coordinates.

`TextEditorApp::render_tab_bar` builds the `TabView`s from `self.tabs`, calls `tab_bar::show`, and applies the action: `Select(i)` sets `active_tab_index`, `Close(i)` calls `trigger_close_tab(i)`, `New` calls `open_new_tab()`.

## Testing

- **Unit:** both palettes match the table; every `Language::all()` entry has a non-empty badge; `TabView` titles carry no `" *"`.
- **Headless interaction** (egui `Context`, no window), using the returned rects:
  - clicking a tab body yields `Select(i)`;
  - clicking the active tab's close slot yields `Close(i)`;
  - clicking `+` yields `New`;
  - a dirty, non-hovered, inactive tab paints `●`; hovering its close slot paints `×`.
- **Existing test** `test_tab_bar_ui_rendering_and_interaction` is rewritten to locate tabs through the response rects instead of `(50, 17)` / `(299, 17)`.
- **Visual check:** before/after screenshots of the real app in dark mode.

## Out of Scope

Drag to reorder, middle-click close, auto-scrolling a new tab into view, file-type icons beyond the text badge, changing the syntax-highlighting theme.
