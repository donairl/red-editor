//! The Agent panel on the right of the window: the transcript of an ACP
//! session, a prompt box with Send / Stop, and the controls to start a new
//! session or restart the agent. The protocol lives in `crate::acp`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::acp::client::{Client, Phase};
use crate::agent_cards::{self, CardAction};
use crate::acp::context::{self, Chip};
use crate::acp::model::{Entry, Notice};
use crate::palette::{self, Item};
use crate::acp::transport::AgentProcess;
use crate::agent_settings::CommandError;
use crate::app::TextEditorApp;
use crate::i18n::I18n;

/// Stderr lines shown under "Agent log".
const LOG_LINES: usize = 200;

/// Why there is no agent to talk to.
#[derive(Clone, Debug, PartialEq)]
pub enum StartError {
    NoCommand,
    UnclosedQuote,
    NoFolder,
    NotFound(String),
    Io(String),
}

#[derive(Default)]
pub struct AgentPanel {
    pub client: Option<Client>,
    pub start_error: Option<StartError>,
    pub input: String,
    /// Files the agent wrote through the editor; only the unsaved ones are
    /// kept (see `agent_unsaved_paths`).
    pub edits: Vec<std::path::PathBuf>,
    /// Files and selections attached to the next prompt.
    pub chips: Vec<Chip>,
    /// The `@` file picker, when open.
    picker: Option<Picker>,
    /// Translation key of a one-line message under the prompt.
    note: Option<&'static str>,
    show_log: bool,
    markdown: CommonMarkCache,
}

/// The `@` file picker: a query over the workspace's files.
#[derive(Default)]
struct Picker {
    query: String,
    selected: usize,
    /// Set on the frame it opens, to focus the query.
    fresh: bool,
}

/// Rows of the picker's list.
const PICKER_ROWS: usize = 8;

impl AgentPanel {
    /// Attaches a chip unless the same one is there already.
    fn add_chip(&mut self, chip: Chip) {
        if !self.chips.contains(&chip) {
            self.chips.push(chip);
        }
    }

    fn remove_chip(&mut self, index: usize) {
        if index < self.chips.len() {
            self.chips.remove(index);
        }
    }
}

/// What a click in the panel asks the app to do after drawing.
#[derive(Default)]
struct Clicks {
    close: bool,
    restart: bool,
    new_session: bool,
    send: bool,
    stop: bool,
    open_settings: bool,
    open_folder: bool,
    save_all: bool,
    sign_in: Option<String>,
    card: Option<CardAction>,
    open_picker: bool,
    close_picker: bool,
    pick: Option<PathBuf>,
    remove_chip: Option<usize>,
}

impl TextEditorApp {
    pub fn toggle_agent_panel(&mut self, ctx: &egui::Context) {
        self.show_agent = !self.show_agent;
        if self.show_agent {
            if self.agent.client.is_none() {
                self.start_agent(ctx);
            }
            ctx.memory_mut(|m| m.request_focus(input_id()));
        } else {
            self.editor_request.focus = true;
        }
    }

    /// Starts (or restarts) the agent from Settings, in the workspace folder.
    pub fn start_agent(&mut self, ctx: &egui::Context) {
        // Dropping the old client cancels its turn and kills its process.
        self.agent.client = None;
        self.agent.start_error = None;
        let (program, args) = match self.agent_settings.program_and_args() {
            Ok(command) => command,
            Err(CommandError::Empty) => {
                self.agent.start_error = Some(StartError::NoCommand);
                return;
            }
            Err(CommandError::UnclosedQuote) => {
                self.agent.start_error = Some(StartError::UnclosedQuote);
                return;
            }
        };
        let Some(root) = self.explorer.as_ref().map(|e| e.root().to_path_buf()) else {
            self.agent.start_error = Some(StartError::NoFolder);
            return;
        };
        let repaint = ctx.clone();
        let waker = Arc::new(move || repaint.request_repaint());
        match AgentProcess::spawn(&program, &args, &root, waker) {
            Ok((conn, process)) => {
                self.agent.client = Some(Client::new(conn, Some(process), root, Instant::now()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.agent.start_error = Some(StartError::NotFound(program));
            }
            Err(error) => self.agent.start_error = Some(StartError::Io(error.to_string())),
        }
    }

    /// A fresh conversation: a new session on the running agent, or a new
    /// agent when it cannot take one.
    pub fn new_agent_session(&mut self, ctx: &egui::Context) {
        match &mut self.agent.client {
            Some(client) if client.can_start_session() => client.new_session(Instant::now()),
            _ => self.start_agent(ctx),
        }
        self.show_agent = true;
        ctx.memory_mut(|m| m.request_focus(input_id()));
    }

    /// Opens a file the agent named in a tool call, at its line. A relative
    /// path is taken from the workspace folder.
    pub fn open_agent_location(&mut self, path: std::path::PathBuf, line: Option<u32>) {
        let path = if path.is_relative()
            && let Some(root) = self.explorer.as_ref().map(|e| e.root())
        {
            root.join(path)
        } else {
            path
        };
        self.open_file_in_tab(path.clone());
        if let Some(line) = line
            && self.active_tab().current_path.as_ref() == Some(&path)
        {
            self.goto_line(line as usize);
        }
    }

    /// Once per frame, shown or not: the agent keeps working while hidden.
    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        if let Some(client) = &mut self.agent.client {
            let now = Instant::now();
            client.poll(now);
        }
        // Answered here, on the UI thread, where the tabs are.
        self.serve_agent_fs();
        if let Some(client) = &mut self.agent.client {
            let now = Instant::now();
            if client.wants_repaint(now) {
                ctx.request_repaint_after(Duration::from_millis(100));
            } else if !client.is_finished() {
                // Notice a child that ends without a word (a process it
                // started still holds its pipes) even while the app idles.
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    pub(crate) fn show_agent_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut clicks = Clicks::default();
        let unsaved = self.agent_unsaved_paths();
        let (picker_items, indexing) = self.picker_items(&ctx);
        let i18n = &self.i18n;
        let AgentPanel { client, start_error, input, show_log, markdown, edits: _, chips, picker, note } =
            &mut self.agent;
        egui::Panel::right("agent_panel")
            .resizable(true)
            .default_size(380.0)
            .min_size(260.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(i18n.t("activity.agent"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clicks.close = ui.small_button("✕").on_hover_text(i18n.t("agent.close")).clicked();
                        clicks.restart =
                            ui.small_button("⟲").on_hover_text(i18n.t("agent.restart")).clicked();
                        clicks.new_session =
                            ui.small_button("+").on_hover_text(i18n.t("agent.new_session")).clicked();
                        if client.is_some() {
                            ui.toggle_value(show_log, i18n.t("agent.show_log"));
                        }
                        if !unsaved.is_empty() {
                            clicks.save_all = ui
                                .small_button(i18n.t("agent.save_all"))
                                .on_hover_text(i18n.t("agent.save_all_hint"))
                                .clicked();
                        }
                    });
                });
                ui.separator();

                if let Some(error) = start_error {
                    let message = match error {
                        StartError::NoCommand => i18n.t("agent.no_command").to_owned(),
                        StartError::UnclosedQuote => i18n.t("agent.unclosed_quote").to_owned(),
                        StartError::NoFolder => i18n.t("agent.no_folder").to_owned(),
                        StartError::NotFound(program) => {
                            i18n.tf("agent.not_found", &[("program", program)])
                        }
                        StartError::Io(error) => i18n.tf("agent.start_failed", &[("error", error)]),
                    };
                    ui.label(message);
                    ui.horizontal(|ui| {
                        if matches!(error, StartError::NoFolder) {
                            clicks.open_folder = ui.button(i18n.t("agent.open_folder")).clicked();
                        } else {
                            clicks.open_settings = ui.button(i18n.t("agent.open_settings")).clicked();
                        }
                        clicks.restart |= ui.button(i18n.t("agent.restart")).clicked();
                    });
                    return;
                }
                // Restored open from the last run: nothing started yet.
                let Some(client) = client else {
                    clicks.restart |= ui.button(i18n.t("agent.restart")).clicked();
                    return;
                };

                // Bottom up: the prompt box first, then the transcript fills
                // what is left above it.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let has_content = !input.trim().is_empty() || !chips.is_empty();
                    ui.horizontal(|ui| {
                        clicks.open_picker |= ui
                            .small_button("@")
                            .on_hover_text(i18n.t("agent.attach_file"))
                            .clicked();
                        if client.phase == Phase::Prompting {
                            clicks.stop = ui.button(i18n.t("agent.stop")).clicked();
                        } else {
                            let can_send = client.phase == Phase::Ready && has_content;
                            clicks.send |= ui
                                .add_enabled(can_send, egui::Button::new(i18n.t("agent.send")))
                                .clicked();
                        }
                    });
                    // Enter sends, Shift+Enter is a newline: take the plain
                    // Enter out of the input before the text box sees it.
                    let focused = ui.memory(|m| m.has_focus(input_id()));
                    if focused && client.phase == Phase::Ready && has_content {
                        clicks.send |= ui.input_mut(|i| {
                            let before = i.events.len();
                            i.events.retain(|event| {
                                !matches!(event, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                                    if !modifiers.shift)
                            });
                            i.events.len() != before
                        });
                    }
                    let response = ui.add(
                        egui::TextEdit::multiline(input)
                            .id(input_id())
                            .hint_text(i18n.t("agent.placeholder"))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    // Typing `@` (at the start or after a space) opens the
                    // picker instead.
                    if response.changed() && take_trailing_at(input) {
                        clicks.open_picker = true;
                    }
                    if !chips.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for (i, chip) in chips.iter().enumerate() {
                                egui::Frame::new()
                                    .fill(ui.visuals().faint_bg_color)
                                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                                    .corner_radius(8)
                                    .inner_margin(egui::Margin::symmetric(6, 1))
                                    .show(ui, |ui| {
                                        ui.label(chip.label()).on_hover_text(chip.path.display().to_string());
                                        if ui
                                            .small_button("✕")
                                            .on_hover_text(i18n.t("agent.remove_context"))
                                            .clicked()
                                        {
                                            clicks.remove_chip = Some(i);
                                        }
                                    });
                            }
                        });
                    }
                    if let Some(key) = note {
                        ui.colored_label(ui.visuals().warn_fg_color, i18n.t(key));
                    }
                    if let Some(picker) = picker {
                        draw_picker(ui, i18n, picker, &picker_items, indexing, &mut clicks);
                    }
                    ui.separator();

                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        agent_cards::draw_plan(ui, i18n, &client.transcript.plan);
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink(false)
                            .show(ui, |ui| {
                                for (i, entry) in client.transcript.entries.iter().enumerate() {
                                    ui.push_id(i, |ui| {
                                        if let Some(action) = draw_entry(ui, i18n, markdown, entry, &unsaved) {
                                            clicks.card = Some(action);
                                        }
                                    });
                                    ui.add_space(6.0);
                                }
                                draw_phase(ui, i18n, &client.phase, &mut clicks);
                                if *show_log {
                                    ui.separator();
                                    let log = client.stderr_tail(LOG_LINES).join("\n");
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(log).monospace().small())
                                            .wrap(),
                                    );
                                }
                            });
                    });
                });
            });

        let now = Instant::now();
        if clicks.close {
            self.toggle_agent_panel(&ctx);
        }
        if clicks.restart {
            self.start_agent(&ctx);
        }
        if clicks.new_session {
            self.new_agent_session(&ctx);
        }
        if clicks.open_settings {
            self.show_settings = true;
        }
        if clicks.open_folder {
            self.trigger_open_folder();
        }
        match clicks.card.take() {
            Some(CardAction::Open(path, line)) => self.open_agent_location(path, line),
            Some(CardAction::Permission { rpc_id, option_id }) => {
                if let Some(client) = &mut self.agent.client {
                    client.answer_permission(&rpc_id, &option_id);
                }
            }
            Some(CardAction::Diff(path)) => self.diff_agent_edit(&ctx, path),
            Some(CardAction::Save(path)) => self.save_agent_edit(&path),
            None => {}
        }
        if clicks.save_all {
            self.save_all_agent_edits();
        }
        if let Some(i) = clicks.remove_chip {
            self.agent.remove_chip(i);
        }
        if clicks.open_picker {
            self.agent.picker = Some(Picker { fresh: true, ..Picker::default() });
        }
        if clicks.close_picker || clicks.pick.is_some() {
            self.agent.picker = None;
            ctx.memory_mut(|m| m.request_focus(input_id()));
        }
        if let Some(path) = clicks.pick.take() {
            self.add_agent_chip(Chip::file(path));
        }
        if let Some(client) = &mut self.agent.client {
            if clicks.stop {
                client.cancel();
            }
            if let Some(method) = clicks.sign_in {
                client.authenticate(&method, now);
            }
            if clicks.send {
                let text = std::mem::take(&mut self.agent.input);
                if client.send_prompt_with(&self.agent.chips, text.trim(), now) {
                    self.agent.chips.clear();
                    self.agent.note = None;
                } else {
                    self.agent.input = text;
                }
            }
        }
    }
}

impl TextEditorApp {
    /// The workspace's files for the `@` picker (empty while it is closed),
    /// and whether the index is still running.
    fn picker_items(&mut self, ctx: &egui::Context) -> (Vec<Item<PathBuf>>, bool) {
        if self.agent.picker.is_none() {
            return (Vec::new(), false);
        }
        let Some(explorer) = self.explorer.as_mut() else { return (Vec::new(), false) };
        let root = explorer.root().to_path_buf();
        let (files, done) = explorer.indexed_files(ctx);
        let items = files
            .iter()
            .map(|f| {
                let relative = f.path.strip_prefix(&root).unwrap_or(&f.path);
                let folder = relative.parent().map(|p| p.display().to_string()).unwrap_or_default();
                Item {
                    label: f.name.clone(),
                    detail: folder,
                    search: relative.display().to_string(),
                    payload: f.path.clone(),
                }
            })
            .collect();
        (items, !done)
    }

    fn add_agent_chip(&mut self, chip: Chip) {
        self.agent.add_chip(chip);
    }

    /// Add Selection to Agent: attaches the editor's selection (the caret's
    /// line when nothing is selected) and shows the panel.
    pub fn add_selection_to_agent(&mut self, ctx: &egui::Context) {
        let note = self.try_add_selection(ctx);
        self.agent.note = note;
        if !self.show_agent {
            self.toggle_agent_panel(ctx);
        } else {
            ctx.memory_mut(|m| m.request_focus(input_id()));
        }
    }

    /// `Some(key)` of the message to show when nothing was attached.
    fn try_add_selection(&mut self, ctx: &egui::Context) -> Option<&'static str> {
        let Some(path) = self.active_tab().current_path.clone() else {
            return Some("agent.context_unsaved");
        };
        let (a, b) = self.editor_selection(ctx)?;
        let text = &self.active_tab().text_content;
        let (start, end, selected) = if a == b {
            // Nothing selected: the caret's whole line.
            let line = crate::find::line_col(text, a).0;
            let row = text.split('\n').nth(line - 1).unwrap_or("");
            (line as u32, line as u32, row.to_owned())
        } else {
            let (start, end) = context::selection_lines(text, a, b);
            (start, end, text.chars().skip(a).take(b - a).collect())
        };
        if selected.len() > context::MAX_SELECTION_BYTES {
            return Some("agent.context_too_large");
        }
        self.add_agent_chip(Chip::selection(path, start, end, selected));
        None
    }
}

/// Removes a `@` the user just typed at the end of `input` (at the start or
/// after whitespace); `true` when there was one.
fn take_trailing_at(input: &mut String) -> bool {
    let Some(before) = input.strip_suffix('@') else { return false };
    if before.is_empty() || before.ends_with(char::is_whitespace) {
        input.pop();
        return true;
    }
    false
}

/// The `@` picker: a query field and the best matches. Up and Down move,
/// Enter picks, Escape closes.
fn draw_picker(
    ui: &mut egui::Ui,
    i18n: &I18n,
    picker: &mut Picker,
    items: &[Item<PathBuf>],
    indexing: bool,
    clicks: &mut Clicks,
) {
    let ranked = palette::rank(&picker.query, items);
    picker.selected = picker.selected.min(ranked.len().saturating_sub(1));
    let field_id = picker_id();
    if std::mem::take(&mut picker.fresh) {
        ui.memory_mut(|m| m.request_focus(field_id));
    }
    if ui.memory(|m| m.has_focus(field_id)) {
        let (up, down, enter, escape) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if up {
            picker.selected = picker.selected.saturating_sub(1);
        }
        if down {
            picker.selected = (picker.selected + 1).min(ranked.len().saturating_sub(1));
        }
        if enter && let Some(&index) = ranked.get(picker.selected) {
            clicks.pick = Some(items[index].payload.clone());
        }
        if escape {
            clicks.close_picker = true;
        }
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label("@");
            ui.add(
                egui::TextEdit::singleline(&mut picker.query)
                    .id(field_id)
                    .hint_text(i18n.t("agent.pick_file"))
                    .desired_width(f32::INFINITY),
            );
        });
        for (row, &index) in ranked.iter().take(PICKER_ROWS).enumerate() {
            let item = &items[index];
            let text = if item.detail.is_empty() {
                item.label.clone()
            } else {
                format!("{}  {}", item.label, item.detail)
            };
            if ui.selectable_label(row == picker.selected, text).clicked() {
                clicks.pick = Some(item.payload.clone());
            }
        }
        if indexing {
            ui.label(egui::RichText::new(i18n.t("palette.indexing")).weak().small());
        } else if ranked.is_empty() {
            ui.label(egui::RichText::new(i18n.t("agent.no_files")).weak().small());
        }
    });
}

fn picker_id() -> egui::Id {
    egui::Id::unique("agent_file_picker")
}

fn input_id() -> egui::Id {
    egui::Id::unique("agent_prompt_input")
}

fn draw_entry(
    ui: &mut egui::Ui,
    i18n: &I18n,
    markdown: &mut CommonMarkCache,
    entry: &Entry,
    unsaved: &[std::path::PathBuf],
) -> Option<CardAction> {
    match entry {
        Entry::User(text) => {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(4)
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(text).selectable(true).wrap());
                });
        }
        Entry::Attached(chips) => {
            let mut action = None;
            ui.horizontal_wrapped(|ui| {
                for chip in chips {
                    if ui.small_button(&chip.label).on_hover_text(chip.path.display().to_string()).clicked() {
                        action = Some(CardAction::Open(chip.path.clone(), chip.line));
                    }
                }
            });
            return action;
        }
        Entry::AgentText(text) => {
            CommonMarkViewer::new().show(ui, markdown, text);
        }
        Entry::Thought(text) => {
            egui::CollapsingHeader::new(i18n.t("agent.thought"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
                });
        }
        Entry::Notice(Notice::Error(message)) => {
            ui.colored_label(ui.visuals().error_fg_color, i18n.tf("agent.failed", &[("error", message)]));
        }
        Entry::TurnEnd(reason) => {
            ui.label(egui::RichText::new(i18n.tf("agent.turn_end", &[("reason", reason)])).weak().small());
        }
        Entry::ToolCall(call) => return agent_cards::draw_tool_call(ui, i18n, markdown, call, unsaved),
        Entry::Permission(card) => return agent_cards::draw_permission(ui, i18n, card),
    }
    None
}

fn draw_phase(ui: &mut egui::Ui, i18n: &I18n, phase: &Phase, clicks: &mut Clicks) {
    match phase {
        Phase::Initializing | Phase::Authenticating | Phase::CreatingSession => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(i18n.t("agent.starting"));
            });
        }
        Phase::Prompting => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(i18n.t("agent.working")).weak());
            });
        }
        Phase::NeedsAuth(choices) => {
            ui.label(i18n.t("agent.sign_in"));
            for choice in choices {
                let mut button = ui.button(&choice.name);
                if let Some(description) = &choice.description {
                    button = button.on_hover_text(description);
                }
                if button.clicked() {
                    clicks.sign_in = Some(choice.id.clone());
                }
            }
        }
        Phase::Ready => {}
        Phase::Failed(message) => {
            ui.colored_label(ui.visuals().error_fg_color, i18n.tf("agent.failed", &[("error", message)]));
            clicks.restart |= ui.button(i18n.t("agent.restart")).clicked();
        }
        Phase::Exited { code, stderr } => {
            let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
            ui.colored_label(ui.visuals().error_fg_color, i18n.tf("agent.exited", &[("code", &code)]));
            if !stderr.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(stderr.join("\n")).monospace().small()).wrap());
            }
            clicks.restart |= ui.button(i18n.t("agent.restart")).clicked();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typed_at_opens_the_picker_only_at_a_word_start() {
        let mut input = "look at @".to_owned();
        assert!(take_trailing_at(&mut input));
        assert_eq!(input, "look at ");
        let mut mail = "me@".to_owned();
        assert!(!take_trailing_at(&mut mail));
        assert_eq!(mail, "me@");
        let mut start = "@".to_owned();
        assert!(take_trailing_at(&mut start));
        assert!(start.is_empty());
    }

    #[test]
    fn test_chips_are_not_duplicated_and_can_be_removed() {
        let mut panel = AgentPanel::default();
        panel.add_chip(Chip::file("/w/a.rs".into()));
        panel.add_chip(Chip::file("/w/a.rs".into()));
        panel.add_chip(Chip::file("/w/b.rs".into()));
        assert_eq!(panel.chips.len(), 2);
        panel.remove_chip(9);
        panel.remove_chip(0);
        assert_eq!(panel.chips, vec![Chip::file("/w/b.rs".into())]);
    }
}
