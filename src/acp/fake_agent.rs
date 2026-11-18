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
