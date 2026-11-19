//! A scripted ACP agent for tests, run in a thread on two pipes. What it
//! does on `session/prompt` depends on the prompt text:
//!
//! - `"permission"`: asks `session/request_permission` (four options) and
//!   waits; echoes the outcome Red chose (`"cancelled"` or
//!   `"selected:<optionId>"`) as agent text and ends the turn.
//! - `"tools"`: a tool call with a location, updates (in progress, then a
//!   diff and `completed`), a two-step plan, then closing text.
//! - `"unknown"`: sends a request Red does not support (`fs/read_text_file`
//!   in stage 1), echoes the error code as agent text, ends the turn.
//! - `"wait"`: does not answer until `session/cancel`, then ends the turn
//!   with `cancelled`.
//! - `"orphan"`: answers with an error whose id is `null`.
//!
//! It also offers a `terminal` sign-in method (which Red must not show),
//! fails `authenticate` for any method but `login`, and before answering a
//! second `session/new` streams a chunk for the previous session.
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

fn chunk(out: &mut PipeWriter, session: &str, text: &str) {
    send(
        out,
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": session,
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}
        }}),
    );
}

fn end_turn(out: &mut PipeWriter, id: &Value, reason: &str) {
    send(out, json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": reason}}));
}

fn run(input: PipeReader, mut out: PipeWriter, options: FakeOptions) {
    let mut authed = false;
    // Sessions handed out so far; the current one is `s{sessions}`.
    let mut sessions = 0;
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
                    json!([
                        {"id": "login", "name": "Log in", "description": "Sign in with a browser"},
                        {"id": "cli", "name": "Terminal login", "type": "terminal", "args": ["--login"]}
                    ])
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
                if message["params"]["methodId"] == "login" {
                    authed = true;
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                } else {
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32603, "message": "Sign-in failed"}}));
                }
            }
            Some("session/new") => {
                if options.require_auth && !authed {
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": "Authentication required"}}));
                } else {
                    if sessions > 0 {
                        chunk(&mut out, &format!("s{sessions}"), "stale");
                    }
                    sessions += 1;
                    send(&mut out, json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": format!("s{sessions}")}}));
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
                                "options": [
                                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                    {"optionId": "allow_always", "name": "Always allow", "kind": "allow_always"},
                                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                                    {"optionId": "reject_always", "name": "Always reject", "kind": "reject_always"}
                                ]
                            }}));
                    }
                    "tools" => {
                        let session = format!("s{sessions}");
                        for update in [
                            json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit main.rs",
                                "kind": "edit", "status": "pending",
                                "locations": [{"path": "/work/main.rs", "line": 3}]}),
                            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"}),
                            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                                "content": [{"type": "diff", "path": "/work/main.rs",
                                    "oldText": "a\nb\n", "newText": "a\nc\n"}]}),
                            json!({"sessionUpdate": "plan", "entries": [
                                {"content": "Edit", "priority": "high", "status": "completed"},
                                {"content": "Test", "priority": "medium", "status": "pending"}]}),
                        ] {
                            send(&mut out, json!({"jsonrpc": "2.0", "method": "session/update",
                                "params": {"sessionId": session, "update": update}}));
                        }
                        chunk(&mut out, &session, "Done");
                        end_turn(&mut out, &id, "end_turn");
                    }
                    "unknown" => {
                        open_prompt = Some(id);
                        send(&mut out, json!({"jsonrpc": "2.0", "id": "fs-1",
                            "method": "fs/read_text_file",
                            "params": {"sessionId": "s1", "path": "/tmp/x"}}));
                    }
                    "wait" => open_prompt = Some(id),
                    "orphan" => send(&mut out, json!({"jsonrpc": "2.0", "id": null,
                        "error": {"code": -32700, "message": "Parse error"}})),
                    _ => {
                        let session = format!("s{sessions}");
                        chunk(&mut out, &session, "Hello");
                        chunk(&mut out, &session, " world");
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
                    Some("perm-1") => {
                        let outcome = &message["result"]["outcome"];
                        match outcome["outcome"].as_str() {
                            Some("selected") => format!("selected:{}", outcome["optionId"].as_str().unwrap_or("?")),
                            other => other.unwrap_or("?").to_owned(),
                        }
                    }
                    Some("fs-1") => message["error"]["code"].to_string(),
                    _ => continue,
                };
                chunk(&mut out, &format!("s{sessions}"), &echo);
                if let Some(prompt) = open_prompt.take() {
                    end_turn(&mut out, &prompt, "end_turn");
                }
            }
        }
    }
}
