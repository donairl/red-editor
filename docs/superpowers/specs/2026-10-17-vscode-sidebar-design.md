# VS Code-Style Sidebar Design

**Date:** 2026-10-17
**Status:** Approved by user
**Project:** `rred_text_editor`

## Objective

Restyle the file browser sidebar after VS Code's Explorer and add an Activity Bar on the far left, matching the "Dark Modern" / "Light Modern" look of the tab strip.

## Motivation

The sidebar is built from stock egui widgets: a large `📂` heading with a `✖` button, `CollapsingHeader`s with `📁` emoji for folders, and a `button` with `📄` per file. It does not match the VS Code-style tab strip next to it. It also rescans every expanded directory from disk on every frame (`render_tree_node` calls `scan_directory` while drawing).

## Requirements & Key Decisions

1. **Scope (chosen: "Explorer + Activity Bar").** Restyle the Explorer and add an Activity Bar. No empty-state sidebar: the sidebar only shows once a folder is open.

2. **Palette.** Picked from `ui.visuals().dark_mode`, shared by the Activity Bar and the sidebar:

   | Role                           | Dark Modern | Light Modern |
   |--------------------------------|-------------|--------------|
   | background                     | `#181818`   | `#F8F8F8`    |
   | border                         | `#2B2B2B`   | `#E5E5E5`    |
   | icon active                    | `#D7D7D7`   | `#1F1F1F`    |
   | icon inactive                  | `#868686`   | `#616161`    |
   | accent (active icon marker)    | `#0078D4`   | `#005FB8`    |
   | foreground (tree and headers)  | `#CCCCCC`   | `#3B3B3B`    |
   | row hover                      | `#2A2D2E`   | `#F2F2F2`    |
   | row selected (active file)     | `#37373D`   | `#E4E6F1`    |
   | indent guide                   | `#404040`   | `#D3D3D3`    |

3. **Activity Bar.**
   - Always visible, 48 px wide, background colour, 1 px border on its right edge.
   - One item: the Explorer icon, a 48×48 px slot at the top. The icon (two overlapping pages) is drawn with shapes, so it never depends on the bundled fonts.
   - Sidebar open: icon in "icon active" with a 2 px accent bar along the slot's left edge. Sidebar closed: icon in "icon inactive"; hovering brightens it to "icon active".
   - Tooltip "Explorer".
   - Click with no folder open: opens the existing "Buka Folder" dialog (`trigger_open_folder`); choosing a folder shows the sidebar. Click with a folder open: toggles the sidebar.

4. **Sidebar panel.**
   - Resizable, default width 240 px, min 170 px, max 500 px. Background colour, 1 px border on its right edge.
   - **Title row**, 35 px high (level with the tab strip): `EXPLORER` at 11 px in the foreground colour, 20 px left padding.
   - **Section header**, 22 px high: chevron (`▾` open, `▸` closed), then the root folder's name in upper case, bold, 11 px. Clicking the header opens or closes the section. While the pointer is over the header, two 22×22 px icon buttons show at its right end: **Refresh** (circular arrow) and **Collapse All** (box with a minus), both drawn with shapes. Clicking either button does not open or close the section.

5. **Tree rows.**
   - Each row is 22 px high and spans the full sidebar width.
   - Left to right: 8 px padding, 8 px indent per depth level, a 16 px chevron column (`▸` / `▾` for folders, empty for files), for files the language badge from `tab_bar::badge(Language::from_path(path))` (same text and colours as the tabs), 6 px gap, name.
   - Folders get no icon (as in VS Code's default Seti theme).
   - Names too long for the sidebar are cut with `…`. The tree scrolls vertically only.
   - A 1 px indent guide is drawn at each ancestor level, always visible.
   - Hovered row: hover background. The row of the active tab's file: selected background.
   - Click a folder: expand or collapse it. Click a file: open it through `open_file_in_tab` (which focuses the existing tab when the file is already open).
   - Hidden files and folders (names starting with `.`) stay skipped; ordering stays folders first, then case-insensitive by name (`scan_directory`, unchanged).

6. **Directory cache.** A directory is read from disk once, the first time it is shown expanded, and kept. **Refresh** drops the cache so every directory is read again on the next frame; expanded folders stay expanded. **Collapse All** collapses every folder; the section itself stays open.

7. **Removed.** The "📂 Sembunyikan / 📁 Tampilkan Sidebar" toolbar button, the sidebar's `📂` heading and its `✖` button.

## Architecture

### `src/file_browser.rs` (rewritten; module name kept)

- `FsEntry` and `scan_directory` stay as they are, with their tests.
- `SidebarPalette` with `DARK`, `LIGHT` and `for_visuals(&egui::Visuals)`: the table in requirement 2.
- `Explorer`, the tree state:

  ```rust
  pub struct Explorer {
      root: PathBuf,
      section_open: bool,
      expanded: HashSet<PathBuf>,
      children: HashMap<PathBuf, Vec<FsEntry>>, // directory cache
  }

  pub struct TreeRow {
      pub entry: FsEntry,
      pub depth: usize,
      pub expanded: bool,
  }
  ```

  Methods: `new(root)`, `root()`, `toggle(&Path)`, `toggle_section()`, `collapse_all()`, `refresh()`, and `rows(&mut self) -> Vec<TreeRow>`, which flattens the visible tree in display order, reading any uncached directory it needs. `section_open` starts `true`; the root's contents are the top-level rows at depth 0.

- `show(ui, &mut Explorer, active_file: Option<&Path>) -> ExplorerResponse`, where `ExplorerResponse { open_file: Option<PathBuf>, section_rect: Rect, refresh_rect: Rect, collapse_all_rect: Rect, rows: Vec<(PathBuf, Rect)> }`. The rects say where each part was drawn; tests click them instead of hard-coded coordinates. Folder clicks, header clicks, Refresh and Collapse All are applied to the `Explorer` inside `show`.

### `src/activity_bar.rs` (new)

- `pub const WIDTH: f32 = 48.0`.
- `show(ui, explorer_open: bool) -> ActivityBarResponse { clicked: bool, explorer_rect: Rect }`.

### `src/app.rs`

- `opened_folder: Option<PathBuf>` and `show_file_browser: bool` are replaced by `explorer: Option<file_browser::Explorer>` and `show_sidebar: bool`.
- `trigger_open_folder` sets `explorer = Some(Explorer::new(folder))` and `show_sidebar = true`.
- New `toggle_explorer()`: with no explorer, calls `trigger_open_folder()`; otherwise flips `show_sidebar`.
- Panel order in `ui`: top menu (full width) → bottom status bar (full width) → `Panel::left("activity_bar")`, exactly `activity_bar::WIDTH` wide → `Panel::left("file_browser_panel")` when `show_sidebar` and an explorer exist → central editor.
- The sidebar gets the active tab's `current_path` as `active_file`, and its `open_file` goes to `open_file_in_tab`.

## Error Handling

Unchanged behaviour: an unreadable directory shows as empty; a deleted root shows an empty tree; clicking a file deleted since the last refresh reports the error in the status bar through `open_file_in_tab`.

## Testing

- **`Explorer` logic** (temporary directories):
  - `rows()` lists folders first, then files, with correct depths, and leaves out the contents of collapsed folders;
  - `toggle` expands then collapses; `collapse_all` leaves only the root's entries;
  - a file created after a directory was read stays hidden until `refresh()`, then appears; expanded folders stay expanded across `refresh()`;
  - with the section closed, `rows()` is empty.
- **Palette:** both palettes match the table; `for_visuals` follows the theme.
- **Headless sidebar UI** (egui `Context`, no window), using the returned rects:
  - clicking a folder row expands it; clicking a file row sets `open_file`;
  - the active file's row is painted with the selected background, other rows are not;
  - Refresh and Collapse All work;
  - a long name is cut and painted inside the sidebar's width.
- **Headless Activity Bar UI:** clicking the icon returns `clicked`; the accent bar is painted only when `explorer_open`.
- **App:** with a folder open, `toggle_explorer` hides and shows the sidebar (the no-folder path opens a native dialog and is not tested); opening a file from the tree opens a tab and never duplicates one.

## Out of Scope

Other Activity Bar views (search, source control), an empty-state sidebar, auto-revealing the active file in the tree, keyboard navigation of the tree, file operations (new, rename, delete), drag and drop, file-type icons beyond the text badge, watching the file system for changes.
