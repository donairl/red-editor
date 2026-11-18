//! One ACP session, driven without blocking: the UI calls [`Client::poll`]
//! every frame and the client moves through [`Phase`]s as the agent answers.
//! initialize → (authenticate) → session/new → Ready ⇄ Prompting.

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
        if let Some((deadline, method)) = self.deadline
            && now >= deadline
        {
            self.fail(format!("the agent did not answer `{method}` within 30 s"));
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
