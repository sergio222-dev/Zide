# Configurable agent command (sandboxed Claude)
Objective: run Claude through a user-configured command (e.g. a bubblewrap wrapper) instead of `claude` from `PATH`, editable from a UI.
Branch: feat/agent-command. Delivery: commit per task, one PR. Runner: `cargo test --workspace`.

## Specs
S1. Config file `~/.config/zide/settings.json` (assumption: global; per-project override not requested) in the shape the user gave:
```
"agent_servers": {
  "Agente aislado": {
    "command": "/home/vos/bin/agente-sandbox.sh",
    "args": ["mi-agente", "--acp"]
  }
}
```
Zide speaks Claude's stream-json, not ACP: `args` are a prefix placed before Claude's own arguments (`command args... -p --verbose ...`), so for the sandbox script they are `["claude"]`, not `--acp`. Only the first entry is used for now ("en un futuro se podra configurar varios agentes pero por ahora encaremos con esto"). Other keys in the file are preserved on save.
S2. No entry (or no file) = today's behavior: `claude` found on `PATH` or the usual places.
S3. Every Claude invocation goes through the configured command: chat sessions, `--version` (install check), `agents`, `rm`.
S4. Optional `config_dir`: where the agent keeps its config and sessions (inside the sandbox, the fake home's `.claude`). Session history and Claude defaults are read from it instead of `CLAUDE_CONFIG_DIR` / `~/.claude`.
S5. "me gustaria que esa config tenga una UI para configurar" — a "Configure Agent" command opens a dialog with Name, Command, Args and Config dir; Save writes the file; a Test button runs `--version` through the command and shows the result; clearing Command goes back to the default.
S6. After saving, the install check runs again and new chats use the new command; running chats keep their process.
S7. "se pued elegir que agente/config usar?" / "si, ademas cuando configuras un agente que haya una lista de los agentes y poder borrar configs" — several entries; `"agent": "<name>"` picks the active one (absent: the first, as S1; `null`: claude from PATH). The dialog lists the agents and the PATH default, edits the selected one, adds new ones, deletes one, and makes one active. Supersedes S1's "only the first entry".

## Tasks
- [x] T1 (S1-S4) zide-claude: launcher (command + prefix args + config_dir), used by session/cli/agents/history/defaults; tests. Route: inline. 3835160
- [x] T2 (S1,S2) zide-ui: settings file load/save (preserving other keys), applied at startup; tests. Route: inline. da121a9
- [x] T3 (S5,S6) zide-ui: "Configure Agent" dialog, Test, re-check on save; README. Route: inline. 79e0a01 (dialog rendering not exercised: no GUI run)
- [x] T4 (S7) settings: active agent, list, delete; dialog with the list; README. Route: inline.

## Log
L1 (2026-11-11) "nueva feature, se quiere agregar la posibilidad de ejecutar claude/el agente con un comando especifico, esto se debe a que se va a usar un .sh con bubblewrap para aislar el agente, entonces el IDE debe soportar configurar como se llama al agente de claude, actualmente es solo el comando claude desde el PATH, se espera una configuracion como: "agent_servers": { "Agente aislado": { "command": "/home/vos/bin/agente-sandbox.sh", "args": ["mi-agente", "--acp"] } } o una interfaz visual, en un futuro se podra configurar varios agentes pero por ahora encaremos con esto."
L2 Explained: Zide is stream-json, not ACP; args are a prefix (`["claude"]`); fake HOME hides history -> `config_dir`. ACP backend (Claude via claude-agent-acp, Copilot `--acp --stdio`) deferred to a later feature behind an `Agent` port.
L3 "si, me gustaria que esa config tenga una UI para configurar" — read as: configurable command first (ACP later), with a UI.
L4 (2026-11-11) T1-T3 done. RED observed per task (compile failures on missing launcher/settings/form fns), GREEN: cargo test --workspace all pass, clippy clean. No new crates in Cargo.lock (shlex already transitive). Dialog not run in a live window.
L5 (2026-11-11) "se pued elegir que agente/config usar?" -> "si, ademas cuando configuras un agente que haya una lista de los agentes y poder borrar configs". Switch Agent command (offered as optional) left out.
L6 (2026-11-11) T4 done: tests and code written together (RED not observed separately for T4). cargo test --workspace green, clippy clean. Dialog still not run in a live window.
