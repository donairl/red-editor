**English** | [Bahasa Indonesia](README.id.md)

---

# Red Text Editor

A lightweight desktop text editor written in Rust and [egui](https://github.com/emilk/egui) (eframe). It features a VS Code-inspired interface (Dark Modern / Light Modern themes) and multilingual UI support.

## Features

- **VS Code-style multi-tab**: Language badge on each tab, `●` indicator for unsaved files, `×` close button on hover, and `+` button for new tabs.
- **Syntax highlighting** for Rust, C, C++, PHP, JavaScript, TypeScript, HTML, CSS (also `.scss` and `.less`), JSON, Markdown, Go, Python, SQL, and the Blade (`.blade.php`), Twig (`.twig`), and Smarty (`.tpl`) templates, auto-detected from file extensions. Language can be switched manually via the **View → Language** menu. Files over 1 MB are rendered without highlighting, and files over 10 MB are not opened.
- **SQL inside strings**: a string literal in Rust, C, C++, PHP, JavaScript, TypeScript, Go or Python that reads as an SQL statement (`"SELECT id FROM users"`, `` `\n  SELECT …` ``) has its SQL keywords coloured, across multi-line strings too. Upper-case keywords are enough; lower case needs a second clause (`select … from`), so UI text like "Select a file" stays a plain string.
- **Line numbers** that can be toggled on or off (**View → Line Numbers**). The active line number is highlighted.
- **Line bookmarks**: right-click a line in the editor and pick **Add Bookmark** (or **Remove Bookmark**). Bookmarked lines get a marker in the gutter and follow their line as you edit. A **Bookmarked: N** badge in the status bar opens the list of bookmarked lines; click one to jump to it. Bookmarks last for the session.
- **Open Recent** (**File → Open Recent**): the last 20 files and folders you opened, newest first, kept across restarts. Click one to reopen it. If it no longer exists, the status bar says so and the entry is removed. **Clear Recently Opened** empties the list.
- **Restore session**: the folder and the saved files open when you quit are reopened at the next start, with the same tab active, each tab at the caret and scroll position it was left at. Untitled tabs are not kept; files that no longer exist are skipped.
- **Word wrap** that can be toggled on or off (**View → Word Wrap**). When disabled, long lines scroll horizontally.
- **Font zoom** (**View → Zoom**, `Ctrl++` / `Ctrl+-` / `Ctrl+0`, or `Ctrl` + scroll): one font size for the editor, the gutter, the diff view and the terminal. The size is remembered across restarts, and the status bar shows it as a percentage when it is not the default one.
- **Drag and drop**: drop files on the window to open them in tabs; a dropped folder becomes the workspace. While files hover over the window a hint says the drop will open them. On Linux this needs X11: winit, the window library, has no drag and drop on Wayland yet.
- **VS Code-style Explorer and Activity Bar**: Open folders, browse tree hierarchy, click files to open in tabs. Includes **Refresh** and **Collapse All** actions in the folder header.
- **Find and Replace** (**Edit → Find**, **Edit → Replace**): Toolbar above the editor with highlighted search matches, case sensitivity option, and Replace All. Also includes **Go to Line…** to jump directly to a line number.
- **Find in Files** (**Edit → Find in Files**, `Ctrl+Shift+F`): Asynchronous background text search across the opened folder. Results are grouped by file in the bottom panel, with the match highlighted on each line; click a file header to fold it, a result to open that line, or walk the results with `Up`/`Down` and `Enter` from the search field. A **files to include** field takes comma-separated globs (`*.rs`, `src/**`). Hidden files and folders, `target`, `node_modules`, files over 2 MB, and non-UTF-8 files are ignored; results are capped at 2,000 matches.
- **Tab context menu**: Right-click tabs for Close Tab, Close Others, Close Tabs to the Right, and Close Saved. Unsaved tabs are protected from accidental closing.
- **Command Palette** (`Ctrl+Shift+P`) and **Quick Open** (`Ctrl+P`): a popup at the top of the window. Type part of a command name to run it, or part of a file name or path (`src/app`) to open that file from the opened folder. `Up`/`Down` pick, `Enter` runs, `Esc` closes. Also in **View → Command Palette…** and **File → Quick Open…**.
- **Unsaved changes confirmation** when closing tabs or quitting the application.
- **Multilingual interface** (English, Indonesian, Spanish, German, Chinese, and additional languages via JSON files). Select via **View → Interface Language**; preference is persisted across restarts. See [Adding interface languages](#adding-interface-languages).
- **Color themes** (**View → Theme**): System (follows the OS), Light Modern, Dark Modern, Solarized Light, Solarized Dark, Ocean Dark, Mocha Dark and Eighties Dark. The theme recolors syntax highlighting and the whole interface at once, without a restart, and is remembered across restarts.
- **Zen Mode** (`F11`): Distraction-free full-screen mode showing only the editor; press `Esc` to exit.
- **Prettify** (**Edit → Prettify**, `Ctrl+Shift+I`): re-indents JSON, HTML and CSS in place, and Rust, Go, C/C++, Python and JS/TS through the installed formatter (`rustfmt`, `gofmt`, `clang-format`, `ruff`/`black`, `prettier`).
- **Markdown preview** (`Ctrl+Shift+V`): rendered preview of `.md` files in a live split view beside the editor.
- **Git integration** (needs `git` on the `PATH`): branch and ahead/behind counts in the status bar, changed files coloured in the Explorer, change markers in the gutter, a **Source Control** view to stage, unstage, discard, commit, pull and push, commit history with file history and blame, and a **Git Graph** of every branch and tag with lanes, a detail pane and branch actions. See [Git](#git).
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

The editor opens on a **start page** with **New File**, **Open File…** and **Open Folder…** and your recent folders and files; click one to reopen it. Closing the last tab brings the page back.

| Menu | Items |
|---|---|
| **File** | New Tab, Close Tab, New Window, Open File…, Open Folder…, Quick Open…, Open Recent, Save, Save As…, Language, Exit |
| **Edit** | Undo, Redo, Cut, Copy, Paste, Find, Replace, Find in Files, Go to Line…, Prettify |
| **View** | Word Wrap, Line Numbers, Zen Mode, Markdown Preview, Command Palette…, Zoom, Theme, Language, Interface Language |
| **Help** | About |

### Keyboard Shortcuts

On macOS, `Ctrl` is mapped to `Cmd`.

| Shortcut | Action |
|---|---|
| `Ctrl+N` | New tab |
| `Ctrl+Shift+N` | New window (a second, independent copy of the editor) |
| `Ctrl+W` | Close tab |
| `Ctrl+O` | Open file |
| `Ctrl+Shift+O` | Open folder |
| `Ctrl+S` | Save |
| `Ctrl+Shift+S` | Save as |
| `Ctrl+Q` | Exit |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+Shift+G` | Source Control view |
| `Ctrl+Alt+B` | Git Blame column |
| `Ctrl+Alt+G` | Git Graph |
| `F11` | Zen mode (full screen, editor only); `Esc` to exit |
| `Ctrl+Shift+V` | Toggle Markdown preview (`.md` files) |
| `Ctrl+Shift+I` or `Shift+Alt+F` | Prettify the document |
| `Alt+Up` / `Alt+Down` | Move the current line (or selected lines) up / down |
| `Shift+Alt+Up` / `Shift+Alt+Down` | Copy the current line (or selected lines) up / down |
| `Ctrl+Shift+K` | Delete the current line (or selected lines) |
| `Ctrl+/` | Toggle line comment, in the document's language |
| `Ctrl+D` | Select the word at the caret, then its next occurrence |
| `Ctrl+Alt+Up` / `Ctrl+Alt+Down` | Previous / next heading in the Markdown preview |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` or `Ctrl+Shift+Z` | Redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Cut / Copy / Paste |
| `Ctrl+F` | Find |
| `Ctrl+H` | Replace |
| `F3` / `Shift+F3` | Next / previous find match |
| `Ctrl+Shift+F` | Find in Files |
| `Ctrl+Shift+P` | Command Palette |
| `Ctrl+P` | Quick Open (file by name) |
| `Ctrl+G` | Go to line |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Zoom the text in / out / back to the default size |
| `Ctrl` + scroll | Zoom the text in / out |
| `Esc` | Close find bar, go-to-line bar, or Find in Files panel |

Shortcuts are also displayed alongside their corresponding menu items.

### Explorer

- Click the **Explorer** icon in the Activity Bar (leftmost strip) to open a folder. Once open, the same icon toggles the sidebar visibility.
- Click a folder in the tree to expand or collapse it; click a file to open it in a tab. Already opened files will not open duplicated tabs—their existing tab is focused instead.
- Folder contents are loaded once and cached. Click **Refresh** in the folder header if files are added or modified externally.

### Git

The editor runs the `git` installed on the machine, so your credential helper, ssh-agent and git config apply. Without git, or outside a repository, the git features stay out of the way.

- **Status bar**: the branch, `*` when files changed, and `↓behind ↑ahead` against the upstream. Click it to switch to another local branch, create one (type a name, press `Enter`), or fetch, pull and push.
- **Explorer**: changed files are coloured and end with their status letter: `M` modified, `A` added, `U` untracked, `D` deleted, `R` renamed, `C` conflicted. A folder holding changes gets a dot.
- **Gutter**: lines that differ from the last commit get a bar between the line numbers and the text, green for added and blue for modified; a red triangle marks removed lines. Markers follow your typing. Click a marker for a popup with the lines as they were (`-`) and as they are (`+`): **Revert Change** puts the old lines back in the editor (the file is not saved), **Stage Change** stages just that change, leaving the rest of the file unstaged. Escape or a click elsewhere closes it.
- **Diff view** (`Ctrl+Shift+D`, **View → Compare with HEAD**): the active document beside HEAD's copy, side by side, in place of the editor. Unsaved edits are included. **View → Compare with File…** compares it with any other file instead. Removed lines are red on the left, added lines green on the right; `↑`/`↓` step between changes, a double click opens that line in the editor, and Escape or `×` closes the view.
- **Source Control** (Activity Bar, `Ctrl+Shift+G`): type a message and press **Commit** (or `Ctrl+Enter`). Files are listed under **Staged Changes** and **Changes** (and **Merge Changes** during a conflict); hover a row or a list header for **Open File**, **Stage** (`+`), **Unstage** (`−`) and **Discard**. Discarding asks first and cannot be undone. Committing with nothing staged offers to stage everything. The title bar has **Pull**, **Push** and **Refresh**; the badge on the Activity Bar counts changed files.
- **Diff**: click a file in Source Control to open it with its changes in a panel beside the editor. Click a line in the diff to jump there.
- **Commits**: the **COMMITS** section below the changes lists the branch's history, 100 commits at a time (short hash, subject, branch and tag names, author, age). Click a commit to see the files it changed; click a file to see what the commit did to it in the diff panel, where **Restore This Version** makes the work-tree file what it was in that commit. Each commit row has a **Checkout** button that detaches HEAD at it; both ask first, and the branch menu in the status bar takes you back.
- **File History**: right-click a tab or an Explorer file and pick **File History** to limit the COMMITS section to the commits touching that file; a chip with the file name clears the filter. Clicking a commit then shows the file's diff in it directly.
- **Blame** (**View → Git Blame**, `Ctrl+Alt+B`): a column left of the line numbers names who last changed each line and how long ago, from the last commit. Hover a line for the commit's hash and message. With unsaved edits the column asks you to save first, as the lines would no longer match.
- **Git Graph** (**View → Git Graph**, `Ctrl+Alt+G`, or the graph icon in the Source Control title bar): every branch and tag as a commit graph in place of the editor, 200 commits at a time, with coloured lanes, branch and tag chips (the checked-out branch outlined), author, age and short hash. Click a commit for its message, parents (click one to jump to it) and files; a file opens its diff in the panel on the right. Right-click a commit for **Checkout Commit**, **Create Branch Here…** and **Copy Hash**; right-click a branch chip for **Switch to Branch**, **Delete Branch…** (fully merged branches only; git refuses the rest) and **Copy Name**. `×` or `Esc` returns to the editor.
- Pull only fast-forwards; if the branch has diverged, merge or rebase in the Console. Git never prompts inside the editor: a pull or push that needs a password or key tells you to run it once in the Console, with a button to open it.

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
| `src/layout_cache.rs` | Keeps the editor's text layout between frames |
| `src/bookmarks.rs` | Line bookmarks that follow edits |
| `src/history.rs` | Recently opened files and folders (File → Open Recent) |
| `src/prettify.rs` | Prettify: JSON/HTML/CSS in-process, other languages via external formatters |
| `src/find.rs` | Find/replace, find bar, and Go to Line bar |
| `src/search.rs` | Find in Files: directory search and results panel |
| `src/highlighter.rs` | Syntax highlighting (syntect) |
| `src/embedded_sql.rs` | SQL colouring inside string literals |
| `src/file_browser.rs` | Explorer: folder tree and sidebar |
| `src/activity_bar.rs` | Activity Bar |
| `src/modals.rs` | Unsaved changes confirmation, disk conflict, bookmark list |
| `src/about.rs` | About dialog: version, links, shortcuts |
| `src/git.rs`, `src/git_worker.rs` | Git through the `git` CLI, run off the UI thread |
| `src/git_graph.rs` | The Git Graph: lane layout and the view in place of the editor |
| `src/source_control.rs` | Source Control view, commit log, diff panel |
| `src/blame.rs`, `src/line_diff.rs` | Blame column and the gutter's change markers |
| `src/i18n.rs` | UI localization loaded from `assets/lang/*.json` |
| `assets/lang/` | Interface language files |

Feature design specifications and implementation plans are located in `docs/superpowers/specs/` and `docs/superpowers/plans/`.

## License

[MIT](LICENSE) © 2026 Donny Airlangga.
