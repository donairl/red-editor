//! The Agent panel on the right of the window: the transcript of an ACP
//! session, a prompt box with Send / Stop, and the controls to start a new
//! session or restart the agent. The protocol lives in `crate::acp`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::acp::client::{Client, Phase};
use crate::acp::model::{Entry, Notice};
use crate::acp::transport::AgentProcess;
use crate::agent_settings::CommandError;
use crate::app::TextEditorApp;
use crate::i18n::I18n;

/// Stderr lines shown under "Agent log".
const LOG_LINES: usize = 200;
/// Notices that need attention but are not errors.
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x99, 0x00);

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
    show_log: bool,
    markdown: CommonMarkCache,
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
    sign_in: Option<String>,
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

    /// Once per frame, shown or not: the agent keeps working while hidden.
    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        if let Some(client) = &mut self.agent.client {
            let now = Instant::now();
            client.poll(now);
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
        let i18n = &self.i18n;
        let AgentPanel { client, start_error, input, show_log, markdown } = &mut self.agent;
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
                    ui.horizontal(|ui| {
                        if client.phase == Phase::Prompting {
                            clicks.stop = ui.button(i18n.t("agent.stop")).clicked();
                        } else {
                            let can_send = client.phase == Phase::Ready && !input.trim().is_empty();
                            clicks.send |= ui
                                .add_enabled(can_send, egui::Button::new(i18n.t("agent.send")))
                                .clicked();
                        }
                    });
                    // Enter sends, Shift+Enter is a newline: take the plain
                    // Enter out of the input before the text box sees it.
                    let focused = ui.memory(|m| m.has_focus(input_id()));
                    if focused && client.phase == Phase::Ready && !input.trim().is_empty() {
                        clicks.send |= ui.input_mut(|i| {
                            let before = i.events.len();
                            i.events.retain(|event| {
                                !matches!(event, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                                    if !modifiers.shift)
                            });
                            i.events.len() != before
                        });
                    }
                    ui.add(
                        egui::TextEdit::multiline(input)
                            .id(input_id())
                            .hint_text(i18n.t("agent.placeholder"))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.separator();

                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink(false)
                            .show(ui, |ui| {
                                for (i, entry) in client.transcript.entries.iter().enumerate() {
                                    ui.push_id(i, |ui| draw_entry(ui, i18n, markdown, entry));
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
        if let Some(client) = &mut self.agent.client {
            if clicks.stop {
                client.cancel();
            }
            if let Some(method) = clicks.sign_in {
                client.authenticate(&method, now);
            }
            if clicks.send {
                let text = std::mem::take(&mut self.agent.input);
                if !client.send_prompt(text.trim(), now) {
                    self.agent.input = text;
                }
            }
        }
    }
}

fn input_id() -> egui::Id {
    egui::Id::unique("agent_prompt_input")
}

fn draw_entry(ui: &mut egui::Ui, i18n: &I18n, markdown: &mut CommonMarkCache, entry: &Entry) {
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
        Entry::Notice(Notice::PermissionDeclined) => {
            ui.colored_label(WARNING, i18n.t("agent.permission_declined"));
        }
        Entry::Notice(Notice::Error(message)) => {
            ui.colored_label(ui.visuals().error_fg_color, i18n.tf("agent.failed", &[("error", message)]));
        }
        Entry::TurnEnd(reason) => {
            ui.label(egui::RichText::new(i18n.tf("agent.turn_end", &[("reason", reason)])).weak().small());
        }
    }
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
