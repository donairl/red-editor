//! Who last changed each line: `git blame` parsed, and painted in a column
//! left of the editor.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, FontId, Rect, Sense, pos2};

use crate::git;
use crate::theme;

/// Authors longer than this are cut with `…`.
const AUTHOR_CHARS: usize = 14;
const PAD_X: f32 = 8.0;

/// The commit that last changed one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
}

impl BlameLine {
    /// Lines not committed yet, as `git blame` reports them.
    pub fn is_uncommitted(&self) -> bool {
        self.hash.chars().all(|c| c == '0')
    }
}

/// Parses `git blame --porcelain`: one line per source line, in order.
///
/// Each group starts with `<hash> <orig> <final> [<count>]`; a commit's
/// `author`, `author-time` and `summary` headers come with its first group
/// only, so they are kept by hash for the groups that follow.
pub fn parse_porcelain(out: &str) -> Vec<BlameLine> {
    let mut commits: HashMap<String, BlameLine> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<String> = None;
    for line in out.lines() {
        if let Some(hash) = current.as_deref() {
            if line.starts_with('\t') {
                if let Some(commit) = commits.get(hash) {
                    lines.push(commit.clone());
                }
                current = None;
                continue;
            }
            let commit = commits.get_mut(hash).expect("header follows its hash line");
            if let Some(author) = line.strip_prefix("author ") {
                commit.author = author.to_owned();
            } else if let Some(time) = line.strip_prefix("author-time ") {
                commit.time = time.parse().unwrap_or(0);
            } else if let Some(summary) = line.strip_prefix("summary ") {
                commit.summary = summary.to_owned();
            }
            continue;
        }
        // `<hash> <orig> <final> [<count>]`
        let mut parts = line.split(' ');
        let (Some(hash), Some(_), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if hash.len() < 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        commits.entry(hash.to_owned()).or_insert_with(|| BlameLine {
            hash: hash.to_owned(),
            author: String::new(),
            time: 0,
            summary: String::new(),
        });
        current = Some(hash.to_owned());
    }
    lines
}

/// `git blame` of one file, ready to paint.
#[derive(Clone, Debug)]
pub struct Blame {
    pub lines: Arc<Vec<BlameLine>>,
    /// Longest `author · age` label, in characters.
    pub label_chars: usize,
}

impl Blame {
    pub fn new(lines: Vec<BlameLine>, now: i64) -> Blame {
        let label_chars = lines
            .iter()
            .map(|l| label(l, now).chars().count())
            .max()
            .unwrap_or(0);
        Blame {
            lines: Arc::new(lines),
            label_chars,
        }
    }

    /// Width of the column for these labels in `font`.
    pub fn width(&self, ui: &egui::Ui, font: &FontId) -> f32 {
        let glyph = ui.fonts_mut(|f| f.glyph_width(font, '0'));
        (self.label_chars.max(8) as f32 * glyph + 2.0 * PAD_X).ceil()
    }
}

/// `author · age` of one line; uncommitted lines are unnamed.
fn label(line: &BlameLine, now: i64) -> String {
    if line.is_uncommitted() {
        return "·".to_owned();
    }
    let mut author: String = line.author.chars().take(AUTHOR_CHARS).collect();
    if line.author.chars().count() > AUTHOR_CHARS {
        author.pop();
        author.push('…');
    }
    format!("{author} · {}", git::age(now - line.time))
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Paints the column into `strip`, one label beside the first row of each
/// visible line of `galley` (placed at `galley_pos`). Hovering a label
/// shows the commit's hash and summary.
pub fn paint_column(
    ui: &mut egui::Ui,
    strip: Rect,
    blame: &Blame,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    font: &FontId,
    now: i64,
) {
    let palette = theme::active(ui.ctx()).gutter;
    let clip = ui.clip_rect().intersect(strip);
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_filled(strip, 0.0, palette.background);
    painter.vline(
        strip.right() - 0.5,
        strip.y_range(),
        egui::Stroke::new(1.0, theme::active(ui.ctx()).sidebar.border),
    );
    let mut line = 1;
    let mut starts_line = true;
    for row in &galley.rows {
        let rect = row.rect().translate(galley_pos.to_vec2());
        if rect.top() > clip.max.y {
            break;
        }
        if starts_line
            && rect.bottom() >= clip.min.y
            && let Some(info) = blame.lines.get(line - 1)
        {
            painter.text(
                pos2(strip.left() + PAD_X, rect.center().y),
                Align2::LEFT_CENTER,
                label(info, now),
                font.clone(),
                palette.number,
            );
            if !info.is_uncommitted() {
                let hover = Rect::from_x_y_ranges(strip.x_range(), rect.y_range());
                ui.interact(hover, egui::Id::unique(("blame", line)), Sense::hover())
                    .on_hover_text(format!("{}  {}", &info.hash[..7], info.summary));
            }
        }
        starts_line = row.ends_with_newline;
        if row.ends_with_newline {
            line += 1;
        }
    }
}

/// Paints the column with only `note` at its top: the blame is loading or
/// cannot be shown.
pub fn paint_note(ui: &mut egui::Ui, strip: Rect, note: &str, font: &FontId) {
    let theme = theme::active(ui.ctx());
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(strip));
    painter.rect_filled(strip, 0.0, theme.gutter.background);
    painter.vline(strip.right() - 0.5, strip.y_range(), egui::Stroke::new(1.0, theme.sidebar.border));
    let mut job = egui::text::LayoutJob::simple(
        note.to_owned(),
        font.clone(),
        theme.gutter.number,
        strip.width() - 2.0 * PAD_X,
    );
    job.wrap.max_rows = 3;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(pos2(strip.left() + PAD_X, strip.top() + 8.0), galley, theme.gutter.number);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_porcelain_repeats_a_commit_without_its_headers() {
        let out = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 2
author Ana
author-mail <a@x>
author-time 1700000000
author-tz +0000
committer Ana
summary First commit
filename f.txt
\tline one
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2
\tline two
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 1 3 1
author Bo
author-time 1700003600
summary Second
previous aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa f.txt
filename f.txt
\tline three
0000000000000000000000000000000000000000 4 4 1
author Not Committed Yet
author-time 1700007200
summary Version of f.txt from f.txt
\tline four
";
        let lines = parse_porcelain(out);
        let view: Vec<(&str, &str, i64, &str)> = lines
            .iter()
            .map(|l| (&l.hash[..1], l.author.as_str(), l.time, l.summary.as_str()))
            .collect();
        assert_eq!(
            view,
            [
                ("a", "Ana", 1_700_000_000, "First commit"),
                ("a", "Ana", 1_700_000_000, "First commit"),
                ("b", "Bo", 1_700_003_600, "Second"),
                ("0", "Not Committed Yet", 1_700_007_200, "Version of f.txt from f.txt"),
            ]
        );
        assert!(lines[3].is_uncommitted() && !lines[0].is_uncommitted());
        assert!(parse_porcelain("").is_empty());
    }

    #[test]
    fn test_labels_shorten_long_authors() {
        let now = 1_700_000_000 + 2 * 86_400;
        let long = BlameLine {
            hash: "a".repeat(40),
            author: "Bartholomew Longname".into(),
            time: 1_700_000_000,
            summary: String::new(),
        };
        assert_eq!(label(&long, now), "Bartholomew L… · 2d");
        let uncommitted = BlameLine {
            hash: "0".repeat(40),
            ..long.clone()
        };
        assert_eq!(label(&uncommitted, now), "·");
        let blame = Blame::new(vec![long, uncommitted], now);
        assert_eq!(blame.label_chars, "Bartholomew L… · 2d".chars().count());
    }
}
