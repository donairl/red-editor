//! Cards the Agent panel draws inside the transcript: tool calls (with
//! text, diffs, location links and raw input/output), permission requests
//! and the plan checklist. Drawing returns a [`CardAction`] for the panel to
//! carry out after the frame.

use std::path::{Path, PathBuf};

use agent_client_protocol_schema::v1::{
    PermissionOptionKind, PlanEntryStatus, ToolCallStatus, ToolKind,
};
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::Value;

use crate::acp::model::{
    Location, PermissionCard, PermissionState, PlanItem, ToolCallView, ToolContent,
};
use crate::diff_view::{self, RowKind};
use crate::i18n::I18n;
use crate::theme;

/// Unchanged lines kept around a change in a diff.
const CONTEXT: usize = 2;
/// Diff lines drawn before the rest is summarised.
const MAX_DIFF_LINES: usize = 300;
/// Characters of raw input or output shown.
const MAX_RAW_CHARS: usize = 4000;
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x99, 0x00);

/// What a click inside a card asks the panel to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CardAction {
    /// Open a file, at a 1-based line when given.
    Open(PathBuf, Option<u32>),
    /// Answer the permission request `rpc_id` with this option.
    Permission { rpc_id: Value, option_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sign {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Line(Sign, String),
    /// This many unchanged lines were left out.
    Skipped(usize),
}

/// The change from `old` (`None`: a new file) to `new` as `-`/`+` lines
/// with a little context. Replaced lines show all removals, then all
/// additions.
pub fn unified(old: Option<&str>, new: &str) -> Vec<DiffLine> {
    let mut lines: Vec<(Sign, String)> = Vec::new();
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let flush = |lines: &mut Vec<(Sign, String)>, removed: &mut Vec<String>, added: &mut Vec<String>| {
        lines.extend(removed.drain(..).map(|t| (Sign::Removed, t)));
        lines.extend(added.drain(..).map(|t| (Sign::Added, t)));
    };
    for row in diff_view::rows(old.unwrap_or(""), new) {
        if row.kind == RowKind::Equal {
            flush(&mut lines, &mut removed, &mut added);
            lines.push((Sign::Context, row.left.map(|l| l.1).unwrap_or_default()));
        } else {
            removed.extend(row.left.map(|l| l.1));
            added.extend(row.right.map(|r| r.1));
        }
    }
    flush(&mut lines, &mut removed, &mut added);

    let near_change: Vec<bool> = (0..lines.len())
        .map(|i| {
            let from = i.saturating_sub(CONTEXT);
            let to = (i + CONTEXT + 1).min(lines.len());
            lines[from..to].iter().any(|(sign, _)| *sign != Sign::Context)
        })
        .collect();
    let mut out = Vec::new();
    let mut skipped = 0;
    for ((sign, text), keep) in lines.into_iter().zip(near_change) {
        if keep {
            if skipped > 0 {
                out.push(DiffLine::Skipped(skipped));
                skipped = 0;
            }
            out.push(DiffLine::Line(sign, text));
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push(DiffLine::Skipped(skipped));
    }
    out
}

fn kind_name(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        _ => "other",
    }
}

fn status_icon(ui: &egui::Ui, status: &ToolCallStatus) -> (&'static str, egui::Color32) {
    let theme = theme::active(ui.ctx());
    match status {
        ToolCallStatus::Pending => ("○", ui.visuals().weak_text_color()),
        ToolCallStatus::InProgress => ("◐", WARNING),
        ToolCallStatus::Completed => ("✔", theme.gutter.added),
        ToolCallStatus::Failed => ("✖", theme.gutter.deleted),
        _ => ("○", ui.visuals().weak_text_color()),
    }
}

fn location_label(location: &Location) -> String {
    let name = location.path.file_name().map_or_else(
        || location.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match location.line {
        Some(line) => format!("{name}:{line}"),
        None => name,
    }
}

fn link(ui: &mut egui::Ui, path: &Path, line: Option<u32>, label: &str) -> Option<CardAction> {
    ui.link(label)
        .on_hover_text(path.display().to_string())
        .clicked()
        .then(|| CardAction::Open(path.to_path_buf(), line))
}

pub fn draw_tool_call(
    ui: &mut egui::Ui,
    i18n: &I18n,
    markdown: &mut CommonMarkCache,
    call: &ToolCallView,
) -> Option<CardAction> {
    let mut action = None;
    let (icon, color) = status_icon(ui, &call.status);
    let kind = i18n.t(&format!("agent.kind.{}", kind_name(&call.kind))).to_owned();
    let title = if call.title.is_empty() { call.id.as_str() } else { call.title.as_str() };
    let has_body = !call.content.is_empty() || call.raw_input.is_some() || call.raw_output.is_some();
    let open = call.status == ToolCallStatus::Pending
        && call.content.iter().any(|c| matches!(c, ToolContent::Diff { .. }));
    egui::Frame::new()
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(4)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(color, icon);
                ui.label(egui::RichText::new(kind).weak().small());
                ui.strong(title);
            });
            if !call.locations.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for location in &call.locations {
                        if let Some(a) = link(ui, &location.path, location.line, &location_label(location)) {
                            action = Some(a);
                        }
                    }
                });
            }
            if has_body {
                egui::CollapsingHeader::new(i18n.t("agent.tool_details"))
                    .id_salt(("tool_body", &call.id))
                    .default_open(open)
                    .show(ui, |ui| {
                        for (i, content) in call.content.iter().enumerate() {
                            ui.push_id(i, |ui| match content {
                                ToolContent::Text(text) => {
                                    CommonMarkViewer::new().show(ui, markdown, text);
                                }
                                ToolContent::Diff { path, old, new } => {
                                    if let Some(a) = draw_diff(ui, path, old.as_deref(), new) {
                                        action = Some(a);
                                    }
                                }
                                ToolContent::Terminal(id) => {
                                    ui.label(
                                        egui::RichText::new(i18n.tf("agent.terminal", &[("id", id)])).weak(),
                                    );
                                }
                            });
                        }
                        draw_raw(ui, i18n.t("agent.raw_input"), ("in", &call.id), call.raw_input.as_ref());
                        draw_raw(ui, i18n.t("agent.raw_output"), ("out", &call.id), call.raw_output.as_ref());
                    });
            }
        });
    action
}

fn draw_raw(ui: &mut egui::Ui, title: &str, id: (&str, &str), value: Option<&Value>) {
    let Some(value) = value else { return };
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    if let Some((cut, _)) = text.char_indices().nth(MAX_RAW_CHARS) {
        text.truncate(cut);
        text.push('…');
    }
    egui::CollapsingHeader::new(title).id_salt(id).default_open(false).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).monospace().small()).wrap());
    });
}

fn draw_diff(ui: &mut egui::Ui, path: &Path, old: Option<&str>, new: &str) -> Option<CardAction> {
    let theme = theme::active(ui.ctx());
    let (added, removed) = (theme.gutter.added.gamma_multiply(0.25), theme.gutter.deleted.gamma_multiply(0.25));
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let action = link(ui, path, None, &name);
    let lines = unified(old, new);
    let total = lines.len();
    for line in lines.iter().take(MAX_DIFF_LINES) {
        let text = match line {
            DiffLine::Line(Sign::Context, text) => egui::RichText::new(format!("  {text}")),
            DiffLine::Line(Sign::Added, text) => egui::RichText::new(format!("+ {text}")).background_color(added),
            DiffLine::Line(Sign::Removed, text) => egui::RichText::new(format!("- {text}")).background_color(removed),
            DiffLine::Skipped(n) => egui::RichText::new(format!("  … {n}")).weak(),
        };
        ui.add(egui::Label::new(text.monospace().small()).wrap());
    }
    if total > MAX_DIFF_LINES {
        ui.label(egui::RichText::new(format!("… {}", total - MAX_DIFF_LINES)).weak().small());
    }
    action
}

pub fn draw_permission(ui: &mut egui::Ui, i18n: &I18n, card: &PermissionCard) -> Option<CardAction> {
    let mut action = None;
    let theme = theme::active(ui.ctx());
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, WARNING))
        .corner_radius(4)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = if card.title.is_empty() { &card.tool_call_id } else { &card.title };
            ui.label(i18n.tf("agent.permission_request", &[("tool", title)]));
            match &card.state {
                PermissionState::Pending => {
                    ui.horizontal_wrapped(|ui| {
                        for option in &card.options {
                            let color = match option.kind {
                                PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways => {
                                    theme.gutter.added
                                }
                                _ => theme.gutter.deleted,
                            };
                            if ui.button(egui::RichText::new(&option.name).color(color)).clicked() {
                                action = Some(CardAction::Permission {
                                    rpc_id: card.rpc_id.clone(),
                                    option_id: option.id.clone(),
                                });
                            }
                        }
                    });
                }
                PermissionState::Chosen(name) => {
                    ui.label(egui::RichText::new(i18n.tf("agent.permission_chosen", &[("option", name)])).weak());
                }
                PermissionState::Cancelled => {
                    ui.label(egui::RichText::new(i18n.t("agent.permission_cancelled")).weak());
                }
            }
        });
    action
}

pub fn draw_plan(ui: &mut egui::Ui, i18n: &I18n, plan: &[PlanItem]) {
    if plan.is_empty() {
        return;
    }
    let done = plan.iter().filter(|p| p.status == PlanEntryStatus::Completed).count();
    let title = i18n.tf("agent.plan", &[("done", &done.to_string()), ("total", &plan.len().to_string())]);
    egui::CollapsingHeader::new(title).id_salt("agent_plan").default_open(true).show(ui, |ui| {
        for item in plan {
            let (mark, text) = match item.status {
                PlanEntryStatus::Completed => ("☑", egui::RichText::new(&item.content).weak().strikethrough()),
                PlanEntryStatus::InProgress => ("◐", egui::RichText::new(&item.content).strong()),
                _ => ("☐", egui::RichText::new(&item.content)),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(mark);
                ui.label(text);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(sign: Sign, text: &str) -> DiffLine {
        DiffLine::Line(sign, text.to_owned())
    }

    #[test]
    fn test_replaced_lines_show_removals_then_additions() {
        let lines = unified(Some("a\nb\nc\nd\n"), "a\nB\nC\nd\n");
        assert_eq!(
            lines,
            vec![
                line(Sign::Context, "a"),
                line(Sign::Removed, "b"),
                line(Sign::Removed, "c"),
                line(Sign::Added, "B"),
                line(Sign::Added, "C"),
                line(Sign::Context, "d"),
            ]
        );
    }

    #[test]
    fn test_new_file_is_all_additions() {
        assert_eq!(unified(None, "x\ny\n"), vec![line(Sign::Added, "x"), line(Sign::Added, "y")]);
    }

    #[test]
    fn test_far_context_is_skipped() {
        let old: String = (1..=20).map(|n| format!("l{n}\n")).collect();
        let new = old.replace("l10\n", "ten\n");
        let lines = unified(Some(&old), &new);
        assert_eq!(lines.first(), Some(&DiffLine::Skipped(7)));
        assert_eq!(lines.last(), Some(&DiffLine::Skipped(8)));
        assert!(lines.contains(&line(Sign::Removed, "l10")));
        assert!(lines.contains(&line(Sign::Added, "ten")));
        assert!(lines.contains(&line(Sign::Context, "l8")));
        assert!(!lines.contains(&line(Sign::Context, "l7")));
    }

    #[test]
    fn test_unchanged_text_gives_no_lines_to_draw() {
        assert_eq!(unified(Some("a\n"), "a\n"), vec![DiffLine::Skipped(1)]);
    }

    #[test]
    fn test_location_label() {
        let at = Location { path: "/w/src/main.rs".into(), line: Some(7) };
        assert_eq!(location_label(&at), "main.rs:7");
        assert_eq!(location_label(&Location { line: None, ..at }), "main.rs");
    }
}
