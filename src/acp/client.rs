//! One ACP session, driven without blocking: the UI calls [`Client::poll`]
//! every frame and the client moves through [`Phase`]s as the agent answers.
//! initialize → (authenticate) → session/new → Ready ⇄ Prompting.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1::{
    AuthenticateRequest, CancelNotification, ClientCapabilities,
    FileSystemCapabilities, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionId,
    SessionNotification, SelectedPermissionOutcome,
};
use serde_json::Value;

use super::context::{Chip, build_blocks};
use super::fs::{self, Buffers, FsCall};
use super::model::{Attached, Entry, Notice, PermissionCard, PermissionChoice, PermissionState, Transcript};
use super::transport::{AgentProcess, Connection, Incoming, RpcError};

/// How long `initialize` and `session/new` may take. Sign-in (a browser
/// round trip) and prompt turns have no limit.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Lines of stderr shown when the agent exits.
const EXIT_STDERR_LINES: usize = 20;
/// After the child process ends, how long its stdout may stay open (held
/// by a process the agent started) before the exit is reported anyway.
const CHILD_EXIT_GRACE: Duration = Duration::from_millis(500);
/// How long after an exit the client keeps looking for the exit code and
/// the last stderr lines.
const EXIT_SETTLE: Duration = Duration::from_secs(2);

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
    /// The child's exit code once it has ended (inner `None`: by a signal).
    exit_code: Option<Option<i32>>,
    /// When the child was seen ended, or the exit reported.
    ended_at: Option<Instant>,
    /// File requests waiting for the UI thread's buffers ([`Self::serve_fs`]).
    fs_queue: Vec<(Value, FsCall)>,
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
            exit_code: None,
            ended_at: None,
            fs_queue: Vec::new(),
        };
        // Files go through the editor's buffers; no terminals yet.
        let request = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(
                ClientCapabilities::new()
                    .fs(FileSystemCapabilities::new().read_text_file(true).write_text_file(true))
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
                Incoming::OrphanError(error) => self.on_orphan_error(error),
                Incoming::Closed => self.on_closed(now),
            }
        }
        if let Some((deadline, method)) = self.deadline
            && now >= deadline
        {
            self.fail(format!("the agent did not answer `{method}` within 30 s"));
        }
        self.watch_process(now);
    }

    /// Something time-based is pending (a setup timeout, an exit settling),
    /// so the UI should keep repainting even without messages.
    pub fn wants_repaint(&self, now: Instant) -> bool {
        self.deadline.is_some()
            || (self.process.is_some()
                && self.ended_at.is_some_and(|at| now.duration_since(at) < EXIT_SETTLE))
    }

    /// Starts a turn; `false` unless the session is ready.
    #[cfg(test)]
    pub fn send_prompt(&mut self, text: &str, now: Instant) -> bool {
        self.send_prompt_with(&[], text, now)
    }

    /// Like [`Self::send_prompt`], with chips attached as context blocks.
    pub fn send_prompt_with(&mut self, chips: &[Chip], text: &str, now: Instant) -> bool {
        let (Phase::Ready, Some(session_id)) = (&self.phase, &self.session_id) else {
            return false;
        };
        let request =
            PromptRequest::new(session_id.clone(), build_blocks(chips, text, self.embedded_context));
        if !chips.is_empty() {
            self.transcript.entries.push(Entry::Attached(
                chips
                    .iter()
                    .map(|c| Attached { label: c.label(), path: c.path.clone(), line: c.line() })
                    .collect(),
            ));
        }
        if !text.is_empty() {
            self.transcript.push_user(text);
        }
        self.send(Pending::Prompt, "session/prompt", params(request), now);
        self.phase = Phase::Prompting;
        true
    }

    /// Asks the agent to stop the running turn. Pending permission requests
    /// are answered `cancelled` first, as the protocol requires; the turn
    /// ends when the agent answers the prompt (with `cancelled`).
    pub fn cancel(&mut self) {
        self.cancel_permissions(true);
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

    /// Answers a pending permission card with one of its options. Does
    /// nothing when the card is gone or already answered.
    pub fn answer_permission(&mut self, rpc_id: &Value, option_id: &str) {
        let Some(card) = self.transcript.pending_permission_mut(rpc_id) else { return };
        let Some(choice) = card.options.iter().find(|o| o.id == option_id) else { return };
        card.state = PermissionState::Chosen(choice.name.clone());
        let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id.to_owned()));
        self.conn.respond(rpc_id.clone(), Ok(params(RequestPermissionResponse::new(outcome))));
    }

    /// Answers the file requests that arrived since the last call, from the
    /// editor's `buffers`. Returns the paths the agent wrote (now unsaved in
    /// their tabs). Call on the UI thread after [`Self::poll`].
    pub fn serve_fs(&mut self, buffers: &mut dyn Buffers) -> Vec<PathBuf> {
        let mut written = Vec::new();
        for (id, call) in std::mem::take(&mut self.fs_queue) {
            let (result, path) = fs::handle(&self.cwd, buffers, &call);
            written.extend(path);
            self.conn.respond(id, result);
        }
        written
    }

    /// Whether file requests wait for [`Self::serve_fs`].
    pub fn has_fs_requests(&self) -> bool {
        !self.fs_queue.is_empty()
    }

    /// Ends every pending permission card as cancelled; `respond` also tells
    /// the agent (not when it is gone).
    fn cancel_permissions(&mut self, respond: bool) {
        let mut ids = Vec::new();
        for entry in &mut self.transcript.entries {
            if let Entry::Permission(card) = entry
                && card.state == PermissionState::Pending
            {
                card.state = PermissionState::Cancelled;
                ids.push(card.rpc_id.clone());
            }
        }
        if respond {
            for id in ids {
                let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
                self.conn.respond(id, Ok(params(response)));
            }
        }
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
        if matches!(pending, Pending::Initialize | Pending::NewSession) {
            self.deadline = Some((now + SETUP_TIMEOUT, method));
        }
    }

    fn request_session(&mut self, now: Instant) {
        self.phase = Phase::CreatingSession;
        // Updates for the old session, still streaming after a cancel,
        // must not land in the new transcript.
        self.session_id = None;
        let request = NewSessionRequest::new(self.cwd.clone());
        self.send(Pending::NewSession, "session/new", params(request), now);
    }

    /// The agent failed or exited; only Restart helps.
    pub fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Failed(_) | Phase::Exited { .. })
    }

    fn fail(&mut self, message: String) {
        self.cancel_permissions(false);
        self.phase = Phase::Failed(message);
        self.pending.clear();
        self.fs_queue.clear();
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
                        self.cancel_permissions(true);
                        self.transcript.notice(Notice::Error(error.message));
                        self.phase = Phase::Ready;
                    }
                    // A failed sign-in (browser closed, wrong account): let
                    // the user try again.
                    Pending::Authenticate if !self.auth_methods.is_empty() => {
                        self.transcript.notice(Notice::Error(error.message));
                        self.phase = Phase::NeedsAuth(self.auth_methods.clone());
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
                // A request left open when the turn ended can no longer be
                // granted.
                self.cancel_permissions(true);
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

    fn on_request(&mut self, id: Value, method: &str, request_params: Value) {
        match method {
            "session/request_permission" => self.on_permission_request(id, request_params),
            _ => match FsCall::parse(method, request_params) {
                Some(Ok(call)) => self.fs_queue.push((id, call)),
                Some(Err(error)) => self.conn.respond(id, Err(error)),
                None => self.conn.respond(id, Err(RpcError::method_not_found(method))),
            },
        }
    }

    /// Shows the request as a card; the user's click answers it later. A
    /// request for another session, or one that cannot be read, is
    /// answered `cancelled` at once.
    fn on_permission_request(&mut self, id: Value, request_params: Value) {
        let request = serde_json::from_value::<RequestPermissionRequest>(request_params)
            .ok()
            .filter(|r| self.session_id.as_ref() == Some(&r.session_id) && !r.options.is_empty());
        let Some(request) = request else {
            let response = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
            self.conn.respond(id, Ok(params(response)));
            return;
        };
        self.transcript.update_tool_call(&request.tool_call);
        let tool_call_id = request.tool_call.tool_call_id.to_string();
        let title = self.transcript.tool_call(&tool_call_id).map(|c| c.title.clone()).unwrap_or_default();
        self.transcript.entries.push(Entry::Permission(PermissionCard {
            rpc_id: id,
            tool_call_id,
            title,
            options: request
                .options
                .iter()
                .map(|o| PermissionChoice { id: o.option_id.to_string(), name: o.name.clone(), kind: o.kind })
                .collect(),
            state: PermissionState::Pending,
        }));
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

    /// An error the agent could not tie to a request: it ends whatever
    /// Red is waiting for.
    fn on_orphan_error(&mut self, error: RpcError) {
        if self.is_finished() {
            return;
        }
        if self.phase == Phase::Prompting {
            self.pending.retain(|_, pending| *pending != Pending::Prompt);
            self.cancel_permissions(true);
            self.transcript.notice(Notice::Error(error.message));
            self.phase = Phase::Ready;
        } else if self.deadline.is_some() {
            self.fail(error.message);
        }
    }

    fn on_closed(&mut self, now: Instant) {
        if self.is_finished() {
            return;
        }
        // Never wait here: the code and late stderr lines are filled in by
        // `watch_process` on later polls.
        let stderr = self.stderr_tail(EXIT_STDERR_LINES);
        self.cancel_permissions(false);
        self.pending.clear();
        self.fs_queue.clear();
        self.deadline = None;
        self.ended_at.get_or_insert(now);
        self.phase = Phase::Exited { code: self.exit_code.flatten(), stderr };
    }

    /// Notices the child ending without blocking: reports the exit when a
    /// process it started keeps stdout open, and fills in the exit code and
    /// the last stderr lines after an exit.
    fn watch_process(&mut self, now: Instant) {
        let Some(process) = &mut self.process else { return };
        if self.exit_code.is_none()
            && let Some(code) = process.try_exit()
        {
            self.exit_code = Some(code);
            self.ended_at.get_or_insert(now);
        }
        if !self.is_finished()
            && self.ended_at.is_some_and(|at| now.duration_since(at) >= CHILD_EXIT_GRACE)
        {
            self.on_closed(now);
        }
        let settling = self.ended_at.is_some_and(|at| now.duration_since(at) < EXIT_SETTLE);
        if settling && matches!(self.phase, Phase::Exited { .. }) {
            let latest = (self.exit_code.flatten(), self.stderr_tail(EXIT_STDERR_LINES));
            if let Phase::Exited { code, stderr } = &mut self.phase {
                (*code, *stderr) = latest;
            }
        }
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
        // A `terminal` method is run by the user in a terminal, never sent
        // to `authenticate`; only agent-handled methods become buttons.
        .filter(|method| method.get("type").and_then(Value::as_str).is_none_or(|kind| kind == "agent"))
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
    /// Tells the agent to stop a running turn. Best effort: the process is
    /// killed right after, when the fields drop.
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::fake_agent::{self, FakeOptions};
    use crate::acp::model::{Entry, Notice, ToolContent};
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

    fn echoed_blocks(client: &mut Client, chips: &[Chip]) -> Value {
        assert!(client.send_prompt_with(chips, "blocks", Instant::now()));
        pump(client, |c| c.phase == Phase::Ready);
        let Some(Entry::AgentText(echo)) =
            client.transcript.entries.iter().rev().find(|e| matches!(e, Entry::AgentText(_)))
        else {
            panic!("no echo")
        };
        serde_json::from_str(echo).unwrap()
    }

    #[test]
    fn test_prompt_sends_chips_as_embedded_context_blocks() {
        let mut client = ready(FakeOptions::default());
        let chips = [Chip::file("/work/a.rs".into()), Chip::selection("/work/b.rs".into(), 2, 3, "x\ny".into())];
        let blocks = echoed_blocks(&mut client, &chips);
        assert_eq!(blocks[0]["type"], "resource_link");
        assert_eq!(blocks[1]["type"], "resource");
        assert_eq!(blocks[1]["resource"]["uri"], "file:///work/b.rs#L2-3");
        assert_eq!(blocks[2]["text"], "blocks");
        assert_eq!(
            client.transcript.entries[0],
            Entry::Attached(vec![
                Attached { label: "a.rs".into(), path: "/work/a.rs".into(), line: None },
                Attached { label: "b.rs:2-3".into(), path: "/work/b.rs".into(), line: Some(2) },
            ])
        );
        assert_eq!(client.transcript.entries[1], Entry::User("blocks".into()));
    }

    #[test]
    fn test_selection_is_inlined_when_the_agent_takes_no_embedded_context() {
        let mut client = ready(FakeOptions::default());
        client.embedded_context = false;
        let blocks = echoed_blocks(&mut client, &[Chip::selection("/work/b.rs".into(), 2, 3, "x\ny".into())]);
        assert_eq!(blocks[0]["type"], "text");
        assert!(blocks[0]["text"].as_str().unwrap().contains("```\nx\ny\n```"));
    }

    #[test]
    fn test_cancel_ends_the_turn_as_cancelled() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("wait", Instant::now());
        client.cancel();
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert_eq!(client.transcript.entries.last(), Some(&Entry::TurnEnd("cancelled".into())));
    }

    fn pending_card(client: &Client) -> PermissionCard {
        client.transcript.pending_permissions().next().cloned().expect("a pending card")
    }

    #[test]
    fn test_permission_card_waits_then_answers_with_the_chosen_option() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("permission", Instant::now());
        pump(&mut client, |c| c.transcript.pending_permissions().next().is_some());
        let card = pending_card(&client);
        assert_eq!(card.title, "Edit main.rs");
        assert_eq!(card.options.len(), 4);
        assert_eq!(client.phase, Phase::Prompting, "the turn waits for the user");
        client.answer_permission(&card.rpc_id, "allow_always");
        pump(&mut client, |c| c.phase == Phase::Ready);
        let entries = &client.transcript.entries;
        assert!(entries.contains(&Entry::AgentText("selected:allow_always".into())), "{entries:?}");
        assert!(entries.iter().any(|e| matches!(e,
            Entry::Permission(c) if c.state == PermissionState::Chosen("Always allow".into()))));
        // A second click on the same card does nothing.
        client.answer_permission(&card.rpc_id, "reject");
    }

    #[test]
    fn test_stop_answers_pending_permissions_cancelled() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("permission", Instant::now());
        pump(&mut client, |c| c.transcript.pending_permissions().next().is_some());
        client.cancel();
        assert!(client.transcript.pending_permissions().next().is_none());
        pump(&mut client, |c| c.phase == Phase::Ready);
        let entries = &client.transcript.entries;
        assert!(entries.contains(&Entry::AgentText("cancelled".into())), "{entries:?}");
        assert!(entries.iter().any(|e| matches!(e,
            Entry::Permission(c) if c.state == PermissionState::Cancelled)));
    }

    #[test]
    fn test_new_session_cancels_pending_permissions() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("permission", Instant::now());
        pump(&mut client, |c| c.transcript.pending_permissions().next().is_some());
        client.new_session(Instant::now());
        assert!(client.transcript.entries.is_empty());
        pump(&mut client, |c| c.phase == Phase::Ready);
    }

    #[test]
    fn test_tool_calls_and_plan_stream_into_the_model() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("tools", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        let call = client.transcript.tool_call("t1").expect("tool call");
        assert_eq!(call.status, agent_client_protocol_schema::v1::ToolCallStatus::Completed);
        assert_eq!(call.title, "Edit main.rs");
        assert_eq!(call.locations.len(), 1);
        assert!(matches!(call.content.as_slice(), [ToolContent::Diff { .. }]));
        assert_eq!(client.transcript.plan.len(), 2);
        assert_eq!(client.transcript.entries.len(), 3, "the prompt, one card, the closing text");
    }

    /// Runs one prompt, serving file requests from `buffers`; returns the
    /// agent's echo of Red's answer and the paths written.
    fn fs_round_trip(
        client: &mut Client,
        buffers: &mut crate::acp::fs::tests::MemBuffers,
        text: &str,
    ) -> (Value, Vec<PathBuf>) {
        client.send_prompt(text, Instant::now());
        let mut written = Vec::new();
        let started = Instant::now();
        while client.phase != Phase::Ready {
            assert!(started.elapsed() < Duration::from_secs(5), "stuck in {:?}", client.phase);
            client.poll(Instant::now());
            written.extend(client.serve_fs(buffers));
            thread::sleep(Duration::from_millis(5));
        }
        let Some(Entry::AgentText(echo)) = client.transcript.entries.iter().rev().find(|e| matches!(e, Entry::AgentText(_)))
        else {
            panic!("no echo: {:?}", client.transcript.entries)
        };
        (serde_json::from_str(echo).unwrap(), written)
    }

    fn fs_client(root: &std::path::Path) -> Client {
        let mut client = Client::new(
            fake_agent::connect(FakeOptions::default()),
            None,
            root.to_path_buf(),
            Instant::now(),
        );
        pump(&mut client, |c| c.phase == Phase::Ready);
        client
    }

    #[test]
    fn test_fs_read_round_trip_uses_buffer_text_and_range() {
        let root = crate::acp::fs::tests::temp_dir("client_read");
        std::fs::write(root.join("a.txt"), "disk 1\ndisk 2\n").unwrap();
        std::fs::write(root.join("b.txt"), "disk\n").unwrap();
        let mut buffers = crate::acp::fs::tests::MemBuffers::default();
        buffers.open.insert(root.join("b.txt"), "tab 1\ntab 2\ntab 3\n".into());
        let mut client = fs_client(&root);
        let request = |path: &std::path::Path, extra: &str| {
            format!(r#"rpc fs/read_text_file {{"path": {:?}{extra}}}"#, path.to_str().unwrap())
        };
        let (echo, _) = fs_round_trip(&mut client, &mut buffers, &request(&root.join("a.txt"), ""));
        assert_eq!(echo["result"]["content"], "disk 1\ndisk 2\n");
        let (echo, _) = fs_round_trip(&mut client, &mut buffers, &request(&root.join("b.txt"), r#", "line": 2, "limit": 1"#));
        assert_eq!(echo["result"]["content"], "tab 2\n");
        let (echo, _) = fs_round_trip(&mut client, &mut buffers, &request(&root.join("nope.txt"), ""));
        assert_eq!(echo["error"]["code"], -32002);
        let (echo, _) = fs_round_trip(&mut client, &mut buffers, r#"rpc fs/read_text_file {"path": "rel.txt"}"#);
        assert_eq!(echo["error"]["code"], -32602);
        let (echo, _) = fs_round_trip(&mut client, &mut buffers, r#"rpc fs/read_text_file {"nope": 1}"#);
        assert_eq!(echo["error"]["code"], -32602, "unparsable params are answered without the UI");
    }

    #[test]
    fn test_fs_write_round_trip_edits_the_buffer_only() {
        let root = crate::acp::fs::tests::temp_dir("client_write");
        std::fs::write(root.join("a.txt"), "old\n").unwrap();
        let mut buffers = crate::acp::fs::tests::MemBuffers::default();
        let mut client = fs_client(&root);
        let text = format!(
            r#"rpc fs/write_text_file {{"path": {:?}, "content": "new\n"}}"#,
            root.join("a.txt").to_str().unwrap()
        );
        let (echo, written) = fs_round_trip(&mut client, &mut buffers, &text);
        assert_eq!(echo["result"], serde_json::json!({}));
        assert_eq!(written, vec![root.join("a.txt")]);
        assert_eq!(buffers.open[&root.join("a.txt")], "new\n");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "old\n");
        let (echo, written) = fs_round_trip(
            &mut client,
            &mut buffers,
            r#"rpc fs/write_text_file {"path": "/etc/passwd", "content": "x"}"#,
        );
        assert_eq!(echo["error"]["code"], -32602);
        assert!(written.is_empty());
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
        // The fake streamed a chunk for the old session before answering.
        assert!(client.transcript.entries.is_empty(), "{:?}", client.transcript.entries);
        client.send_prompt("hi", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert_eq!(client.transcript.entries.last(), Some(&Entry::AgentText("Hello world".into())));
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
    fn test_failed_sign_in_offers_the_choices_again() {
        let mut client = start(FakeOptions { require_auth: true, ..FakeOptions::default() });
        pump(&mut client, |c| matches!(c.phase, Phase::NeedsAuth(_)));
        client.authenticate("bad", Instant::now());
        pump(&mut client, |c| matches!(c.phase, Phase::NeedsAuth(_)));
        assert!(matches!(client.transcript.entries.last(), Some(Entry::Notice(Notice::Error(_)))));
    }

    #[test]
    fn test_orphan_error_ends_the_turn() {
        let mut client = ready(FakeOptions::default());
        client.send_prompt("orphan", Instant::now());
        pump(&mut client, |c| c.phase == Phase::Ready);
        assert!(matches!(client.transcript.entries.last(), Some(Entry::Notice(Notice::Error(_)))));
    }

    #[cfg(unix)]
    #[test]
    fn test_real_process_exit_reports_code_and_stderr() {
        let args = ["-c".to_owned(), "echo oops >&2; exit 3".to_owned()];
        let (conn, process) =
            AgentProcess::spawn("sh", &args, std::path::Path::new("."), std::sync::Arc::new(|| {})).unwrap();
        let mut client = Client::new(conn, Some(process), PathBuf::from("/work"), Instant::now());
        pump(&mut client, |c| matches!(&c.phase, Phase::Exited { code: Some(_), stderr } if !stderr.is_empty()));
        assert_eq!(client.phase, Phase::Exited { code: Some(3), stderr: vec!["oops".to_owned()] });
    }

    #[test]
    fn test_setup_times_out() {
        let mut client = start(FakeOptions { silent: true, ..FakeOptions::default() });
        assert!(client.wants_repaint(Instant::now()));
        client.poll(Instant::now() + SETUP_TIMEOUT + Duration::from_secs(1));
        let Phase::Failed(message) = &client.phase else { panic!("{:?}", client.phase) };
        assert!(message.contains("initialize"), "{message}");
        assert!(!client.wants_repaint(Instant::now()));
    }
}
