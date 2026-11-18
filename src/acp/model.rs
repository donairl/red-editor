//! The conversation as plain data: what the panel draws. Stage 1 keeps
//! text, thoughts, notices and turn ends; tool calls and the plan come in
//! stage 2.

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
