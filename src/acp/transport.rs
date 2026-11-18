//! JSON-RPC 2.0 over a pair of byte streams, one message per line, as ACP's
//! stdio transport wants it. A reader thread parses lines into [`Incoming`]
//! messages and wakes the UI; a writer thread sends what the UI queues. The
//! UI never blocks on the agent.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;

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
    /// An error answer with `"id": null`: the agent could not tell which
    /// request it answers (for example one it could not parse).
    OrphanError(RpcError),
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
    let error = object.get("error").filter(|error| !error.is_null());
    if let (Some(Value::Null), Some(error)) = (object.get("id"), error) {
        return Some(Incoming::OrphanError(RpcError::from_value(error)));
    }
    let id = object.get("id")?.as_u64()?;
    let result = match error {
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
        self.incoming.try_recv().ok()
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
        // Its own process group, so Drop can kill what the agent started
        // too (`npx` runs the real agent as a child).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LINES)));
        let sink = Arc::clone(&stderr);
        // Owned from here on, so every early return below kills the child.
        let process = AgentProcess { child, stderr };
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
        Ok((Connection::new(stdout, stdin, waker), process))
    }

    /// The last `n` lines the agent wrote to stderr, oldest first.
    pub fn stderr_tail(&self, n: usize) -> Vec<String> {
        let lines = self.stderr.lock().expect("stderr lock");
        lines.iter().skip(lines.len().saturating_sub(n)).cloned().collect()
    }

    /// `Some(code)` once the process has ended (`code` is `None` when a
    /// signal ended it); `None` while it runs. Never blocks.
    pub fn try_exit(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|status| status.code())
    }

}

impl Drop for AgentProcess {
    /// Kills the agent and everything it started, then reaps it. On Unix
    /// the group is killed even when the child itself has already ended
    /// (`npx` gone, its node agent still running). On Windows the tree can
    /// only be found while the child runs.
    fn drop(&mut self) {
        #[cfg(unix)]
        kill_tree(self.child.id());
        #[cfg(windows)]
        if self.child.try_wait().ok().flatten().is_none() {
            kill_tree(self.child.id());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The agent's process group (Unix; see `process_group(0)` in `spawn`).
#[cfg(unix)]
fn kill_tree(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The agent and its child processes.
#[cfg(windows)]
fn kill_tree(pid: u32) {
    let mut command = Command::new("taskkill");
    command.args(["/F", "/T", "/PID", &pid.to_string()]);
    crate::process::hide_console(&mut command);
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

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
    fn test_parse_null_id_error_and_null_error_field() {
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#),
            Some(Incoming::OrphanError(RpcError { code: -32700, message: "Parse error".to_owned() }))
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":5,"result":{},"error":null}"#),
            Some(Incoming::Response { id: 5, result: Ok(json!({})) })
        );
    }

    #[test]
    fn test_spawn_missing_program_is_not_found() {
        let error = AgentProcess::spawn("red-no-such-acp-agent", &[], Path::new("."), Arc::new(|| {}))
            .err()
            .expect("spawn fails");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn test_drop_kills_the_agent_and_its_children() {
        let args = ["-c".to_owned(), "sleep 60 & wait".to_owned()];
        let (conn, process) = AgentProcess::spawn("sh", &args, Path::new("."), Arc::new(|| {})).unwrap();
        drop(process);
        // `sleep` held stdout too: EOF means every process of the group died.
        assert_eq!(conn.recv_timeout(Duration::from_secs(5)), Some(Incoming::Closed));
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
