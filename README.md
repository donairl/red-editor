**English** | [Bahasa Indonesia](README.id.md)

---

# Red Text Editor

A lightweight desktop text editor written in Rust and [egui](https://github.com/emilk/egui) (eframe). It features a VS Code-inspired interface (Dark Modern / Light Modern themes) and multilingual UI support.

## Features

- **VS Code-style multi-tab**: Language badge on each tab, `●` indicator for unsaved files, `×` close button on hover, and `+` button for new tabs.
- **Syntax highlighting** for Rust, C, C++, PHP, JavaScript, TypeScript, HTML, CSS (also `.scss` and `.less`), JSON, Markdown, Go, Python, SQL, and the Blade (`.blade.php`), Twig (`.twig`), and Smarty (`.tpl`) templates, auto-detected from file extensions. Language can be switched manually via the **View → Language** menu. Files over 1 MB are rendered without highlighting.
- **SQL inside strings**: a string literal in Rust, C, C++, PHP, JavaScript, TypeScript, Go or Python that reads as an SQL statement (`"SELECT id FROM users"`, `` `\n  SELECT …` ``) has its SQL keywords coloured, across multi-line strings too. Upper-case keywords are enough; lower case needs a second clause (`select … from`), so UI text like "Select a file" stays a plain string.
- **Line numbers** that can be toggled on or off (**View → Line Numbers**). The active line number is highlighted.
- **Line bookmarks**: right-click a line in the editor and pick **Add Bookmark** (or **Remove Bookmark**). Bookmarked lines get a marker in the gutter and follow their line as you edit. A **Bookmarked: N** badge in the status bar opens the list of bookmarked lines; click one to jump to it. Bookmarks last for the session.
- **Word wrap** that can be toggled on or off (**View → Word Wrap**). When disabled, long lines scroll horizontally.
- **VS Code-style Explorer and Activity Bar**: Open folders, browse tree hierarchy, click files to open in tabs. Includes **Refresh** and **Collapse All** actions in the folder header.
- **Find and Replace** (**Edit → Find**, **Edit → Replace**): Toolbar above the editor with highlighted search matches, case sensitivity option, and Replace All. Also includes **Go to Line…** to jump directly to a line number.
- **Find in Files** (**Edit → Find in Files**, `Ctrl+Shift+F`): Asynchronous background text search across the opened folder. Results are displayed in the bottom panel; click a result to navigate to that line. Hidden files and folders, `target`, `node_modules`, files over 2 MB, and non-UTF-8 files are ignored; results are capped at 2,000 matches.
- **Tab context menu**: Right-click tabs for Close Tab, Close Others, Close Tabs to the Right, and Close Saved. Unsaved tabs are protected from accidental closing.
- **Unsaved changes confirmation** when closing tabs or quitting the application.
- **Multilingual interface** (English, Indonesian, Spanish, German, Chinese, and additional languages via JSON files). Select via **View → Interface Language**; preference is persisted across restarts. See [Adding interface languages](#adding-interface-languages).
- **Zen Mode** (`F11`): Distraction-free full-screen mode showing only the editor; press `Esc` to exit.
- **Prettify** (**Edit → Prettify**, `Ctrl+Shift+I`): re-indents JSON, HTML and CSS in place, and Rust, Go, C/C++, Python and JS/TS through the installed formatter (`rustfmt`, `gofmt`, `clang-format`, `ruff`/`black`, `prettier`).
- **Markdown preview** (`Ctrl+Shift+V`): rendered preview of `.md` files in a live split view beside the editor.
- System light/dark theme detection.

## Requirements

- **Rust 1.95 or later** (edition 2024). The easiest way to install Rust is via [rustup](https://rustup.rs):

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- **Linux:** System libraries required by eframe. On Debian/Ubuntu:

  ```bash
  sudo apt-get install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
  ```

  File open/save dialogs use **xdg-desktop-portal**. Ensure the portal and one of its backends (such as `xdg-desktop-portal-gtk`, `-kde`, or `-gnome`) are installed; most modern desktop environments have this preinstalled.

Build tested on Arch Linux with Rust 1.99. eframe and rfd also support Windows and macOS, but builds on those platforms have not been verified yet.

## Compile

```bash
git clone <url-repo> red-text-editor
cd red-text-editor

# Debug build and run immediately
cargo run

# Release build (optimized and smaller binary)
cargo build --release
```

The release binary will be available at `target/release/red_text_editor`.

## Installation

### Via Cargo (Recommended)

```bash
cargo install --path .
```

The binary will be installed to `~/.cargo/bin/red_text_editor`. Ensure `~/.cargo/bin` is in your `PATH` (rustup usually configures this automatically), then run:

```bash
red_text_editor
```

To uninstall:

```bash
cargo uninstall red_text_editor
```

### Manual Installation for All Users (Linux)

```bash
cargo build --release
sudo install -Dm755 target/release/red_text_editor /usr/local/bin/red_text_editor
```

To uninstall: `sudo rm /usr/local/bin/red_text_editor`.

### Desktop Application Shortcut (Linux, Optional)

To make the application appear in desktop application menus, create `~/.local/share/applications/red-text-editor.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Red Text Editor
Comment=Lightweight VS Code-style text editor
Exec=red_text_editor
Terminal=false
Categories=Utility;TextEditor;Development;
```

If installed outside `PATH`, provide the full binary path for `Exec=`, e.g. `Exec=/home/username/.cargo/bin/red_text_editor`.

## Usage

| Menu | Items |
|---|---|
| **File** | New Tab, Close Tab, Open File…, Open Folder…, Save, Save As…, Language, Exit |
| **Edit** | Undo, Redo, Cut, Copy, Paste, Find, Replace, Find in Files, Go to Line…, Prettify |
| **View** | Word Wrap, Line Numbers, Zen Mode, Markdown Preview, Language, Interface Language |
| **Help** | About |

### Keyboard Shortcuts

On macOS, `Ctrl` is mapped to `Cmd`.

| Shortcut | Action |
|---|---|
| `Ctrl+N` | New tab |
| `Ctrl+W` | Close tab |
| `Ctrl+O` | Open file |
| `Ctrl+Shift+O` | Open folder |
| `Ctrl+S` | Save |
| `Ctrl+Shift+S` | Save as |
| `Ctrl+Q` | Exit |
| `Ctrl+B` | Toggle sidebar |
| `F11` | Zen mode (full screen, editor only); `Esc` to exit |
| `Ctrl+Shift+V` | Toggle Markdown preview (`.md` files) |
| `Ctrl+Shift+I` | Prettify the document |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` or `Ctrl+Shift+Z` | Redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Cut / Copy / Paste |
| `Ctrl+F` | Find |
| `Ctrl+H` | Replace |
| `F3` / `Shift+F3` | Next / previous find match |
| `Ctrl+Shift+F` | Find in Files |
| `Ctrl+G` | Go to line |
| `Esc` | Close find bar, go-to-line bar, or Find in Files panel |

Shortcuts are also displayed alongside their corresponding menu items.

### Explorer

- Click the **Explorer** icon in the Activity Bar (leftmost strip) to open a folder. Once open, the same icon toggles the sidebar visibility.
- Click a folder in the tree to expand or collapse it; click a file to open it in a tab. Already opened files will not open duplicated tabs—their existing tab is focused instead.
- Folder contents are loaded once and cached. Click **Refresh** in the folder header if files are added or modified externally.

## Adding Interface Languages

Each language is a JSON file placed in `assets/lang/`, named after its language code (`id.json`, `en.json`, `ja.json`, etc.). It consists of a flat key-value object; `"name"` is the display name in the menu, and placeholders like `{error}` or `{name}` are dynamically formatted at runtime.

```json
{
  "name": "日本語",
  "menu.file": "ファイル",
  "menu.view": "表示",
  "msg.open_failed": "ファイルを開けませんでした: {error}"
}
```

Steps:

1. Copy `assets/lang/en.json` to `assets/lang/<code>.json` and translate the values (keep keys unchanged).
2. Place the file in the `assets/lang/` directory located **next to the executable** (e.g. `~/.cargo/bin/assets/lang/` for `cargo install`, or `/usr/local/bin/assets/lang/`), or in `assets/lang/` within the **working directory where the app is launched**.
3. Relaunch the application; the new language will appear under **View → Interface Language**.

Missing keys will fall back to Indonesian text. Files matching built-in codes (e.g., `id.json`, `en.json`, `zh.json`) override existing keys. Invalid JSON files are skipped gracefully with an error message shown in the status bar.

Indonesian, English, Spanish, German, and Chinese are embedded into the binary, ensuring they remain available even if `assets/` is omitted. On first launch, the app selects the system locale (`LANG`) if supported, defaulting to English otherwise. Chinese text utilizes system CJK fonts (e.g., Noto Sans CJK) when available.

## Development

```bash
cargo test -- --skip test_save_tab_restores_active_tab_index
cargo clippy --all-targets
```

The test `test_save_tab_restores_active_tab_index` is skipped during tests because it launches a genuine modal Save dialog and blocks `cargo test` until closed.

Source structure:

| File | Description |
|---|---|
| `src/main.rs` | Entry point, window initialization |
| `src/app.rs` | Application state, menus, panels, editor |
| `src/tab_bar.rs` | VS Code-style tab strip |
| `src/gutter.rs` | Line number gutter and bookmark markers |
| `src/bookmarks.rs` | Line bookmarks that follow edits |
| `src/prettify.rs` | Prettify: JSON/HTML/CSS in-process, other languages via external formatters |
| `src/find.rs` | Find/replace, find bar, and Go to Line bar |
| `src/search.rs` | Find in Files: directory search and results panel |
| `src/highlighter.rs` | Syntax highlighting (syntect) |
| `src/embedded_sql.rs` | SQL colouring inside string literals |
| `src/file_browser.rs` | Explorer: folder tree and sidebar |
| `src/activity_bar.rs` | Activity Bar |
| `src/modals.rs` | About dialog, unsaved changes confirmation, bookmark list |
| `src/i18n.rs` | UI localization loaded from `assets/lang/*.json` |
| `assets/lang/` | Interface language files |

Feature design specifications and implementation plans are located in `docs/superpowers/specs/` and `docs/superpowers/plans/`.
