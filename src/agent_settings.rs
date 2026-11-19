//! The agent setting: the command that starts an ACP agent, typed once in
//! Settings and kept in eframe's storage under `agent_settings`.

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

/// A known agent: a button in Settings → Agent that fills in its command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentPreset {
    /// The button label, a product name (not translated).
    pub name: &'static str,
    /// The command it fills in. `npx -y` skips npm's install prompt, which
    /// would otherwise wait on the agent's stdin.
    pub command: &'static str,
}

/// The agents offered as buttons; anything else is typed by hand ("Others").
pub const PRESETS: [AgentPreset; 4] = [
    AgentPreset { name: "Claude", command: "npx -y @zed-industries/claude-code-acp" },
    AgentPreset { name: "Codex", command: "npx -y @zed-industries/codex-acp" },
    AgentPreset { name: "AntiGravity", command: "agy acp" },
    AgentPreset { name: "Hermes", command: "hermes acp" },
];

/// The preset whose command `command` is, word for word (extra spaces and
/// quoting don't matter); `None` for a hand-typed or empty command.
pub fn preset_of(command: &str) -> Option<usize> {
    let words = split_command(command).ok()?;
    PRESETS.iter().position(|preset| {
        split_command(preset.command).is_ok_and(|preset_words| preset_words == words)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Result<Vec<String>, CommandError> {
        split_command(text)
    }

    /// Each preset's own command is recognised as that preset.
    #[test]
    fn test_preset_of_finds_each_preset() {
        for (index, preset) in PRESETS.iter().enumerate() {
            assert_eq!(preset_of(preset.command), Some(index), "{}", preset.name);
        }
    }

    /// Spacing and quoting don't matter; other or empty commands are none.
    #[test]
    fn test_preset_of_compares_words() {
        assert_eq!(preset_of("  hermes   'acp' "), Some(3));
        assert_eq!(preset_of("hermes acp --verbose"), None);
        assert_eq!(preset_of("gemini --experimental-acp"), None);
        assert_eq!(preset_of(""), None);
        assert_eq!(preset_of("agy \"acp"), None);
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
