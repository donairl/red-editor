//! Integrated terminal: the user's shell on a pseudo-terminal, drawn with
//! egui.
//!
//! `portable-pty` runs the shell and `vt100` keeps the screen it draws. A
//! reader thread feeds the shell's output to the screen and asks egui to
//! repaint; while the terminal has focus, key presses go back to the shell
//! as the bytes a terminal sends for them.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui::{self, Color32, Key, Modifiers, Rect, vec2};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::gutter::GutterPalette;
use crate::theme;

/// Lines kept above the screen for scrolling back.
const SCROLLBACK: usize = 5000;
/// Space between the panel edges and the text.
const PADDING: f32 = 4.0;

/// Id of the terminal widget, so the app can tell when it has focus.
pub fn widget_id() -> egui::Id {
    egui::Id::unique("console_terminal")
}

/// Answers the queries a shell sends to learn about its terminal. `vt100`
/// parses them but leaves them unanswered, and fish, for one, waits on the
/// device attributes reply before showing its prompt.
#[derive(Default)]
struct Replies {
    out: Vec<u8>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // Primary device attributes: a VT220 with ANSI colour.
            (None, 'c', 0) => self.out.extend_from_slice(b"\x1b[?62;22c"),
            // Secondary device attributes.
            (Some(b'>'), 'c', 0) => self.out.extend_from_slice(b"\x1b[>1;10;0c"),
            // Device status: all well.
            (None, 'n', 5) => self.out.extend_from_slice(b"\x1b[0n"),
            // Cursor position, 1-based.
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.out
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            _ => {}
        }
    }
}

type Parser = Arc<Mutex<vt100::Parser<Replies>>>;
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Locks `mutex`, also after a thread panicked while holding it: the
/// screen and the pipe stay usable.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A running shell and the screen it draws.
pub struct Terminal {
    parser: Parser,
    writer: Writer,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Set by the reader thread once the shell's output ends.
    exited: Arc<AtomicBool>,
    /// Rows and columns of the screen.
    size: (u16, u16),
    /// Wheel movement not yet worth a whole line of scrollback.
    scroll_rest: f32,
}

impl Terminal {
    /// Starts the user's shell in `cwd`.
    pub fn spawn(ctx: &egui::Context, cwd: &Path) -> Result<Self, String> {
        Self::spawn_command(ctx, CommandBuilder::new_default_prog(), cwd)
    }

    fn spawn_command(
        ctx: &egui::Context,
        mut command: CommandBuilder,
        cwd: &Path,
    ) -> Result<Self, String> {
        let size = (24, 80);
        let pty = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|e| e.to_string())?;
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pty.slave.spawn_command(command).map_err(|e| e.to_string())?;
        // The shell holds its own end; ours would keep the pipe open after
        // the shell exits.
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: Writer = Arc::new(Mutex::new(
            pty.master.take_writer().map_err(|e| e.to_string())?,
        ));
        let parser: Parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            size.0,
            size.1,
            SCROLLBACK,
            Replies::default(),
        )));
        let exited = Arc::new(AtomicBool::new(false));

        let (thread_parser, thread_writer, thread_exited, ctx) =
            (parser.clone(), writer.clone(), exited.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("terminal-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                // Ends with an error (EIO on Linux) once the shell is gone.
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    let replies = {
                        let mut parser = lock(&thread_parser);
                        parser.process(&buf[..n]);
                        std::mem::take(&mut parser.callbacks_mut().out)
                    };
                    if !replies.is_empty() {
                        let mut writer = lock(&thread_writer);
                        let _ = writer.write_all(&replies).and_then(|_| writer.flush());
                    }
                    ctx.request_repaint();
                }
                thread_exited.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;

        Ok(Self {
            parser,
            writer,
            master: pty.master,
            child,
            exited,
            size,
            scroll_rest: 0.0,
        })
    }

    /// Whether the shell has ended (the user typed `exit`, say).
    pub fn has_exited(&mut self) -> bool {
        self.exited.load(Ordering::Relaxed) || matches!(self.child.try_wait(), Ok(Some(_)))
    }

    fn write(&self, bytes: &[u8]) {
        let mut writer = lock(&self.writer);
        let _ = writer.write_all(bytes).and_then(|_| writer.flush());
    }

    fn resize(&mut self, size: (u16, u16)) {
        if size != self.size {
            self.size = size;
            let _ = self.master.resize(pty_size(size));
            lock(&self.parser).screen_mut().set_size(size.0, size.1);
        }
    }

    #[cfg(test)]
    fn contents(&self) -> String {
        lock(&self.parser).screen().contents()
    }

    /// Draws the screen over the available space. While focused, the
    /// frame's key presses go to the shell.
    pub fn show(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let (cell_w, cell_h) =
            ui.fonts_mut(|f| (f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
        let rect = ui.available_rect_before_wrap();
        let response = ui.interact(rect, widget_id(), egui::Sense::click());
        ui.advance_cursor_after_rect(rect);
        if response.clicked() {
            response.request_focus();
        }
        let focused = response.has_focus();
        if focused {
            // Tab, the arrows and Escape belong to the shell, not to egui's
            // keyboard navigation.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    widget_id(),
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }

        let text_rect = rect.shrink(PADDING);
        let rows = (text_rect.height() / cell_h).floor().max(1.0) as u16;
        let cols = (text_rect.width() / cell_w).floor().max(1.0) as u16;
        self.resize((rows, cols));

        if focused {
            let (app_cursor, bracketed) = {
                let parser = lock(&self.parser);
                let screen = parser.screen();
                (screen.application_cursor(), screen.bracketed_paste())
            };
            let bytes = ui.input(|i| input_bytes(&i.events, app_cursor, bracketed));
            if !bytes.is_empty() {
                // Typing jumps back to the live screen, as in other terminals.
                lock(&self.parser).screen_mut().set_scrollback(0);
                self.write(&bytes);
            }
        }

        if response.hovered() {
            self.scroll_rest += ui.input(|i| i.smooth_scroll_delta.y);
            let lines = (self.scroll_rest / cell_h).trunc();
            if lines != 0.0 {
                self.scroll_rest -= lines * cell_h;
                let mut parser = lock(&self.parser);
                let screen = parser.screen_mut();
                let back = screen.scrollback() as isize + lines as isize;
                screen.set_scrollback(back.max(0) as usize);
            }
        }

        let palette = theme::active(ui.ctx()).terminal;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, palette.background);
        let parser = lock(&self.parser);
        paint_screen(
            &painter,
            parser.screen(),
            &palette,
            &font_id,
            text_rect.min,
            vec2(cell_w, cell_h),
            focused,
        );
        response
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Colours of the terminal. `DARK` and `LIGHT` are VS Code's integrated
/// terminal; the other themes derive theirs in `theme.rs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPalette {
    pub background: Color32,
    pub foreground: Color32,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan,
    /// white, then their bright versions.
    pub ansi: [Color32; 16],
}

impl TerminalPalette {
    const fn rgb(hex: u32) -> Color32 {
        Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    pub const DARK: TerminalPalette = TerminalPalette {
        background: GutterPalette::DARK.background,
        foreground: Self::rgb(0xCCCCCC),
        ansi: [
            Self::rgb(0x000000),
            Self::rgb(0xCD3131),
            Self::rgb(0x0DBC79),
            Self::rgb(0xE5E510),
            Self::rgb(0x2472C8),
            Self::rgb(0xBC3FBC),
            Self::rgb(0x11A8CD),
            Self::rgb(0xE5E5E5),
            Self::rgb(0x666666),
            Self::rgb(0xF14C4C),
            Self::rgb(0x23D18B),
            Self::rgb(0xF5F543),
            Self::rgb(0x3B8EEA),
            Self::rgb(0xD670D6),
            Self::rgb(0x29B8DB),
            Self::rgb(0xE5E5E5),
        ],
    };

    pub const LIGHT: TerminalPalette = TerminalPalette {
        background: GutterPalette::LIGHT.background,
        foreground: Self::rgb(0x333333),
        ansi: [
            Self::rgb(0x000000),
            Self::rgb(0xCD3131),
            Self::rgb(0x00BC00),
            Self::rgb(0x949800),
            Self::rgb(0x0451A5),
            Self::rgb(0xBC05BC),
            Self::rgb(0x0598BC),
            Self::rgb(0x555555),
            Self::rgb(0x666666),
            Self::rgb(0xCD3131),
            Self::rgb(0x14CE14),
            Self::rgb(0xB5BA00),
            Self::rgb(0x0451A5),
            Self::rgb(0xBC05BC),
            Self::rgb(0x0598BC),
            Self::rgb(0xA5A5A5),
        ],
    };

    /// `color` as egui paints it; `Default` is `fallback`.
    fn color(&self, color: vt100::Color, fallback: Color32) -> Color32 {
        match color {
            vt100::Color::Default => fallback,
            vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
            vt100::Color::Idx(i @ 0..16) => self.ansi[i as usize],
            // The 6×6×6 colour cube.
            vt100::Color::Idx(i @ 16..232) => {
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                let i = i - 16;
                Color32::from_rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            // The grey ramp.
            vt100::Color::Idx(i) => {
                let v = 8 + 10 * (i - 232);
                Color32::from_rgb(v, v, v)
            }
        }
    }

    /// Text and background colour of `cell`.
    fn cell_colors(&self, cell: &vt100::Cell) -> (Color32, Color32) {
        // Bold brightens the eight plain colours, as in most terminals.
        let fg = match cell.fgcolor() {
            vt100::Color::Idx(i @ 0..8) if cell.bold() => vt100::Color::Idx(i + 8),
            other => other,
        };
        let mut fg = self.color(fg, self.foreground);
        let mut bg = self.color(cell.bgcolor(), self.background);
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.dim() {
            fg = fg.gamma_multiply(0.6);
        }
        (fg, bg)
    }
}

/// Cells of one row sharing a look, drawn as one piece of text.
struct Run {
    col: u16,
    text: String,
    color: Color32,
    italic: bool,
    underline: bool,
    /// A double-width character, kept alone so the cells after it line up.
    wide: bool,
}

fn paint_screen(
    painter: &egui::Painter,
    screen: &vt100::Screen,
    palette: &TerminalPalette,
    font_id: &egui::FontId,
    origin: egui::Pos2,
    cell: egui::Vec2,
    focused: bool,
) {
    let (rows, cols) = screen.size();
    let cell_rect = |row: u16, col: u16, width: f32| {
        Rect::from_min_size(
            origin + vec2(col as f32 * cell.x, row as f32 * cell.y),
            vec2(cell.x * width, cell.y),
        )
    };

    for row in 0..rows {
        let mut runs: Vec<Run> = Vec::new();
        for col in 0..cols {
            let Some(c) = screen.cell(row, col) else {
                continue;
            };
            if c.is_wide_continuation() {
                continue;
            }
            let (fg, bg) = palette.cell_colors(c);
            let width = if c.is_wide() { 2.0 } else { 1.0 };
            if bg != palette.background {
                painter.rect_filled(cell_rect(row, col, width), 0.0, bg);
            }
            let text = if c.has_contents() { c.contents() } else { " " };
            let (italic, underline, wide) = (c.italic(), c.underline(), c.is_wide());
            match runs.last_mut() {
                Some(run)
                    if !run.wide
                        && !wide
                        && run.color == fg
                        && run.italic == italic
                        && run.underline == underline =>
                {
                    run.text.push_str(text);
                }
                _ => runs.push(Run {
                    col,
                    text: text.to_owned(),
                    color: fg,
                    italic,
                    underline,
                    wide,
                }),
            }
        }
        for run in runs {
            if run.text.trim().is_empty() && !run.underline {
                continue;
            }
            let format = egui::TextFormat {
                font_id: font_id.clone(),
                color: run.color,
                italics: run.italic,
                underline: if run.underline {
                    egui::Stroke::new(1.0, run.color)
                } else {
                    egui::Stroke::NONE
                },
                ..Default::default()
            };
            let mut job = egui::text::LayoutJob::single_section(run.text, format);
            job.wrap.max_width = f32::INFINITY;
            let galley = painter.layout_job(job);
            painter.galley(cell_rect(row, run.col, 1.0).min, galley, run.color);
        }
    }

    // No caret while looking at scrollback, or when the program hides it.
    if screen.scrollback() == 0 && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        let rect = cell_rect(row, col, 1.0);
        let color = palette.foreground;
        if focused {
            painter.rect_filled(rect, 0.0, color.gamma_multiply(0.6));
        } else {
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// The bytes a terminal sends for this frame's input `events`.
/// `app_cursor`: the program asked for application cursor keys (`ESC O A`
/// instead of `ESC [ A`). `bracketed`: it asked for pastes to be marked.
fn input_bytes(events: &[egui::Event], app_cursor: bool, bracketed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    // An Alt+letter is sent as Escape and the letter; the text event that
    // follows it carries the same letter and is dropped.
    let mut skip_text = false;
    for event in events {
        match event {
            egui::Event::Text(text) => {
                if !std::mem::take(&mut skip_text) {
                    out.extend_from_slice(text.as_bytes());
                }
            }
            egui::Event::Paste(text) => {
                // Terminals send a pasted line break as Enter.
                let text = text.replace("\r\n", "\r").replace('\n', "\r");
                if bracketed {
                    out.extend_from_slice(b"\x1b[200~");
                }
                out.extend_from_slice(text.as_bytes());
                if bracketed {
                    out.extend_from_slice(b"\x1b[201~");
                }
            }
            // egui turns Ctrl+C and Ctrl+X into clipboard commands; a shell
            // wants the control characters (interrupt, for Ctrl+C).
            egui::Event::Copy => out.push(0x03),
            egui::Event::Cut => out.push(0x18),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if let Some(bytes) = key_bytes(*key, *modifiers, app_cursor) {
                    skip_text = modifiers.alt && !modifiers.ctrl && letter(*key).is_some();
                    out.extend_from_slice(&bytes);
                }
            }
            _ => {}
        }
    }
    out
}

/// The letter `key` types, upper case.
fn letter(key: Key) -> Option<u8> {
    let name = key.name();
    (name.len() == 1)
        .then(|| name.as_bytes()[0])
        .filter(u8::is_ascii_uppercase)
}

/// The bytes a terminal sends for `key`, or `None` when the key only types
/// text (that arrives as a text event).
fn key_bytes(key: Key, modifiers: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // xterm's modifier code: 1 + Shift + 2·Alt + 4·Ctrl.
    let code = 1 + modifiers.shift as u8 + 2 * modifiers.alt as u8 + 4 * modifiers.ctrl as u8;
    let cursor = |c: char| {
        if code > 1 {
            format!("\x1b[1;{code}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };
    let tilde = |n: u8| {
        if code > 1 {
            format!("\x1b[{n};{code}~")
        } else {
            format!("\x1b[{n}~")
        }
    };
    let text = match key {
        Key::Enter => "\r".to_owned(),
        Key::Tab if modifiers.shift => "\x1b[Z".to_owned(),
        Key::Tab => "\t".to_owned(),
        Key::Backspace if modifiers.ctrl => "\x08".to_owned(),
        Key::Backspace if modifiers.alt => "\x1b\x7f".to_owned(),
        Key::Backspace => "\x7f".to_owned(),
        Key::Escape => "\x1b".to_owned(),
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => "\x1bOP".to_owned(),
        Key::F2 => "\x1bOQ".to_owned(),
        Key::F3 => "\x1bOR".to_owned(),
        Key::F4 => "\x1bOS".to_owned(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Space if modifiers.ctrl => "\0".to_owned(),
        _ => {
            let letter = letter(key)?;
            return if modifiers.ctrl && !modifiers.alt {
                // Ctrl+A is 1, … Ctrl+Z is 26.
                Some(vec![letter - b'A' + 1])
            } else if modifiers.alt && !modifiers.ctrl {
                let typed = if modifiers.shift { letter } else { letter.to_ascii_lowercase() };
                Some(vec![0x1b, typed])
            } else {
                None
            };
        }
    };
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn test_keys_send_terminal_sequences() {
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::Enter, none, false).unwrap(), b"\r");
        assert_eq!(key_bytes(Key::Backspace, none, false).unwrap(), b"\x7f");
        assert_eq!(key_bytes(Key::Tab, Modifiers::SHIFT, false).unwrap(), b"\x1b[Z");
        assert_eq!(key_bytes(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA", "application cursor keys");
        assert_eq!(key_bytes(Key::ArrowLeft, Modifiers::CTRL, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes(Key::Delete, none, false).unwrap(), b"\x1b[3~");
        assert_eq!(key_bytes(Key::F5, none, false).unwrap(), b"\x1b[15~");
        assert_eq!(key_bytes(Key::R, Modifiers::CTRL, false).unwrap(), [0x12]);
        assert_eq!(key_bytes(Key::B, Modifiers::ALT, false).unwrap(), b"\x1bb");
        assert_eq!(key_bytes(Key::A, none, false), None, "plain letters come as text");
    }

    #[test]
    fn test_input_events_become_bytes() {
        let events = [
            egui::Event::Text("ls".into()),
            key(Key::Enter, Modifiers::NONE),
            egui::Event::Copy,
        ];
        assert_eq!(input_bytes(&events, false, false), b"ls\r\x03");

        let paste = [egui::Event::Paste("a\nb".into())];
        assert_eq!(input_bytes(&paste, false, false), b"a\rb");
        assert_eq!(input_bytes(&paste, false, true), b"\x1b[200~a\rb\x1b[201~");

        let alt_b = [key(Key::B, Modifiers::ALT), egui::Event::Text("b".into())];
        assert_eq!(input_bytes(&alt_b, false, false), b"\x1bb", "the text of Alt+B is dropped");
    }

    #[test]
    fn test_colors_follow_xterm_256() {
        let p = TerminalPalette::DARK;
        let fallback = Color32::WHITE;
        assert_eq!(p.color(vt100::Color::Default, fallback), fallback);
        assert_eq!(p.color(vt100::Color::Idx(1), fallback), p.ansi[1]);
        assert_eq!(p.color(vt100::Color::Idx(196), fallback), Color32::from_rgb(255, 0, 0));
        assert_eq!(p.color(vt100::Color::Idx(232), fallback), Color32::from_rgb(8, 8, 8));
        assert_eq!(p.color(vt100::Color::Rgb(1, 2, 3), fallback), Color32::from_rgb(1, 2, 3));
    }

    #[test]
    fn test_queries_are_answered() {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, Replies::default());
        parser.process(b"ab\x1b[c\x1b[6n");
        assert_eq!(parser.callbacks().out, b"\x1b[?62;22c\x1b[1;3R");
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_output_reaches_the_screen() {
        let ctx = egui::Context::default();
        let mut command = CommandBuilder::new("sh");
        command.args(["-c", "printf 'hello from sh'"]);
        let mut terminal =
            Terminal::spawn_command(&ctx, command, &std::env::temp_dir()).expect("pty opens");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !terminal.contents().contains("hello from sh") {
            assert!(std::time::Instant::now() < deadline, "no output: {:?}", terminal.contents());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        while !terminal.has_exited() {
            assert!(std::time::Instant::now() < deadline, "shell did not exit");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Drawing fits the screen to the space it is given.
    #[cfg(unix)]
    #[test]
    fn test_show_sizes_the_screen_to_the_panel() {
        let ctx = egui::Context::default();
        let mut command = CommandBuilder::new("sh");
        command.args(["-c", "printf '\\033[31mred\\033[0m'; sleep 2"]);
        let mut terminal =
            Terminal::spawn_command(&ctx, command, &std::env::temp_dir()).expect("pty opens");
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 200.0))),
            ..Default::default()
        };
        let mut cell = egui::Vec2::ZERO;
        let mut output = ctx.run_ui(input, |ui| {
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            cell = ui.fonts_mut(|f| vec2(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
            terminal.show(ui);
        });
        output.textures_delta.clear();
        let (rows, cols) = terminal.size;
        assert!(rows > 1 && cols > 10, "{rows}x{cols}");
        assert!(cols as f32 * cell.x <= 400.0 && rows as f32 * cell.y <= 200.0);
        assert_eq!(lock(&terminal.parser).screen().size(), (rows, cols));
    }
}
