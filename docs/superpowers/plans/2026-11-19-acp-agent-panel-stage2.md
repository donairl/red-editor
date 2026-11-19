# Agent panel, stage 2: tool calls, permission, plan

Spec: `docs/superpowers/specs/2026-11-18-acp-agent-panel-design.md`
(Tool calls, Plan, session/request_permission). Capabilities still
advertise no fs and no terminal.

## Tasks

1. **Model** (`src/acp/model.rs`): `Entry::ToolCall(ToolCallView)` merged by
   id from `tool_call` / `tool_call_update` (title, kind, status, content,
   locations, raw input/output; an update for an unknown id creates the
   card); `Transcript.plan` replaced by each `plan` update;
   `Entry::Permission(PermissionCard)` with `PermissionState`
   (Pending / Chosen / Cancelled). Drop `Notice::PermissionDeclined`.
   Tests: card creation, partial merge, unknown id, plan replacement.
2. **Fake agent** (`src/acp/fake_agent.rs`): prompt `"tools"` (tool call,
   updates, diff, plan) and a `"permission"` request with four options that
   echoes `selected:<id>` or `cancelled`.
3. **Client** (`src/acp/client.rs`): `session/request_permission` becomes a
   pending card, never blocking; `answer_permission(rpc_id, option_id)`
   responds `Selected`; `cancel()` (Stop, New Session via `cancel`, turn
   end, orphan error) answers pending cards `Cancelled` before
   `session/cancel`; failure / exit marks them cancelled without writing.
   Tests: chosen option round trip, Stop, New Session, tool calls + plan
   through the fake.
4. **Cards** (`src/agent_cards.rs`): tool-call card (status icon, kind,
   title, location links, collapsible text / diff / terminal id / raw
   JSON), permission card (option buttons coloured by kind), plan
   checklist. `unified()` turns old/new into `-`/`+` lines with context via
   `diff_view::rows`, coloured with the theme's gutter colours. Tests for
   `unified` and the location label.
5. **Panel and app** (`src/agent_panel.rs`): `CardAction::{Open,
   Permission}` applied after the frame; `open_agent_location` opens the
   file and jumps to the line through `open_file_in_tab` + `goto_line`.
6. **Strings**: new `agent.*` keys, English text in all five language files
   (the key-parity test requires it; translations come in stage 6).
7. **Docs**: tick stage 2 and the spec drift item in `todo.md`.

## Verify

`cargo build`, `cargo test`, `cargo clippy -- -D warnings`.
