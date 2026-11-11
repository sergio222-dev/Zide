# ACP agents in the chat (Copilot first)
Objective: the chat can talk to an Agent Client Protocol agent (`copilot --acp --stdio`) as well as to Claude Code's stream-json, behind one agent port.
Branch: feat/acp-agent, stacked on feat/agent-command (PR #118). Delivery: single-pr ("con el primer PR completo"). Runner: `cargo test --workspace`.

## Specs
S1. "con el primer PR completo" — the accepted proposal: "el puerto `Agent` en el chat más el adaptador ACP, probado con Copilot", "sin `fs` ni `terminal`" (the client declares neither capability).
S2. Claude over stream-json stays the default and keeps every feature it has today; nothing changes for an agent without `protocol`.
S3. An agent entry takes `"protocol": "acp"` (absent or `"claude"`: stream-json), e.g. `"Copilot": { "command": "copilot", "args": ["--acp", "--stdio"], "protocol": "acp" }`. Configure Agent lets you pick the protocol.
S4. An ACP chat shows streamed text and thoughts, tool calls with their result, asks permission through the chat's existing permission card (`session/request_permission`), interrupts with Esc (`session/cancel`), and sends pasted images when the agent declares `promptCapabilities.image`.
S5. Claude-only features are not offered for an ACP chat: rate-limit and context gauges, the earlier-conversations picker and resume from `~/.claude`, background agents, model/effort switching.
S6. Test in Configure Agent, and the check that enables the chat, run the `initialize` handshake for an ACP agent (instead of `--version`).
S7. Assumption: an ACP process that exits starts a new session on the next message (no `session/load`); resume and `session/list` are a later PR.

## Tasks
- [x] T1 (S2) Port: `AgentSession` trait in zide-claude (send_with_images, interrupt, allow, deny, answer, set_model, set_effort); `Session` implements it; the chat holds `Box<dyn AgentSession>`; transcript treats a neutral cancel as interrupted. Route: inline. 9c9c21b (+425aa83 export command_for)
- [x] T2 (S1,S4,S6) zide-acp crate: newline JSON-RPC connection with agent->client requests, ACP client (initialize, session/new, session/prompt, session/cancel, permission replies), `session/update` -> `Event` mapping; unit tests on synthetic fixtures; live Copilot test `#[ignore]`. Route: delegated (context backstop, one writer). 8d318e2
- [x] T3 (S3,S5,S6) UI: protocol in settings and dialog; spawn per protocol; hide Claude-only features for ACP; ACP probe; each chat keeps its launcher+protocol; README. Route: inline. 007a8be

## Log
L1 (2026-11-11) "sobre el modo ACP, es posible hacerlo?" -> proposal (port + ACP adapter, Copilot as test agent, no fs/terminal) -> "podemos arrancar con esto" -> asked: full PR or spike -> "con el primer PR completo".
L2 Handshake probed live: Copilot CLI 1.0.108 declares loadSession, sessionCapabilities.list/close, promptCapabilities.image, models+modes+configOptions in session/new, available_commands_update. session/list returns sessions of other projects: never commit captured output; fixtures are synthetic.
L3 Mapping (explorer): Event -> Transcript::apply is protocol-neutral except "AskUserQuestion" and terminal_reason "aborted_streaming"; the chat calls Session only via spawn/send_with_images/interrupt/allow/deny/answer/set_model/set_effort; LSP Connection uses Content-Length framing and auto-answers server requests (not reusable as-is).
L4 (2026-11-11) T1-T3 done. Tests: zide-acp 36 (+1 live ignored), zide-claude 78, zide-ui 236, workspace green, clippy clean. Live Copilot test run once: passed (12 s). RED observed for transcript CANCELLED, settings protocol, protocol.rs mappings; client/e2e and UI wiring written with their code. Design: each Chat snapshots launcher+protocol at creation (Options.launcher) so switching agents never sends one protocol to the other's process.
L5 Risk: item 4 (concurrency in the ACP reader/worker threads). Independent verifier launched on the whole feature.
