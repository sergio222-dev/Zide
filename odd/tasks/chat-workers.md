# Workers panel in the Claude chat
Objective: show the subagents (workers/writers) a Claude chat launches, live and read-only, in a panel at the bottom of the chat list.
Branch: feat/chat-workers, stacked on feat/acp-agent. Delivery: single-pr. Runner: `cargo test --workspace`.

## Specs
S1. "hoy en dia los writter o procesos que larga claude no se muestran, deberia haber en la lista de chats debajo de todo un panel que aparece cuando hay un worker/writer corriendo, no se puede chatear con ese worker" — a "Workers" section at the bottom of the chat list, shown only while a worker exists; one row per worker (description, subagent type, its current activity); no input to it.
S2. "me gustaria poder ver lo que va haciendo tambien" — clicking a worker shows, in the chat area, what it does as it happens: its text, thinking and tool calls with their results, read-only (no composer).
S3. "cuando un worker termino deberia borrarse de la lista" — a finished worker leaves the list. Accepted detail: if its view is open when it finishes, the view stays (marked finished) until the user goes back to a chat, then it is removed. Its result stays in the chat's `Agent` tool card, as today.
S4. Workers belong to the chat that launched them; when that chat's process exits or is stopped, its workers are dropped. Claude chats only (ACP exposes no subagents).
S5. The main conversation is unchanged: worker messages never enter the chat's own transcript.

## Protocol (Claude Code 2.1.322, captured; fixture crates/claude/tests/fixtures/workers.jsonl, sanitized)
- `system/task_started` {task_id, tool_use_id, description, subagent_type}: a worker starts; `tool_use_id` is the main chat's `Agent` tool call.
- Worker messages: `assistant`/`user` lines with `parent_tool_use_id` = that tool_use_id; full messages (no stream deltas): text, thinking, tool_use, tool_result blocks.
- `system/task_progress` {tool_use_id, description = current activity, usage}.
- `system/task_notification` {task_id, tool_use_id, status: "completed" (others possible: failed/killed/stopped)}: the worker ended.
- In `-p` mode every Agent call runs in the background (`is_backgrounded: true`, tool_result "Async agent launched").

## Tasks
- [x] T1 (S1-S5) Parse worker events in zide-claude (started / message events / progress / ended) without touching the main transcript; per-chat workers with their own Transcript in the UI; Workers section in the chat list; read-only worker view; removal rules. Route: delegated (context backstop: parent context is long).

## Log
L1 (2026-11-11) "hoy en dia los writter o procesos que larga claude no se muestran, deberia haber en la lista de chats debajo de todo un panel que aparece cuando hay un worker/writer corriendo, no se puede chatear con ese worker"
L2 Found: event.rs:169 drops every line with parent_tool_use_id; the list's "Background" section is `claude agents` (separate sessions), not in-chat workers.
L3 "me gustaria poder ver lo que va haciendo tambien, pero cuando un worker termino deberia borrarse de la lista, es posible eso?" -> proposed: live read-only view; removed when finished; an open view stays until the user leaves it -> "dale".
L4 Captured one real stream (haiku, no user settings, tools Agent+Read, scratch dir) and committed a sanitized fixture (hostname, paths, session ids replaced; capture deleted).
L5 (2026-11-11) T1 done by one writer: Event::{WorkerStarted, Worker, WorkerActivity, WorkerEnded} (Transcript ignores them), chat/workers.rs model, Workers section + read-only view (Esc/back), README. Parent check: 0 clippy warnings, workspace green (zide-claude 79 + 5 fixture tests, zide-ui 242). Parent fix: Ctrl+V ignored while a worker view is open. Not seen on screen. Open: a nested worker (Agent inside a worker) shows only if its task_started arrives.
