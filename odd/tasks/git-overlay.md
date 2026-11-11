# git-overlay

Objective: one `Overlay` enum in the Workspace for the floating git dialogs (branches, push, git op, conflicts + merge editor), replacing five `Option` fields kept exclusive by hand. Step 1 of the GitDialogs plan.
Branch: refactor/improvements. Delivery: single-pr. Test runner: `cargo test --workspace` (+ fmt, clippy).

## Specs

- S1 `Workspace.overlay: Overlay` replaces `branches`, `push`, `git_op`, `conflicts`, `merge`; `Overlay::Conflicts(popup, Option<MergeEditor>)` holds the merge editor over the conflicts it resolves. At most one dialog is open by construction.
- S2 Key routing (capture chain), the chat interceptor gate, SwitchTab and the overlay render read the one field.
- S3 Opening the search popup closes any git dialog, the branch switcher included (today it stays open) — accepted by the user ("dale" to "abrir el buscador cerraría ramas").
- S4 While the merge editor is open, no other dialog takes its place (today the opener dropped the conflicts under it and the editor stayed on top, keeping the keys): nothing resolved in it is lost. Conflicts reported meanwhile update the list under it.
- S5 `git_task` and the conflicts' `proceed` share one read-back of what git changed (commit panel, gutter, branch, log, files) instead of two copies.
- S6 Everything else unchanged: opening, closing, keys and focus of each dialog, git operations and their toasts, conflicts → merge editor → resolve → continue.

## Tasks

- T1 S1-S6. Route: inline (one behavior unit, no writer trigger). Status: done in "refactor(ui): one Overlay enum for the git dialogs"; manual app check passed.

## Log

- L1 (2026-11-11) "podemos seguir con lo de gitDialog", then "dale" to step 1 (`enum Overlay`, accepting that the search popup closes the branch switcher).
- L2 (2026-11-11) Explorer map: exclusion by hand in open_branches (main.rs), toggle_push, toggle_git_op, show_conflicts, open_popup (which misses `branches`); nothing clears `merge`, it stacks over conflicts and reads `self.conflicts`; `proceed` duplicates git_task's completion block.
- L3 (2026-11-11) Implemented inline: crates/ui/src/overlay.rs (`Overlay` + accessors, close_x only closes its own variant, `merging`, `clear`), openers guard on merging, main.rs gate/keys/render as one match, `Workspace::after_git` shared by git_task and proceed. Risk item 6 -> independent verifier: no blockers, S1-S6 pass. Advisory fixed: conflicts reported while the merge editor is open now update the list under it (HEAD replaced the popup); imports tidied. Not done (new behavior, not asked): a toast when an opener is refused while merging. build, clippy --all-targets, fmt --check, workspace tests pass.
- L4 (2026-11-11) User checked it in the app: "funciona bien". Feature closed.
