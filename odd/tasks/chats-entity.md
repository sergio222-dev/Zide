# chats-entity

Objective: move the Claude chat out of the `Workspace` god-struct into its own gpui `Entity<Chats>`, behavior unchanged.
Branch: refactor/improvements (rebased on origin/main). Delivery: single-pr. Test runner: `cargo test --workspace` (+ fmt, clippy).

## Specs

- S1 Chat logic lives in `impl Chats` (an `Entity`), not `impl Workspace`: `Workspace.claude: Option<Entity<Chats>>`, `Chats` implements `Render`.
- S2 `Chats` owns what it needs: `root`, `bare`, a copy of the file index and recent files (kept current by the Workspace where it writes them).
- S3 The Workspace keeps only the panel glue: which view fills the center, focus, autosave, the agents poll, forwarding chat keys from the keystroke interceptor.
- S4 Chat → Workspace goes through events (`cx.emit`), never a direct call into the Workspace, so no entity is updated while leased.
- S5 Behavior unchanged: Alt+2, Esc, New Chat, Resume Session, `@` file and `/` command menus, images paste, asks, model/effort pickers, status bar state, layout restore.

## Tasks

- T1 S1-S5 extract `Chats` into an Entity. Route: inline (one behavior unit, no writer trigger). Status: done in "refactor(ui): move the Claude chat into its own Chats entity"; manual app check pending.

## Log

- L1 (2026-11-11) "arranca con eso" (after: "El paso siguiente del plan es sacar `Chats` del `Workspace` y convertirlo en su propia `Entity`").
- L2 (2026-11-11) Explorer map: chat's non-chat deps are root, center, focus, claude_bare, index, recent, autosave(), notify(). Inbound: open_claude, toggle_claude, claude_new, claude_history, claude_key, claude_view, claude_conversation, restore_conversation, focus_active_chat, Chats::overall.
- L3 (2026-11-11) Implemented inline: `Entity<Chats>` + `impl Render`, glue `impl Workspace` in chat.rs, `ChatEvent {Hide, Info, Error}` via subscribe_in, observe for the status bar, `files`/`recent` synced in replace_index/show_file/creation, bare synced on toggle. Risk items 4 (entity update ordering) and 6 (no UI tests) -> independent verifier: no blockers, S1-S5 pass; advisories applied (visibility, docs, leftovers). build, clippy --all-targets (0 warnings), workspace tests pass. Next: user checks S5 in the app.
