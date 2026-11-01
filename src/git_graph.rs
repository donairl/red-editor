//! The Git Graph: every branch and tag as a commit list with coloured
//! lanes, a detail pane for the selected commit, and branch actions.
//!
//! The lane layout is pure and incremental; the view returns what the
//! user asked for as a [`GraphAction`] and `app.rs` runs it.

use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::file_browser::{self, ACTION_SIZE, BADGE_GAP, ROW_HEIGHT, ROW_PADDING_X, SidebarPalette, TITLE_HEIGHT, TITLE_PADDING_X};
use crate::git::{self, Branch, Commit};
use crate::git_worker::{GitState, Log};
use crate::i18n::I18n;
use crate::theme;

/// How many colours lanes cycle through.
pub const PALETTE_LEN: usize = 8;

/// A line from this row's lane `from` to the next row's lane `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// Where one commit sits and the lines leaving it downward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub edges: Vec<Edge>,
}

/// A lane waiting for a commit to appear further down.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    hash: String,
    color: usize,
    /// Lanes of the row above that feed this slot, each with its line
    /// colour: itself when it merely continues, the committing lane when
    /// it was just opened or joined.
    sources: Vec<(usize, usize)>,
}

/// Lanes for commits in topological order (children before parents).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<GraphRow>,
    active: Vec<Option<Slot>>,
    next_color: usize,
    /// The widest the graph has been.
    pub lanes: usize,
}

impl Layout {
    /// Lays out commits from scratch.
    #[cfg(test)]
    pub fn new(commits: &[Commit]) -> Layout {
        let mut layout = Layout::default();
        layout.extend(commits);
        layout
    }

    /// Lays out further commits; earlier rows keep their lanes, and the
    /// last row gains its edges once the next commit is laid out.
    pub fn extend(&mut self, commits: &[Commit]) {
        for commit in commits {
            self.push(commit);
        }
    }

    /// Places one commit and the lines into it from the row above.
    fn push(&mut self, commit: &Commit) {
        // Joins reuse a waiting slot, so at most one slot waits for any hash.
        // 1. The commit's lane: the first slot waiting for it, else a free one.
        let waiting = self.active.iter().position(|s| s.as_ref().is_some_and(|s| s.hash == commit.hash));
        let lane = match waiting {
            Some(lane) => lane,
            None => self.free_slot(),
        };
        let color = match &self.active[lane] {
            Some(slot) => slot.color,
            None => self.new_color(),
        };

        // 2. Lines from the row above into this row.
        if let Some(prev) = self.rows.last_mut() {
            for (i, slot) in self.active.iter_mut().enumerate() {
                let Some(slot) = slot else { continue };
                for &(from, color) in &slot.sources {
                    prev.edges.push(Edge { from, to: i, color });
                }
                slot.sources.clear();
                slot.sources.push((i, slot.color));
            }
        }

        // 3. Every slot waiting for this commit ends here.
        if let Some(l) = waiting {
            self.active[l] = None;
        }

        // 4. Parents: the first keeps the lane, others open or join lanes.
        for (k, parent) in commit.parents.iter().enumerate() {
            let joined = self.active.iter().position(|s| s.as_ref().is_some_and(|s| s.hash == *parent));
            match joined {
                Some(j) => {
                    let slot = self.active[j].as_mut().unwrap();
                    let line = if k == 0 { color } else { slot.color };
                    slot.sources.push((lane, line));
                }
                None if k == 0 => {
                    self.active[lane] = Some(Slot { hash: parent.clone(), color, sources: vec![(lane, color)] });
                }
                None => {
                    let j = self.free_slot();
                    let color = self.new_color();
                    self.active[j] = Some(Slot { hash: parent.clone(), color, sources: vec![(lane, color)] });
                }
            }
        }

        while self.active.last().is_some_and(Option::is_none) {
            self.active.pop();
        }
        self.lanes = self.lanes.max(self.active.len()).max(lane + 1);
        self.rows.push(GraphRow { lane, color, edges: Vec::new() });
    }

    /// The first empty slot, opening one at the end when none is.
    fn free_slot(&mut self) -> usize {
        match self.active.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.active.push(None);
                self.active.len() - 1
            }
        }
    }

    /// The next palette colour, cycling.
    fn new_color(&mut self) -> usize {
        let color = self.next_color % PALETTE_LEN;
        self.next_color += 1;
        color
    }
}

/// Width of one lane column.
const LANE_WIDTH: f32 = 14.0;
/// Lanes past this one share the last column.
const MAX_LANES: usize = 10;
const DOT_RADIUS: f32 = 3.5;
const LINE_WIDTH: f32 = 2.0;
const CHIP_HEIGHT: f32 = 16.0;
const CHIP_PADDING: f32 = 6.0;
/// Chips past this many fold into `+n`.
const MAX_CHIPS: usize = 4;
const MAX_CHIP_CHARS: usize = 20;

/// What the view keeps between frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitGraphUi {
    /// Lanes for `GitState::graph`, extended as pages arrive.
    pub layout: Layout,
    /// How many commits `layout` covers, and of which generation.
    laid_out: (u64, usize),
    /// The hash of the commit shown in the detail pane.
    pub selected: Option<String>,
    /// Scroll so this row is in view, once.
    pub scroll_to: Option<usize>,
    /// A parent to select that is not loaded yet; pages load until it is.
    pub seek: Option<String>,
    /// The branch-name prompt of Create Branch Here, with the commit.
    pub new_branch: Option<NewBranch>,
}

/// The open Create Branch Here prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewBranch {
    pub hash: String,
    pub name: String,
    /// The name field has had its first focus.
    pub focused: bool,
}

/// What the user asked of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphAction {
    /// Close the Git Graph.
    Close,
    /// Show this commit in the detail pane.
    Select(String),
    /// Load the next page of the log.
    LoadMore,
    /// Check this commit out, once the user confirms.
    Checkout(String),
    /// Create a branch with this name at this commit.
    CreateBranch { name: String, hash: String },
    /// Switch to this local branch.
    Switch(String),
    /// Delete this branch, once the user confirms.
    DeleteBranch(String),
    /// Show what the commit did to the file in the diff panel.
    CommitDiff { hash: String, path: PathBuf },
    /// Put this text on the clipboard.
    Copy(String),
}

/// A ref name on a commit row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChipKind {
    /// A local branch; `true` when it is checked out.
    Local(bool),
    Remote,
    Tag,
}

/// Sorts a `%D` entry into its kind and display name.
fn chip(name: &str, branches: &[Branch]) -> (ChipKind, String) {
    if let Some(tag) = name.strip_prefix("tag: ") {
        return (ChipKind::Tag, tag.to_owned());
    }
    match branches.iter().find(|b| b.name == name) {
        Some(b) => (ChipKind::Local(b.current), name.to_owned()),
        None => (ChipKind::Remote, name.to_owned()),
    }
}

/// Draws the Git Graph down the available height.
pub fn show(ui: &mut egui::Ui, graph: &mut GitGraphUi, git: &mut GitState, i18n: &I18n) -> Option<GraphAction> {
    let theme = theme::active(ui.ctx());
    let palette = theme.sidebar;
    let lane_colors = theme.graph_lanes();
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let mut action = None;

    sync_layout(graph, &git.graph);
    if let Some(a) = follow_seek(graph, &git.graph) {
        action = Some(a);
    }
    if let Some(a) = show_header(ui, &palette, git, i18n) {
        action = Some(a);
    }

    // Detail pane first: a bottom panel takes its height before the rows.
    if let Some(selected) = graph.selected.clone() {
        if let Some(commit) = git.graph.commits.iter().find(|c| c.hash == selected).cloned() {
            let height = ui.available_height() * 0.35;
            egui::Panel::bottom("git_graph_detail")
                .resizable(true)
                .default_size(height)
                .min_size(100.0)
                .max_size(ui.available_height() * 0.8)
                .show(ui, |ui| {
                    if let Some(a) = show_detail(ui, graph, git, &commit, i18n) {
                        action = Some(a);
                    }
                });
        } else if git.graph.complete && !git.graph.loading {
            graph.selected = None;
        }
    }

    if let Some(a) = show_rows(ui, graph, git, &palette, lane_colors, i18n) {
        action = Some(a);
    }
    if let Some(a) = show_new_branch_modal(ui.ctx(), graph, i18n) {
        action = Some(a);
    }
    action
}

/// Brings the lanes up to date with the loaded commits.
fn sync_layout(graph: &mut GitGraphUi, log: &Log) {
    if graph.laid_out.0 != log.generation || graph.laid_out.1 > log.commits.len() {
        // A reload: select the same commit again once it is back.
        if graph.laid_out.0 != log.generation
            && let Some(selected) = graph.selected.take()
        {
            graph.seek = Some(selected);
        }
        graph.layout = Layout::default();
        graph.laid_out = (log.generation, 0);
    }
    if graph.laid_out.1 < log.commits.len() {
        graph.layout.extend(&log.commits[graph.laid_out.1..]);
        graph.laid_out.1 = log.commits.len();
    }
}

/// Selects the sought commit once it is loaded, asking for pages until
/// then.
fn follow_seek(graph: &mut GitGraphUi, log: &Log) -> Option<GraphAction> {
    let seek = graph.seek.clone()?;
    match log.commits.iter().position(|c| c.hash == seek) {
        Some(row) => {
            graph.seek = None;
            graph.selected = Some(seek.clone());
            graph.scroll_to = Some(row);
            Some(GraphAction::Select(seek))
        }
        None if log.complete || log.error.is_some() => {
            graph.seek = None;
            None
        }
        None if !log.loading => Some(GraphAction::LoadMore),
        None => None,
    }
}

/// The title, the branch count and the close button.
fn show_header(ui: &mut egui::Ui, palette: &SidebarPalette, git: &GitState, i18n: &I18n) -> Option<GraphAction> {
    let width = ui.available_width();
    let (title_rect, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT), Sense::hover());
    let painter = ui.painter();
    let title = painter.text(
        pos2(title_rect.left() + TITLE_PADDING_X, title_rect.center().y),
        Align2::LEFT_CENTER,
        i18n.t("graph.title"),
        FontId::proportional(13.0),
        palette.foreground,
    );
    let count = i18n.tf("graph.branches", &[("count", &git.branches.len().to_string())]);
    painter.text(
        pos2(title.right() + 12.0, title_rect.center().y),
        Align2::LEFT_CENTER,
        count,
        FontId::proportional(11.0),
        palette.icon_inactive,
    );
    let close_rect = Rect::from_center_size(
        pos2(title_rect.right() - ROW_PADDING_X - ACTION_SIZE / 2.0, title_rect.center().y),
        Vec2::splat(ACTION_SIZE),
    );
    let close = ui
        .interact(close_rect, egui::Id::unique("git_graph_close"), Sense::click())
        .on_hover_text(i18n.t("graph.close"));
    if close.hovered() {
        ui.painter().rect_filled(close_rect.shrink(2.0), CornerRadius::same(4), palette.row_hover);
    }
    ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(16.0), palette.foreground);
    ui.painter().hline(title_rect.x_range(), title_rect.bottom() - 0.5, Stroke::new(1.0, palette.border));
    close.clicked().then_some(GraphAction::Close)
}

/// The commit rows and the tail row, in a scroll area.
fn show_rows(
    ui: &mut egui::Ui,
    graph: &mut GitGraphUi,
    git: &GitState,
    palette: &SidebarPalette,
    lane_colors: &[Color32; PALETTE_LEN],
    i18n: &I18n,
) -> Option<GraphAction> {
    let log = &git.graph;
    let branches = &git.branches;
    let mut action = None;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let extra = usize::from(log.loading || !log.complete || log.error.is_some() || log.commits.is_empty());
    let total = log.commits.len() + extra;
    let lane_count = graph.layout.lanes.clamp(1, MAX_LANES);
    let lanes_width = ROW_PADDING_X + lane_count as f32 * LANE_WIDTH;
    let mut scroll = egui::ScrollArea::vertical().id_salt("git_graph_rows").auto_shrink([false, false]);
    if let Some(row) = graph.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset((row as f32 * ROW_HEIGHT - ROW_HEIGHT * 3.0).max(0.0));
    }
    scroll.show_rows(ui, ROW_HEIGHT, total, |ui, range| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let width = ui.available_width();
        for index in range {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            let Some(commit) = log.commits.get(index) else {
                let response = ui.interact(rect, egui::Id::unique("git_graph_tail"), Sense::click());
                if let Some(a) = show_tail_row(ui, palette, rect, &response, log, i18n) {
                    action = Some(a);
                }
                continue;
            };
            let Some(row) = graph.layout.rows.get(index) else { continue };
            let response = ui.interact(rect, egui::Id::unique(("git_graph_row", &commit.hash)), Sense::click());
            let lit = graph.selected.as_deref() == Some(commit.hash.as_str());
            if lit {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, palette.row_selected);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
            }
            let prev = index.checked_sub(1).and_then(|p| graph.layout.rows.get(p));
            paint_lanes(&ui.painter().with_clip_rect(rect), rect, row, prev, commit.head, lane_colors, palette.accent);
            let text_left = rect.left() + lanes_width + BADGE_GAP;
            let chips_end = paint_chips(ui, palette, rect, text_left, commit, branches, i18n, &mut action);
            paint_text(ui, palette, rect, chips_end, commit, now);
            let response = response.on_hover_text(&commit.hash);
            if response.clicked() {
                graph.selected = Some(commit.hash.clone());
                action = Some(GraphAction::Select(commit.hash.clone()));
            }
            response.context_menu(|ui| {
                if ui.button(i18n.t("history.checkout")).clicked() {
                    action = Some(GraphAction::Checkout(commit.hash.clone()));
                    ui.close();
                }
                if ui.button(i18n.t("graph.create_branch")).clicked() {
                    graph.new_branch = Some(NewBranch { hash: commit.hash.clone(), ..NewBranch::default() });
                    ui.close();
                }
                if ui.button(i18n.t("graph.copy_hash")).clicked() {
                    action = Some(GraphAction::Copy(commit.hash.clone()));
                    ui.close();
                }
            });
        }
    });
    action
}

/// The row after the commits: a spinner, Load more, the error, or
/// "No commits yet".
fn show_tail_row(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    rect: Rect,
    response: &egui::Response,
    log: &Log,
    i18n: &I18n,
) -> Option<GraphAction> {
    let x = rect.left() + ROW_PADDING_X + LANE_WIDTH;
    if log.loading {
        egui::Spinner::new()
            .size(12.0)
            .color(palette.icon_inactive)
            .paint_at(ui, Rect::from_center_size(pos2(x + 6.0, rect.center().y), Vec2::splat(12.0)));
        return None;
    }
    if let Some(e) = &log.error {
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, e.to_string(), FontId::proportional(13.0), palette.git_deleted);
        return None;
    }
    if !log.complete {
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, palette.row_hover);
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, i18n.t("graph.load_more"), FontId::proportional(13.0), palette.accent);
        return response.clicked().then_some(GraphAction::LoadMore);
    }
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, i18n.t("history.no_commits"), FontId::proportional(13.0), palette.icon_inactive);
    None
}

/// The centre of a lane column in a row.
fn lane_x(rect: Rect, lane: usize) -> f32 {
    rect.left() + ROW_PADDING_X + LANE_WIDTH / 2.0 + lane.min(MAX_LANES - 1) as f32 * LANE_WIDTH
}

/// The commit's dot, the lines into it from the row above and the lines
/// out of it; `painter` clips to the row.
fn paint_lanes(
    painter: &egui::Painter,
    rect: Rect,
    row: &GraphRow,
    prev: Option<&GraphRow>,
    head: bool,
    colors: &[Color32; PALETTE_LEN],
    accent: Color32,
) {
    let centre = rect.center().y;
    if let Some(prev) = prev {
        paint_edges(painter, rect, &prev.edges, centre - ROW_HEIGHT, colors);
    }
    paint_edges(painter, rect, &row.edges, centre, colors);
    let center = pos2(lane_x(rect, row.lane), centre);
    let color = colors[row.color % PALETTE_LEN];
    if head {
        painter.circle_filled(center, DOT_RADIUS + 1.0, color);
        painter.circle_stroke(center, DOT_RADIUS + 3.0, Stroke::new(1.5, accent));
    } else {
        painter.circle_filled(center, DOT_RADIUS, color);
    }
}

/// Lines from one row's dot height at `top` to the next row's.
fn paint_edges(painter: &egui::Painter, rect: Rect, edges: &[Edge], top: f32, colors: &[Color32; PALETTE_LEN]) {
    let bottom = top + ROW_HEIGHT;
    for edge in edges {
        let stroke = Stroke::new(LINE_WIDTH, colors[edge.color % PALETTE_LEN]);
        let (x0, x1) = (lane_x(rect, edge.from), lane_x(rect, edge.to));
        if edge.from == edge.to {
            painter.line_segment([pos2(x0, top), pos2(x1, bottom)], stroke);
        } else {
            let mid = (top + bottom) / 2.0;
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [pos2(x0, top), pos2(x0, mid), pos2(x1, mid), pos2(x1, bottom)],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
        }
    }
}

/// Branch and tag chips after the lanes; returns where they end. A chip's
/// right-click menu offers what fits its kind.
#[allow(clippy::too_many_arguments)]
fn paint_chips(
    ui: &mut egui::Ui,
    palette: &SidebarPalette,
    rect: Rect,
    mut x: f32,
    commit: &Commit,
    branches: &[Branch],
    i18n: &I18n,
    action: &mut Option<GraphAction>,
) -> f32 {
    let font = FontId::proportional(11.0);
    // A detached HEAD is already marked by the dot's ring.
    let refs: Vec<&String> = commit.refs.iter().filter(|r| r.as_str() != "HEAD").collect();
    let folded = refs.len().saturating_sub(MAX_CHIPS);
    for name in refs.into_iter().take(MAX_CHIPS) {
        let (kind, label) = chip(name, branches);
        let text: String = if label.chars().count() > MAX_CHIP_CHARS {
            format!("{}…", label.chars().take(MAX_CHIP_CHARS - 1).collect::<String>())
        } else {
            label.clone()
        };
        let (fill, text_color, outline) = match kind {
            ChipKind::Local(_) => (palette.accent, Color32::WHITE, None),
            ChipKind::Remote => (Color32::TRANSPARENT, palette.icon_inactive, Some(palette.icon_inactive)),
            ChipKind::Tag => (palette.git_added, Color32::BLACK, None),
        };
        let galley = ui.painter().layout_no_wrap(text, font.clone(), text_color);
        let chip_rect = Rect::from_min_size(
            pos2(x, rect.center().y - CHIP_HEIGHT / 2.0),
            vec2(galley.size().x + CHIP_PADDING * 2.0, CHIP_HEIGHT),
        );
        let response = ui
            .interact(chip_rect, egui::Id::unique(("git_graph_chip", &commit.hash, name)), Sense::click())
            .on_hover_text(&label);
        let painter = ui.painter();
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(chip_rect, CornerRadius::same(8), fill);
        }
        if let Some(outline) = outline {
            painter.rect_stroke(chip_rect, CornerRadius::same(8), Stroke::new(1.0, outline), egui::StrokeKind::Inside);
        }
        painter.galley(pos2(chip_rect.left() + CHIP_PADDING, chip_rect.center().y - galley.size().y / 2.0), galley, text_color);
        if kind == ChipKind::Local(true) {
            painter.rect_stroke(chip_rect, CornerRadius::same(8), Stroke::new(1.5, palette.foreground), egui::StrokeKind::Outside);
        }
        response.context_menu(|ui| match kind {
            ChipKind::Local(current) => {
                if ui.add_enabled(!current, egui::Button::new(i18n.t("graph.switch"))).clicked() {
                    *action = Some(GraphAction::Switch(label.clone()));
                    ui.close();
                }
                if ui.add_enabled(!current, egui::Button::new(i18n.t("graph.delete_branch"))).clicked() {
                    *action = Some(GraphAction::DeleteBranch(label.clone()));
                    ui.close();
                }
                if ui.button(i18n.t("graph.copy_name")).clicked() {
                    *action = Some(GraphAction::Copy(label.clone()));
                    ui.close();
                }
            }
            ChipKind::Remote | ChipKind::Tag => {
                if ui.button(i18n.t("graph.copy_name")).clicked() {
                    *action = Some(GraphAction::Copy(label.clone()));
                    ui.close();
                }
            }
        });
        x = chip_rect.right() + BADGE_GAP;
    }
    if folded > 0 {
        let galley = ui.painter().layout_no_wrap(format!("+{folded}"), font, palette.icon_inactive);
        let width = galley.size().x;
        ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, palette.icon_inactive);
        x += width + BADGE_GAP;
    }
    x
}

/// Subject, then `author · age` and the short hash on the right.
fn paint_text(ui: &egui::Ui, palette: &SidebarPalette, rect: Rect, left: f32, commit: &Commit, now: i64) {
    let painter = ui.painter();
    let mut right = rect.right() - ROW_PADDING_X;
    let hash = file_browser::elided(ui, commit.short.clone(), FontId::monospace(11.0), palette.icon_inactive, 80.0);
    right -= hash.size().x;
    painter.galley(pos2(right, rect.center().y - hash.size().y / 2.0), hash, palette.icon_inactive);
    right -= BADGE_GAP * 2.0;
    let meta = format!("{} · {}", commit.author, git::age(now - commit.time));
    let meta = file_browser::elided(ui, meta, FontId::proportional(11.0), palette.icon_inactive, (right - left) * 0.4);
    right -= meta.size().x;
    painter.galley(pos2(right, rect.center().y - meta.size().y / 2.0), meta, palette.icon_inactive);
    right -= BADGE_GAP;
    let subject = file_browser::elided(ui, commit.subject.clone(), FontId::proportional(13.0), palette.foreground, (right - left).max(0.0));
    painter.galley(pos2(left, rect.center().y - subject.size().y / 2.0), subject, palette.foreground);
}

/// The branch-name prompt of Create Branch Here; Enter creates, Escape
/// closes.
fn show_new_branch_modal(ctx: &egui::Context, graph: &mut GitGraphUi, i18n: &I18n) -> Option<GraphAction> {
    let new_branch = graph.new_branch.as_mut()?;
    let short: String = new_branch.hash.chars().take(7).collect();
    let title = i18n.tf("graph.create_branch_title", &[("hash", &short)]);
    let mut action = None;
    let mut close = false;
    egui::Window::new(title)
        .id(egui::Id::unique("git_graph_new_branch"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut new_branch.name).hint_text(i18n.t("git.new_branch_hint")));
            if !new_branch.focused {
                field.request_focus();
                new_branch.focused = true;
            }
            let name = new_branch.name.trim().to_owned();
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                let create = ui.add_enabled(!name.is_empty(), egui::Button::new(i18n.t("graph.create_branch_confirm")));
                if (create.clicked() || enter) && !name.is_empty() {
                    action = Some(GraphAction::CreateBranch { name, hash: new_branch.hash.clone() });
                    close = true;
                }
                let escape = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                if ui.button(i18n.t("unsaved.cancel")).clicked() || escape {
                    close = true;
                }
            });
        });
    if close {
        graph.new_branch = None;
    }
    action
}

/// Hash, author, date, refs, message, parents and files of `commit`.
fn show_detail(ui: &mut egui::Ui, graph: &mut GitGraphUi, git: &mut GitState, commit: &Commit, i18n: &I18n) -> Option<GraphAction> {
    let palette = theme::active(ui.ctx()).sidebar;
    let root = git.repo().map(|r| r.root().to_path_buf()).unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let details = git.commit_details(ui.ctx(), &commit.hash).cloned();
    let files = git.commit_files(ui.ctx(), &commit.hash).cloned();
    let mut action = None;
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&commit.short).monospace().color(palette.foreground));
            if ui.small_button(i18n.t("graph.copy_hash")).clicked() {
                action = Some(GraphAction::Copy(commit.hash.clone()));
            }
            ui.label(egui::RichText::new(&commit.author).color(palette.foreground));
            let date = match &details {
                Some(Ok(d)) => format!("{} · {}", d.date, git::age(now - commit.time)),
                _ => git::age(now - commit.time),
            };
            ui.label(egui::RichText::new(date).color(palette.icon_inactive));
        });
        ui.horizontal_wrapped(|ui| {
            for name in &commit.refs {
                let (_, label) = chip(name, &git.branches);
                ui.label(egui::RichText::new(label).small().color(palette.accent));
            }
            if ui.button(i18n.t("history.checkout")).clicked() {
                action = Some(GraphAction::Checkout(commit.hash.clone()));
            }
            if ui.button(i18n.t("graph.create_branch")).clicked() {
                graph.new_branch = Some(NewBranch { hash: commit.hash.clone(), name: String::new(), focused: false });
            }
        });
        egui::ScrollArea::vertical()
            .id_salt(("git_graph_msg", &commit.hash))
            .max_height(ui.available_height() * 0.5)
            .auto_shrink([false, true])
            .show(ui, |ui| match &details {
                Some(Ok(d)) => {
                    ui.label(egui::RichText::new(&d.message).monospace().color(palette.foreground));
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e.to_string()).color(palette.git_deleted));
                }
                None => {
                    ui.label(egui::RichText::new(&commit.subject).monospace().color(palette.foreground));
                }
            });
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(i18n.t("graph.parents")).small().color(palette.icon_inactive));
            for parent in &commit.parents {
                let short: String = parent.chars().take(7).collect();
                if ui.link(egui::RichText::new(short).monospace()).clicked() {
                    match git.graph.commits.iter().position(|c| c.hash == *parent) {
                        Some(row) => {
                            graph.selected = Some(parent.clone());
                            graph.scroll_to = Some(row);
                            action = Some(GraphAction::Select(parent.clone()));
                        }
                        None => graph.seek = Some(parent.clone()),
                    }
                }
            }
        });
        ui.label(egui::RichText::new(i18n.t("graph.files")).small().color(palette.icon_inactive));
    });
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let width = ui.available_width();
    match files {
        None => {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            egui::Spinner::new().size(12.0).color(palette.icon_inactive).paint_at(
                ui,
                Rect::from_center_size(pos2(rect.left() + 24.0, rect.center().y), Vec2::splat(12.0)),
            );
        }
        Some(Err(e)) => {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            ui.painter().text(
                pos2(rect.left() + 12.0, rect.center().y),
                Align2::LEFT_CENTER,
                e.to_string(),
                FontId::proportional(13.0),
                palette.git_deleted,
            );
        }
        Some(Ok(files)) => {
            egui::ScrollArea::vertical()
                .id_salt(("git_graph_files", &commit.hash))
                .auto_shrink([false, false])
                .max_height(ui.available_height())
                .show_rows(ui, ROW_HEIGHT, files.len(), |ui, range| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let width = ui.available_width();
                    for file in &files[range] {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                        let row = ui.interact(rect, egui::Id::unique(("git_graph_file", &commit.hash, &file.path)), Sense::click());
                        let change = git::FileChange { path: file.path.clone(), orig_path: None, staged: Some(file.kind), unstaged: None };
                        crate::source_control::paint_file_row(ui, &palette, rect, &change, file.kind, &root, row.hovered(), 0);
                        if row.clicked() {
                            action = Some(GraphAction::CommitDiff { hash: commit.hash.clone(), path: file.path.clone() });
                        }
                    }
                });
        }
    }
    // Fill the rest so the panel keeps its height while files load.
    ui.allocate_space(ui.available_size());
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.to_owned(),
            short: hash.chars().take(7).collect(),
            author: "Test".into(),
            time: 0,
            subject: hash.to_owned(),
            refs: Vec::new(),
            head: false,
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn lanes(layout: &Layout) -> Vec<usize> {
        layout.rows.iter().map(|r| r.lane).collect()
    }

    #[test]
    fn test_linear_history_is_one_lane() {
        let layout = Layout::new(&[commit("a", &["b"]), commit("b", &["c"]), commit("c", &[])]);
        assert_eq!(lanes(&layout), [0, 0, 0]);
        assert_eq!(layout.lanes, 1);
        assert_eq!(layout.rows[0].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert_eq!(layout.rows[1].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert!(layout.rows[2].edges.is_empty());
        assert!(layout.active.is_empty(), "nothing waits after the root");
    }

    #[test]
    fn test_branch_and_merge_use_two_lanes() {
        let commits = [
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 0, 1, 0]);
        assert_eq!(layout.lanes, 2);
        // The merge forks into lane 1 for its second parent.
        assert!(layout.rows[0].edges.contains(&Edge { from: 0, to: 1, color: 1 }));
        assert!(layout.rows[0].edges.contains(&Edge { from: 0, to: 0, color: 0 }));
        // `b` sits in lane 1 and its line joins `r` in lane 0.
        assert!(layout.rows[2].edges.contains(&Edge { from: 1, to: 0, color: 1 }));
        assert!(layout.rows[2].edges.contains(&Edge { from: 0, to: 0, color: 0 }));
        assert_eq!(layout.rows[2].color, 1);
        assert!(layout.active.is_empty());
    }

    #[test]
    fn test_octopus_merge_opens_a_lane_per_extra_parent() {
        let commits = [
            commit("m", &["a", "b", "c"]),
            commit("a", &[]),
            commit("b", &[]),
            commit("c", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 0, 1, 2]);
        assert_eq!(layout.lanes, 3);
        let colors: Vec<usize> = layout.rows.iter().map(|r| r.color).collect();
        assert_eq!(colors, [0, 0, 1, 2]);
    }

    #[test]
    fn test_second_root_takes_a_free_lane() {
        let layout = Layout::new(&[commit("a", &["b"]), commit("x", &[]), commit("b", &[])]);
        assert_eq!(lanes(&layout), [0, 1, 0]);
        assert_eq!(layout.lanes, 2);
        assert_eq!(layout.rows[0].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert_eq!(layout.rows[1].edges, [Edge { from: 0, to: 0, color: 0 }], "the line passes `x`");
    }

    #[test]
    fn test_extending_in_two_halves_matches_one_pass() {
        let commits = [
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let whole = Layout::new(&commits);
        for split in 0..=commits.len() {
            let mut halves = Layout::new(&commits[..split]);
            halves.extend(&commits[split..]);
            assert_eq!(halves, whole, "split at {split}");
        }
    }

    #[test]
    fn test_colors_cycle_through_the_palette() {
        let mut commits = vec![commit("m", &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8"])];
        for i in 0..9 {
            commits.push(commit(&format!("p{i}"), &[]));
        }
        let layout = Layout::new(&commits);
        assert_eq!(layout.rows[9].color, 0, "the ninth lane reuses the first colour");
        assert_eq!(layout.lanes, 9);
    }

    #[test]
    fn test_merge_second_parent_joins_a_waiting_lane() {
        let commits = [
            commit("x", &["b"]),
            commit("m", &["a", "b"]),
            commit("a", &["b"]),
            commit("b", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 1, 1, 0]);
        assert_eq!(layout.lanes, 2);
        assert_eq!(layout.rows[0].edges, [Edge { from: 0, to: 0, color: 0 }]);
        assert_eq!(
            layout.rows[1].edges,
            [Edge { from: 0, to: 0, color: 0 }, Edge { from: 1, to: 0, color: 0 }, Edge { from: 1, to: 1, color: 1 }],
            "the merge line into b keeps b's lane colour; a continues in its own"
        );
        assert_eq!(
            layout.rows[2].edges,
            [Edge { from: 0, to: 0, color: 0 }, Edge { from: 1, to: 0, color: 1 }],
            "a's first-parent curve into b keeps a's colour"
        );
        assert!(layout.active.is_empty());
    }

    #[test]
    fn test_freed_lane_is_reused_and_joins_to_the_right() {
        let commits = [
            commit("a", &["x"]),
            commit("b", &["y"]),
            commit("x", &[]),
            commit("c", &["y"]),
            commit("y", &[]),
        ];
        let layout = Layout::new(&commits);
        assert_eq!(lanes(&layout), [0, 1, 0, 0, 1]);
        let colors: Vec<usize> = layout.rows.iter().map(|r| r.color).collect();
        assert_eq!(colors, [0, 1, 0, 2, 1]);
        assert_eq!(layout.rows[2].edges, [Edge { from: 1, to: 1, color: 1 }]);
        assert_eq!(layout.rows[3].edges, [Edge { from: 1, to: 1, color: 1 }, Edge { from: 0, to: 1, color: 2 }]);
        assert_eq!(layout.lanes, 2);
        assert!(layout.active.is_empty());
    }

    use crate::git_worker::GitState;
    use crate::i18n::I18n;
    use eframe::egui::{self, Rect, vec2};
    use std::fs;
    use std::path::PathBuf;

    /// A repository with `first` on main, `second` on `feature`, `third`
    /// on main, and a state with the graph loaded.
    fn fixture(name: &str, ctx: &egui::Context) -> Option<(GitState, PathBuf)> {
        let repo = crate::git::temp_repo(name)?;
        let root = repo.root().to_path_buf();
        fs::write(root.join("a.txt"), "one\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("first").unwrap();
        repo.create_branch("feature").unwrap();
        fs::write(root.join("b.txt"), "bee\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("second").unwrap();
        repo.switch("main").unwrap();
        fs::write(root.join("a.txt"), "two\n").unwrap();
        repo.stage_all().unwrap();
        repo.commit("third").unwrap();
        let mut git = GitState::default();
        git.open(ctx, root.clone());
        wait(ctx, &mut git, |g| g.status.is_some());
        git.load_graph(ctx);
        wait(ctx, &mut git, |g| !g.graph.loading);
        Some((git, root))
    }

    fn wait(ctx: &egui::Context, git: &mut GitState, done: impl Fn(&GitState) -> bool) {
        for _ in 0..250 {
            git.poll(ctx);
            if done(git) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("timed out");
    }

    fn frame(
        ctx: &egui::Context,
        git: &mut GitState,
        graph: &mut GitGraphUi,
        events: Vec<egui::Event>,
    ) -> (Option<GraphAction>, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            action = show(ui, graph, git, &I18n::builtin());
        });
        output.textures_delta.clear();
        (action, output.shapes)
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), Rect::from_min_size(t.pos, t.galley.size()))),
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
    fn test_graph_lists_every_branch_with_chips_and_selects_a_row() {
        let ctx = egui::Context::default();
        let Some((mut git, root)) = fixture("graph_rows", &ctx) else { return };
        let mut graph = GitGraphUi::default();
        let (_, shapes) = frame(&ctx, &mut git, &mut graph, Vec::new());
        let painted = texts(&shapes);
        let names: Vec<&str> = painted.iter().map(|(t, _)| t.as_str()).collect();
        for expected in ["Git Graph", "third", "second", "first", "main", "feature"] {
            assert!(names.contains(&expected), "{expected} missing from {names:?}");
        }
        assert!(!names.contains(&I18n::builtin().t("graph.load_more")), "the log is complete");
        assert!(graph.layout.lanes >= 2, "two branches need two lanes");

        let (_, second) = painted.iter().find(|(t, _)| t == "second").unwrap();
        let (action, _) = frame(&ctx, &mut git, &mut graph, click(second.center()));
        let hash = git.graph.commits.iter().find(|c| c.subject == "second").unwrap().hash.clone();
        assert_eq!(action, Some(GraphAction::Select(hash.clone())));
        assert_eq!(graph.selected, Some(hash));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_load_more_row_only_while_incomplete() {
        let ctx = egui::Context::default();
        let Some((mut git, root)) = fixture("graph_more", &ctx) else { return };
        let mut graph = GitGraphUi::default();
        git.graph.complete = false;
        let (_, shapes) = frame(&ctx, &mut git, &mut graph, Vec::new());
        let painted = texts(&shapes);
        let (_, more) = painted.iter().find(|(t, _)| t == I18n::builtin().t("graph.load_more")).expect("load more row");
        let (action, _) = frame(&ctx, &mut git, &mut graph, click(more.center()));
        assert_eq!(action, Some(GraphAction::LoadMore));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_close_button_asks_to_close() {
        let ctx = egui::Context::default();
        let Some((mut git, root)) = fixture("graph_close", &ctx) else { return };
        let mut graph = GitGraphUi::default();
        let (_, shapes) = frame(&ctx, &mut git, &mut graph, Vec::new());
        let (_, close) = texts(&shapes).into_iter().find(|(t, _)| t == "×").expect("close button");
        let (action, _) = frame(&ctx, &mut git, &mut graph, click(close.center()));
        assert_eq!(action, Some(GraphAction::Close));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_repeated_commit_does_not_break_the_rows() {
        let ctx = egui::Context::default();
        let Some((mut git, root)) = fixture("graph_repeat", &ctx) else { return };
        let mut graph = GitGraphUi::default();
        let repeated = git.graph.commits[0].clone();
        git.graph.commits.push(repeated);
        frame(&ctx, &mut git, &mut graph, Vec::new());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_detail_pane_lists_files_parents_and_opens_a_diff() {
        let ctx = egui::Context::default();
        let Some((mut git, root)) = fixture("graph_detail", &ctx) else { return };
        let mut graph = GitGraphUi::default();
        let third = git.graph.commits.iter().find(|c| c.subject == "third").unwrap().clone();
        graph.selected = Some(third.hash.clone());
        frame(&ctx, &mut git, &mut graph, Vec::new());
        wait(&ctx, &mut git, |g| {
            g.commit_details.get(&third.hash).is_some_and(|d| d.as_ref().is_some_and(|d| d.is_ok()))
                && g.commit_files.get(&third.hash).is_some_and(|f| matches!(f, Some(Ok(v)) if !v.is_empty()))
        });
        let (_, shapes) = frame(&ctx, &mut git, &mut graph, Vec::new());
        let painted = texts(&shapes);
        let names: Vec<&str> = painted.iter().map(|(t, _)| t.as_str()).collect();
        let (_, hash) = painted
            .iter()
            .filter(|(t, _)| t.starts_with(&third.short))
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("short hash");
        assert!(hash.top() > 300.0, "short hash in the pane: {names:?}");
        assert!(names.contains(&"Induk"), "parents heading: {names:?}");
        assert!(names.contains(&"File"), "files heading: {names:?}");
        assert!(names.contains(&"M"), "a.txt was modified: {names:?}");

        // The file row opens the diff of that commit.
        let (_, file) = painted
            .iter()
            .filter(|(t, _)| t == "a.txt")
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("file row");
        let (action, _) = frame(&ctx, &mut git, &mut graph, click(file.center()));
        assert_eq!(action, Some(GraphAction::CommitDiff { hash: third.hash.clone(), path: root.join("a.txt") }));

        // The parent link selects the parent.
        let parent = third.parents[0].clone();
        let short: String = parent.chars().take(7).collect();
        let (_, link) = painted
            .iter()
            .filter(|(t, _)| *t == short)
            .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("parent link");
        let (action, _) = frame(&ctx, &mut git, &mut graph, click(link.center()));
        assert_eq!(action, Some(GraphAction::Select(parent.clone())));
        assert_eq!(graph.selected, Some(parent));
        let _ = fs::remove_dir_all(&root);
    }
}
