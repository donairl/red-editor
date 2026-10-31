//! Runs a short-lived external program (a formatter, `git`) and collects its
//! output, giving up on it after a timeout.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Why a program produced no output.
#[derive(Debug)]
pub enum RunError {
    /// The program is not on the `PATH`.
    NotInstalled,
    /// It could not be started or waited on.
    Io(std::io::Error),
    /// It ran past the timeout and was killed.
    TimedOut,
}

/// Keeps a console program from opening a console window of its own: on
/// Windows the editor runs without a console for it to share.
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(windows)]
fn kill(pid: u32) {
    let mut command = Command::new("taskkill");
    command.args(["/F", "/T", "/PID", &pid.to_string()]);
    hide_console(&mut command);
    let _ = command.status();
}

#[cfg(not(windows))]
fn kill(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).status();
}

/// Runs `command`, feeding it `stdin`, and returns what it wrote once it
/// exits. The exit status is the caller's to judge.
pub fn run(mut command: Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<Output, RunError> {
    command
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => RunError::NotInstalled,
        _ => RunError::Io(e),
    })?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let input = input.to_owned();
        // Feed stdin from a thread so a program that writes before it has
        // read everything cannot deadlock on full pipes.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let (tx, rx) = mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result.map_err(RunError::Io),
        Err(_) => {
            kill(pid);
            Err(RunError::TimedOut)
        }
    }
}
