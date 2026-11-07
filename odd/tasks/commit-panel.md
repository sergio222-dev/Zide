# Commit panel (IntelliJ-style)
Objective: left side panel to review changed files, tick them and commit (or commit and push). Branch: feat/commit-panel. Delivery: ask-on-risk. Runner: cargo test.

## Specs
- S1: "moveremos el chat a alt+3 y con alt+2 abriremos este panel" — Alt+3 toggles the Claude chat (was Alt+2); Alt+2 opens the commit panel.
- S2: "integracion de un panel lateral para commitear" — side panel titled "Commit" beside the editor.
- S3: Changes shown as a tree (`Changes` root with file count, nested directories, per-file icon), each node has a checkbox; a directory checkbox ticks/unticks its files; counts like "2 files".
- S4: Under the tree: an `Amend` checkbox and "N modified" label.
- S5: A multi-line commit message input below, then `Commit` and `Commit and Push…` buttons.
- S6 (assumed): Commit commits only ticked files; Push never forced.

## Tasks
- T1 (S3,S6) zide_git: changed files (status), commit selected paths, amend — inline — done (3ba3a00)
- T2 (S2-S5) ui/commit.rs panel state + render — inline — done
- T3 (S1) Alt+2 / Alt+3, action, Actions-list entry, README — inline — done

## Log
L1 (2026-11-07): "mejor no, haremos una integracion de un panel lateral para commitear, ejemplo [Image #11] moveremos el chat a alt+3 y con alt+2 abriremos este panel"
L2 (2026-11-07): cargo clippy clean; zide-git 21 tests, zide-ui 156 tests pass; app runs 8s without panic. Not observed: the panel on screen, Alt+2/Alt+3, a real commit from the UI (needs a person at the keyboard).
