# Claude Code chat view (Alt+2) — phases 0 and 1
Objective: a full-screen Claude Code chat inside zide, driven by the user's own `claude` CLI over stream-json. This document covers phase 0 (land the spike) and phase 1 (event model, transcript, status; no UI).
Branch: feat/claude-chat (from main, spike commits cherry-picked). Delivery: ask-on-risk, ~400 changed lines per slice. Runner: `cargo test -p zide-claude`.

## Specs
S1. Phase 0: the spike (`spike/claude-code-session`, crate `zide-claude`) lands on main's line, with a minimum CLI version pinned and the live tests ignored by default.
S2. Phase 1: `Event` carries what a chat needs: tool calls with id and input, tool results, thinking, usage, and the end of the process. Unknown fields and events are tolerated, never fatal.
S3. Phase 1: a pure `Transcript` (no UI, no process) built from events: user, assistant text, thinking, tool call with its result, permission, question, interruption and error blocks. Tool calls repeated by the stream are not duplicated.
S4. Phase 1: `Status` derived from the transcript, with the four states of the UI: "gris no activo, verde esperando input, naranja pensando", "azul cuando necesita al usuario". A pending permission or question wins over a running turn; the end of the turn clears anything still pending.
S5. Tests use fixtures captured from the real CLI (2.1.319), trimmed and without personal data.
S6. No vim in the chat input box, the chat is a full-screen view on Alt+2 and the status bar shows the Claude icon with the colors above (later phases, out of scope here).

## Tasks
- [x] T1 (S1) Spike cherry-picked onto feat/claude-chat (72b0468, 4e7c532), `cli` module (`installed()`, `MIN_VERSION` = 2.1.319, the release the fixture came from), live test stays `#[ignore]`. 8c451a8.
- [x] T2 (S2, S5) Rich `Event` and `parse_events` (one line can yield several events; subagent messages left out), fixture `tests/fixtures/read-turn.jsonl` from a real turn. 8c451a8.
- [x] T3 (S3, S4) `Transcript` and `Status`, with the resolution of permissions and questions. 8c451a8.
- [x] T4 (S2) Session emits `Event::Exit` when the process ends; example updated. 8c451a8. Not observed: the live test was not re-run (it costs about $1); Exit and the new parser are covered by unit tests and the real-capture fixture only.

Route declaration: all inline. No Writer trigger: one coherent unit in one crate, each task depends on the previous API.

## Log
L1 (2026-11-07) "dale, arranquemos con la fase 0 y 1"
L2 (2026-11-07) Interface clarification: "no sera un panel que se abre a un costado, al darle al atajo alt+2 se mostrara el chat full screen con el historial, en el status bar se mostrara que se esta usando claude con un icono de claude si esta disponible, y con los colores, gris no activo, verde esperando input, naranja pensando. No se usara vim en el input box de el chat." Then: "azul cuando necesita al usuario".
L3 (2026-11-07) Real capture (claude 2.1.319, one turn with a Read tool call): `assistant` lines carry the tool_use block (id, name, input) and the final text; the tool output comes as a `user` line with a `tool_result` block (content is a string); `rate_limit_event` and `system status/informational/hook_*` lines are noise; `result` has `total_cost_usd`, `usage`, `stop_reason`. The user's heavy config makes a bare turn cost ~$0.59, so live tests stay ignored.
L4 (2026-11-07) Verification: `cargo test -p zide-claude` 38 unit + 3 fixture tests pass, `cargo clippy -p zide-claude --all-targets` clean, `cargo test --workspace` green. The pre-commit hook runs `cargo fmt --all --check`; the cherry-picked spike was unformatted, so fmt landed with the feature commit. Assumption to confirm in phase 4: `control_request` carries a `tool_use_id` that could link a permission to its tool call; not parsed yet.
