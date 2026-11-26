# Themes and a theme selector (#166)
Objective: switchable color themes (built-in + user JSON) picked from Find Action with live preview.
Branch: feat/themes (from main). Delivery: stacked-to-main, one PR per task as the issue proposes. Runner: `cargo test -p zide-ui`.

## Specs
S1. "la idea es que se pueda ir a la seleccion de temas en ctr+a, search command, y de ahi mostrar otro popup donde estan los temas" — a **Theme** command in Find Action (Ctrl+Shift+A) opens a list of themes (a popup tab, like Recent projects).
S2. "al desplazarse con las flechas se cambia el tema automaticamente" — moving the selection with the arrows applies that theme at once. Issue #166: Esc restores the previous one, Enter keeps it, saved as `"theme": "<name>"` in `settings.json`.
S3. "puedes usar git worktree para paralelizar el diseño de varios temas" — several built-in themes are shipped (assumption: the current One Dark–like palette as default, plus a light theme and a few more dark ones).
S4. Issue #166: palette becomes a `Theme` value read at render through a global; current colors are the default theme; syntax colors and stray literals (block caret, minimap white) move into it; gpui-kit tokens and `ThemeMode` follow the theme.
S5. Issue #166: themes as data — built-ins as JSON, user themes in `~/.config/zide/themes/*.json`; a file may give only some colors (missing ones fall back to the default); invalid entries are reported (notification) and skipped, never crash.

## Tasks
- [x] T1 (S4) `Theme` struct + global `theme::current()` + mechanical migration of every `theme::X` site, syntax colors and stray literals; no visible change. Route: inline. PR A.
- [x] T2 (S3, S5) theme files: bundled JSON themes, user dir loading, partial fallback, validation reports. Route: delegated (parallel with T3, worktree). PR B. Commit 812d126.
- [x] T3 (S1, S2) selector: Theme action, popup tab with live preview, Esc restore, Enter save `"theme"`, apply at startup, gpui-kit mode switch. Route: delegated (parallel with T2, worktree). PR C. Commit d6fc627 (+ docs).
- [x] T4 (S3) six popular built-in ports (Catppuccin Mocha, Gruvbox Dark, Tokyo Night, Nord, Dracula, Solarized Light), each a JSON with the full key set, registered after the Zide ones. Route: delegated (Hermes kanban, one card per theme + integration card). Branch feat/themes-more (from feat/themes-selector).

Route declaration: T1 inline (everything depends on it). T2 and T3 are independent units with disjoint surfaces after T1 (Writer trigger a, parallel).

## Log
L1 (2026-11-26) "vamos a encarar el #166 que es agregar temas a zide, puedes usar git worktree para paralelizar el diseño de varios temas o trabajo, la idea es que se pueda ir a la seleccion de temas en ctr+a, search command, y de ahi mostrar otro popup donde estan los temas, al desplazarse con las flechas se cambia el tema automaticamente"
L2 (2026-11-26) Map: 492 `theme::X` uses in 32 files; viewer lines store `Tint` (not colors) so syntax colors resolve at paint; Projects tab (main.rs Tab::Projects) is the model for a list opened from Actions; settings live in crates/ui/src/settings.rs; `apply_palette` in main.rs maps to gpui-kit.
L3 (2026-11-26) T1 committed d43a769; shared API stub `all()/find()/problems()` 51153fa so T2/T3 build in parallel. User: "la idea es abrir agentes para que si tienen consultas lo pueda administrar en hedr" — T2 and T3 run as interactive Claude agents under herdr-core (`hc run --bg`, names themes-files / themes-selector) in worktrees ../zide-worktrees/themes-files and ../zide-worktrees/themes-selector. README/docs left to the parent (docs/readme-pages branch splits the README).
L4 (2026-11-26) T2 done by agent themes-files (812d126): 5 built-ins (Zide Dark, Light, Ember, Fjord, High Contrast), user dir, partial fallback, problems as '<file>: <issue>', docs/themes.md. T3 done by agent themes-selector (d6fc627): Tab::Themes from the Theme action, live preview, Esc/close restore, Enter saves "theme", apply_palette switches kit ThemeMode and keeps font_family/margins. Stack: feat/themes -> feat/themes-files -> feat/themes-selector. fmt, clippy -D warnings, cargo test --workspace (36 suites) green. Pending: manual app check.
L5 (2026-11-26) Manual app check passed: "se ve bien, puedes abrir los PR?". Stack rebased on main after #172-#174 (README split, restore, git env fix); README links docs/themes.md.
L6 (2026-11-26) T4: six theme cards on Hermes kanban, each adding one crates/ui/themes/<slug>.json in its own worktree (shared CARGO_TARGET_DIR); integration card cherry-picked them onto feat/themes-more, registered them in BUILT_IN and updated the hardcoded name lists in the theme tests and docs/themes.md. Pending: manual app check.
