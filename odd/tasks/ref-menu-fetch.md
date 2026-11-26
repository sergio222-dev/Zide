# ref-menu-fetch

Objective: Fetch action, right-click menu on branches/tags in the git log refs tree, push dialog that pushes with nothing outgoing.
Branch: feat/ref-menu-fetch (from main e9d1d0e+). Delivery: single-pr. Test runner: `cargo test --workspace` (pre-push hook runs fmt, clippy, tests).

## Specs

- S1 "agregar la accion fetch para actualizar las ramas": a Fetch action (Find Action) fetches every remote (branches and tags, pruned) and records which tags each remote has.
- S2 "en la ventana de git, al hacer click derecho sobre una rama/tag se mostrara el menu": right click on a branch or tag row in the git log refs tree opens a context menu.
- S3 A local branch or tag not on the remote yet (image 1) shows only "Push to origin" ("Show Diff with Working Tree" ignored).
- S4 "en una rama/tag que ya existe en el origen el menu mostrara como en la segunda imagen (ignorar showing diff with working tree)": Checkout, Merge '<ref>' into '<current>', Push to origin, Delete.
- S5 "push commit deberia dejar pushear igual, hoy cuando no hay nada que pushea no deja pushear": the push dialog's Push works with no outgoing commits.
- S6 "ademas se debe mostrar a donde se pushea (que remoto y rama)": the dialog shows `branch → remote : branch`.
- Assumption: remote branches (origin/x) offer Checkout and Merge only; the current branch offers no Checkout/Merge; Delete of a branch is `git branch -d` (git refuses unmerged), of a tag is local only (the remote keeps it).

## Tasks

- T1 S1,S3,S4 zide_git: fetch, remote_tags, delete_branch, delete_tag, push_tag + tests. Route: inline. Status: done 5f98ca3.
- T2 S1 Fetch action, remote tags kept per session. Route: inline. Status: done 1f7a4d3.
- T3 S2-S4 refs tree context menu. Route: inline. Status: done a9b426b.
- T4 S5,S6 push dialog. Route: inline. Status: done db65fb1 (S6 header already existed).

## Log

- L1 (2026-11-11) "biien ahora nuevo feature, agregar la accion fetch para actualizar las ramas, luego en la ventana de git, al hacer click derecho sobre una rama/tag se mostrara el menu como muestra la imagen, en una rama/tag que ya existe en el orgine el menu mostrara como en la segunda imagen (ignorar showing diff with working tree) push commit deberia dejar pushear igual, hoy cuando no hay nada que pushea no deja pushear ademas se debe mostrar a donde se pushea (que remoto y rama) como en la tercera imagen"
- L2 (2026-11-11) user confirmed: local-only ref → only "Push to origin"; on origin → image 2 menu. "por eso se quiere agregar fetch".
- L3 (2026-11-11) All tasks done inline (no writer trigger). Verification: git tests (fetch prunes + brings tags, push_tag/remote_tags/delete_tag, delete_branch refuses unmerged), session remote-tags round trip, ref_entries tests for S3/S4, push can_push test; fmt, clippy, workspace tests pass. UI wiring (right click, Fetch action) not driven in the real app. Next: try it in Zide, then PR.
