//! The Git Log panel: every git command the editor ran, with its exit code,
//! timing and output, so a failure can be read in full instead of the one
//! line the status bar shows.
//!
//! `git.rs` records each command into a process-wide log through
//! [`record`]; git runs on worker threads, so the log sits behind a mutex.
//! The newest [`CAPACITY`] commands are kept.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::i18n::I18n;

/// How many commands the log keeps.
pub const CAPACITY: usize = 300;
/// How much of stdout or stderr an entry keeps.
const OUTPUT_LIMIT: usize = 8 * 1024;

/// One git command and how it ended.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Unique and increasing; the panel's id for the row.
    pub id: u64,
    pub started: Instant,
    /// The folder git ran in (`git -C`).
    pub dir: PathBuf,
    /// The arguments after `git`, without the options every call adds.
    pub args: Vec<String>,
    pub duration: Duration,
    /// The exit code; `None` when git never ran or was killed.
    pub exit: Option<i32>,
    /// Why the command did not finish on its own (not installed, timeout).
    pub failure: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl Entry {
    pub fn ok(&self) -> bool {
        self.failure.is_none() && self.exit == Some(0)
    }

    /// `git status --porcelain`, the way a shell would show it.
    pub fn command_line(&self) -> String {
        let mut line = String::from("git");
        for arg in &self.args {
            line.push(' ');
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                line.push('"');
                line.push_str(arg);
                line.push('"');
            } else {
                line.push_str(arg);
            }
        }
        line
    }

    /// The entry as plain text, for the clipboard.
    fn to_text(&self) -> String {
        let mut text = format!(
            "$ {}\n  dir: {}\n  {} in {} ms\n",
            self.command_line(),
            self.dir.display(),
            self.outcome(),
            self.duration.as_millis()
        );
        for (name, output) in [("stderr", &self.stderr), ("stdout", &self.stdout)] {
            if !output.trim().is_empty() {
                text.push_str(&format!("  {name}:\n"));
                for line in output.trim_end().lines() {
                    text.push_str("    ");
                    text.push_str(line);
                    text.push('\n');
                }
            }
        }
        text
    }

    /// `exit 0`, `exit 128`, or why git did not exit.
    fn outcome(&self) -> String {
        match (&self.failure, self.exit) {
            (Some(failure), _) => failure.clone(),
            (None, Some(code)) => format!("exit {code}"),
            (None, None) => "killed".to_owned(),
        }
    }
}

struct Log {
    entries: VecDeque<Arc<Entry>>,
    next_id: u64,
}

static LOG: Mutex<Log> = Mutex::new(Log { entries: VecDeque::new(), next_id: 0 });

fn log() -> MutexGuard<'static, Log> {
    // A panic while recording leaves the log usable.
    LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What [`record`] needs about one finished command.
pub struct Record<'a> {
    pub started: Instant,
    pub dir: PathBuf,
    pub args: &'a [&'a str],
    pub exit: Option<i32>,
    pub failure: Option<String>,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
}

/// Adds a finished command to the log, dropping the oldest past
/// [`CAPACITY`].
pub fn record(record: Record<'_>) {
    let entry = |id| Entry {
        id,
        started: record.started,
        dir: record.dir.clone(),
        args: record.args.iter().map(|a| (*a).to_owned()).collect(),
        duration: record.started.elapsed(),
        exit: record.exit,
        failure: record.failure.clone(),
        stdout: clip(record.stdout),
        stderr: clip(record.stderr),
    };
    let mut log = log();
    let id = log.next_id;
    log.next_id += 1;
    log.entries.push_back(Arc::new(entry(id)));
    while log.entries.len() > CAPACITY {
        log.entries.pop_front();
    }
}

/// The logged commands, oldest first.
pub fn entries() -> Vec<Arc<Entry>> {
    log().entries.iter().cloned().collect()
}

pub fn clear() {
    log().entries.clear();
}

/// `bytes` as text, cut to [`OUTPUT_LIMIT`] on a character boundary.
fn clip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= OUTPUT_LIMIT {
        return text.into_owned();
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} bytes more)", &text[..end], text.len() - end)
}

/// How long ago, shortly: `4s`, `12m`, `3h`.
fn ago(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    match s {
        _ if s < 60 => format!("{s}s"),
        _ if s < 3600 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

/// The panel's own settings.
#[derive(Default)]
pub struct GitLogPanel {
    /// List only the commands that failed.
    pub errors_only: bool,
}

impl GitLogPanel {
    /// Draws the panel below the editor. Returns `true` when Hide was
    /// clicked.
    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) -> bool {
        let entries = entries();
        let shown: Vec<&Arc<Entry>> =
            entries.iter().filter(|e| !self.errors_only || !e.ok()).collect();
        let failed = entries.iter().filter(|e| !e.ok()).count();
        let mut hide = false;
        egui::Panel::bottom("git_log_panel")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(i18n.t("git_log.title"));
                    let counts = i18n.tf(
                        "git_log.counts",
                        &[("total", &entries.len().to_string()), ("failed", &failed.to_string())],
                    );
                    ui.weak(counts);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        hide = ui.small_button(i18n.t("git_log.hide")).clicked();
                        if ui.small_button(i18n.t("git_log.clear")).clicked() {
                            clear();
                        }
                        if ui.small_button(i18n.t("git_log.copy")).clicked() {
                            let text: String = shown.iter().map(|e| e.to_text()).collect();
                            ui.ctx().copy_text(text);
                        }
                        ui.checkbox(&mut self.errors_only, i18n.t("git_log.errors_only"));
                    });
                });
                ui.separator();
                if shown.is_empty() {
                    ui.weak(i18n.t("git_log.empty"));
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &shown {
                            show_entry(ui, entry);
                        }
                    });
            });
        // Keep the "how long ago" column current.
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        hide
    }
}

fn show_entry(ui: &mut egui::Ui, entry: &Entry) {
    let error = ui.visuals().error_fg_color;
    let ok = entry.ok();
    let (mark, color) = if ok {
        ("✔", ui.visuals().weak_text_color())
    } else {
        ("✖", error)
    };
    let mono = |text: String| egui::RichText::new(text).monospace();
    let title = egui::RichText::new(format!(
        "{mark} {:>4}  {}   {} ms",
        ago(entry.started.elapsed()),
        entry.command_line(),
        entry.duration.as_millis()
    ))
    .monospace()
    .color(if ok { ui.visuals().text_color() } else { color });
    egui::CollapsingHeader::new(title)
        .id_salt(("git_log_entry", entry.id))
        .default_open(!ok)
        .show(ui, |ui| {
            ui.label(mono(format!("dir: {}", entry.dir.display())));
            let outcome = mono(entry.outcome());
            ui.label(if ok { outcome } else { outcome.color(error) });
            if !entry.stderr.trim().is_empty() {
                let text = mono(entry.stderr.trim_end().to_owned());
                ui.label(if ok { text } else { text.color(error) });
            }
            if !entry.stdout.trim().is_empty() {
                ui.label(mono(entry.stdout.trim_end().to_owned()).weak());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(args: &[&str], exit: Option<i32>, failure: Option<&str>) -> Entry {
        Entry {
            id: 0,
            started: Instant::now(),
            dir: PathBuf::from("/repo"),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            duration: Duration::from_millis(12),
            exit,
            failure: failure.map(str::to_owned),
            stdout: String::new(),
            stderr: "fatal: bad\n".to_owned(),
        }
    }

    #[test]
    fn test_command_line_quotes_spaced_arguments() {
        let e = entry(&["commit", "-m", "two words", ""], Some(0), None);
        assert_eq!(e.command_line(), r#"git commit -m "two words" """#);
    }

    #[test]
    fn test_ok_needs_exit_zero_and_no_failure() {
        assert!(entry(&["status"], Some(0), None).ok());
        assert!(!entry(&["status"], Some(128), None).ok());
        assert!(!entry(&["push"], None, Some("git timed out")).ok());
        assert!(!entry(&["push"], None, None).ok());
    }

    #[test]
    fn test_to_text_lists_outcome_and_output() {
        let text = entry(&["push"], Some(1), None).to_text();
        assert_eq!(text, "$ git push\n  dir: /repo\n  exit 1 in 12 ms\n  stderr:\n    fatal: bad\n");
    }

    #[test]
    fn test_clip_cuts_long_output_on_a_char_boundary() {
        let long = "é".repeat(OUTPUT_LIMIT);
        let clipped = clip(long.as_bytes());
        assert!(clipped.starts_with("éé"));
        assert!(clipped.ends_with(&format!("({} bytes more)", OUTPUT_LIMIT)));
    }

    #[test]
    fn test_ago_units() {
        assert_eq!(ago(Duration::from_secs(5)), "5s");
        assert_eq!(ago(Duration::from_secs(125)), "2m");
        assert_eq!(ago(Duration::from_secs(7300)), "2h");
    }
}
