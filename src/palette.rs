//! The Command Palette: a popup that runs any command, or opens any file
//! in the open folder, by typing part of its name.

use std::path::PathBuf;

use eframe::egui::{self, Align2, CornerRadius, Key, Modifiers, Sense, pos2, vec2};

use crate::shortcuts::ShortcutAction;
use crate::file_browser::ROW_HEIGHT;
use crate::i18n::I18n;
use crate::theme::{self, ThemeId};

/// What the palette lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteMode {
    /// Every editor command, run through `apply_shortcut`.
    #[default]
    Commands,
    /// Every file under the open folder.
    Files,
}

/// State of the Command Palette popup.
#[derive(Debug, Default)]
pub struct Palette {
    pub visible: bool,
    pub mode: PaletteMode,
    pub query: String,
    /// Put the caret in the field on the next frame.
    pub focus: bool,
    /// Index into the ranked list.
    selected: usize,
}

impl Palette {
    /// Opens in `mode` with an empty query. Opening again in the same mode
    /// closes; in the other mode switches and clears the query.
    pub fn open(&mut self, mode: PaletteMode) {
        if self.visible && self.mode == mode {
            self.visible = false;
            return;
        }
        *self = Self {
            visible: true,
            mode,
            focus: true,
            ..Self::default()
        };
    }

    /// Hides the popup.
    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Moves the selection by `delta` rows within `count` rows, no wrap.
    fn move_selection(&mut self, delta: isize, count: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
    }

    fn clamp_selection(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }
}

/// One row of the list. The app builds these each frame.
#[derive(Clone, Debug)]
pub struct Item<T> {
    pub label: String,
    /// Dimmed text at the right: the shortcut, or the folder of a file.
    pub detail: String,
    /// What the query is scored against: the label for a command, the path
    /// relative to the folder for a file (so `src/app` finds `src/app.rs`).
    pub search: String,
    pub payload: T,
}

/// What the user asked of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    /// Run this command.
    Run(ShortcutAction),
    /// Open this file in a tab.
    Open(PathBuf),
    /// Close the popup, nothing picked.
    Close,
}

/// One command the palette can run.
pub struct Command {
    /// Translation key of the label. `cmd.theme` takes a `{name}`.
    pub key: &'static str,
    pub action: ShortcutAction,
}

const fn command(key: &'static str, action: ShortcutAction) -> Command {
    Command { key, action }
}

/// Every command, in menu order: File, Edit, View, Git, then the themes.
pub const COMMANDS: &[Command] = &[
    command("file.new_tab", ShortcutAction::NewTab),
    command("file.new_window", ShortcutAction::NewWindow),
    command("file.close_tab", ShortcutAction::CloseTab),
    command("file.open_file", ShortcutAction::OpenFile),
    command("file.open_folder", ShortcutAction::OpenFolder),
    command("file.save", ShortcutAction::Save),
    command("file.save_as", ShortcutAction::SaveAs),
    command("file.exit", ShortcutAction::Exit),
    command("edit.find", ShortcutAction::Find),
    command("edit.replace", ShortcutAction::Replace),
    command("cmd.find_next", ShortcutAction::FindNext),
    command("cmd.find_prev", ShortcutAction::FindPrev),
    command("edit.find_in_files", ShortcutAction::FindInFiles),
    command("edit.goto_line", ShortcutAction::GotoLine),
    command("edit.goto_symbol", ShortcutAction::GotoSymbol),
    command("edit.goto_definition", ShortcutAction::GotoDefinition),
    command("edit.find_references", ShortcutAction::FindReferences),
    command("edit.prettify", ShortcutAction::Prettify),
    command("edit.move_line_up", ShortcutAction::MoveLineUp),
    command("edit.move_line_down", ShortcutAction::MoveLineDown),
    command("edit.copy_line_up", ShortcutAction::CopyLineUp),
    command("edit.copy_line_down", ShortcutAction::CopyLineDown),
    command("edit.delete_line", ShortcutAction::DeleteLine),
    command("edit.toggle_comment", ShortcutAction::ToggleComment),
    command("edit.select_next_match", ShortcutAction::SelectNextMatch),
    command("cmd.prev_heading", ShortcutAction::PrevHeading),
    command("cmd.next_heading", ShortcutAction::NextHeading),
    command("cmd.toggle_sidebar", ShortcutAction::ToggleSidebar),
    command("cmd.next_tab", ShortcutAction::NextTab),
    command("cmd.prev_tab", ShortcutAction::PrevTab),
    command("view.zen_mode", ShortcutAction::ZenMode),
    command("view.markdown_preview", ShortcutAction::MarkdownPreview),
    command("view.console", ShortcutAction::ToggleConsole),
    command("view.source_control", ShortcutAction::SourceControl),
    command("view.blame", ShortcutAction::Blame),
    command("view.git_graph", ShortcutAction::GitGraph),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::System)),
    command(
        "cmd.theme",
        ShortcutAction::SelectTheme(ThemeId::LightModern),
    ),
    command(
        "cmd.theme",
        ShortcutAction::SelectTheme(ThemeId::DarkModern),
    ),
    command(
        "cmd.theme",
        ShortcutAction::SelectTheme(ThemeId::SolarizedLight),
    ),
    command(
        "cmd.theme",
        ShortcutAction::SelectTheme(ThemeId::SolarizedDark),
    ),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::OceanDark)),
    command("cmd.theme", ShortcutAction::SelectTheme(ThemeId::MochaDark)),
    command(
        "cmd.theme",
        ShortcutAction::SelectTheme(ThemeId::EightiesDark),
    ),
];

/// Indices of the items matching `query`, best first; ties keep their
/// original order.
pub fn rank<T>(query: &str, items: &[Item<T>]) -> Vec<usize> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut ranked: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score_chars(&query, &item.search).map(|s| (s, i)))
        .collect();
    ranked.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// Width of the popup.
const WIDTH: f32 = 520.0;
/// Rows of the list shown at once.
const VISIBLE_ROWS: f32 = 12.0;
const PADDING: f32 = 8.0;

/// Draws the popup at the top center of the window and returns what the
/// user asked. `status` is a dimmed line under the list ("Indexing…").
/// Up and Down move the selection, Enter picks it, Escape or a click
/// outside closes. `to_action` turns the picked item's payload into the
/// action to return.
pub fn show<T>(
    ctx: &egui::Context,
    state: &mut Palette,
    items: &[Item<T>],
    status: Option<&str>,
    i18n: &I18n,
    to_action: impl Fn(&T) -> PaletteAction,
) -> Option<PaletteAction> {
    let palette = theme::active(ctx).sidebar;
    let ranked = rank(&state.query, items);
    state.clamp_selection(ranked.len());
    let mut action = None;

    let field_id = egui::Id::unique("command_palette_query");
    // Focused before it draws, so its key filter holds from the first frame.
    if std::mem::take(&mut state.focus) {
        ctx.memory_mut(|m| m.request_focus(field_id));
    }
    // Taken before the field draws, which would move its caret.
    if ctx.memory(|m| m.has_focus(field_id)) {
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            state.move_selection(-1, ranked.len());
        }
        if down {
            state.move_selection(1, ranked.len());
        }
        if enter && let Some(&index) = ranked.get(state.selected) {
            action = Some(to_action(&items[index].payload));
        }
        if escape {
            action = Some(PaletteAction::Close);
        }
    }

    let mut frame = egui::Frame::popup(&ctx.global_style());
    frame.fill = palette.background;
    frame.stroke = egui::Stroke::new(1.0, palette.border);
    let area = egui::Area::new(egui::Id::unique("command_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    if state.mode == PaletteMode::Commands {
                        ui.label(
                            egui::RichText::new(">")
                                .monospace()
                                .color(palette.icon_inactive),
                        );
                    }
                    let hint = match state.mode {
                        PaletteMode::Commands => "palette.commands_hint",
                        PaletteMode::Files => "palette.files_hint",
                    };
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut state.query)
                            .id(field_id)
                            // Up and Down pick a row instead of moving focus.
                            .event_filter(egui::EventFilter {
                                tab: true,
                                vertical_arrows: true,
                                ..Default::default()
                            })
                            .hint_text(i18n.t(hint))
                            .desired_width(f32::INFINITY),
                    );
                    if field.changed() {
                        state.selected = 0;
                    }
                });
                ui.separator();
                if ranked.is_empty() {
                    ui.weak(i18n.t("palette.no_results"));
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(ROW_HEIGHT * VISIBLE_ROWS)
                        .auto_shrink([false, true])
                        .show_rows(ui, ROW_HEIGHT, ranked.len(), |ui, rows| {
                            for row in rows {
                                let item = &items[ranked[row]];
                                let selected = row == state.selected;
                                let (rect, response) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), ROW_HEIGHT),
                                    Sense::click(),
                                );
                                if selected {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(3),
                                        palette.row_selected,
                                    );
                                } else if response.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(3),
                                        palette.row_hover,
                                    );
                                }
                                let font = egui::TextStyle::Body.resolve(ui.style());
                                let detail = truncated(
                                    ui,
                                    &item.detail,
                                    font.clone(),
                                    palette.icon_inactive,
                                    rect.width() * 0.5,
                                );
                                let label = truncated(
                                    ui,
                                    &item.label,
                                    font,
                                    palette.foreground,
                                    rect.width() - detail.size().x - 3.0 * PADDING,
                                );
                                let y = rect.center().y;
                                ui.painter().galley(
                                    pos2(rect.left() + PADDING, y - label.size().y / 2.0),
                                    label,
                                    palette.foreground,
                                );
                                ui.painter().galley(
                                    pos2(
                                        rect.right() - PADDING - detail.size().x,
                                        y - detail.size().y / 2.0,
                                    ),
                                    detail,
                                    palette.icon_inactive,
                                );
                                if selected {
                                    ui.scroll_to_rect(rect, None);
                                }
                                if response.clicked() {
                                    action = Some(to_action(&item.payload));
                                }
                            }
                        });
                }
                if let Some(status) = status {
                    ui.weak(status);
                }
            });
        });
    if action.is_none() && area.response.clicked_elsewhere() {
        action = Some(PaletteAction::Close);
    }
    action
}

/// `text` laid out on one line, cut with `…` to fit `max_width`.
fn truncated(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}

/// How well `query` matches `text`, higher is better; `None` when it does
/// not match. Case-insensitive subsequence: every char of the query must
/// appear in the text, in order. The first occurrence of each char is
/// taken (greedy), which is good enough for short queries.
///
/// A char matched at the start of a word (the start of the text, or right
/// after `/`, `\`, `_`, `-`, `.` or a space) scores 3; one right after the
/// previous match scores 2; any other 1, minus one per text char skipped
/// since the previous match. Jumping to a word start costs nothing, so
/// `tc` prefers "Toggle Console" to "Watch".
#[cfg(test)]
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    score_chars(&query, text)
}

/// `score` with the query already lowercased into chars.
fn score_chars(query: &[char], text: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut score: i64 = 0;
    let mut next = 0;
    let mut previous: Option<usize> = None;
    let mut prev_char: Option<char> = None;
    for (at, c) in text.chars().flat_map(char::to_lowercase).enumerate() {
        if next == query.len() {
            break;
        }
        if c == query[next] {
            let word_start =
                prev_char.is_none_or(|p| matches!(p, '/' | '\\' | '_' | '-' | '.' | ' '));
            let adjacent = previous.is_some_and(|p| p + 1 == at);
            score += if word_start {
                3
            } else if adjacent {
                2
            } else {
                let skipped = previous.map_or(0, |p| at - p - 1) as i64;
                1 - skipped
            };
            previous = Some(at);
            next += 1;
        }
        prev_char = Some(c);
    }
    (next == query.len()).then(|| score.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_everything_with_zero() {
        assert_eq!(score("", ""), Some(0));
        assert_eq!(score("", "Toggle Console"), Some(0));
    }

    #[test]
    fn test_chars_out_of_order_or_missing_do_not_match() {
        assert_eq!(score("xyz", "Toggle"), None);
        assert_eq!(score("ct", "Toggle Console"), None);
        assert_eq!(score("togglee", "Toggle"), None);
    }

    #[test]
    fn test_word_starts_outrank_mid_word_matches() {
        assert!(score("tc", "Toggle Console") > score("tc", "Watch"));
        assert!(score("app", "src/app.rs") > score("app", "src/wrapper.rs"));
    }

    #[test]
    fn test_adjacent_chars_outrank_spread_ones() {
        assert!(score("app", "app") > score("app", "apxp"));
    }

    #[test]
    fn test_score_ignores_case() {
        assert_eq!(score("TC", "toggle console"), score("tc", "Toggle Console"));
    }

    #[test]
    fn test_score_lowercases_non_ascii() {
        assert_eq!(score("é", "Éclair"), Some(3));
    }

    #[test]
    fn test_query_longer_than_text_does_not_match() {
        assert_eq!(score("toggle console x", "Toggle Console"), None);
    }

    #[test]
    fn test_score_never_goes_negative() {
        assert_eq!(score("ae", "axxxxxxxxxe"), Some(0));
    }

    fn items(labels: &[&str]) -> Vec<Item<usize>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| Item {
                label: l.to_string(),
                detail: String::new(),
                search: l.to_string(),
                payload: i,
            })
            .collect()
    }

    #[test]
    fn test_rank_keeps_original_order_for_empty_query_and_ties() {
        let items = items(&["b", "a", "c"]);
        assert_eq!(rank("", &items), vec![0, 1, 2]);
    }

    #[test]
    fn test_rank_sorts_by_score_then_original_order() {
        let items = items(&["Watch", "Toggle Console", "Toggle Comment", "Save"]);
        assert_eq!(rank("tc", &items), vec![1, 2, 0]);
    }

    #[test]
    fn test_open_same_mode_twice_toggles_closed() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        assert!(palette.visible && palette.focus);
        palette.open(PaletteMode::Commands);
        assert!(!palette.visible);
    }

    #[test]
    fn test_open_other_mode_switches_and_clears_query() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        palette.query = "tog".into();
        palette.selected = 2;
        palette.open(PaletteMode::Files);
        assert!(palette.visible);
        assert_eq!(palette.mode, PaletteMode::Files);
        assert_eq!(palette.query, "");
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn test_selection_moves_within_bounds() {
        let mut palette = Palette::default();
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        palette.move_selection(1, 3);
        assert_eq!(palette.selected, 2);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        palette.move_selection(-1, 3);
        assert_eq!(palette.selected, 0);
        palette.selected = 5;
        palette.clamp_selection(2);
        assert_eq!(palette.selected, 1);
        palette.clamp_selection(0);
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn test_commands_are_unique_and_leave_out_the_palette_itself() {
        let mut seen = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(
                seen.insert(command.action),
                "duplicate {:?}",
                command.action
            );
            assert!(
                !matches!(
                    command.action,
                    ShortcutAction::CommandPalette | ShortcutAction::QuickOpen
                ),
                "{:?} must not list itself",
                command.action
            );
        }
    }

    #[test]
    fn test_every_command_label_is_translated() {
        let i18n = crate::i18n::I18n::builtin();
        for command in COMMANDS {
            assert_ne!(
                i18n.t(command.key),
                command.key,
                "missing key {}",
                command.key
            );
        }
    }

    #[test]
    fn test_every_keyboard_action_is_a_command() {
        use crate::shortcuts::consume_shortcut;
        use eframe::egui::{Event, InputOptions, InputState, Key, Modifiers, RawInput};
        // Drive consume_shortcut with every key/modifier combination it
        // knows, and check each action it produces is in the table.
        let listed: std::collections::HashSet<ShortcutAction> =
            COMMANDS.iter().map(|c| c.action).collect();
        // Hand-maintained: a shortcut on a key missing here is not checked.
        let keys = [
            Key::N,
            Key::W,
            Key::O,
            Key::S,
            Key::Q,
            Key::B,
            Key::G,
            Key::Tab,
            Key::F,
            Key::I,
            Key::F11,
            Key::V,
            Key::Backtick,
            Key::H,
            Key::F3,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::K,
            Key::Slash,
            Key::D,
            Key::R,
            Key::F12,
            Key::P,
        ];
        let modifiers = [
            Modifiers::NONE,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::COMMAND,
            Modifiers::COMMAND.plus(Modifiers::SHIFT),
            Modifiers::COMMAND.plus(Modifiers::ALT),
            Modifiers::SHIFT.plus(Modifiers::ALT),
        ];
        let mut produced = 0;
        for key in keys {
            for modifiers in modifiers {
                let raw = RawInput {
                    events: vec![Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                };
                let mut input =
                    InputState::default().begin_pass(raw, false, 1.0, InputOptions::default());
                if let Some(action) = consume_shortcut(&mut input) {
                    produced += 1;
                    if !matches!(
                        action,
                        ShortcutAction::CommandPalette | ShortcutAction::QuickOpen
                    ) {
                        assert!(listed.contains(&action), "{action:?} is not in COMMANDS");
                    }
                }
            }
        }
        assert!(
            produced > 20,
            "consume_shortcut produced only {produced} actions"
        );
    }
}
