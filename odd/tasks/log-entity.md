# log-entity

Objective: move the git log tab out of the `Workspace` god-struct into its own gpui `Entity<LogPanel>`, behavior unchanged.
Branch: refactor/improvements (rebased on origin/main). Delivery: single-pr. Test runner: `cargo test --workspace` (+ fmt, clippy).

## Specs

- S1 The log's loading, refs tree, filters, keys, scroll, view and context menu (open/close/keys) live in `impl LogPanel` (an `Entity`, `impl Render`), not `impl Workspace`: `Workspace.log: Option<Entity<LogPanel>>`.
- S2 The menu's actions on the repository (checkout, merge, push, delete, reset, cherry-pick, revert, new branch/tag, copy hash, show diff, file changes) stay with the Workspace, which owns `git_task`, `ask_name`, `switch_branch`, `show_commit_diff` and the toasts; the log asks for them through an event.
- S3 The log gets what it needs without copies kept in sync: `root` and the Workspace's focus handle (keys go back to the lists); the current branch a ref menu merges into is handed over by the Workspace when that menu opens.
- S4 Log → Workspace only through events (`cx.emit`): Hide (Esc), Menu action, Show diff, Ref menu.
- S5 Behavior unchanged: Alt+3, Esc, Tab, search and paths boxes, user/date filters, refs tree (enter, fold, HEAD line), commit/file panes, paging, context menus on commits, files and refs with every action, reload after git operations.

## Tasks

- T1 S1-S5 extract `LogPanel` into an Entity. Route: inline (one behavior unit, no writer trigger). Status: done in "refactor(ui): move the git log into its own LogPanel entity"; manual app check pending.

## Log

- L1 (2026-11-11) "sigue con gitlog" (after the chat extraction: "Los próximos candidatos son gitlog, con el mismo molde").
- L2 (2026-11-11) Coupling map: gitlog.rs `impl Workspace` (open/close/toggle/reload, load refs/log/files, filters, keys, scroll, view) uses log, center, focus, root, root_name, show_commit_diff, toggle_claude (close_tab). menu.rs `impl Workspace`: open menus (reads `branch` for "into") + actions using git_task (13), switch_branch, ask_name, show_commit_diff, notify. Inbound: open_git_log, close_tab, toggle_git_log, reload_log, log_tab, log_key, log_view.
- L3 (2026-11-11) Implemented inline: `Entity<LogPanel>` + `impl Render`; glue `impl Workspace` in gitlog.rs; menu actions stay `impl Workspace` (`run_menu_action`, `ref_action`, `confirm_reset`). Risk items 4 and 6 -> independent verifier: no blockers, S1-S5 pass. Advisory A1 (ref menu `into` read with the refs went stale after an external checkout with the tab open; HEAD read `Workspace.branch` at menu open) fixed for exact parity: right click emits `LogEvent::RefMenu`, the Workspace opens the menu with `branch.name`. A2/A3 applied (blank lines, main.rs render indentation, private fns, entity locals renamed so they do not read as `zide_git::log`). build, clippy --all-targets (0 warnings), fmt --check, workspace tests pass. Next: user checks S5 in the app.
