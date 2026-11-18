# Agent panel (Agent Client Protocol) — design

An **Agent panel** on the right of the window that talks to any coding
agent speaking the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP): Red is the *client*, the agent is a subprocess it spawns, and the
two exchange JSON-RPC 2.0 messages over the agent's stdin/stdout, one JSON
object per line.

The user types the agent's command once in Settings (for example
`npx @zed-industries/claude-code-acp` or `gemini --experimental-acp`); Red
starts it, opens a session in the workspace folder and streams the turns.
The agent can read and write files through Red's open buffers, ask for
permission, run commands in terminals Red owns, and receive files and
selections from the editor as context.

Out of scope: several agents or sessions at once, saving sessions across
restarts (`session/load`), agent modes and config options, slash-command
menus (`available_commands_update` is ignored), MCP server configuration,
images and audio in prompts, the unstable protocol features.

## Approach

- Protocol types come from the official schema crate,
  `agent-client-protocol-schema = { version = "1.18", default-features = false }`
  (serde types only; no `schemars`, no async runtime). Unknown
  `SessionUpdate` variants and fields are tolerated.
- Transport is written in Red: one reader thread, one writer thread,
  `std::sync::mpsc` channels to the UI thread and `ctx.request_repaint()`
  on every incoming message — the same shape as `git_worker.rs`. No tokio.
- The UI thread never blocks. It drains the event channel once per frame,
  updates the transcript model, answers the agent's requests that need the
  editor (`fs/*`), and queues replies to the writer thread.

## Modules

```
src/acp/
  mod.rs        AgentEvent (agent → UI), Outgoing (UI → agent), re-exports
  transport.rs  spawn, reader/writer threads, line framing, request ids,
                pending-response map, stderr ring buffer, kill on Drop
  client.rs     session state machine: initialize → (authenticate) →
                session/new → prompt turns; routes the agent's requests
                (fs/*, terminal/*, session/request_permission)
  fs.rs         workspace-bounded read/write handlers (pure functions over
                a trait the app implements for its tabs)
  terminals.rs  terminal/create|output|wait_for_exit|kill|release
  model.rs      the transcript, pure data:
                Entry::{User, AgentText, Thought, ToolCall, Permission,
                TurnEnd}, plus the current Plan
src/agent_panel.rs   the right SidePanel: plan, transcript, context chips,
                     prompt input, Send/Stop, cards
src/agent_settings.rs AgentSettings { command: String }, own storage key
```

`AgentSettings` lives apart from `EditorSettings` because that one is
`Copy`. It is stored under the eframe key `agent_settings` as JSON and is
forgiving like `session.rs`: missing or unreadable text gives an empty
command. The Settings window gets an **Agent** section with one text
field, *Agent command*. The command is split into program and arguments
by a small shell-words splitter (spaces, single and double quotes,
backslash escapes); no shell is involved.

## Wiring into the app

- `activity_bar.rs`: a fifth item, **Agent**, that toggles the panel.
- `Ctrl+Shift+A` toggles the panel; View → Agent and the command palette
  entries *Agent: Toggle Panel*, *Agent: New Session*, *Agent: Add
  Selection to Agent*, *Agent: Save All Agent Edits*.
- The panel is an `egui::SidePanel::right`, resizable, default width
  380 px, open state kept in storage.
- The session's `cwd` is the open workspace folder (ACP wants an absolute
  path). With no folder open the panel says so and offers Open Folder.
- The agent process starts when the panel is first opened with a command
  set, not at app start. *New Session* sends `session/cancel` if a turn is
  running, then `session/new` on the same process. Restart kills and
  respawns the process.
- Closing the app cancels the turn and kills the agent and every terminal
  (`Drop` on the transport and terminal handles).

## Protocol behaviour

### initialize

Red sends protocol version `1` and these client capabilities:
`fs.read_text_file = true`, `fs.write_text_file = true`, `terminal = true`,
plus `client_info` (`red`, crate version). It keeps the agent's
`prompt_capabilities` (for `embedded_context`) and `auth_methods`.

If the agent answers with another protocol version Red cannot speak, the
panel shows both versions and stops.

### authenticate

If `session/new` fails with the auth-required error (`-32000`) and the agent listed
`auth_methods`, the panel shows one button per method (name and
description). A click sends `authenticate` with that method id, then
retries `session/new`.

### Prompt turns

- Enter sends, Shift+Enter inserts a newline. While a turn runs, Send
  becomes **Stop**, which sends `session/cancel`.
- `agent_message_chunk` appends to the current `AgentText` entry,
  rendered as Markdown with `egui_commonmark`. `agent_thought_chunk`
  appends to a `Thought` entry, collapsed by default and dimmed.
  `user_message_chunk` is shown as a user entry.
- When `session/prompt` returns, a `TurnEnd` entry is added only if the
  stop reason is not `end_turn` (`max_tokens`, `max_turn_requests`,
  `refusal`, `cancelled`).

### Tool calls

- A `tool_call` adds a card: icon per `kind` (read, edit, delete, move,
  search, execute, think, fetch, other), title, status (pending,
  in progress, completed, failed). `tool_call_update` merges into the card
  with the same `tool_call_id`; only the fields present change.
- The card expands to its content: text (Markdown), **diff** (old/new
  text, rendered as unified hunks with `similar`, coloured like the diff
  view), and an embedded **terminal** (see below).
- Each entry in `locations` is a link that opens the file at that line.

### Plan

A `plan` update replaces the whole plan. It is drawn above the transcript
as a checklist: ☐ pending, ◐ in progress, ☑ completed. The plan update
variants behind unstable features are not enabled.

### session/request_permission

- Adds a `Permission` card under the tool call it names, with one button
  per option, using the agent's own labels. `allow_once` / `allow_always`
  are drawn as accept buttons, `reject_once` / `reject_always` as reject
  buttons.
- A click answers with `Selected { option_id }` and the card shows the
  choice. Red keeps no "always" rules of its own; the agent does.
- Stop, New Session, Restart and the agent exiting answer every pending
  permission request with `Cancelled`, as the spec requires.

### fs/read_text_file

Answered on the UI thread.

- The path must be absolute and, after canonicalisation, inside the
  workspace folder. Anything else (relative paths, `..` escapes, symlinks
  that point outside) is answered with a JSON-RPC error. This bounds only
  Red's own channel; the agent's built-in tools are its business.
- If a tab holds the file, the reply is the buffer text, unsaved edits
  included; otherwise the file is read from disk.
- `line` (1-based) and `limit` select a range of lines.

### fs/write_text_file

- Same workspace check.
- If a tab holds the file, its whole text is replaced as **one undo
  step** and the tab becomes dirty. Otherwise a new tab opens for that
  path with the new text, dirty. **The disk is not touched**; the user
  reviews and saves.
- The tool-call card that caused the write shows an **unsaved** badge and
  two buttons: **Diff** (the existing diff view, buffer vs HEAD) and
  **Save**.
- The panel header shows **Save All Agent Edits** while any file the
  agent wrote is dirty.
- Known consequence, stated in the panel tooltip and the README: a
  command the agent runs afterwards (a build, a test) reads the old file
  from disk until the user saves.
- If the file also changed on disk, the existing conflict prompt handles
  it when the user saves.

### terminal/*

- `terminal/create` runs `command` with `args`, extra `env` and `cwd`
  (default: the workspace) through `std::process::Command`, stdout and
  stderr merged into one buffer, stdin closed. Not a PTY: the agent needs
  no interaction, and pipes work the same on every platform.
- The buffer keeps at most `output_byte_limit` bytes, dropping from the
  front at a UTF-8 character boundary and setting `truncated`.
- `terminal/output` returns the buffer, `truncated` and the exit status
  once known. `terminal/wait_for_exit` is answered when a watcher thread
  sees the process end. `terminal/kill` kills the process (the kill helper
  in `process.rs` becomes `pub(crate)`). `terminal/release` kills it if it
  still runs and drops the buffer.
- A tool call whose content names a terminal shows its output live in the
  card: monospace, scrollable, at most 12 rows tall.

### Context from the editor

- An **@** button by the input, or typing `@`, opens a file picker using
  Quick Open's fuzzy matcher from `palette.rs`. The pick becomes a chip
  above the input.
- *Add Selection to Agent* (palette and the editor's context menu) turns
  the current selection into a chip such as `app.rs:12-30`.
- On send, a file chip becomes `ContentBlock::ResourceLink` with a
  `file://` URI. A selection chip becomes an embedded
  `ContentBlock::Resource` (the selected text, URI with the line range)
  when the agent's `prompt_capabilities.embedded_context` is true, and
  otherwise plain text: the path and range, then the text in a code
  fence. The typed text comes last as `ContentBlock::Text`.

## Error handling

- **No command set, or the program is not found**: the panel shows the
  message and an **Open Settings** button. No modal.
- **The agent exits** (stdout EOF or a reaped child): `AgentEvent::Exited
  { code, stderr_tail }`. The panel shows "Agent exited (code N)", the
  last 20 stderr lines and **Restart**. Pending requests fail, pending
  permissions are cancelled, terminals are killed.
- **stderr** goes to a ring buffer (last 200 lines), shown on exit or with
  *Show agent log* in the panel menu.
- **Invalid JSON lines** are logged and skipped. A request with an
  unknown method is answered `-32601 Method not found`; unknown
  notifications are ignored.
- **Timeouts**: 30 s for `initialize`, `authenticate` and `session/new`;
  none for `session/prompt` (the user has Stop).

## Testing

Inline `#[cfg(test)]` modules, as elsewhere in the crate, plus one
integration test.

- `transport.rs`: partial lines, several messages in one read, a UTF-8
  character split across reads, request/response id matching, error
  responses.
- `model.rs`: consecutive chunks merge, partial `tool_call_update` merge,
  plan replacement, cancel resolves pending permissions.
- `fs.rs`: workspace bounds (relative path, `..`, symlink outside),
  `line`/`limit`, buffer text wins over disk.
- `terminals.rs`: truncation at a UTF-8 boundary, exit status, kill.
- `agent_settings.rs`: storage round trip, shell-words splitting.
- **Fake agent**: `examples/fake_acp_agent.rs`, a scripted ACP agent
  (initialize, streamed chunks, a tool call with a diff, a permission
  request, `fs/write_text_file`, `terminal/create`). The integration test
  `tests/acp_fake_agent.rs` spawns it through the transport and client and
  checks the events and Red's replies. No real agent, no network.
- Manual check with `claude-code-acp` and `gemini --experimental-acp`,
  noted in `todo.md`.

## Stages

Each stage builds, passes its tests and is usable on its own.

1. **Transport and core chat** — schema crate, `transport.rs`,
   `client.rs`, `model.rs`, `agent_settings.rs` and the Settings field,
   the right panel, the activity-bar item, `Ctrl+Shift+A`, prompt /
   stream / Stop, authenticate, exit and Restart, the fake agent. In this
   stage capabilities advertise no fs and no terminal, and a permission
   request is answered `Cancelled` with a note in the panel.
2. **Tool calls, permission, plan** — cards, diff rendering, location
   links, permission cards.
3. **fs through the editor** — `fs.rs`, the fs capabilities, writes into
   buffers, unsaved badge, Diff / Save / Save All Agent Edits.
4. **Terminals for the agent** — `terminals.rs`, the terminal
   capability, live output in cards.
5. **Editor context** — `@` file chips, Add Selection to Agent,
   `ResourceLink` / `Resource` blocks.
6. **Polish** — the new strings in all five `assets/lang/*.json` (stages
   1–5 add English only), README (en, id), `todo.md`, release notes.
