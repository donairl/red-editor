//! Keyboard shortcuts of the editor. `COMMAND` is Ctrl, or Cmd on macOS.

use eframe::egui::{self, Context, Key, KeyboardShortcut, Modifiers};

use crate::theme::ThemeId;

/// The shortcut as the menus print it. egui spells `/` and `` ` `` out
/// by name, which reads oddly next to the other key caps.
pub fn label(ctx: &Context, shortcut: &KeyboardShortcut) -> String {
    ctx.format_shortcut(shortcut)
        .replace("Slash", "/")
        .replace("Backtick", "`")
        .replace("OpenBracket", "[")
        .replace("CloseBracket", "]")
}

const fn cmd(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND, key)
}
const fn cmd_shift(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), key)
}

pub const NEW_TAB: KeyboardShortcut = cmd(Key::N);
pub const NEW_WINDOW: KeyboardShortcut = cmd_shift(Key::N);
pub const CLOSE_TAB: KeyboardShortcut = cmd(Key::W);
pub const OPEN_FILE: KeyboardShortcut = cmd(Key::O);
pub const OPEN_FOLDER: KeyboardShortcut = cmd_shift(Key::O);
pub const SAVE: KeyboardShortcut = cmd(Key::S);
pub const SAVE_AS: KeyboardShortcut = cmd_shift(Key::S);
pub const EXIT: KeyboardShortcut = cmd(Key::Q);
pub const TOGGLE_SIDEBAR: KeyboardShortcut = cmd(Key::B);
pub const SOURCE_CONTROL: KeyboardShortcut = cmd_shift(Key::G);
pub const BLAME: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::B);
pub const GIT_GRAPH: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::G);
pub const NEXT_TAB: KeyboardShortcut = cmd(Key::Tab);
pub const PREV_TAB: KeyboardShortcut = cmd_shift(Key::Tab);
pub const FIND_IN_FILES: KeyboardShortcut = cmd_shift(Key::F);
pub const GOTO_LINE: KeyboardShortcut = cmd(Key::G);
pub const PRETTIFY: KeyboardShortcut = cmd_shift(Key::I);
pub const ZEN_MODE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F11);
pub const MARKDOWN_PREVIEW: KeyboardShortcut = cmd_shift(Key::V);
pub const TOGGLE_CONSOLE: KeyboardShortcut = cmd(Key::Backtick);
pub const FIND: KeyboardShortcut = cmd(Key::F);
pub const REPLACE: KeyboardShortcut = cmd(Key::H);
pub const FIND_NEXT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F3);
pub const FIND_PREV: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);
// Ctrl+Alt: Alt alone moves lines, as in other editors.
pub const PREV_HEADING: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::ArrowUp);
pub const NEXT_HEADING: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::ArrowDown);
pub const MOVE_LINE_UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowUp);
pub const MOVE_LINE_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowDown);
pub const COPY_LINE_UP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT.plus(Modifiers::ALT), Key::ArrowUp);
pub const COPY_LINE_DOWN: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT.plus(Modifiers::ALT), Key::ArrowDown);
pub const DELETE_LINE: KeyboardShortcut = cmd_shift(Key::K);
pub const TOGGLE_COMMENT: KeyboardShortcut = cmd(Key::Slash);
pub const SELECT_NEXT_MATCH: KeyboardShortcut = cmd(Key::D);
pub const GO_TO_BRACKET: KeyboardShortcut = cmd_shift(Key::Backslash);
/// `GO_TO_BRACKET` as egui hears it on layouts where Shift turns `\` into
/// `|` (US and others): it reports the shifted key.
const GO_TO_BRACKET_SHIFTED: KeyboardShortcut = cmd_shift(Key::Pipe);
pub const FOLD: KeyboardShortcut = cmd_shift(Key::OpenBracket);
pub const UNFOLD: KeyboardShortcut = cmd_shift(Key::CloseBracket);
/// `FOLD` and `UNFOLD` as egui hears them on layouts where Shift turns `[`
/// and `]` into `{` and `}` (US and others).
const FOLD_SHIFTED: KeyboardShortcut = cmd_shift(Key::OpenCurlyBracket);
const UNFOLD_SHIFTED: KeyboardShortcut = cmd_shift(Key::CloseCurlyBracket);
/// A second key for Prettify, the one other editors use.
pub const FORMAT_DOCUMENT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT.plus(Modifiers::ALT), Key::F);
pub const GOTO_SYMBOL: KeyboardShortcut = cmd(Key::R);
pub const GOTO_DEFINITION: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F12);
pub const FIND_REFERENCES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F12);
pub const COMMAND_PALETTE: KeyboardShortcut = cmd_shift(Key::P);
pub const QUICK_OPEN: KeyboardShortcut = cmd(Key::P);
// Shown in the Edit menu only: the focused text editor handles these
// keys itself.
pub const UNDO: KeyboardShortcut = cmd(Key::Z);
pub const REDO: KeyboardShortcut = cmd(Key::Y);
pub const CUT: KeyboardShortcut = cmd(Key::X);
pub const COPY: KeyboardShortcut = cmd(Key::C);
pub const PASTE: KeyboardShortcut = cmd(Key::V);

/// What a keyboard shortcut asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    NewTab,
    NewWindow,
    CloseTab,
    OpenFile,
    OpenFolder,
    Save,
    SaveAs,
    Exit,
    ToggleSidebar,
    SourceControl,
    Blame,
    GitGraph,
    NextTab,
    PrevTab,
    GotoLine,
    Prettify,
    ZenMode,
    MarkdownPreview,
    ToggleConsole,
    FindInFiles,
    Find,
    Replace,
    FindNext,
    FindPrev,
    PrevHeading,
    NextHeading,
    GotoSymbol,
    GotoDefinition,
    FindReferences,
    MoveLineUp,
    MoveLineDown,
    CopyLineUp,
    CopyLineDown,
    DeleteLine,
    ToggleComment,
    SelectNextMatch,
    /// Put the caret by the partner of the bracket it touches.
    GoToBracket,
    /// Fold the innermost unfolded block holding the caret's line.
    Fold,
    /// Unfold the innermost folded block holding the caret's line.
    Unfold,
    /// Fold every foldable block (a menu and palette entry; no key).
    FoldAll,
    /// Unfold every block (a menu and palette entry; no key).
    UnfoldAll,
    /// Open the Command Palette listing every command.
    CommandPalette,
    /// Open the Command Palette listing the folder's files.
    QuickOpen,
    /// Switch to this color theme (a Command Palette entry; no key).
    SelectTheme(ThemeId),
}

impl ShortcutAction {
    /// Opens a bar, a dialog or a panel that takes the keyboard, so the
    /// editor must not ask for focus in the same frame.
    pub fn takes_keyboard(self) -> bool {
        use ShortcutAction::*;
        matches!(
            self,
            Find | Replace
                | GotoLine
                | GotoSymbol
                | FindInFiles
                | ToggleConsole
                | SourceControl
                | GitGraph
                | OpenFile
                | OpenFolder
                | SaveAs
                | NewWindow
                | Exit
                | CommandPalette
                | QuickOpen
        )
    }

    /// The key the action is bound to, as the menus print it; `None` for
    /// actions only the Command Palette offers.
    pub fn shortcut(self) -> Option<egui::KeyboardShortcut> {
        use ShortcutAction::*;
        Some(match self {
            NewTab => NEW_TAB,
            NewWindow => NEW_WINDOW,
            CloseTab => CLOSE_TAB,
            OpenFile => OPEN_FILE,
            OpenFolder => OPEN_FOLDER,
            Save => SAVE,
            SaveAs => SAVE_AS,
            Exit => EXIT,
            ToggleSidebar => TOGGLE_SIDEBAR,
            SourceControl => SOURCE_CONTROL,
            Blame => BLAME,
            GitGraph => GIT_GRAPH,
            NextTab => NEXT_TAB,
            PrevTab => PREV_TAB,
            GotoLine => GOTO_LINE,
            Prettify => PRETTIFY,
            ZenMode => ZEN_MODE,
            MarkdownPreview => MARKDOWN_PREVIEW,
            ToggleConsole => TOGGLE_CONSOLE,
            FindInFiles => FIND_IN_FILES,
            Find => FIND,
            Replace => REPLACE,
            FindNext => FIND_NEXT,
            FindPrev => FIND_PREV,
            PrevHeading => PREV_HEADING,
            NextHeading => NEXT_HEADING,
            GotoSymbol => GOTO_SYMBOL,
            GotoDefinition => GOTO_DEFINITION,
            FindReferences => FIND_REFERENCES,
            MoveLineUp => MOVE_LINE_UP,
            MoveLineDown => MOVE_LINE_DOWN,
            CopyLineUp => COPY_LINE_UP,
            CopyLineDown => COPY_LINE_DOWN,
            DeleteLine => DELETE_LINE,
            ToggleComment => TOGGLE_COMMENT,
            SelectNextMatch => SELECT_NEXT_MATCH,
            GoToBracket => GO_TO_BRACKET,
            Fold => FOLD,
            Unfold => UNFOLD,
            CommandPalette => COMMAND_PALETTE,
            QuickOpen => QUICK_OPEN,
            SelectTheme(_) | FoldAll | UnfoldAll => return None,
        })
    }

    /// Acts on the editor, which the Git Graph and the start page hide.
    pub fn needs_editor(self) -> bool {
        use ShortcutAction::*;
        matches!(
            self,
            Find | Replace
                | FindNext
                | FindPrev
                | GotoLine
                | GotoSymbol
                | GotoDefinition
                | FindReferences
                | Prettify
                | PrevHeading
                | NextHeading
                | MoveLineUp
                | MoveLineDown
                | CopyLineUp
                | CopyLineDown
                | DeleteLine
                | ToggleComment
                | SelectNextMatch
                | GoToBracket
                | Fold
                | Unfold
                | FoldAll
                | UnfoldAll
        )
    }
}

/// Takes at most one shortcut out of this frame's input. Shift variants are
/// tried first: egui lets `Ctrl+S` match while Shift is also held.
pub fn consume_shortcut(input: &mut egui::InputState) -> Option<ShortcutAction> {
    use ShortcutAction::*;
    [
        (COMMAND_PALETTE, CommandPalette),
        (FIND_PREV, FindPrev),
        (FIND_REFERENCES, FindReferences),
        (FIND_NEXT, FindNext),
        (FIND_IN_FILES, FindInFiles),
        (FORMAT_DOCUMENT, Prettify),
        (PREV_HEADING, PrevHeading),
        (NEXT_HEADING, NextHeading),
        (COPY_LINE_UP, CopyLineUp),
        (COPY_LINE_DOWN, CopyLineDown),
        (MOVE_LINE_UP, MoveLineUp),
        (MOVE_LINE_DOWN, MoveLineDown),
        (DELETE_LINE, DeleteLine),
        (TOGGLE_COMMENT, ToggleComment),
        (SELECT_NEXT_MATCH, SelectNextMatch),
        (GO_TO_BRACKET, GoToBracket),
        (GO_TO_BRACKET_SHIFTED, GoToBracket),
        (FOLD, Fold),
        (FOLD_SHIFTED, Fold),
        (UNFOLD, Unfold),
        (UNFOLD_SHIFTED, Unfold),
        (MARKDOWN_PREVIEW, MarkdownPreview),
        (TOGGLE_CONSOLE, ToggleConsole),
        (PRETTIFY, Prettify),
        (SAVE_AS, SaveAs),
        (OPEN_FOLDER, OpenFolder),
        (NEW_WINDOW, NewWindow),
        (PREV_TAB, PrevTab),
        (SOURCE_CONTROL, SourceControl),
        (BLAME, Blame),
        (GIT_GRAPH, GitGraph),
        (SAVE, Save),
        (OPEN_FILE, OpenFile),
        (NEW_TAB, NewTab),
        (CLOSE_TAB, CloseTab),
        (EXIT, Exit),
        (TOGGLE_SIDEBAR, ToggleSidebar),
        (NEXT_TAB, NextTab),
        (GOTO_LINE, GotoLine),
        (ZEN_MODE, ZenMode),
        (FIND, Find),
        (REPLACE, Replace),
        (GOTO_SYMBOL, GotoSymbol),
        (GOTO_DEFINITION, GotoDefinition),
        (QUICK_OPEN, QuickOpen),
    ]
    .into_iter()
    .find(|(shortcut, _)| input.consume_shortcut(shortcut))
    .map(|(_, action)| action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fold_labels_show_the_bracket_keys() {
        let ctx = egui::Context::default();
        assert!(label(&ctx, &FOLD).ends_with('['), "{}", label(&ctx, &FOLD));
        assert!(label(&ctx, &UNFOLD).ends_with(']'), "{}", label(&ctx, &UNFOLD));
    }

    #[test]
    fn test_consume_shortcut_prefers_shift_variant() {
        let ctx = egui::Context::default();
        let press = |key, modifiers| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let ctrl = egui::Modifiers::COMMAND;
        let ctrl_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        for (key, mods, want) in [
            (egui::Key::S, ctrl, ShortcutAction::Save),
            (egui::Key::S, ctrl_shift, ShortcutAction::SaveAs),
            (egui::Key::O, ctrl, ShortcutAction::OpenFile),
            (egui::Key::O, ctrl_shift, ShortcutAction::OpenFolder),
            (egui::Key::Tab, ctrl, ShortcutAction::NextTab),
            (egui::Key::Tab, ctrl_shift, ShortcutAction::PrevTab),
            (egui::Key::Q, ctrl, ShortcutAction::Exit),
            (egui::Key::N, ctrl, ShortcutAction::NewTab),
            (egui::Key::N, ctrl_shift, ShortcutAction::NewWindow),
            (egui::Key::V, ctrl_shift, ShortcutAction::MarkdownPreview),
            (
                egui::Key::G,
                ctrl | egui::Modifiers::ALT,
                ShortcutAction::GitGraph,
            ),
            (egui::Key::G, ctrl, ShortcutAction::GotoLine),
            (egui::Key::G, ctrl_shift, ShortcutAction::SourceControl),
            (
                egui::Key::ArrowUp,
                ctrl | egui::Modifiers::ALT,
                ShortcutAction::PrevHeading,
            ),
            (
                egui::Key::ArrowDown,
                ctrl | egui::Modifiers::ALT,
                ShortcutAction::NextHeading,
            ),
            (
                egui::Key::ArrowUp,
                egui::Modifiers::ALT,
                ShortcutAction::MoveLineUp,
            ),
            (
                egui::Key::ArrowDown,
                egui::Modifiers::ALT,
                ShortcutAction::MoveLineDown,
            ),
            (
                egui::Key::ArrowUp,
                egui::Modifiers::SHIFT | egui::Modifiers::ALT,
                ShortcutAction::CopyLineUp,
            ),
            (
                egui::Key::ArrowDown,
                egui::Modifiers::SHIFT | egui::Modifiers::ALT,
                ShortcutAction::CopyLineDown,
            ),
            (egui::Key::K, ctrl_shift, ShortcutAction::DeleteLine),
            (egui::Key::Slash, ctrl, ShortcutAction::ToggleComment),
            (egui::Key::D, ctrl, ShortcutAction::SelectNextMatch),
            (egui::Key::Backslash, ctrl_shift, ShortcutAction::GoToBracket),
            // What a US layout reports for Shift+\.
            (egui::Key::Pipe, ctrl_shift, ShortcutAction::GoToBracket),
            (egui::Key::OpenBracket, ctrl_shift, ShortcutAction::Fold),
            (egui::Key::CloseBracket, ctrl_shift, ShortcutAction::Unfold),
            // What a US layout reports for Shift+[ and Shift+].
            (egui::Key::OpenCurlyBracket, ctrl_shift, ShortcutAction::Fold),
            (egui::Key::CloseCurlyBracket, ctrl_shift, ShortcutAction::Unfold),
            (
                egui::Key::F,
                egui::Modifiers::SHIFT | egui::Modifiers::ALT,
                ShortcutAction::Prettify,
            ),
            (egui::Key::F, ctrl_shift, ShortcutAction::FindInFiles),
            (egui::Key::F, ctrl, ShortcutAction::Find),
            (egui::Key::R, ctrl, ShortcutAction::GotoSymbol),
            (
                egui::Key::F12,
                egui::Modifiers::NONE,
                ShortcutAction::GotoDefinition,
            ),
            (
                egui::Key::F12,
                egui::Modifiers::SHIFT,
                ShortcutAction::FindReferences,
            ),
        ] {
            let mut got = None;
            let mut output = ctx.run_ui(press(key, mods), |ui| {
                got = ui.ctx().input_mut(consume_shortcut);
            });
            output.textures_delta.clear();
            assert_eq!(got, Some(want), "{key:?} {mods:?}");
        }
    }
}
