//! The Source Control view of the sidebar: a commit box over the changed
//! files, staged and not, with buttons to stage, unstage and discard them.
//!
//! Like the Explorer it only reports what was asked: `app.rs` runs the git
//! commands and asks before anything that cannot be undone.
//!
//! Also draws the diff panel a click on a file opens beside the editor.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::file_browser::{
    self, ACTION_SIZE, BADGE_GAP, CHEVRON_WIDTH, ROW_HEIGHT, ROW_PADDING_X, SidebarPalette,
    TITLE_HEIGHT, TITLE_PADDING_X,
};
use crate::git::{DiffKind, FileChange, GitError, Kind};
use crate::git_worker::{FileDiff, GitState, Op};
use crate::i18n::I18n;
use crate::theme;

/// Room for the status letter at the end of a row.
const LETTER_WIDTH: f32 = 12.0;

/// What the view keeps between frames.
pub struct ScmUi {
    /// The commit message being typed.
    pub message: String,
    staged_open: bool,
    changes_open: bool,
    conflicts_open: bool,
}

impl Default for ScmUi {
    fn default() -> Self {
        ScmUi {
            message: String::new(),
            staged_open: true,
            changes_open: true,
            conflicts_open: true,
        }
    }
}

/// What the user asked of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScmAction {
    Run(Op),
    /// Throw these work-tree changes away, once the user confirms.
    Discard(Vec<FileChange>),
    /// Commit with nothing staged: stage everything first, once the user
    /// confirms.
    CommitAll(String),
    Open(PathBuf),
    /// Show the file's staged or unstaged changes.
    Diff { path: PathBuf, staged: bool },
    Refresh,
}

/// The three lists, by what the user can do with their files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Conflicts,
    Staged,
    Changes,
}

impl Section {
    fn title_key(self) -> &'static str {
        match self {
            Section::Conflicts => "scm.conflicts",
            Section::Staged => "scm.staged",
            Section::Changes => "scm.changes",
        }
    }

    /// The change shown for `change` in this list.
    fn kind(self, change: &FileChange) -> Option<Kind> {
        match self {
            Section::Conflicts => change.is_conflicted().then_some(Kind::Conflicted),
            _ if change.is_conflicted() => None,
            Section::Staged => change.staged,
            Section::Changes => change.unstaged,
        }
    }
}

/// A hover button at the end of a row or section header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Open,
    Discard,
    Stage,
    Unstage,
}

impl Button {
    fn tooltip_key(self, all: bool) -> &'static str {
        match (self, all) {
            (Button::Open, _) => "scm.open_file",
            (Button::Discard, false) => "scm.discard",
            (Button::Discard, true) => "scm.discard_all",
            (Button::Stage, false) => "scm.stage",
            (Button::Stage, true) => "scm.stage_all",
            (Button::Unstage, false) => "scm.unstage",
            (Button::Unstage, true) => "scm.unstage_all",
        }
    }
}

/// The buttons each list offers, left to right.
fn buttons(section: Section, header: bool) -> &'static [Button] {
    match (section, header) {
        (Section::Staged, false) => &[Button::Open, Button::Unstage],
        (Section::Staged, true) => &[Button::Unstage],
        (Section::Changes, false) => &[Button::Open, Button::Discard, Button::Stage],
        (Section::Changes, true) => &[Button::Discard, Button::Stage],
        // Staging a conflicted file marks it resolved.
        (Section::Conflicts, false) => &[Button::Open, Button::Stage],
        (Section::Conflicts, true) => &[Button::Stage],
    }
}

/// Draws the view down the available height. `folder` is where a new
/// repository would be made when there is none.
pub fn show(
    ui: &mut egui::Ui,
    git: &GitState,
    scm: &mut ScmUi,
    folder: Option<&Path>,
    i18n: &I18n,
) -> Option<ScmAction> {
    let palette = theme::active(ui.ctx()).sidebar;
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let width = ui.available_width();
    let panel = ui.max_rect();
    ui.painter().vline(
        panel.right() - 0.5,
        panel.y_range(),
        Stroke::new(1.0, palette.border),
    );

    let mut action = None;
    let (title_rect, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT), Sense::hover());
    ui.painter().text(
        pos2(title_rect.left() + TITLE_PADDING_X, title_rect.center().y),
        Align2::LEFT_CENTER,
        i18n.t("scm.title"),
        FontId::proportional(11.0),
        palette.foreground,
    );
    if git.repo().is_some() {
        let refresh_rect = Rect::from_center_size(
            pos2(title_rect.right() - ROW_PADDING_X - ACTION_SIZE / 2.0, title_rect.center().y),
            Vec2::splat(ACTION_SIZE),
        );
        let refresh = ui
            .interact(refresh_rect, egui::Id::unique("scm_refresh"), Sense::click())
            .on_hover_text(i18n.t("scm.refresh"));
        if git.is_refreshing() || git.busy().is_some() {
            egui::Spinner::new()
                .size(12.0)
                .color(palette.icon_inactive)
                .paint_at(ui, Rect::from_center_size(refresh_rect.center(), Vec2::splat(12.0)));
        } else {
            if refresh.hovered() {
                ui.painter()
                    .rect_filled(refresh_rect.shrink(2.0), CornerRadius::same(4), palette.row_hover);
            }
            file_browser::paint_refresh_icon(ui.painter(), refresh_rect.center(), palette.foreground);
        }
        if refresh.clicked() {
            action = Some(ScmAction::Refresh);
        }
    }

    let Some(status) = git.status.as_ref().filter(|_| git.repo().is_some()) else {
        let note = |ui: &mut egui::Ui, text: &str| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(TITLE_PADDING_X as i8, 8))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(palette.foreground));
                });
        };
        match &git.missing {
            None => {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(TITLE_PADDING_X);
                    ui.spinner();
                });
            }
            Some(GitError::NotInstalled) => note(ui, i18n.t("git.not_installed")),
            Some(_) => {
                note(ui, i18n.t("scm.no_repo"));
                if let Some(folder) = folder {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::symmetric(TITLE_PADDING_X as i8, 0))
                        .show(ui, |ui| {
                            let button = egui::Button::new(i18n.t("scm.init"))
                                .min_size(vec2(ui.available_width(), 26.0));
                            if ui.add_enabled(git.busy().is_none(), button).clicked() {
                                action = Some(ScmAction::Run(Op::Init(folder.to_path_buf())));
                            }
                        });
                }
            }
        }
        return action;
    };

    let in_section = |section: Section| -> Vec<&FileChange> {
        status
            .files
            .iter()
            .filter(|c| section.kind(c).is_some())
            .collect()
    };
    let conflicts = in_section(Section::Conflicts);
    let staged = in_section(Section::Staged);
    let changes = in_section(Section::Changes);

    // The commit box.
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let hint = i18n.tf("scm.message_hint", &[("branch", &status.head_label())]);
            let field = ui.add(
                egui::TextEdit::multiline(&mut scm.message)
                    .id_salt("scm_message")
                    .hint_text(hint)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            let message = scm.message.trim().to_owned();
            let can_commit = !message.is_empty()
                && git.busy().is_none()
                && (!staged.is_empty() || !changes.is_empty())
                && conflicts.is_empty();
            let button = egui::Button::new(i18n.t("scm.commit")).min_size(vec2(ui.available_width(), 26.0));
            let shortcut = field.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
            let clicked = ui
                .add_enabled(can_commit, button)
                .on_hover_text(i18n.t("scm.commit_tooltip"))
                .clicked();
            if (clicked || shortcut) && can_commit {
                action = Some(if staged.is_empty() {
                    ScmAction::CommitAll(message)
                } else {
                    ScmAction::Run(Op::Commit(message))
                });
            }
        });

    if status.files.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        ui.painter().text(
            pos2(rect.left() + TITLE_PADDING_X, rect.center().y),
            Align2::LEFT_CENTER,
            i18n.t("scm.no_changes"),
            FontId::proportional(13.0),
            palette.icon_inactive,
        );
        return action;
    }

    let root = git.repo().map(|r| r.root().to_path_buf()).unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt("scm_lists")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (section, files, open) in [
                (Section::Conflicts, &conflicts, &mut scm.conflicts_open),
                (Section::Staged, &staged, &mut scm.staged_open),
                (Section::Changes, &changes, &mut scm.changes_open),
            ] {
                // Staged Changes only shows once something is staged, and
                // Merge Changes during a merge, as in VS Code.
                if files.is_empty() && section != Section::Changes {
                    continue;
                }
                if let Some(a) = show_section(ui, &palette, section, files, open, &root, i18n) {
                    action = Some(a);
                }
            }
        });
    action
}

/// One list: its header and, while open, a row per file.
fn show_section(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    section: Section,
    files: &[&FileChange],
    open: &mut bool,
    root: &Path,
    i18n: &I18n,
) -> Option<ScmAction> {
    let width = ui.available_width();
    let (rect, header) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
    let header_buttons = if files.is_empty() { &[][..] } else { buttons(section, true) };
    let (pressed, any_hovered) = row_buttons(ui, rect, header.id, header_buttons, true, i18n);
    if header.hovered() || any_hovered {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
    }
    file_browser::paint_chevron(
        ui.painter(),
        pos2(rect.left() + ROW_PADDING_X + CHEVRON_WIDTH / 2.0, rect.center().y),
        *open,
        palette.foreground,
    );
    let title = format!("{} ({})", i18n.t(section.title_key()), files.len());
    ui.painter().text(
        pos2(rect.left() + ROW_PADDING_X + CHEVRON_WIDTH, rect.center().y),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::proportional(11.0),
        palette.foreground,
    );
    if header.hovered() || any_hovered {
        paint_buttons(ui, palette, rect, header_buttons, true);
    }
    let all: Vec<FileChange> = files.iter().map(|c| (*c).clone()).collect();
    let mut action = pressed.map(|button| file_action(button, &all));
    if header.clicked() && pressed.is_none() {
        *open = !*open;
    }
    if !*open {
        return action;
    }

    for change in files {
        let Some(kind) = section.kind(change) else { continue };
        let (rect, row) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
        let row_buttons_list = buttons(section, false);
        let (pressed, any_hovered) = row_buttons(ui, rect, row.id, row_buttons_list, false, i18n);
        let hovered = row.hovered() || any_hovered;
        paint_file_row(ui, palette, rect, change, kind, root, hovered, row_buttons_list.len());
        if hovered {
            paint_buttons(ui, palette, rect, row_buttons_list, false);
        }
        if let Some(button) = pressed {
            action = Some(file_action(button, std::slice::from_ref(*change)));
        } else if row.clicked() {
            action = Some(ScmAction::Diff {
                path: change.path.clone(),
                staged: section == Section::Staged,
            });
        }
    }
    action
}

/// What pressing `button` on `changes` (one row, or a whole list) asks for.
fn file_action(button: Button, changes: &[FileChange]) -> ScmAction {
    let paths = || changes.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
    match button {
        Button::Open => ScmAction::Open(changes[0].path.clone()),
        Button::Discard => ScmAction::Discard(changes.to_vec()),
        Button::Stage => ScmAction::Run(Op::Stage(paths())),
        Button::Unstage => {
            // A staged rename unstages both its ends.
            let mut paths = paths();
            paths.extend(changes.iter().filter_map(|c| c.orig_path.clone()));
            ScmAction::Run(Op::Unstage(paths))
        }
    }
}

/// The rects of `list`'s buttons at the end of `rect`, left to right.
fn button_rects(rect: Rect, count: usize, header: bool) -> impl Iterator<Item = Rect> {
    // A file row keeps its status letter at the very end.
    let end = rect.right() - ROW_PADDING_X - if header { 0.0 } else { LETTER_WIDTH + BADGE_GAP };
    (0..count).map(move |i| {
        let right = end - (count - 1 - i) as f32 * ACTION_SIZE;
        Rect::from_min_max(pos2(right - ACTION_SIZE, rect.top()), pos2(right, rect.bottom()))
    })
}

/// Registers the hover buttons of a row; returns the one clicked and
/// whether any is hovered.
fn row_buttons(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    list: &[Button],
    header: bool,
    i18n: &I18n,
) -> (Option<Button>, bool) {
    let mut pressed = None;
    let mut hovered = false;
    for (button, button_rect) in list.iter().zip(button_rects(rect, list.len(), header)) {
        let response = ui
            .interact(button_rect, id.with(*button as u8), Sense::click())
            .on_hover_text(i18n.t(button.tooltip_key(header)));
        hovered |= response.hovered();
        if response.clicked() {
            pressed = Some(*button);
        }
    }
    (pressed, hovered)
}

fn paint_buttons(ui: &egui::Ui, palette: &SidebarPalette, rect: Rect, list: &[Button], header: bool) {
    let painter = ui.painter();
    let hover = ui.ctx().pointer_hover_pos();
    for (button, r) in list.iter().zip(button_rects(rect, list.len(), header)) {
        if hover.is_some_and(|p| r.contains(p)) {
            painter.rect_filled(r.shrink(2.0), CornerRadius::same(4), palette.row_selected);
        }
        paint_button_icon(painter, *button, r.center(), palette.foreground);
    }
}

/// Plus, minus, a curved arrow back, or a page: drawn as lines, so they
/// never depend on the bundled fonts.
fn paint_button_icon(painter: &egui::Painter, button: Button, c: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.3, color);
    match button {
        Button::Stage => {
            painter.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
            painter.line_segment([c + vec2(0.0, -5.0), c + vec2(0.0, 5.0)], stroke);
        }
        Button::Unstage => {
            painter.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
        }
        Button::Discard => {
            use std::f32::consts::PI;
            // A half circle open to the left, with an arrow head at its top
            // end pointing left.
            let center = c + vec2(1.0, 1.0);
            let points: Vec<_> = (0..=16)
                .map(|i| {
                    let angle = -PI / 2.0 + PI * i as f32 / 16.0;
                    center + 4.0 * vec2(angle.cos(), angle.sin())
                })
                .collect();
            let top = points[0];
            let bottom = *points.last().unwrap();
            painter.line(points, stroke);
            painter.line_segment([bottom, bottom + vec2(-3.0, 0.0)], stroke);
            painter.line_segment([top, top + vec2(-5.0, 0.0)], stroke);
            let tip = top + vec2(-5.0, 0.0);
            painter.line_segment([tip, tip + vec2(2.5, -2.5)], stroke);
            painter.line_segment([tip, tip + vec2(2.5, 2.5)], stroke);
        }
        Button::Open => {
            let page = Rect::from_center_size(c, vec2(9.0, 11.0));
            let fold = 3.0;
            painter.line(
                vec![
                    page.left_top(),
                    pos2(page.right() - fold, page.top()),
                    pos2(page.right(), page.top() + fold),
                    page.right_bottom(),
                    page.left_bottom(),
                    page.left_top(),
                ],
                stroke,
            );
        }
    }
}

/// `name  folder` in the change's colour, ending with its status letter.
#[allow(clippy::too_many_arguments)]
fn paint_file_row(
    ui: &egui::Ui,
    palette: &SidebarPalette,
    rect: Rect,
    change: &FileChange,
    kind: Kind,
    root: &Path,
    hovered: bool,
    button_count: usize,
) {
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
    }
    let color = palette.git(kind);
    let letter_x = rect.right() - ROW_PADDING_X - LETTER_WIDTH / 2.0;
    painter.text(
        pos2(letter_x, rect.center().y),
        Align2::CENTER_CENTER,
        kind.letter(),
        FontId::proportional(12.0),
        color,
    );

    let x = rect.left() + ROW_PADDING_X + CHEVRON_WIDTH + 4.0;
    let mut right = rect.right() - ROW_PADDING_X - LETTER_WIDTH - BADGE_GAP;
    if hovered {
        right -= button_count as f32 * ACTION_SIZE;
    }
    let name = change
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A deleted file is struck through, as in VS Code.
    let name_galley = file_browser::elided(ui, name, FontId::proportional(13.0), color, right - x);
    let name_width = name_galley.size().x;
    let name_pos = pos2(x, rect.center().y - name_galley.size().y / 2.0);
    painter.galley(name_pos, name_galley, color);
    if kind == Kind::Deleted {
        painter.hline(x..=x + name_width, rect.center().y, Stroke::new(1.0, color));
    }

    let folder = change
        .path
        .parent()
        .and_then(|dir| dir.strip_prefix(root).ok())
        .map(|dir| dir.display().to_string())
        .filter(|dir| !dir.is_empty());
    let detail_x = x + name_width + BADGE_GAP;
    if let Some(folder) = folder
        && right - detail_x > 16.0
    {
        let detail = file_browser::elided(
            ui,
            folder,
            FontId::proportional(12.0),
            palette.icon_inactive,
            right - detail_x,
        );
        painter.galley(
            pos2(detail_x, rect.center().y - detail.size().y / 2.0),
            detail,
            palette.icon_inactive,
        );
    }
}

/// What the user asked of the diff panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffAction {
    Close,
    /// Go to this 1-based line of the file.
    Goto(usize),
}

/// The changes of one file as a unified diff: old and new line numbers,
/// then the line on a green (added) or red (removed) ground. A click on a
/// line asks to go to it in the editor.
pub fn show_diff(ui: &mut egui::Ui, diff: &FileDiff, i18n: &I18n) -> Option<DiffAction> {
    let theme = theme::active(ui.ctx());
    let palette = theme.sidebar;
    let mut action = None;

    ui.horizontal(|ui| {
        let name = diff
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let side = i18n.t(if diff.staged { "scm.diff_staged" } else { "scm.diff_working" });
        ui.label(egui::RichText::new(name).strong());
        ui.label(egui::RichText::new(side).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new("×").frame(false))
                .on_hover_text(i18n.t("scm.diff_close"))
                .clicked()
            {
                action = Some(DiffAction::Close);
            }
        });
    });
    ui.separator();

    let lines = match &diff.lines {
        None => {
            ui.spinner();
            return action;
        }
        Some(Err(e)) => {
            ui.colored_label(palette.git_deleted, e.to_string());
            return action;
        }
        Some(Ok(lines)) if lines.is_empty() => {
            ui.weak(i18n.t("scm.diff_empty"));
            return action;
        }
        Some(Ok(lines)) => lines,
    };

    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
    let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let widest = lines.iter().filter_map(|l| l.old.max(l.new)).max().unwrap_or(1);
    let number_width = (widest.ilog10() as f32 + 1.0) * digit + 8.0;
    let added_bg = theme.gutter.added.gamma_multiply(0.18);
    let removed_bg = theme.gutter.deleted.gamma_multiply(0.18);
    let dim = palette.icon_inactive;
    let fg = palette.foreground;

    egui::ScrollArea::both()
        .id_salt("git_diff_lines")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let start = range.start;
            for (index, line) in lines[range].iter().enumerate() {
                let text_x = 2.0 * number_width + 16.0;
                let text_width = ui.fonts_mut(|f| {
                    f.layout_no_wrap(line.text.clone(), font.clone(), fg).size().x
                });
                let width = ui.available_width().max(text_x + text_width + 8.0);
                let (rect, response) = ui.allocate_exact_size(vec2(width, row_height), Sense::click());
                let painter = ui.painter();
                let (bg, sign) = match line.kind {
                    DiffKind::Added => (Some(added_bg), "+"),
                    DiffKind::Removed => (Some(removed_bg), "-"),
                    DiffKind::Hunk => (Some(palette.row_hover), ""),
                    DiffKind::Context => (None, ""),
                };
                if let Some(bg) = bg {
                    painter.rect_filled(rect, CornerRadius::ZERO, bg);
                }
                if response.hovered() && line.kind != DiffKind::Hunk {
                    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, palette.border), egui::StrokeKind::Inside);
                }
                let y = rect.center().y;
                let number = |n: Option<usize>, right: f32| {
                    if let Some(n) = n {
                        painter.text(pos2(right, y), Align2::RIGHT_CENTER, n.to_string(), font.clone(), dim);
                    }
                };
                number(line.old, rect.left() + number_width);
                number(line.new, rect.left() + 2.0 * number_width);
                painter.text(
                    pos2(rect.left() + 2.0 * number_width + 6.0, y),
                    Align2::LEFT_CENTER,
                    sign,
                    font.clone(),
                    dim,
                );
                let color = if line.kind == DiffKind::Hunk { dim } else { fg };
                painter.text(
                    pos2(rect.left() + text_x, y),
                    Align2::LEFT_CENTER,
                    &line.text,
                    font.clone(),
                    color,
                );
                if response.clicked()
                    && let Some(target) = goto_target(lines, start + index)
                {
                    action = Some(DiffAction::Goto(target));
                }
            }
        });
    action
}

/// The line of the new file a click on `lines[index]` goes to: its own,
/// or for a removed line the one now in its place.
fn goto_target(lines: &[crate::git::DiffLine], index: usize) -> Option<usize> {
    let line = &lines[index];
    match line.kind {
        DiffKind::Hunk => None,
        DiffKind::Removed => lines[index..]
            .iter()
            .take_while(|l| l.kind != DiffKind::Hunk)
            .find_map(|l| l.new)
            .or_else(|| lines[..index].iter().rev().find_map(|l| l.new).map(|n| n + 1))
            .or(Some(1)),
        _ => line.new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A repository with one committed file modified and one new file, and
    /// its state once git reported it.
    fn fixture(name: &str, ctx: &egui::Context) -> Option<(GitState, PathBuf)> {
        let repo = crate::git::temp_repo(name)?;
        let root = repo.root().to_path_buf();
        fs::write(root.join("kept.txt"), "one\n").unwrap();
        repo.stage(&[root.join("kept.txt")]).unwrap();
        repo.commit("first").unwrap();
        fs::write(root.join("kept.txt"), "two\n").unwrap();
        fs::write(root.join("new.txt"), "x\n").unwrap();
        let mut git = GitState::default();
        git.open(ctx, root.clone());
        for _ in 0..250 {
            git.poll(ctx);
            if git.status.is_some() {
                return Some((git, root));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("no status");
    }

    fn frame(
        ctx: &egui::Context,
        git: &GitState,
        scm: &mut ScmUi,
        events: Vec<egui::Event>,
    ) -> (Option<ScmAction>, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(300.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            action = show(ui, git, scm, None, &I18n::builtin());
        });
        output.textures_delta.clear();
        (action, output.shapes)
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((
                    t.galley.text().to_owned(),
                    Rect::from_min_size(t.pos, t.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn test_lists_changes_and_stages_a_row() {
        let ctx = egui::Context::default();
        let Some((git, root)) = fixture("scm_rows", &ctx) else { return };
        let mut scm = ScmUi::default();
        let (_, shapes) = frame(&ctx, &git, &mut scm, Vec::new());
        let painted = texts(&shapes);
        let names: Vec<&str> = painted.iter().map(|(t, _)| t.as_str()).collect();
        assert!(names.contains(&"PERUBAHAN (2)"), "{names:?}");
        assert!(names.contains(&"kept.txt") && names.contains(&"new.txt"), "{names:?}");
        assert!(names.contains(&"M") && names.contains(&"U"), "{names:?}");

        // The rightmost hover button of a Changes row stages it.
        let (_, kept) = painted.iter().find(|(t, _)| t == "kept.txt").unwrap();
        let row_right = 300.0 - ROW_PADDING_X - LETTER_WIDTH - BADGE_GAP - ACTION_SIZE / 2.0;
        let (action, _) = frame(&ctx, &git, &mut scm, click(pos2(row_right, kept.center().y)));
        assert_eq!(action, Some(ScmAction::Run(Op::Stage(vec![root.join("kept.txt")]))));

        // A click on the row itself asks for its diff.
        let (action, _) = frame(&ctx, &git, &mut scm, click(kept.center()));
        assert_eq!(
            action,
            Some(ScmAction::Diff {
                path: root.join("kept.txt"),
                staged: false
            })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_commit_with_nothing_staged_asks_to_stage_all() {
        let ctx = egui::Context::default();
        let Some((git, root)) = fixture("scm_commit", &ctx) else { return };
        let mut scm = ScmUi {
            message: "  fix things \n".into(),
            ..ScmUi::default()
        };
        let (_, shapes) = frame(&ctx, &git, &mut scm, Vec::new());
        let painted = texts(&shapes);
        let (_, commit) = painted.iter().find(|(t, _)| t == "Commit").expect("commit button");
        let (action, _) = frame(&ctx, &git, &mut scm, click(commit.center()));
        assert_eq!(action, Some(ScmAction::CommitAll("fix things".into())));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_removed_lines_go_to_the_line_now_in_their_place() {
        let lines = crate::git::parse_diff("@@ -1,4 +1,3 @@\n a\n-b\n-c\n d\n+e\n@@ -9,1 +8,0 @@\n-z\n");
        let targets: Vec<_> = (0..lines.len()).map(|i| goto_target(&lines, i)).collect();
        assert_eq!(
            targets,
            [None, Some(1), Some(2), Some(2), Some(2), Some(3), None, Some(4)]
        );
    }

    #[test]
    fn test_no_repository_offers_to_make_one() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("red_scm_none_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut git = GitState::default();
        git.open(&ctx, dir.clone());
        for _ in 0..250 {
            git.poll(&ctx);
            if git.missing.is_some() || git.repo().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The temp folder may sit in a repository on some machines.
        if git.missing != Some(GitError::NotRepo) {
            return;
        }
        let mut scm = ScmUi::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(300.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            show(ui, &git, &mut scm, Some(&dir), &I18n::builtin());
        });
        output.textures_delta.clear();
        let names: Vec<String> = texts(&output.shapes).into_iter().map(|(t, _)| t).collect();
        assert!(names.iter().any(|t| t == "Buat Repository"), "{names:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
