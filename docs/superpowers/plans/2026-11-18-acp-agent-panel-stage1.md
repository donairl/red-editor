# Agent Panel (ACP) — Stage 1: Transport and Core Chat — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A right-hand Agent panel that starts the ACP agent named in Settings, opens a session in the workspace folder, sends prompts, streams the agent's text and thoughts, and supports Stop, New Session, sign-in, and Restart after a crash.

**Architecture:** `src/acp/transport.rs` speaks newline-delimited JSON-RPC 2.0 over any `Read`/`Write` pair, with a reader thread and a writer thread feeding `mpsc` channels; `AgentProcess` wires it to a child process. `src/acp/client.rs` is a non-blocking state machine polled once per frame by the UI. `src/acp/model.rs` is the transcript as plain data. `src/agent_panel.rs` draws it. Protocol types come from `agent-client-protocol-schema` (serde only, no async runtime).

**Tech Stack:** Rust 2024, eframe/egui 0.37, `agent-client-protocol-schema` 1.18 (`default-features = false`), `serde_json`, `egui_commonmark`, `std::io::pipe` (tests).

**Spec:** `docs/superpowers/specs/2026-11-18-acp-agent-panel-design.md` (stage 1 of 6). Stages 2–6 get their own plans once this one has landed.

**Deviations from the spec, decided while planning:**
- The crate is a binary with no `lib.rs`, so `tests/` cannot reach its modules. The fake agent is a `#[cfg(test)]` module, `src/acp/fake_agent.rs`, that runs in a thread on `std::io::pipe()` pairs, instead of `examples/fake_acp_agent.rs`.
- `i18n.rs` falls back to Indonesian and a test demands that every locale has the same keys, so every stage adds its strings to all five `assets/lang/*.json` files at once (not English-only until stage 6).

---

## Before you start

The working tree has **uncommitted user work** (several Console terminals) in `src/app.rs`, `src/shortcuts.rs`, `src/menus.rs`, `src/palette.rs`, `src/console_panel.rs`, `src/terminal.rs`, `src/about.rs` and `assets/lang/*.json`. Tasks 7–9 edit several of the same files.

- [ ] **Step 0: Ask the user to commit or stash that work before Task 7.** Do not commit, stash or discard it yourself. Tasks 1–6 touch only new files plus `Cargo.toml`, `src/main.rs` and `src/process.rs`, and may run first.

Commands used throughout:
- Build: `cargo build`
- One test module: `cargo test acp::transport` (the filter matches test paths)
- All tests: `cargo test`
- Lints: `cargo clippy --all-targets -- -D warnings`

## File map

| File | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | add `agent-client-protocol-schema` |
| `src/main.rs` | modify | `mod acp; mod agent_panel; mod agent_settings;` |
| `src/process.rs` | modify | `hide_console` becomes `pub(crate)` |
| `src/agent_settings.rs` | create | `AgentSettings { command }`, storage, command splitting |
| `src/acp/mod.rs` | create | module declarations |
| `src/acp/transport.rs` | create | JSON-RPC framing, `Connection`, `AgentProcess` |
| `src/acp/model.rs` | create | `Transcript`, `Entry`, `Notice` |
| `src/acp/client.rs` | create | `Client` state machine, `Phase` |
| `src/acp/fake_agent.rs` | create (test only) | scripted ACP agent on pipes |
| `src/agent_panel.rs` | create | `AgentPanel` state, the right panel UI, start/restart |
| `src/app.rs` | modify | fields, storage keys, poll, panel placement, activity item, actions |
| `src/activity_bar.rs` | modify | fifth item "Agent" with its icon |
| `src/shortcuts.rs` | modify | `TOGGLE_AGENT` (`Ctrl+Shift+A`), `ToggleAgent`, `NewAgentSession` |
| `src/menus.rs` | modify | View → Agent Panel |
| `src/palette.rs` | modify | two palette commands |
| `src/settings_window.rs` | modify | Agent section with the command field |
| `assets/lang/{en,id,de,es,zh}.json` | modify | new strings |

---

### Task 1: Dependency and module skeleton

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Create: `src/acp/mod.rs`

- [ ] **Step 1: Add the schema crate**

In `Cargo.toml`, under `[dependencies]`, after `trash = "5"`:

```toml
# Agent Client Protocol wire types (serde only; no async runtime).
agent-client-protocol-schema = { version = "1.18", default-features = false }
```

- [ ] **Step 2: Create `src/acp/mod.rs`**

```rust
//! The Agent Client Protocol (<https://agentclientprotocol.com>): Red is the
//! client, a coding agent is a subprocess that speaks JSON-RPC 2.0 over its
//! stdin and stdout, one message per line.

pub mod client;
#[cfg(test)]
mod fake_agent;
pub mod model;
pub mod transport;
```

- [ ] **Step 3: Declare the modules in `src/main.rs`**

Add, keeping the list alphabetical (`acp` goes first, `agent_panel` and `agent_settings` after `activity_bar`):

```rust
mod acp;
mod activity_bar;
mod agent_panel;
mod agent_settings;
```

Do not build yet: the declared files do not exist. Task 2 creates `agent_settings.rs`; build after Task 5, when all `acp` files exist. Create empty placeholder files now so later tasks can build each module as they go:

```bash
mkdir -p src/acp
for f in src/acp/client.rs src/acp/fake_agent.rs src/acp/model.rs src/acp/transport.rs src/agent_panel.rs src/agent_settings.rs; do printf '' > "$f"; done
cargo build
```

Expected: builds (warnings about empty modules are fine).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/acp src/agent_panel.rs src/agent_settings.rs
git commit -m "chore(agent): add the ACP schema crate and module skeleton"
```

---

### Task 2: Agent settings and command splitting

**Files:**
- Modify: `src/agent_settings.rs`

- [ ] **Step 1: Write the failing tests**

Put this in `src/agent_settings.rs`:

```rust
//! The agent setting: the command that starts an ACP agent, typed once in
//! Settings and kept in eframe's storage under `agent_settings`.

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Result<Vec<String>, CommandError> {
        split_command(text)
    }

    #[test]
    fn test_split_on_whitespace() {
        assert_eq!(
            split("npx  @zed-industries/claude-code-acp").unwrap(),
            vec!["npx", "@zed-industries/claude-code-acp"]
        );
    }

    #[test]
    fn test_quotes_group_words() {
        assert_eq!(
            split(r#"agent --name "my agent" 'a b'"#).unwrap(),
            vec!["agent", "--name", "my agent", "a b"]
        );
    }

    #[test]
    fn test_backslash_escapes_only_space_quotes_and_backslash() {
        assert_eq!(split(r"a\ b").unwrap(), vec!["a b"]);
        assert_eq!(split(r#""say \"hi\"""#).unwrap(), vec![r#"say "hi""#]);
        // A Windows path keeps its backslashes.
        assert_eq!(split(r"C:\tools\agent.exe").unwrap(), vec![r"C:\tools\agent.exe"]);
    }

    #[test]
    fn test_empty_and_unclosed() {
        assert_eq!(split("   "), Err(CommandError::Empty));
        assert_eq!(split(r#"agent "oops"#), Err(CommandError::UnclosedQuote));
    }

    #[test]
    fn test_storage_round_trip_and_forgiveness() {
        let settings = AgentSettings { command: "gemini --experimental-acp".to_owned() };
        assert_eq!(AgentSettings::from_storage(Some(&settings.to_storage())), settings);
        assert_eq!(AgentSettings::from_storage(None), AgentSettings::default());
        assert_eq!(AgentSettings::from_storage(Some("not json")), AgentSettings::default());
        assert_eq!(AgentSettings::from_storage(Some(r#"{"command":7}"#)), AgentSettings::default());
    }

    #[test]
    fn test_program_and_args() {
        let settings = AgentSettings { command: "gemini --experimental-acp".to_owned() };
        assert_eq!(
            settings.program_and_args().unwrap(),
            ("gemini".to_owned(), vec!["--experimental-acp".to_owned()])
        );
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test agent_settings`
Expected: compile errors — `split_command`, `CommandError`, `AgentSettings` not found.

- [ ] **Step 3: Implement**

Insert above the `#[cfg(test)]` module:

```rust
use serde_json::{Value, json};

/// What the user typed in Settings → Agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSettings {
    /// Program and arguments, as typed: `npx @zed-industries/claude-code-acp`.
    pub command: String,
}

/// Why the command cannot be run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandError {
    /// Nothing but whitespace.
    Empty,
    /// A `'` or `"` was never closed.
    UnclosedQuote,
}

impl AgentSettings {
    pub fn to_storage(&self) -> String {
        json!({ "command": self.command }).to_string()
    }

    /// Forgiving, as `session.rs` is: anything unreadable gives the default.
    pub fn from_storage(text: Option<&str>) -> Self {
        let command = text
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|value| value.get("command")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        Self { command }
    }

    /// The program to start and its arguments.
    pub fn program_and_args(&self) -> Result<(String, Vec<String>), CommandError> {
        let mut words = split_command(&self.command)?.into_iter();
        let program = words.next().ok_or(CommandError::Empty)?;
        Ok((program, words.collect()))
    }
}

/// Splits a command line into words without a shell: whitespace separates,
/// `'…'` is literal, `"…"` allows `\"` and `\\`, and outside quotes a
/// backslash escapes only whitespace, a quote or a backslash, so a Windows
/// path such as `C:\tools\agent.exe` keeps its backslashes.
pub fn split_command(text: &str) -> Result<Vec<String>, CommandError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(CommandError::UnclosedQuote),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                            word.push(chars.next().expect("peeked"));
                        }
                        Some(c) => word.push(c),
                        None => return Err(CommandError::UnclosedQuote),
                    }
                }
            }
            '\\' if chars.peek().is_some_and(|n| n.is_whitespace() || matches!(n, '"' | '\'' | '\\')) => {
                in_word = true;
                word.push(chars.next().expect("peeked"));
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err(CommandError::Empty);
    }
    Ok(words)
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test agent_settings`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/agent_settings.rs
git commit -m "feat(agent): agent command setting with shell-free splitting"
```

---

### Task 3: JSON-RPC transport

**Files:**
- Modify: `src/process.rs:22-30` (`hide_console` → `pub(crate)`)
- Modify: `src/acp/transport.rs`

- [ ] **Step 1: Make `hide_console` reachable**

In `src/process.rs`, change both definitions:

```rust
#[cfg(windows)]
pub(crate) fn hide_console(command: &mut Command) {
```

```rust
#[cfg(not(windows))]
pub(crate) fn hide_console(_command: &mut Command) {}
```

- [ ] **Step 2: Write the failing tests**

Put this in `src/acp/transport.rs`:

```rust
//! JSON-RPC 2.0 over a pair of byte streams, one message per line, as ACP's
//! stdio transport wants it. A reader thread parses lines into [`Incoming`]
//! messages and wakes the UI; a writer thread sends what the UI queues. The
//! UI never blocks on the agent.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn test_parse_request_notification_and_responses() {
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":"a","method":"fs/read_text_file","params":{"path":"/x"}}"#),
            Some(Incoming::Request {
                id: json!("a"),
                method: "fs/read_text_file".to_owned(),
                params: json!({"path": "/x"}),
            })
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#),
            Some(Incoming::Notification { method: "session/update".to_owned(), params: json!({}) })
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#),
            Some(Incoming::Response { id: 3, result: Ok(json!({"ok": true})) })
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"Authentication required"}}"#),
            Some(Incoming::Response {
                id: 4,
                result: Err(RpcError { code: -32000, message: "Authentication required".to_owned() }),
            })
        );
    }

    #[test]
    fn test_parse_rejects_garbage() {
        assert_eq!(parse_line("not json"), None);
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0","id":"x","result":1}"#), None); // not our id
        assert_eq!(parse_line(r#"[1,2]"#), None);
    }

    /// A connection whose far side the test plays by hand.
    fn connected() -> (Connection, std::io::PipeWriter, BufReader<std::io::PipeReader>) {
        let (client_reads, agent_writes) = std::io::pipe().unwrap();
        let (agent_reads, client_writes) = std::io::pipe().unwrap();
        let conn = Connection::new(client_reads, client_writes, Arc::new(|| {}));
        (conn, agent_writes, BufReader::new(agent_reads))
    }

    fn sent(agent_reads: &mut BufReader<std::io::PipeReader>) -> Value {
        let mut line = String::new();
        agent_reads.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn test_requests_get_increasing_ids_and_one_line_each() {
        let (mut conn, _agent_writes, mut agent_reads) = connected();
        assert_eq!(conn.request("initialize", json!({"protocolVersion": 1})), 1);
        assert_eq!(conn.request("session/new", json!({})), 2);
        conn.notify("session/cancel", json!({"sessionId": "s"}));
        conn.respond(json!("p1"), Err(RpcError::method_not_found("x/y")));
        assert_eq!(
            sent(&mut agent_reads),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}})
        );
        assert_eq!(sent(&mut agent_reads)["id"], json!(2));
        assert_eq!(
            sent(&mut agent_reads),
            json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}})
        );
        assert_eq!(
            sent(&mut agent_reads),
            json!({"jsonrpc":"2.0","id":"p1","error":{"code":-32601,"message":"method not found: x/y"}})
        );
    }

    #[test]
    fn test_reads_partial_lines_split_characters_and_batches() {
        let (conn, mut agent_writes, _agent_reads) = connected();
        // "é" is 0xC3 0xA9; split it across two writes, and put two
        // messages in the second write.
        agent_writes.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"a\",\"params\":{\"t\":\"\xC3").unwrap();
        agent_writes.flush().unwrap();
        agent_writes
            .write_all(b"\xA9\"}}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"b\",\"params\":null}\n")
            .unwrap();
        let wait = Duration::from_secs(2);
        assert_eq!(
            conn.recv_timeout(wait),
            Some(Incoming::Notification { method: "a".to_owned(), params: json!({"t": "é"}) })
        );
        assert_eq!(
            conn.recv_timeout(wait),
            Some(Incoming::Notification { method: "b".to_owned(), params: Value::Null })
        );
        drop(agent_writes);
        assert_eq!(conn.recv_timeout(wait), Some(Incoming::Closed));
        // After Closed the channel is quiet, so a drain loop ends.
        assert_eq!(conn.try_recv(), None);
    }

    #[test]
    fn test_invalid_lines_are_skipped() {
        let (conn, mut agent_writes, _agent_reads) = connected();
        writeln!(agent_writes, "garbage").unwrap();
        writeln!(agent_writes, r#"{{"jsonrpc":"2.0","method":"ok","params":{{}}}}"#).unwrap();
        assert_eq!(
            conn.recv_timeout(Duration::from_secs(2)),
            Some(Incoming::Notification { method: "ok".to_owned(), params: json!({}) })
        );
    }
}
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test acp::transport`
Expected: compile errors — `parse_line`, `Incoming`, `Connection`, `RpcError` not found.

- [ ] **Step 4: Implement**

Insert above the test module:

```rust
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Called from the reader thread after each message; the app passes one
/// that asks egui for a repaint.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    /// ACP's "authentication required".
    pub const AUTH_REQUIRED: i64 = -32000;

    pub fn method_not_found(method: &str) -> Self {
        Self { code: -32601, message: format!("method not found: {method}") }
    }

    fn from_value(value: &Value) -> Self {
        Self {
            code: value.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned(),
        }
    }

    fn to_value(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

/// One message from the agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// The answer to a request Red sent; Red's ids are numbers.
    Response { id: u64, result: Result<Value, RpcError> },
    /// The agent asks Red for something; answer with [`Connection::respond`].
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    /// The agent's stdout ended. Sent once, last.
    Closed,
}

/// Reads one line of JSON-RPC; `None` for anything else.
pub fn parse_line(line: &str) -> Option<Incoming> {
    let message: Value = serde_json::from_str(line.trim()).ok()?;
    let object = message.as_object()?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        return Some(match object.get("id") {
            Some(id) if !id.is_null() => {
                Incoming::Request { id: id.clone(), method: method.to_owned(), params }
            }
            _ => Incoming::Notification { method: method.to_owned(), params },
        });
    }
    let id = object.get("id")?.as_u64()?;
    let result = match object.get("error") {
        Some(error) => Err(RpcError::from_value(error)),
        None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
    };
    Some(Incoming::Response { id, result })
}

/// Both directions of one JSON-RPC conversation.
pub struct Connection {
    outgoing: Sender<String>,
    incoming: Receiver<Incoming>,
    next_id: u64,
}

impl Connection {
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        waker: Waker,
    ) -> Self {
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (in_tx, in_rx) = mpsc::channel();
        thread::Builder::new()
            .name("acp-writer".to_owned())
            .spawn(move || write_loop(writer, out_rx))
            .expect("spawn the ACP writer thread");
        thread::Builder::new()
            .name("acp-reader".to_owned())
            .spawn(move || read_loop(reader, in_tx, waker))
            .expect("spawn the ACP reader thread");
        Self { outgoing: out_tx, incoming: in_rx, next_id: 1 }
    }

    /// Sends a request and returns its id, which the response will carry.
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Answers a request from the agent.
    pub fn respond(&self, id: Value, result: Result<Value, RpcError>) {
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }),
        };
        self.send(message);
    }

    /// The next message, if one has arrived. `None` once the reader has
    /// ended and its `Closed` was taken, so a drain loop terminates.
    pub fn try_recv(&self) -> Option<Incoming> {
        match self.incoming.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    #[cfg(test)]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        self.incoming.recv_timeout(timeout).ok()
    }

    fn send(&self, message: Value) {
        // A closed writer means the agent is gone; the reader reports that.
        let _ = self.outgoing.send(message.to_string());
    }
}

fn read_loop(reader: impl Read, tx: Sender<Incoming>, waker: Waker) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                if text.trim().is_empty() {
                    continue;
                }
                match parse_line(&text) {
                    Some(message) => {
                        if tx.send(message).is_err() {
                            return;
                        }
                        waker();
                    }
                    None => eprintln!("acp: skipped a line that is not JSON-RPC: {}", text.trim_end()),
                }
            }
        }
    }
    let _ = tx.send(Incoming::Closed);
    waker();
}

fn write_loop(mut writer: impl Write, rx: Receiver<String>) {
    for line in rx {
        let written = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        if written.is_err() {
            break;
        }
    }
}

/// Lines of the agent's stderr kept for the log and the exit message.
const STDERR_LINES: usize = 200;

/// The agent's child process. Dropping it kills the agent.
pub struct AgentProcess {
    child: Child,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl AgentProcess {
    /// Starts `program` in `cwd` with piped stdio and returns the connection
    /// to it. A missing program gives `ErrorKind::NotFound`.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        waker: Waker,
    ) -> io::Result<(Connection, AgentProcess)> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::hide_console(&mut command);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LINES)));
        let sink = Arc::clone(&stderr);
        thread::Builder::new()
            .name("acp-stderr".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(stderr_pipe);
                let mut line = Vec::new();
                while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                    line.clear();
                    let mut lines = sink.lock().expect("stderr lock");
                    if lines.len() == STDERR_LINES {
                        lines.pop_front();
                    }
                    lines.push_back(text);
                }
            })?;
        Ok((Connection::new(stdout, stdin, waker), AgentProcess { child, stderr }))
    }

    /// The last `n` lines the agent wrote to stderr, oldest first.
    pub fn stderr_tail(&self, n: usize) -> Vec<String> {
        let lines = self.stderr.lock().expect("stderr lock");
        lines.iter().skip(lines.len().saturating_sub(n)).cloned().collect()
    }

    /// The exit code, waiting at most `wait` for the process to end.
    pub fn exit_code_within(&mut self, wait: Duration) -> Option<i32> {
        let until = Instant::now() + wait;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.code();
            }
            if Instant::now() >= until {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test acp::transport`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add src/process.rs src/acp/transport.rs
git commit -m "feat(agent): newline-delimited JSON-RPC transport for ACP"
```

---

### Task 4: Transcript model

**Files:**
- Modify: `src/acp/model.rs`

- [ ] **Step 1: Write the failing tests**

```rust
//! The conversation as plain data: what the panel draws. Stage 1 keeps
//! text, thoughts, notices and turn ends; tool calls and the plan come in
//! stage 2.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(value: serde_json::Value) -> SessionUpdate {
        serde_json::from_value(value).unwrap()
    }

    fn agent_chunk(text: &str) -> SessionUpdate {
        update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}))
    }

    #[test]
    fn test_consecutive_chunks_merge() {
        let mut t = Transcript::default();
        t.push_user("hi");
        assert!(t.apply(&agent_chunk("Hel")));
        assert!(t.apply(&agent_chunk("lo")));
        assert_eq!(t.entries, vec![Entry::User("hi".into()), Entry::AgentText("Hello".into())]);
    }

    #[test]
    fn test_thought_between_texts_splits_them() {
        let mut t = Transcript::default();
        t.apply(&agent_chunk("a"));
        t.apply(&update(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}})));
        t.apply(&agent_chunk("b"));
        assert_eq!(
            t.entries,
            vec![Entry::AgentText("a".into()), Entry::Thought("hmm".into()), Entry::AgentText("b".into())]
        );
    }

    #[test]
    fn test_resource_link_chunk_becomes_markdown_link() {
        let mut t = Transcript::default();
        t.apply(&update(json!({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "resource_link", "name": "main.rs", "uri": "file:///w/main.rs"}})));
        assert_eq!(t.entries, vec![Entry::AgentText("[main.rs](file:///w/main.rs)".into())]);
    }

    #[test]
    fn test_other_updates_are_ignored() {
        let mut t = Transcript::default();
        let plan = update(json!({"sessionUpdate": "plan", "entries": []}));
        assert!(!t.apply(&plan));
        assert!(t.entries.is_empty());
    }

    #[test]
    fn test_turn_end_only_for_unusual_reasons() {
        let mut t = Transcript::default();
        t.end_turn(&serde_json::from_value(json!("end_turn")).unwrap());
        assert!(t.entries.is_empty());
        t.end_turn(&serde_json::from_value(json!("max_tokens")).unwrap());
        assert_eq!(t.entries, vec![Entry::TurnEnd("max_tokens".into())]);
    }

    #[test]
    fn test_notice() {
        let mut t = Transcript::default();
        t.notice(Notice::PermissionDeclined);
        assert_eq!(t.entries, vec![Entry::Notice(Notice::PermissionDeclined)]);
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test acp::model`
Expected: compile errors — `Transcript`, `Entry`, `Notice`, `SessionUpdate` not found.

- [ ] **Step 3: Implement**

Insert above the test module:

```rust
use agent_client_protocol_schema::v1::{ContentBlock, SessionUpdate, StopReason};

/// Something Red itself tells the user inside the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The agent asked for a permission Red cannot grant yet (stage 1), so
    /// the request was answered `cancelled`.
    PermissionDeclined,
    /// A prompt turn failed with this message.
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(String),
    /// Markdown from the agent.
    AgentText(String),
    Thought(String),
    Notice(Notice),
    /// A turn stopped for a reason other than `end_turn`; the wire name.
    TurnEnd(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    User,
    Agent,
    Thought,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub entries: Vec<Entry>,
}

impl Transcript {
    pub fn push_user(&mut self, text: &str) {
        self.entries.push(Entry::User(text.to_owned()));
    }

    pub fn notice(&mut self, notice: Notice) {
        self.entries.push(Entry::Notice(notice));
    }

    /// Applies a `session/update`; `false` when the update is not one the
    /// transcript keeps.
    pub fn apply(&mut self, update: &SessionUpdate) -> bool {
        match update {
            SessionUpdate::UserMessageChunk(chunk) => self.append(Kind::User, &chunk.content),
            SessionUpdate::AgentMessageChunk(chunk) => self.append(Kind::Agent, &chunk.content),
            SessionUpdate::AgentThoughtChunk(chunk) => self.append(Kind::Thought, &chunk.content),
            _ => return false,
        }
        true
    }

    /// Records why a turn stopped, unless it simply ended.
    pub fn end_turn(&mut self, reason: &StopReason) {
        if matches!(reason, StopReason::EndTurn) {
            return;
        }
        let name = serde_json::to_value(reason)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{reason:?}"));
        self.entries.push(Entry::TurnEnd(name));
    }

    fn append(&mut self, kind: Kind, content: &ContentBlock) {
        let text = match content {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::ResourceLink(link) => format!("[{}]({})", link.name, link.uri),
            _ => return,
        };
        match (self.entries.last_mut(), kind) {
            (Some(Entry::User(last)), Kind::User)
            | (Some(Entry::AgentText(last)), Kind::Agent)
            | (Some(Entry::Thought(last)), Kind::Thought) => last.push_str(&text),
            _ => self.entries.push(match kind {
                Kind::User => Entry::User(text),
                Kind::Agent => Entry::AgentText(text),
                Kind::Thought => Entry::Thought(text),
            }),
        }
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test acp::model`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/acp/model.rs
git commit -m "feat(agent): transcript model for streamed agent text"
```

---

### Task 5: Fake agent for tests

**Files:**
- Modify: `src/acp/fake_agent.rs`

No tests of its own: Task 6's client tests exercise it.

- [ ] **Step 1: Write the fake agent**

```rust
//! A scripted ACP agent for tests, run in a thread on two pipes. What it
//! does on `session/prompt` depends on the prompt text:
//!
//! - `"permission"`: asks `session/request_permission`, then echoes the
//!   outcome Red chose (`"cancelled"`) as agent text and ends the turn.
//! - `"unknown"`: sends a request Red does not support (`fs/read_text_file`
//!   in stage 1), echoes the error code as agent text, ends the turn.
//! - `"wait"`: does not answer until `session/cancel`, then ends the turn
//!   with `cancelled`.
//! - anything else: streams `"Hello"` and `" world"`, ends with `end_turn`.

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::Arc;
use std::thread;

use serde_json::{Value, json};

use super::transport::Connection;

#[derive(Clone, Copy, Debug)]
pub struct FakeOptions {
    /// `session/new` fails with `-32000` until `authenticate` was called.
    pub require_auth: bool,
    pub protocol_version: u16,
    /// Never answers anything.
    pub silent: bool,
    /// Ends (closing its stdout) right after reading `initialize`.
    pub exit_on_initialize: bool,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self { require_auth: false, protocol_version: 1, silent: false, exit_on_initialize: false }
    }
}

/// A connection to a fresh fake agent.
pub fn connect(options: FakeOptions) -> Connection {
    let (agent_reads, client_writes) = std::io::pipe().expect("pipe");
    let (client_reads, agent_writes) = std::io::pipe().expect("pipe");
    thread::spawn(move || run(agent_reads, agent_writes, options));
    Connection::new(client_reads, client_writes, Arc::new(|| {}))
}

fn send(out: &mut PipeWriter, message: Value) {
    let _ = writeln!(out, "{message}");
}

fn chunk(out: &mut PipeWriter, text: &str) {
    send(
        out,
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "s1",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}
        }}),
    );
}

fn end_turn(out: &mut PipeWriter, id: &Value, reason: &str) {
    send(out, json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": reason}}));
}

fn run(input: PipeReader, mut out: PipeWriter, options: FakeOptions) {
    let mut authed = false;
    // The prompt waiting for Red's answer to our request, or for a cancel.
    let mut open_prompt: Option<Value> = None;
    for line in BufReader::new(input).lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if options.silent {
            continue;
        }
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                if options.exit_on_initialize {
                    return;
                }
                let auth = if options.require_auth {
                    json!([{"id": "login", "name": "Log in", "description": "Sign in with a browser"}])
                } else {
                    json!([])
                };
                send(&mut out, json!({"jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": options.protocol_version,
                    "agentCapabilities": {"promptCapabilities": {"embeddedContext": true}},
                    "authMethods": auth,
                }}));
            }
            Some("authenticate") => {
                authed = true;
                send(&mut out, json!({"jsonrpc": "2.0", "id": id, "result": {}}));
            }
            Some("session/new") => {
                if options.require_auth && !authed {
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": "Authentication required"}}));
                } else {
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": "s1"}}));
                }
            }
            Some("session/prompt") => {
                let text = message["params"]["prompt"][0]["text"].as_str().unwrap_or("").to_owned();
                match text.as_str() {
                    "permission" => {
                        open_prompt = Some(id);
                        send(&mut out, json!({"jsonrpc": "2.0", "id": "perm-1",
                            "method": "session/request_permission", "params": {
                                "sessionId": "s1",
                                "toolCall": {"toolCallId": "t1", "title": "Edit main.rs"},
                                "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}]
                            }}));
                    }
                    "unknown" => {
                        open_prompt = Some(id);
                        send(&mut out, json!({"jsonrpc": "2.0", "id": "fs-1",
                            "method": "fs/read_text_file",
                            "params": {"sessionId": "s1", "path": "/tmp/x"}}));
                    }
                    "wait" => open_prompt = Some(id),
                    _ => {
                        chunk(&mut out, "Hello");
                        chunk(&mut out, " world");
                        end_turn(&mut out, &id, "end_turn");
                    }
                }
            }
            Some("session/cancel") => {
                if let Some(prompt) = open_prompt.take() {
                    end_turn(&mut out, &prompt, "cancelled");
                }
            }
            Some(_) => {}
            // Red's answer to one of our requests.
            None => {
                let echo = match id.as_str() {
                    Some("perm-1") => message["result"]["outcome"]["outcome"].as_str().unwrap_or("?").to_owned(),
                    Some("fs-1") => message["error"]["code"].to_string(),
                    _ => continue,
                };
                chunk(&mut out, &echo);
                if let Some(prompt) = open_prompt.take() {
                    end_turn(&mut out, &prompt, "end_turn");
                }
            }
        }
    }
}
```

- [ ] **Step 2: Check it compiles**

Run: `cargo build --tests`
Expected: builds (an unused-function warning for `connect` until Task 6 is fine).

- [ ] **Step 3: Commit**

```bash
git add src/acp/fake_agent.rs
git commit -m "test(agent): scripted fake ACP agent on pipes"
```

---

### Task 6: Client state machine

**Files:**
- Modify: `src/acp/client.rs`

- [ ] **Step 1: Write the failing tests**

```rust
//! One ACP session, driven without blocking: the UI calls [`Client::poll`]
//! every frame and the client moves through [`Phase`]s as the agent answers.
//! initialize → (authenticate) → session/new → Ready ⇄ Prompting.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::fake_agent::{self, FakeOptions};
    use crate::acp::model::{Entry, Notice};
    use std::thread;

    fn start(options: FakeOptions) -> Client {
        Client::new(fake_agent::connect(options), None, PathBuf::from("/work"), Instant::now())
    }

    /// Polls until `done` holds, failing after five seconds.
    fn pump(client: &mut Client, done: impl Fn(&Client) -> bool) {
        let started = Instant::now();
        while !done(client) {
            assert!(started.elapsed() < Duration::from_secs(5), "stuck in {:?}", client.phase);
            client.poll(Instant::now());
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn ready(options: FakeOptions) -> Client {
        let mut client = start(options);
        pump(&mut client, |c| c.phase == Phase::Ready);
        client
    }

    #[test]
    fn test_reaches_ready_and_keeps_prompt_capabilities() {
        let client = ready(FakeOptions::default());
        assert!(client.embedded_context);
    }

    #[test]
    fn test_prompt_streams_text_and_returns_to_ready() {
        let mut client = ready(FakeOptions::default());
        assert!(client.send_prompt("hi", Instant::now()));
        assert_eq!(client.phase, Phase::Prompting);
        assert!(!client.send_prompt("again", Instant::now()), "one turn at a time");
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert_eq!(
            client.transcript.entries,
            vec![Entry::User("hi".into()), Entry::AgentText("Hello world".into())]
        );
    }

    #[test]
    fn test_cancel_ends_the_turn_as_cancelled() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("wait", Instant::now());
        client.cancel();
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert_eq!(client.transcript.entries.last(), Some(&Entry::TurnEnd("cancelled".into())));
    }

    #[test]
    fn test_permission_requests_are_declined_in_stage_one() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("permission", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        let entries = &client.transcript.entries;
        assert!(entries.contains(&Entry::Notice(Notice::PermissionDeclined)));
        assert!(entries.contains(&Entry::AgentText("cancelled".into())), "{entries:?}");
    }

    #[test]
    fn test_unknown_requests_get_method_not_found() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("unknown", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert!(client.transcript.entries.contains(&Entry::AgentText("-32601".into())));
    }

    #[test]
    fn test_sign_in_then_session() {
        let mut client = start(FakeOptions { require_auth: true, ..FakeOptions::default() });
        pump(&mut client, |c| matches!(c.phase, Phase::NeedsAuth(_)));
        let Phase::NeedsAuth(choices) = client.phase.clone() else { unreachable!() };
        assert_eq!(
            choices,
            vec![AuthChoice {
                id: "login".into(),
                name: "Log in".into(),
                description: Some("Sign in with a browser".into()),
            }]
        );
        client.authenticate("login", Instant::now());
        assert_eq!(client.phase, Phase::Authenticating);
        pump(&mut client, |c| c.phase == Phase::Ready);
    }

    #[test]
    fn test_new_session_clears_the_transcript() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("hi", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        client.new_session(Instant::now());
        assert_eq!(client.phase, Phase::CreatingSession);
        assert!(client.transcript.entries.is_empty());
        pump(&mut client, |c| c.phase == Phase::Ready);
    }

    #[test]
    fn test_other_protocol_version_fails() {
        let mut client = start(FakeOptions { protocol_version: 2, ..FakeOptions::default() });
        pump(&mut client, |c| matches!(c.phase, Phase::Failed(_)));
        let Phase::Failed(message) = &client.phase else { unreachable!() };
        assert!(message.contains("version 2"), "{message}");
    }

    #[test]
    fn test_agent_exit_is_reported() {
        let mut client = start(FakeOptions { exit_on_initialize: true, ..FakeOptions::default() });
        pump(&mut client, |c| matches!(c.phase, Phase::Exited { .. }));
        assert_eq!(client.phase, Phase::Exited { code: None, stderr: vec![] });
    }

    #[test]
    fn test_setup_times_out() {
        let mut client = start(FakeOptions { silent: true, ..FakeOptions::default() });
        assert!(client.is_waiting_on_setup());
        client.poll(Instant::now() + SETUP_TIMEOUT + Duration::from_secs(1));
        let Phase::Failed(message) = &client.phase else { panic!("{:?}", client.phase) };
        assert!(message.contains("initialize"), "{message}");
        assert!(!client.is_waiting_on_setup());
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test acp::client`
Expected: compile errors — `Client`, `Phase`, `AuthChoice`, `SETUP_TIMEOUT` not found.

- [ ] **Step 3: Implement**

Insert above the test module:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1::{
    AuthenticateRequest, CancelNotification, ClientCapabilities, ContentBlock,
    FileSystemCapabilities, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionResponse, SessionId, SessionNotification,
    TextContent,
};
use serde_json::Value;

use super::model::{Notice, Transcript};
use super::transport::{AgentProcess, Connection, Incoming, RpcError};

/// How long `initialize`, `authenticate` and `session/new` may take.
/// Prompt turns have no limit; the user has Stop.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Lines of stderr shown when the agent exits.
const EXIT_STDERR_LINES: usize = 20;

/// A way to sign in that the agent offered.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Initializing,
    /// `session/new` asked for sign-in; the user picks a method.
    NeedsAuth(Vec<AuthChoice>),
    Authenticating,
    CreatingSession,
    Ready,
    Prompting,
    Failed(String),
    Exited { code: Option<i32>, stderr: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    Initialize,
    Authenticate,
    NewSession,
    Prompt,
}

pub struct Client {
    conn: Connection,
    process: Option<AgentProcess>,
    cwd: PathBuf,
    pub phase: Phase,
    pub transcript: Transcript,
    /// The agent accepts embedded resources in prompts (used in stage 5).
    pub embedded_context: bool,
    session_id: Option<SessionId>,
    auth_methods: Vec<AuthChoice>,
    pending: HashMap<u64, Pending>,
    /// When the outstanding setup request times out, and its method.
    deadline: Option<(Instant, &'static str)>,
}

fn params(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("ACP messages serialise")
}

impl Client {
    /// Sends `initialize` at once; the rest follows from [`Self::poll`].
    pub fn new(conn: Connection, process: Option<AgentProcess>, cwd: PathBuf, now: Instant) -> Self {
        let mut client = Self {
            conn,
            process,
            cwd,
            phase: Phase::Initializing,
            transcript: Transcript::default(),
            embedded_context: false,
            session_id: None,
            auth_methods: Vec::new(),
            pending: HashMap::new(),
            deadline: None,
        };
        // Stage 1 offers no file system and no terminals.
        let request = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(
                ClientCapabilities::new()
                    .fs(FileSystemCapabilities::new().read_text_file(false).write_text_file(false))
                    .terminal(false),
            )
            .client_info(Implementation::new("red", env!("CARGO_PKG_VERSION")));
        client.send(Pending::Initialize, "initialize", params(request), now);
        client
    }

    /// Handles everything the agent sent since the last call, then checks
    /// the setup timeout.
    pub fn poll(&mut self, now: Instant) {
        while let Some(message) = self.conn.try_recv() {
            match message {
                Incoming::Response { id, result } => self.on_response(id, result, now),
                Incoming::Request { id, method, params } => self.on_request(id, &method, params),
                Incoming::Notification { method, params } => self.on_notification(&method, params),
                Incoming::Closed => self.on_closed(),
            }
        }
        if let Some((deadline, method)) = self.deadline {
            if now >= deadline {
                self.fail(format!("the agent did not answer `{method}` within 30 s"));
            }
        }
    }

    /// A setup request is outstanding, so the UI should keep repainting to
    /// notice the timeout.
    pub fn is_waiting_on_setup(&self) -> bool {
        self.deadline.is_some()
    }

    /// Starts a turn; `false` unless the session is ready.
    pub fn send_prompt(&mut self, text: &str, now: Instant) -> bool {
        let (Phase::Ready, Some(session_id)) = (&self.phase, &self.session_id) else {
            return false;
        };
        let request = PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(text))],
        );
        self.transcript.push_user(text);
        self.send(Pending::Prompt, "session/prompt", params(request), now);
        self.phase = Phase::Prompting;
        true
    }

    /// Asks the agent to stop the running turn. The turn ends when the agent
    /// answers the prompt (with `cancelled`).
    pub fn cancel(&mut self) {
        if let (Phase::Prompting, Some(session_id)) = (&self.phase, &self.session_id) {
            self.conn.notify("session/cancel", params(CancelNotification::new(session_id.clone())));
        }
    }

    /// The session can be replaced without restarting the agent.
    pub fn can_start_session(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::Prompting)
    }

    /// Drops the conversation and opens a new session on the same agent.
    pub fn new_session(&mut self, now: Instant) {
        if !self.can_start_session() {
            return;
        }
        self.cancel();
        // The old turn's answer, when it comes, belongs to nobody.
        self.pending.retain(|_, pending| *pending != Pending::Prompt);
        self.transcript = Transcript::default();
        self.request_session(now);
    }

    pub fn authenticate(&mut self, method_id: &str, now: Instant) {
        if !matches!(self.phase, Phase::NeedsAuth(_)) {
            return;
        }
        self.phase = Phase::Authenticating;
        let request = AuthenticateRequest::new(method_id.to_owned());
        self.send(Pending::Authenticate, "authenticate", params(request), now);
    }

    /// The last `n` lines of the agent's stderr; empty without a process.
    pub fn stderr_tail(&self, n: usize) -> Vec<String> {
        self.process.as_ref().map(|p| p.stderr_tail(n)).unwrap_or_default()
    }

    fn send(&mut self, pending: Pending, method: &'static str, params: Value, now: Instant) {
        let id = self.conn.request(method, params);
        self.pending.insert(id, pending);
        if pending != Pending::Prompt {
            self.deadline = Some((now + SETUP_TIMEOUT, method));
        }
    }

    fn request_session(&mut self, now: Instant) {
        self.phase = Phase::CreatingSession;
        let request = NewSessionRequest::new(self.cwd.clone());
        self.send(Pending::NewSession, "session/new", params(request), now);
    }

    fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Failed(_) | Phase::Exited { .. })
    }

    fn fail(&mut self, message: String) {
        self.phase = Phase::Failed(message);
        self.pending.clear();
        self.deadline = None;
    }

    fn on_response(&mut self, id: u64, result: Result<Value, RpcError>, now: Instant) {
        let Some(kind) = self.pending.remove(&id) else { return };
        if kind != Pending::Prompt {
            self.deadline = None;
        }
        if self.is_finished() {
            return;
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                match kind {
                    Pending::NewSession
                        if error.code == RpcError::AUTH_REQUIRED && !self.auth_methods.is_empty() =>
                    {
                        self.phase = Phase::NeedsAuth(self.auth_methods.clone());
                    }
                    Pending::Prompt => {
                        self.transcript.notice(Notice::Error(error.message));
                        self.phase = Phase::Ready;
                    }
                    _ => self.fail(error.message),
                }
                return;
            }
        };
        match kind {
            Pending::Initialize => self.on_initialized(value, now),
            Pending::Authenticate => self.request_session(now),
            Pending::NewSession => match serde_json::from_value::<NewSessionResponse>(value) {
                Ok(response) => {
                    self.session_id = Some(response.session_id);
                    self.phase = Phase::Ready;
                }
                Err(error) => self.fail(format!("unreadable session/new answer: {error}")),
            },
            Pending::Prompt => {
                if let Ok(response) = serde_json::from_value::<PromptResponse>(value) {
                    self.transcript.end_turn(&response.stop_reason);
                }
                self.phase = Phase::Ready;
            }
        }
    }

    fn on_initialized(&mut self, value: Value, now: Instant) {
        let response = match serde_json::from_value::<InitializeResponse>(value.clone()) {
            Ok(response) => response,
            Err(error) => return self.fail(format!("unreadable initialize answer: {error}")),
        };
        if response.protocol_version != ProtocolVersion::V1 {
            return self.fail(format!(
                "the agent speaks ACP version {}, Red speaks version 1",
                response.protocol_version.as_u16()
            ));
        }
        self.embedded_context = response.agent_capabilities.prompt_capabilities.embedded_context;
        self.auth_methods = auth_choices(&value);
        self.request_session(now);
    }

    fn on_request(&mut self, id: Value, method: &str, _params: Value) {
        match method {
            "session/request_permission" => {
                let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
                self.conn.respond(id, Ok(params(response)));
                self.transcript.notice(Notice::PermissionDeclined);
            }
            _ => self.conn.respond(id, Err(RpcError::method_not_found(method))),
        }
    }

    fn on_notification(&mut self, method: &str, params: Value) {
        if method != "session/update" {
            return;
        }
        let Ok(notification) = serde_json::from_value::<SessionNotification>(params) else {
            return;
        };
        if self.session_id.as_ref() == Some(&notification.session_id) {
            self.transcript.apply(&notification.update);
        }
    }

    fn on_closed(&mut self) {
        if self.is_finished() {
            return;
        }
        let (code, stderr) = match &mut self.process {
            Some(process) => (
                process.exit_code_within(Duration::from_millis(200)),
                process.stderr_tail(EXIT_STDERR_LINES),
            ),
            None => (None, Vec::new()),
        };
        self.pending.clear();
        self.deadline = None;
        self.phase = Phase::Exited { code, stderr };
    }
}

/// The sign-in methods from a raw `initialize` answer. Read from JSON, not
/// the typed enum, so every variant gives its id, name and description.
fn auth_choices(value: &Value) -> Vec<AuthChoice> {
    value
        .get("authMethods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|method| {
            Some(AuthChoice {
                id: method.get("id")?.as_str()?.to_owned(),
                name: method.get("name")?.as_str()?.to_owned(),
                description: method.get("description").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect()
}

impl Drop for Client {
    /// Stops a running turn politely before the process is killed.
    fn drop(&mut self) {
        self.cancel();
    }
}
```

`params` needs `serde::Serialize`. `serde` is not yet a direct dependency; add it to `Cargo.toml` `[dependencies]` (it is already in the lock file through `serde_json`):

```toml
serde = "1"
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test acp::client`
Expected: 10 passed. If `ProtocolVersion` does not implement `PartialEq`, compare `response.protocol_version.as_u16() != 1` instead.

- [ ] **Step 5: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass. Unused-code warnings for items only the panel uses (`stderr_tail`, `can_start_session`, `AgentProcess::spawn`) are expected until Task 8; if clippy denies them, add `#[allow(dead_code)]` on those items now and remove it in Task 8.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/acp/client.rs
git commit -m "feat(agent): ACP client state machine with sign-in, cancel and exit handling"
```

---

### Task 7: Strings in all five languages

> Requires Step 0: the user's Console work is committed or stashed.

**Files:**
- Modify: `assets/lang/en.json`, `assets/lang/id.json`, `assets/lang/de.json`, `assets/lang/es.json`, `assets/lang/zh.json`

- [ ] **Step 1: Add the keys**

Add these keys to each file (keep the file's existing grouping: `activity.*` with the activity keys, `view.*` with the view keys, `settings.*` with the settings keys, the `agent.*` block at the end before the closing `}`; mind the commas). `{program}`, `{error}`, `{code}`, `{reason}` are `tf` placeholders and must stay untranslated.

| key | en | id | de | es | zh |
|---|---|---|---|---|---|
| `activity.agent` | Agent | Agen | Agent | Agente | 智能体 |
| `view.agent` | Agent Panel | Panel Agen | Agent-Bereich | Panel del agente | 智能体面板 |
| `agent.new_session` | New Session | Sesi Baru | Neue Sitzung | Nueva sesión | 新会话 |
| `agent.restart` | Restart Agent | Mulai Ulang Agen | Agent neu starten | Reiniciar agente | 重启智能体 |
| `agent.close` | Close | Tutup | Schließen | Cerrar | 关闭 |
| `agent.send` | Send | Kirim | Senden | Enviar | 发送 |
| `agent.stop` | Stop | Hentikan | Stopp | Detener | 停止 |
| `agent.placeholder` | Ask the agent… (Enter to send, Shift+Enter for a new line) | Tanya agen… (Enter untuk kirim, Shift+Enter untuk baris baru) | Frag den Agenten… (Enter zum Senden, Umschalt+Enter für eine neue Zeile) | Pregunta al agente… (Intro para enviar, Mayús+Intro para nueva línea) | 向智能体提问…（回车发送，Shift+回车换行） |
| `agent.no_command` | No agent command set. Add one in Settings → Agent. | Perintah agen belum diatur. Tambahkan di Pengaturan → Agen. | Kein Agent-Befehl festgelegt. Lege einen unter Einstellungen → Agent fest. | No hay un comando de agente. Añade uno en Configuración → Agente. | 未设置智能体命令。请在 设置 → 智能体 中添加。 |
| `agent.unclosed_quote` | The agent command has an unclosed quote. | Perintah agen punya tanda kutip yang tidak ditutup. | Im Agent-Befehl fehlt ein schließendes Anführungszeichen. | El comando del agente tiene comillas sin cerrar. | 智能体命令中有未闭合的引号。 |
| `agent.open_settings` | Open Settings | Buka Pengaturan | Einstellungen öffnen | Abrir configuración | 打开设置 |
| `agent.no_folder` | Open a folder first: the agent works inside the workspace. | Buka folder dulu: agen bekerja di dalam workspace. | Öffne zuerst einen Ordner: Der Agent arbeitet im Arbeitsbereich. | Abre primero una carpeta: el agente trabaja dentro del espacio de trabajo. | 请先打开文件夹：智能体在工作区内工作。 |
| `agent.open_folder` | Open Folder | Buka Folder | Ordner öffnen | Abrir carpeta | 打开文件夹 |
| `agent.not_found` | Agent program not found: {program} | Program agen tidak ditemukan: {program} | Agent-Programm nicht gefunden: {program} | No se encontró el programa del agente: {program} | 找不到智能体程序：{program} |
| `agent.start_failed` | Could not start the agent: {error} | Gagal memulai agen: {error} | Agent konnte nicht gestartet werden: {error} | No se pudo iniciar el agente: {error} | 无法启动智能体：{error} |
| `agent.starting` | Starting the agent… | Memulai agen… | Agent wird gestartet… | Iniciando el agente… | 正在启动智能体… |
| `agent.working` | The agent is working… | Agen sedang bekerja… | Der Agent arbeitet… | El agente está trabajando… | 智能体正在工作… |
| `agent.sign_in` | The agent needs you to sign in: | Agen meminta kamu masuk: | Der Agent möchte, dass du dich anmeldest: | El agente necesita que inicies sesión: | 智能体需要你登录： |
| `agent.exited` | Agent exited (code {code}) | Agen berhenti (kode {code}) | Agent beendet (Code {code}) | El agente terminó (código {code}) | 智能体已退出（代码 {code}） |
| `agent.failed` | Agent error: {error} | Galat agen: {error} | Agent-Fehler: {error} | Error del agente: {error} | 智能体错误：{error} |
| `agent.permission_declined` | The agent asked for a permission this version of Red cannot grant yet, so it was declined. | Agen meminta izin yang belum bisa diberikan Red versi ini, jadi permintaan ditolak. | Der Agent bat um eine Berechtigung, die diese Red-Version noch nicht erteilen kann; sie wurde abgelehnt. | El agente pidió un permiso que esta versión de Red aún no puede conceder, así que se rechazó. | 智能体请求了此版本 Red 尚不能授予的权限，已拒绝。 |
| `agent.turn_end` | Turn ended: {reason} | Giliran berakhir: {reason} | Runde beendet: {reason} | Turno terminado: {reason} | 回合结束：{reason} |
| `agent.thought` | Thinking | Berpikir | Überlegung | Razonamiento | 思考 |
| `agent.show_log` | Agent log | Log agen | Agent-Protokoll | Registro del agente | 智能体日志 |
| `settings.agent` | Agent | Agen | Agent | Agente | 智能体 |
| `settings.agent_command` | Agent command | Perintah agen | Agent-Befehl | Comando del agente | 智能体命令 |
| `settings.agent_command_hint` | e.g. npx @zed-industries/claude-code-acp | mis. npx @zed-industries/claude-code-acp | z. B. npx @zed-industries/claude-code-acp | p. ej. npx @zed-industries/claude-code-acp | 例如 npx @zed-industries/claude-code-acp |
| `settings.agent_restart_note` | Takes effect when the agent restarts. | Berlaku setelah agen dimulai ulang. | Wirkt beim nächsten Neustart des Agenten. | Se aplica al reiniciar el agente. | 重启智能体后生效。 |

- [ ] **Step 2: Run the i18n tests**

Run: `cargo test i18n`
Expected: all pass (every locale lists the same keys). The "used keys exist" test passes too, since nothing uses the new keys yet.

- [ ] **Step 3: Commit**

```bash
git add assets/lang
git commit -m "feat(agent): strings for the Agent panel in five languages"
```

---

### Task 8: The Agent panel

**Files:**
- Modify: `src/agent_panel.rs`
- Modify: `src/app.rs` (fields, storage, poll, placement)

- [ ] **Step 1: Write `src/agent_panel.rs`**

```rust
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
use crate::theme;

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
            Err(CommandError::Empty) => return self.agent.start_error = Some(StartError::NoCommand),
            Err(CommandError::UnclosedQuote) => {
                return self.agent.start_error = Some(StartError::UnclosedQuote);
            }
        };
        let Some(root) = self.explorer.as_ref().map(|e| e.root().to_path_buf()) else {
            return self.agent.start_error = Some(StartError::NoFolder);
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
    }

    /// Once per frame, shown or not: the agent keeps working while hidden.
    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        if let Some(client) = &mut self.agent.client {
            client.poll(Instant::now());
            if client.is_waiting_on_setup() {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    pub(crate) fn show_agent_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::active(&ctx);
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
                        clicks.restart = ui.small_button("⟲").on_hover_text(i18n.t("agent.restart")).clicked();
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
                        StartError::NotFound(program) => i18n.tf("agent.not_found", &[("program", program)]),
                        StartError::Io(error) => i18n.tf("agent.start_failed", &[("error", error)]),
                    };
                    ui.label(message);
                    if matches!(error, StartError::NoFolder) {
                        clicks.open_folder = ui.button(i18n.t("agent.open_folder")).clicked();
                    } else {
                        clicks.open_settings = ui.button(i18n.t("agent.open_settings")).clicked();
                    }
                    return;
                }
                let Some(client) = client else { return };

                // Bottom up: the prompt box first, then the transcript fills
                // what is left above it.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        if client.phase == Phase::Prompting {
                            clicks.stop = ui.button(i18n.t("agent.stop")).clicked();
                        } else {
                            let can_send = client.phase == Phase::Ready && !input.trim().is_empty();
                            clicks.send |= ui.add_enabled(can_send, egui::Button::new(i18n.t("agent.send"))).clicked();
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
                                    ui.push_id(i, |ui| draw_entry(ui, i18n, markdown, &palette, entry));
                                    ui.add_space(6.0);
                                }
                                draw_phase(ui, i18n, &client.phase, &mut clicks);
                                if *show_log {
                                    ui.separator();
                                    let log = client.stderr_tail(LOG_LINES).join("\n");
                                    ui.add(egui::Label::new(egui::RichText::new(log).monospace().small()).wrap());
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
    egui::Id::new("agent_prompt_input")
}

fn draw_entry(
    ui: &mut egui::Ui,
    i18n: &crate::i18n::I18n,
    markdown: &mut CommonMarkCache,
    palette: &theme::Theme,
    entry: &Entry,
) {
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
            ui.colored_label(palette.warning, i18n.t("agent.permission_declined"));
        }
        Entry::Notice(Notice::Error(message)) => {
            ui.colored_label(ui.visuals().error_fg_color, i18n.tf("agent.failed", &[("error", message)]));
        }
        Entry::TurnEnd(reason) => {
            ui.label(egui::RichText::new(i18n.tf("agent.turn_end", &[("reason", reason)])).weak().small());
        }
    }
}

fn draw_phase(ui: &mut egui::Ui, i18n: &crate::i18n::I18n, phase: &Phase, clicks: &mut Clicks) {
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
```

Two names in this file depend on the existing code; check them before building and adjust if they differ:
- the i18n type: `grep -n "pub struct" src/i18n.rs` (assumed `I18n`);
- the theme type and a warning colour: `grep -n "pub struct\|pub fn active\|warning" src/theme.rs`. If `theme::active` returns a type without a `warning` colour, use `egui::Color32::from_rgb(0xcc, 0x99, 0x00)` in `draw_entry` and drop the `palette` parameter.

- [ ] **Step 2: Wire the panel into `src/app.rs`**

1. Imports at the top of `src/app.rs`:

```rust
use crate::agent_panel::AgentPanel;
use crate::agent_settings::AgentSettings;
```

2. Storage keys, next to `const EDITOR_SETTINGS_KEY` (line ~65):

```rust
const AGENT_SETTINGS_KEY: &str = "agent_settings";
const AGENT_PANEL_OPEN_KEY: &str = "agent_panel_open";
```

3. Fields of `pub struct TextEditorApp` (line ~352), next to `pub show_console: bool`:

```rust
    /// The Agent panel on the right is shown.
    pub show_agent: bool,
    /// The ACP session and the panel's own state.
    pub agent: AgentPanel,
    /// Settings → Agent.
    pub agent_settings: AgentSettings,
```

4. Every place that builds a `TextEditorApp` must set them. Find them with `grep -n "show_console: false" src/app.rs` and add beside each:

```rust
            show_agent: false,
            agent: AgentPanel::default(),
            agent_settings: AgentSettings::default(),
```

In `TextEditorApp::new` (line ~623), load the saved values instead:

```rust
            show_agent: cc
                .storage
                .and_then(|storage| storage.get_string(AGENT_PANEL_OPEN_KEY))
                .is_some_and(|text| text == "true"),
            agent: AgentPanel::default(),
            agent_settings: AgentSettings::from_storage(
                cc.storage.and_then(|storage| storage.get_string(AGENT_SETTINGS_KEY)).as_deref(),
            ),
```

A panel restored open shows a Restart button (no client yet) rather than starting an agent at app start, as the spec asks.

5. In `fn save(&mut self, storage: &mut dyn eframe::Storage)` (line ~2852), after the `EDITOR_SETTINGS_KEY` line:

```rust
        storage.set_string(AGENT_SETTINGS_KEY, self.agent_settings.to_storage());
        storage.set_string(AGENT_PANEL_OPEN_KEY, self.show_agent.to_string());
```

6. Poll every frame: at the start of `fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame)` (line ~2862), after `ctx` is bound (look for `let ctx = ui.ctx().clone();` near the top; add that line if absent):

```rust
        self.poll_agent(&ctx);
```

7. Place the panel just before the Console panel (line ~3177, the comment "Console panel, below the editor"), so the Agent panel takes the full height on the right and the Console spans only the editor column:

```rust
        // Agent panel, full height on the right: declared before the
        // Console so the Console stays under the editor only.
        if self.show_agent && !self.zen_mode {
            self.show_agent_panel(ui);
        }
```

8. With the panel shown but no client and no error (restored open), offer Restart: in `show_agent_panel`, the header's ⟲ button already does that, so nothing more is needed.

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: builds; all tests pass. Remove any `#[allow(dead_code)]` added in Task 6 and re-run `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 4: Commit**

```bash
git add src/agent_panel.rs src/app.rs
git commit -m "feat(agent): Agent panel on the right with prompt, transcript and restart"
```

---

### Task 9: Activity bar, shortcut, menu, palette, Settings

**Files:**
- Modify: `src/activity_bar.rs`, `src/shortcuts.rs`, `src/app.rs`, `src/menus.rs`, `src/palette.rs`, `src/settings_window.rs`

- [ ] **Step 1: Activity bar item**

In `src/activity_bar.rs`:

- `ActivityBarResponse`: add `pub agent_clicked: bool,` after `console_clicked`, and `pub agent_rect: Rect,` (with the same `#[cfg_attr(not(test), allow(dead_code))]`) after `console_rect`.
- `ActivityItems`: add `pub agent_open: bool,` after `console_open` and `pub agent_tooltip: &'a str,` after `console_tooltip`.
- Update the module doc's first lines to name five items: "the Explorer, Search (Find in Files), Source Control, the Console and the Agent".
- In `show`, after the console item:

```rust
    let (agent_rect, agent_clicked) = item(
        ui,
        &palette,
        items.agent_open,
        items.agent_tooltip,
        paint_agent_icon,
    );
```

  and add `agent_clicked,` and `agent_rect,` to the returned struct.
- The icon, after `paint_console_icon`:

```rust
/// A speech bubble with three dots: a conversation with the agent. Drawn
/// as lines, so it never depends on the bundled fonts.
fn paint_agent_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let bubble = Rect::from_center_size(center + vec2(0.0, -1.5), vec2(18.0, 13.0));
    painter.rect_stroke(bubble, CornerRadius::same(3), stroke, egui::StrokeKind::Middle);
    // The tail, bottom left.
    let tail = pos2(bubble.left() + 4.0, bubble.bottom());
    painter.line(vec![tail, tail + vec2(-1.0, 4.0), tail + vec2(4.0, 0.0)], stroke);
    for dx in [-4.0, 0.0, 4.0] {
        painter.circle_filled(bubble.center() + vec2(dx, 0.0), 1.2, color);
    }
}
```

- Existing tests in `activity_bar.rs` build `ActivityItems` literals; add `agent_open: false, agent_tooltip: "Agent",` to each (`grep -n "console_tooltip:" src/activity_bar.rs`).

In `src/app.rs`, in the `activity_bar::ActivityItems { … }` literal (line ~3077) add:

```rust
                        agent_open: self.show_agent,
                        agent_tooltip: self.i18n.t("activity.agent"),
```

and after `if activity.console_clicked { … }`:

```rust
            if activity.agent_clicked {
                self.toggle_agent_panel(&ctx);
            }
```

- [ ] **Step 2: Shortcut and actions**

In `src/shortcuts.rs`:

- After `pub const TOGGLE_CONSOLE …`:

```rust
pub const TOGGLE_AGENT: KeyboardShortcut = cmd_shift(Key::A);
```

- In `pub enum ShortcutAction`, after `NewConsole`:

```rust
    /// Show or hide the Agent panel.
    ToggleAgent,
    /// A new conversation with the agent (a Command Palette entry; no key).
    NewAgentSession,
```

- In `takes_keyboard`, add `| ToggleAgent` after `| NewConsole`.
- In `shortcut()`: add `ToggleAgent => TOGGLE_AGENT,` after `NewConsole => NEW_CONSOLE,`, and add `NewAgentSession` to the `=> return None` arm: `SelectTheme(_) | FoldAll | UnfoldAll | DiffWithFile | AutoSave | NewAgentSession => return None,`.
- In `consume_shortcut`'s table, add `(TOGGLE_AGENT, ToggleAgent),` among the `cmd_shift` entries **before** any plain `cmd(Key::A)` entry (Select All is handled by egui's text edit, not this table; check with `grep -n "Key::A" src/shortcuts.rs` that nothing else is bound to Ctrl+Shift+A).
- In the test table near line 494, add:

```rust
            (egui::Key::A, ctrl_shift, ShortcutAction::ToggleAgent),
```

In `src/app.rs`, in the action `match` (line ~1517), after `ShortcutAction::NewConsole => …`:

```rust
            ShortcutAction::ToggleAgent => self.toggle_agent_panel(ctx),
            ShortcutAction::NewAgentSession => self.new_agent_session(ctx),
```

- [ ] **Step 3: View menu**

In `src/menus.rs`, after the Console button block (line ~220):

```rust
        let agent_label = self.i18n.t("view.agent");
        let agent_shortcut = shortcuts::label(&ctx, &shortcuts::TOGGLE_AGENT);
        if ui
            .add(
                egui::Button::new(agent_label)
                    .selected(self.show_agent)
                    .shortcut_text(agent_shortcut),
            )
            .clicked()
        {
            self.toggle_agent_panel(&ctx);
            ui.close();
        }
```

- [ ] **Step 4: Palette commands**

In `src/palette.rs`, after `command("view.new_console", ShortcutAction::NewConsole),`:

```rust
    command("view.agent", ShortcutAction::ToggleAgent),
    command("agent.new_session", ShortcutAction::NewAgentSession),
```

- [ ] **Step 5: Settings → Agent**

In `src/settings_window.rs`, at the end of `settings_contents`, after the General grid:

```rust
        ui.separator();
        ui.heading(self.i18n.t("settings.agent"));
        egui::Grid::new("settings_agent").num_columns(2).show(ui, |ui| {
            ui.label(self.i18n.t("settings.agent_command"));
            ui.add(
                egui::TextEdit::singleline(&mut self.agent_settings.command)
                    .hint_text(self.i18n.t("settings.agent_command_hint"))
                    .desired_width(320.0),
            );
            ui.end_row();
        });
        ui.label(egui::RichText::new(self.i18n.t("settings.agent_restart_note")).weak().small());
```

- [ ] **Step 6: Build, test, lint**

Run: `cargo build && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all pass, including the shortcut table test with the new `Ctrl+Shift+A` row and the i18n "used keys exist" test.

- [ ] **Step 7: Commit**

```bash
git add src/activity_bar.rs src/shortcuts.rs src/app.rs src/menus.rs src/palette.rs src/settings_window.rs
git commit -m "feat(agent): Agent item, Ctrl+Shift+A, View menu, palette and Settings field"
```

---

### Task 10: Hand check with a real agent and docs

**Files:**
- Modify: `todo.md`

- [ ] **Step 1: Run the app against a real agent**

```bash
cargo run --release
```

Check, writing down what happens:
1. Settings (`Ctrl+,`) → Agent command empty → `Ctrl+Shift+A` → panel says no command, Open Settings works.
2. No folder open → panel asks for a folder; Open Folder works.
3. Command `doesnotexist` → "Agent program not found: doesnotexist".
4. Command `npx @zed-industries/claude-code-acp` (needs Node and a Claude login) or `gemini --experimental-acp`, folder open, ⟲ → spinner, then ready; send "Say hello in one word" → streamed answer; Enter sends, Shift+Enter makes a newline.
5. Ask for something long, click Stop → turn ends with "Turn ended: cancelled".
6. Ask the agent to edit a file → it asks permission → the panel shows the "declined" notice and the agent carries on or stops (stage 1 behaviour).
7. `+` → transcript clears, new session ready.
8. Kill the agent from a shell (`pkill -f claude-code-acp`) → "Agent exited (code …)" with stderr lines and a Restart button that works.
9. Hide and show the panel during a turn → the turn keeps streaming.
10. Quit Red during a turn → no agent process left (`pgrep -f claude-code-acp` prints nothing).

- [ ] **Step 2: Update `todo.md`**

Under "Languages and tooling", add:

```markdown
- [ ] **Agent panel (ACP)** — stage 1 done YYYY-MM-DD (`src/acp/`,
  `src/agent_panel.rs`): any Agent Client Protocol agent from Settings →
  Agent, chat with streaming, Stop, New Session, sign-in, Restart.
  Hand-checked with <agent>. Next: stage 2 (tool calls, permission,
  plan), then fs, terminals, editor context. Spec:
  `docs/superpowers/specs/2026-11-18-acp-agent-panel-design.md`.
```

Replace `YYYY-MM-DD` with the day the hand check passed and `<agent>` with the agent(s) used.

- [ ] **Step 3: Commit**

```bash
git add todo.md
git commit -m "docs(todo): Agent panel stage 1"
```

---

## Later stages

Each gets its own plan, written against the code this plan produced:

2. Tool calls, permission cards, plan — `docs/superpowers/plans/…-acp-stage2.md`
3. fs read/write through the editor
4. Terminals for the agent
5. Editor context (`@` files, selections)
6. Polish: README (en, id), release notes
