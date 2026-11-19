//! The conversation as plain data: what the panel draws: text, thoughts,
//! notices, turn ends, tool calls (merged by id), permission cards and the
//! latest plan.

use std::path::PathBuf;

use agent_client_protocol_schema::v1::{
    ContentBlock, PermissionOptionKind, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
    StopReason, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolKind,
};
use serde_json::Value;

/// Something Red itself tells the user inside the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
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
    ToolCall(ToolCallView),
    Permission(PermissionCard),
}

/// What a tool call produced, reduced to what the panel draws.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    /// Markdown or plain text.
    Text(String),
    /// A proposed or applied edit; `old` is `None` for a new file.
    Diff { path: PathBuf, old: Option<String>, new: String },
    /// An embedded terminal, by id (shown live in stage 4).
    Terminal(String),
}

/// A file (and line, 1-based) a tool call touches.
#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// One tool call, kept up to date by `tool_call` and `tool_call_update`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallView {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    pub content: Vec<ToolContent>,
    pub locations: Vec<Location>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl ToolCallView {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: String::new(),
            kind: ToolKind::Other,
            status: ToolCallStatus::Pending,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        }
    }
}

/// One option the agent offers for a permission request.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionChoice {
    pub id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PermissionState {
    /// Waiting for the user; the agent's request is still unanswered.
    Pending,
    /// The user picked this option (its name).
    Chosen(String),
    /// Answered `cancelled`: Stop, a new session, or the agent went away.
    Cancelled,
}

/// A `session/request_permission` shown as a card.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionCard {
    /// The JSON-RPC id to answer with.
    pub rpc_id: Value,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<PermissionChoice>,
    pub state: PermissionState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub content: String,
    pub status: PlanEntryStatus,
    pub priority: PlanEntryPriority,
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
    /// The agent's latest plan (each `plan` update replaces it).
    pub plan: Vec<PlanItem>,
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
            SessionUpdate::ToolCall(call) => self.start_tool_call(call),
            SessionUpdate::ToolCallUpdate(update) => self.update_tool_call(update),
            SessionUpdate::Plan(plan) => {
                self.plan = plan
                    .entries
                    .iter()
                    .map(|e| PlanItem {
                        content: e.content.clone(),
                        status: e.status.clone(),
                        priority: e.priority.clone(),
                    })
                    .collect();
            }
            _ => return false,
        }
        true
    }

    /// A `tool_call`: adds a card, or replaces the one with the same id.
    fn start_tool_call(&mut self, call: &ToolCall) {
        let mut view = ToolCallView::new(&call.tool_call_id.to_string());
        view.title = call.title.clone();
        view.kind = call.kind;
        view.status = call.status;
        view.content = convert_content(&call.content);
        view.locations = convert_locations(&call.locations);
        view.raw_input = call.raw_input.clone();
        view.raw_output = call.raw_output.clone();
        match self.tool_call_mut(&view.id) {
            Some(existing) => *existing = view,
            None => self.entries.push(Entry::ToolCall(view)),
        }
    }

    /// A `tool_call_update`: only the fields it carries change. An update
    /// for a call never announced creates the card.
    pub fn update_tool_call(&mut self, update: &ToolCallUpdate) {
        let id = update.tool_call_id.to_string();
        if self.tool_call_mut(&id).is_none() {
            self.entries.push(Entry::ToolCall(ToolCallView::new(&id)));
        }
        let view = self.tool_call_mut(&id).expect("just added");
        let fields = &update.fields;
        if let Some(title) = &fields.title {
            view.title = title.clone();
        }
        if let Some(kind) = &fields.kind {
            view.kind = *kind;
        }
        if let Some(status) = &fields.status {
            view.status = *status;
        }
        if let Some(content) = &fields.content {
            view.content = convert_content(content);
        }
        if let Some(locations) = &fields.locations {
            view.locations = convert_locations(locations);
        }
        if let Some(input) = &fields.raw_input {
            view.raw_input = Some(input.clone());
        }
        if let Some(output) = &fields.raw_output {
            view.raw_output = Some(output.clone());
        }
    }

    pub fn tool_call(&self, id: &str) -> Option<&ToolCallView> {
        self.entries.iter().find_map(|e| match e {
            Entry::ToolCall(call) if call.id == id => Some(call),
            _ => None,
        })
    }

    fn tool_call_mut(&mut self, id: &str) -> Option<&mut ToolCallView> {
        self.entries.iter_mut().find_map(|e| match e {
            Entry::ToolCall(call) if call.id == id => Some(call),
            _ => None,
        })
    }

    /// The permission card for this request id, if it is still pending.
    pub fn pending_permission_mut(&mut self, rpc_id: &Value) -> Option<&mut PermissionCard> {
        self.entries.iter_mut().find_map(|e| match e {
            Entry::Permission(card) if card.state == PermissionState::Pending && &card.rpc_id == rpc_id => {
                Some(card)
            }
            _ => None,
        })
    }

    /// Cards still waiting for the user.
    #[cfg(test)]
    pub fn pending_permissions(&self) -> impl Iterator<Item = &PermissionCard> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Permission(card) if card.state == PermissionState::Pending => Some(card),
            _ => None,
        })
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

fn convert_content(content: &[ToolCallContent]) -> Vec<ToolContent> {
    content
        .iter()
        .filter_map(|item| match item {
            ToolCallContent::Content(c) => match &c.content {
                ContentBlock::Text(text) => Some(ToolContent::Text(text.text.clone())),
                ContentBlock::ResourceLink(link) => {
                    Some(ToolContent::Text(format!("[{}]({})", link.name, link.uri)))
                }
                _ => None,
            },
            ToolCallContent::Diff(diff) => Some(ToolContent::Diff {
                path: diff.path.clone(),
                old: diff.old_text.clone(),
                new: diff.new_text.clone(),
            }),
            ToolCallContent::Terminal(t) => Some(ToolContent::Terminal(t.terminal_id.to_string())),
            _ => None,
        })
        .collect()
}

fn convert_locations(locations: &[ToolCallLocation]) -> Vec<Location> {
    locations.iter().map(|l| Location { path: l.path.clone(), line: l.line }).collect()
}

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
        let other = update(json!({"sessionUpdate": "available_commands_update", "availableCommands": []}));
        assert!(!t.apply(&other));
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
        t.notice(Notice::Error("boom".into()));
        assert_eq!(t.entries, vec![Entry::Notice(Notice::Error("boom".into()))]);
    }

    fn start_call() -> SessionUpdate {
        update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit main.rs",
            "kind": "edit", "status": "pending",
            "locations": [{"path": "/w/main.rs", "line": 3}],
            "rawInput": {"path": "/w/main.rs"}}))
    }

    #[test]
    fn test_tool_call_adds_a_card() {
        let mut t = Transcript::default();
        assert!(t.apply(&start_call()));
        let call = t.tool_call("t1").unwrap();
        assert_eq!(call.title, "Edit main.rs");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(call.locations, vec![Location { path: "/w/main.rs".into(), line: Some(3) }]);
        assert_eq!(call.raw_input, Some(json!({"path": "/w/main.rs"})));
    }

    #[test]
    fn test_tool_call_update_changes_only_the_fields_it_carries() {
        let mut t = Transcript::default();
        t.apply(&start_call());
        t.apply(&update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"})));
        t.apply(&update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "content": [
                {"type": "content", "content": {"type": "text", "text": "done"}},
                {"type": "diff", "path": "/w/main.rs", "oldText": "a\n", "newText": "b\n"}
            ],
            "rawOutput": {"ok": true}})));
        assert_eq!(t.entries.len(), 1, "merged into the same card");
        let call = t.tool_call("t1").unwrap();
        assert_eq!(call.status, ToolCallStatus::InProgress);
        assert_eq!(call.title, "Edit main.rs");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.locations.len(), 1);
        assert_eq!(
            call.content,
            vec![
                ToolContent::Text("done".into()),
                ToolContent::Diff { path: "/w/main.rs".into(), old: Some("a\n".into()), new: "b\n".into() }
            ]
        );
        assert_eq!(call.raw_output, Some(json!({"ok": true})));
        t.apply(&update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed", "title": "Edited main.rs"})));
        let call = t.tool_call("t1").unwrap();
        assert_eq!((call.status, call.title.as_str()), (ToolCallStatus::Completed, "Edited main.rs"));
        assert_eq!(call.content.len(), 2, "content kept when the update has none");
    }

    #[test]
    fn test_update_for_unknown_call_creates_it_and_text_splits_around_cards() {
        let mut t = Transcript::default();
        t.apply(&agent_chunk("before"));
        t.apply(&update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "x", "title": "Run"})));
        t.apply(&agent_chunk("after"));
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.tool_call("x").unwrap().title, "Run");
    }

    #[test]
    fn test_plan_update_replaces_the_plan() {
        let mut t = Transcript::default();
        t.apply(&update(json!({"sessionUpdate": "plan", "entries": [
            {"content": "one", "priority": "high", "status": "pending"},
            {"content": "two", "priority": "low", "status": "pending"}]})));
        assert_eq!(t.plan.len(), 2);
        t.apply(&update(json!({"sessionUpdate": "plan", "entries": [
            {"content": "one", "priority": "high", "status": "completed"}]})));
        assert_eq!(
            t.plan,
            vec![PlanItem {
                content: "one".into(),
                status: PlanEntryStatus::Completed,
                priority: PlanEntryPriority::High
            }]
        );
        assert!(t.entries.is_empty(), "the plan is not a transcript entry");
    }
}
