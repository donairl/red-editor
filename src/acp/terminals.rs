//! Terminals the agent asks Red to run: `terminal/create`, `output`,
//! `wait_for_exit`, `kill` and `release`.
//!
//! Each terminal is a plain child process (not a PTY) with stdin closed and
//! stdout and stderr merged into one pipe. A reader thread fills a bounded
//! [`OutputBuffer`]; a watcher thread owns the child, notices its end and
//! answers any `terminal/wait_for_exit` that was left open. Both run off the
//! UI thread, which only reads the buffer to draw it ([`Terminals::view`]).

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use agent_client_protocol_schema::v1::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, TerminalExitStatus, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use serde_json::Value;

use super::transport::{Responder, RpcError, kill_tree};

/// Buffer size when the agent names no `outputByteLimit`.
const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// Largest buffer an agent may ask for.
const MAX_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
/// After the process ends, how long its output pipe may stay open (held by
/// something it started) before the exit is reported anyway.
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// How much of the buffer a card shows.
const VIEW_BYTES: usize = 64 * 1024;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL: i64 = -32603;

/// The last `limit` bytes of what a process wrote, cut at a UTF-8 character
/// boundary when the front is dropped.
#[derive(Debug)]
pub struct OutputBuffer {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl OutputBuffer {
    pub fn new(limit: usize) -> Self {
        Self { bytes: Vec::new(), limit, truncated: false }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        if self.bytes.len() > self.limit {
            let mut cut = self.bytes.len() - self.limit;
            // Never start in the middle of a character.
            while cut < self.bytes.len() && self.bytes[cut] & 0xC0 == 0x80 {
                cut += 1;
            }
            self.bytes.drain(..cut);
            self.truncated = true;
        }
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The text so far. A character still arriving at the end is left out
    /// until it is complete.
    pub fn text(&self) -> String {
        Self::decode(&self.bytes)
    }

    /// Like [`Self::text`], for the last `max` bytes only.
    fn tail(&self, max: usize) -> String {
        let mut start = self.bytes.len().saturating_sub(max);
        while start < self.bytes.len() && self.bytes[start] & 0xC0 == 0x80 {
            start += 1;
        }
        Self::decode(&self.bytes[start..])
    }

    fn decode(bytes: &[u8]) -> String {
        match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
            Err(_) => String::from_utf8_lossy(bytes).into_owned(),
        }
    }
}

struct State {
    buffer: OutputBuffer,
    exit: Option<TerminalExitStatus>,
    /// JSON-RPC ids of `terminal/wait_for_exit` requests left open.
    waiters: Vec<Value>,
    /// The agent released it: the id is invalid, the output stays for the card.
    released: bool,
}

struct Terminal {
    pid: u32,
    state: Mutex<State>,
    /// The child was waited for; its pid may be reused from here on.
    reaped: AtomicBool,
    responder: Responder,
}

impl Terminal {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("terminal lock")
    }

    /// Kills the process and what it started. `group_too`: also when the
    /// child is gone, for stragglers in its process group (Unix only).
    fn kill(&self, group_too: bool) {
        let reaped = self.reaped.load(Ordering::SeqCst);
        if !reaped || (group_too && cfg!(unix)) {
            kill_tree(self.pid);
        }
    }
}

/// What a card draws for one terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalView {
    pub text: String,
    pub truncated: bool,
    /// `None` while it runs.
    pub exit: Option<TerminalExitStatus>,
}

struct Inner {
    responder: Responder,
    cwd: PathBuf,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
    next_id: AtomicU64,
}

/// The terminals of one agent connection. Cloning gives another handle to
/// the same set, which the panel uses to draw the output.
#[derive(Clone)]
pub struct Terminals {
    inner: Arc<Inner>,
}

impl Terminals {
    /// `cwd` is the default working directory (the workspace).
    pub fn new(responder: Responder, cwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                responder,
                cwd,
                terminals: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Answers `method` when it is a `terminal/*` request (now, or later for
    /// `wait_for_exit`) and returns `true`; `false` for any other method.
    pub fn handle(&self, id: Value, method: &str, params: Value) -> bool {
        let result = match method {
            "terminal/create" => parse(params).and_then(|r| self.create(r)),
            "terminal/output" => parse(params).and_then(|r| self.output(r)),
            "terminal/wait_for_exit" => match parse::<WaitForTerminalExitRequest>(params) {
                Ok(request) => match self.find(request.terminal_id.0.as_ref()) {
                    Ok(terminal) => {
                        let mut state = terminal.state();
                        match state.exit.clone() {
                            Some(exit) => Ok(to_value(WaitForTerminalExitResponse::new(exit))),
                            None => {
                                // The watcher answers it when the process ends.
                                state.waiters.push(id);
                                return true;
                            }
                        }
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            },
            "terminal/kill" => parse(params).and_then(|r| self.kill(r)),
            "terminal/release" => parse(params).and_then(|r| self.release(r)),
            _ => return false,
        };
        self.inner.responder.respond(id, result);
        true
    }

    fn create(&self, request: CreateTerminalRequest) -> Result<Value, RpcError> {
        let limit = request
            .output_byte_limit
            .map_or(DEFAULT_OUTPUT_LIMIT, |n| usize::try_from(n).unwrap_or(usize::MAX))
            .min(MAX_OUTPUT_LIMIT);
        let cwd = match &request.cwd {
            Some(dir) => self.inner.cwd.join(dir),
            None => self.inner.cwd.clone(),
        };
        let (reader, writer) = std::io::pipe().map_err(internal)?;
        let mut command = Command::new(&request.command);
        command
            .args(&request.args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer.try_clone().map_err(internal)?))
            .stderr(Stdio::from(writer));
        for variable in &request.env {
            command.env(&variable.name, &variable.value);
        }
        crate::process::hide_console(&mut command);
        // Its own process group, so a kill reaches what it started too.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|e| RpcError {
            code: INTERNAL,
            message: format!("could not start `{}`: {e}", request.command),
        })?;
        // The parent's copies of the pipe's write end must close, or the
        // reader never sees the end.
        drop(command);

        let terminal = Arc::new(Terminal {
            pid: child.id(),
            state: Mutex::new(State {
                buffer: OutputBuffer::new(limit),
                exit: None,
                waiters: Vec::new(),
                released: false,
            }),
            reaped: AtomicBool::new(false),
            responder: self.inner.responder.clone(),
        });
        let drained = Arc::new(AtomicBool::new(false));

        let sink = Arc::clone(&terminal);
        let done = Arc::clone(&drained);
        thread::Builder::new().name("acp-terminal-out".to_owned()).spawn(move || {
            let mut reader = reader;
            let mut chunk = [0u8; 8192];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.state().buffer.push(&chunk[..n]);
            }
            done.store(true, Ordering::SeqCst);
        }).map_err(internal)?;

        let watched = Arc::clone(&terminal);
        thread::Builder::new().name("acp-terminal-wait".to_owned()).spawn(move || {
            let status = child.wait();
            watched.reaped.store(true, Ordering::SeqCst);
            // Let the reader take what is left in the pipe.
            let started = Instant::now();
            while !drained.load(Ordering::SeqCst) && started.elapsed() < DRAIN_GRACE {
                thread::sleep(Duration::from_millis(5));
            }
            let exit = match status {
                Ok(status) => exit_status(status),
                Err(_) => TerminalExitStatus::new(),
            };
            let waiters = {
                let mut state = watched.state();
                state.exit = Some(exit.clone());
                std::mem::take(&mut state.waiters)
            };
            for id in waiters {
                watched.responder.respond(id, Ok(to_value(WaitForTerminalExitResponse::new(exit.clone()))));
            }
        }).map_err(internal)?;

        let id = format!("term-{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        self.map().insert(id.clone(), terminal);
        Ok(to_value(CreateTerminalResponse::new(id)))
    }

    fn output(&self, request: TerminalOutputRequest) -> Result<Value, RpcError> {
        let terminal = self.find(request.terminal_id.0.as_ref())?;
        let state = terminal.state();
        let mut response = TerminalOutputResponse::new(state.buffer.text(), state.buffer.truncated());
        response.exit_status = state.exit.clone();
        Ok(to_value(response))
    }

    fn kill(&self, request: KillTerminalRequest) -> Result<Value, RpcError> {
        self.find(request.terminal_id.0.as_ref())?.kill(false);
        Ok(to_value(KillTerminalResponse::new()))
    }

    fn release(&self, request: ReleaseTerminalRequest) -> Result<Value, RpcError> {
        let terminal = self.find(request.terminal_id.0.as_ref())?;
        terminal.state().released = true;
        terminal.kill(true);
        Ok(to_value(ReleaseTerminalResponse::new()))
    }

    /// Kills every terminal and forgets them all (session end, restart,
    /// agent exit, drop).
    pub fn kill_all(&self) {
        let all: Vec<_> = self.map().drain().map(|(_, t)| t).collect();
        for terminal in all {
            terminal.state().released = true;
            terminal.kill(true);
        }
    }

    /// What a card shows for terminal `id`; `None` for an id never created
    /// (or cleared by a new session).
    pub fn view(&self, id: &str) -> Option<TerminalView> {
        let terminal = self.map().get(id).cloned()?;
        let state = terminal.state();
        Some(TerminalView {
            text: state.buffer.tail(VIEW_BYTES),
            truncated: state.buffer.truncated(),
            exit: state.exit.clone(),
        })
    }

    /// Some terminal still runs, so its card should keep repainting.
    pub fn any_running(&self) -> bool {
        self.map().values().any(|t| t.state().exit.is_none())
    }

    fn map(&self) -> MutexGuard<'_, HashMap<String, Arc<Terminal>>> {
        self.inner.terminals.lock().expect("terminals lock")
    }

    /// A terminal the agent may still use (not released).
    fn find(&self, id: &str) -> Result<Arc<Terminal>, RpcError> {
        let terminal = self.map().get(id).cloned().filter(|t| !t.state().released);
        terminal.ok_or_else(|| RpcError { code: INVALID_PARAMS, message: format!("unknown terminal: {id}") })
    }

    #[cfg(test)]
    pub(crate) fn pid(&self, id: &str) -> Option<u32> {
        self.map().get(id).map(|t| t.pid)
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|e| RpcError { code: INVALID_PARAMS, message: format!("invalid params: {e}") })
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("ACP messages serialise")
}

fn internal(error: std::io::Error) -> RpcError {
    RpcError { code: INTERNAL, message: error.to_string() }
}

fn exit_status(status: std::process::ExitStatus) -> TerminalExitStatus {
    let mut exit = TerminalExitStatus::new().exit_code(status.code().and_then(|c| u32::try_from(c).ok()));
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        exit = exit.signal(signal_name(signal));
    }
    exit
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP".to_owned(),
        2 => "SIGINT".to_owned(),
        3 => "SIGQUIT".to_owned(),
        6 => "SIGABRT".to_owned(),
        9 => "SIGKILL".to_owned(),
        11 => "SIGSEGV".to_owned(),
        13 => "SIGPIPE".to_owned(),
        14 => "SIGALRM".to_owned(),
        15 => "SIGTERM".to_owned(),
        other => format!("SIG{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc::Receiver;

    #[test]
    fn test_buffer_keeps_everything_under_the_limit() {
        let mut buffer = OutputBuffer::new(10);
        buffer.push(b"abc");
        buffer.push(b"def");
        assert_eq!(buffer.text(), "abcdef");
        assert!(!buffer.truncated());
    }

    #[test]
    fn test_buffer_drops_from_the_front() {
        let mut buffer = OutputBuffer::new(4);
        buffer.push(b"abcdef");
        assert_eq!(buffer.text(), "cdef");
        assert!(buffer.truncated());
    }

    #[test]
    fn test_buffer_truncates_at_a_utf8_boundary() {
        // "héllo": 'é' is two bytes. A limit of 4 would start inside it.
        let mut buffer = OutputBuffer::new(4);
        buffer.push("héllo".as_bytes());
        assert_eq!(buffer.text(), "llo");
        assert!(buffer.truncated());
        // A three-byte character split across pushes, and trimmed.
        let mut buffer = OutputBuffer::new(5);
        buffer.push("a€".as_bytes());
        buffer.push("b€".as_bytes());
        assert_eq!(buffer.text(), "b€");
    }

    #[test]
    fn test_buffer_waits_for_a_character_split_across_reads() {
        let mut buffer = OutputBuffer::new(100);
        let bytes = "x€".as_bytes();
        buffer.push(&bytes[..2]);
        assert_eq!(buffer.text(), "x");
        buffer.push(&bytes[2..]);
        assert_eq!(buffer.text(), "x€");
    }

    #[test]
    fn test_buffer_with_zero_limit_is_empty() {
        let mut buffer = OutputBuffer::new(0);
        buffer.push("héllo".as_bytes());
        assert_eq!(buffer.text(), "");
        assert!(buffer.truncated());
    }

    fn script(text: &str) -> (String, Vec<String>) {
        if cfg!(windows) {
            ("cmd".to_owned(), vec!["/C".to_owned(), text.to_owned()])
        } else {
            ("sh".to_owned(), vec!["-c".to_owned(), text.to_owned()])
        }
    }

    fn long_running() -> (String, Vec<String>) {
        script(if cfg!(windows) { "ping -n 30 127.0.0.1" } else { "sleep 30" })
    }

    /// Terminals plus the lines they answer with.
    struct Rig {
        terminals: Terminals,
        answers: Receiver<String>,
        next: u32,
    }

    impl Rig {
        fn new() -> Self {
            let (responder, answers) = Responder::channel();
            Self { terminals: Terminals::new(responder, std::env::temp_dir()), answers, next: 0 }
        }

        /// Sends a request; returns its id.
        fn send(&mut self, method: &str, params: Value) -> Value {
            self.next += 1;
            let id = json!(self.next);
            assert!(self.terminals.handle(id.clone(), method, params));
            id
        }

        /// The next answer: `(id, result, error)`.
        fn answer(&self) -> (Value, Value, Value) {
            let line = self.answers.recv_timeout(Duration::from_secs(10)).expect("an answer");
            let message: Value = serde_json::from_str(&line).unwrap();
            (message["id"].clone(), message["result"].clone(), message["error"].clone())
        }

        fn call(&mut self, method: &str, params: Value) -> (Value, Value) {
            let id = self.send(method, params);
            let (got, result, error) = self.answer();
            assert_eq!(got, id);
            (result, error)
        }

        fn create(&mut self, (command, args): (String, Vec<String>), extra: Value) -> String {
            let mut params = json!({"sessionId": "s1", "command": command, "args": args});
            params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let (result, error) = self.call("terminal/create", params);
            assert_eq!(error, Value::Null, "create failed");
            result["terminalId"].as_str().unwrap().to_owned()
        }

        fn wait(&mut self, id: &str) -> Value {
            let (result, error) = self.call("terminal/wait_for_exit", json!({"sessionId": "s1", "terminalId": id}));
            assert_eq!(error, Value::Null);
            result
        }

        fn output(&mut self, id: &str) -> Value {
            let (result, error) = self.call("terminal/output", json!({"sessionId": "s1", "terminalId": id}));
            assert_eq!(error, Value::Null);
            result
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            self.terminals.kill_all();
        }
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(unix)]
    fn assert_dies(pid: u32) {
        let started = Instant::now();
        while alive(pid) {
            assert!(started.elapsed() < Duration::from_secs(5), "process {pid} still runs");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn test_create_output_and_wait_round_trip() {
        let mut rig = Rig::new();
        let id = rig.create(script("echo hello; echo oops 1>&2"), json!({}));
        let exit = rig.wait(&id);
        assert_eq!(exit["exitCode"], json!(0));
        let output = rig.output(&id);
        let text = output["output"].as_str().unwrap();
        assert!(text.contains("hello") && text.contains("oops"), "{text:?}");
        assert_eq!(output["truncated"], json!(false));
        assert_eq!(output["exitStatus"]["exitCode"], json!(0));
    }

    #[test]
    fn test_exit_code_is_reported() {
        let mut rig = Rig::new();
        let id = rig.create(script("exit 3"), json!({}));
        assert_eq!(rig.wait(&id)["exitCode"], json!(3));
    }

    #[test]
    fn test_wait_for_exit_is_answered_later_when_the_process_ends() {
        let mut rig = Rig::new();
        let id = rig.create(script(if cfg!(windows) { "ping -n 2 127.0.0.1 >nul" } else { "sleep 0.3" }), json!({}));
        let waited = rig.send("terminal/wait_for_exit", json!({"sessionId": "s1", "terminalId": id}));
        // Another request is answered while the wait stays open.
        let output = rig.send("terminal/output", json!({"sessionId": "s1", "terminalId": id}));
        let (first, ..) = rig.answer();
        assert_eq!(first, output);
        let (second, result, _) = rig.answer();
        assert_eq!(second, waited);
        assert_eq!(result["exitCode"], json!(0));
    }

    #[test]
    fn test_output_limit_truncates_and_reports_it() {
        let mut rig = Rig::new();
        let id = rig.create(script("echo 0123456789abcdef"), json!({"outputByteLimit": 5}));
        rig.wait(&id);
        let output = rig.output(&id);
        assert_eq!(output["truncated"], json!(true));
        assert!(output["output"].as_str().unwrap().len() <= 5);
    }

    #[test]
    fn test_env_and_cwd_are_used() {
        let mut rig = Rig::new();
        let dir = std::env::temp_dir().join("red-terminal-cwd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let (command, args) = script(if cfg!(windows) { "echo %RED_T%&cd" } else { "echo $RED_T; pwd" });
        let id = rig.create(
            (command, args),
            json!({"env": [{"name": "RED_T", "value": "marker"}], "cwd": dir}),
        );
        rig.wait(&id);
        let text = rig.output(&id)["output"].as_str().unwrap().to_owned();
        assert!(text.contains("marker"), "{text:?}");
        assert!(text.contains("red-terminal-cwd-test"), "{text:?}");
    }

    #[test]
    fn test_a_program_that_is_missing_is_an_error() {
        let mut rig = Rig::new();
        let (result, error) = rig.call(
            "terminal/create",
            json!({"sessionId": "s1", "command": "red-no-such-program-xyz"}),
        );
        assert_eq!(result, Value::Null);
        assert_eq!(error["code"], json!(INTERNAL));
    }

    #[test]
    fn test_unknown_terminal_id_is_an_error_for_every_method() {
        let mut rig = Rig::new();
        for method in ["output", "wait_for_exit", "kill", "release"] {
            let (result, error) =
                rig.call(&format!("terminal/{method}"), json!({"sessionId": "s1", "terminalId": "nope"}));
            assert_eq!(result, Value::Null, "{method}");
            assert_eq!(error["code"], json!(INVALID_PARAMS), "{method}");
            assert!(error["message"].as_str().unwrap().contains("nope"));
        }
    }

    #[test]
    fn test_bad_params_and_other_methods() {
        let mut rig = Rig::new();
        let (_, error) = rig.call("terminal/create", json!({"sessionId": "s1"}));
        assert_eq!(error["code"], json!(INVALID_PARAMS));
        assert!(!rig.terminals.handle(json!(9), "fs/read_text_file", json!({})));
    }

    #[test]
    fn test_kill_stops_a_long_running_process() {
        let mut rig = Rig::new();
        let id = rig.create(long_running(), json!({}));
        let started = Instant::now();
        let waited = rig.send("terminal/wait_for_exit", json!({"sessionId": "s1", "terminalId": id}));
        let (_, error) = rig.call("terminal/kill", json!({"sessionId": "s1", "terminalId": id}));
        assert_eq!(error, Value::Null);
        let (got, result, _) = rig.answer();
        assert_eq!(got, waited);
        assert!(started.elapsed() < Duration::from_secs(10));
        #[cfg(unix)]
        {
            assert_eq!(result["signal"], json!("SIGKILL"));
            assert_eq!(result["exitCode"], Value::Null);
        }
        // Killed, not released: the id still works.
        assert!(rig.output(&id)["exitStatus"].is_object());
    }

    #[cfg(unix)]
    #[test]
    fn test_kill_reaches_what_the_command_started() {
        let mut rig = Rig::new();
        let id = rig.create(script("sleep 30 & echo $!; wait"), json!({}));
        let started = Instant::now();
        let child = loop {
            let text = rig.terminals.view(&id).unwrap().text;
            if let Ok(pid) = text.trim().parse::<u32>() {
                break pid;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "no pid printed");
            thread::sleep(Duration::from_millis(20));
        };
        rig.call("terminal/kill", json!({"sessionId": "s1", "terminalId": id}));
        assert_dies(child);
    }

    #[test]
    fn test_release_kills_and_invalidates_but_the_card_keeps_the_output() {
        let mut rig = Rig::new();
        let id = rig.create(script(if cfg!(windows) { "echo kept&ping -n 30 127.0.0.1" } else { "echo kept; sleep 30" }), json!({}));
        let started = Instant::now();
        while !rig.terminals.view(&id).unwrap().text.contains("kept") {
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(20));
        }
        let (_, error) = rig.call("terminal/release", json!({"sessionId": "s1", "terminalId": id}));
        assert_eq!(error, Value::Null);
        let (_, error) = rig.call("terminal/output", json!({"sessionId": "s1", "terminalId": id}));
        assert_eq!(error["code"], json!(INVALID_PARAMS));
        let view = rig.terminals.view(&id).unwrap();
        assert!(view.text.contains("kept"));
        let started = Instant::now();
        while rig.terminals.any_running() {
            assert!(started.elapsed() < Duration::from_secs(5), "still running after release");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_kill_all_leaves_no_process_behind() {
        let mut rig = Rig::new();
        let id = rig.create(long_running(), json!({}));
        let pid = rig.terminals.pid(&id).unwrap();
        assert!(alive(pid));
        rig.terminals.kill_all();
        assert_dies(pid);
        assert!(rig.terminals.view(&id).is_none());
    }

    #[test]
    fn test_view_shows_output_and_exit() {
        let mut rig = Rig::new();
        let id = rig.create(script("echo hi"), json!({}));
        rig.wait(&id);
        let view = rig.terminals.view(&id).unwrap();
        assert!(view.text.contains("hi"));
        assert_eq!(view.exit.unwrap().exit_code, Some(0));
        assert!(rig.terminals.view("other").is_none());
        assert!(!rig.terminals.any_running());
    }
}
