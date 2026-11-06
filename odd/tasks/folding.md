# Code folding in the editor
Objective: fold and unfold regions of the file in the viewer, IntelliJ style.
Branch: feat/folding (from feat/next-method; PRs stacked to main, one per slice). Delivery: stacked-to-main, ~400 changed lines per PR. Runner: `cargo test -p zide-syntax -p zide-ui`.

## Specs
S1. "Todo lo de IntelliJ" — the foldable regions are blocks, declarations, imports and long comments (not only declarations). Only regions that span 2 or more lines.
S2. Languages: the ones with a grammar in zide-syntax. Rust, JS, TS, TSX first; Go and Python only if their node kinds are cheap to add (assumption).
S3. A collapsed region shows its first line followed by a placeholder (assumption: `{...}` style, a dimmed `...`), and its other lines disappear from the list: line numbers keep the real numbers, so they skip.
S4. The cursor never rests on a hidden line (assumption). Moving down or up over a collapsed region lands on its first line / the line after it; a search, a jump (`n`, `/`, Alt+Down/Up, go to line) that lands inside a collapsed region expands it.
S5. Keys (assumption, IntelliJ plus Vim): `za` toggle, `zo` open, `zc` close, `zM` close all, `zR` open all; Ctrl+minus collapses and Ctrl+plus expands the region at the cursor, Ctrl+Shift+minus / plus all of them.
S6. A chevron in the gutter marks the first line of each foldable region and clicking it toggles it (assumption).
S7. After an edit the regions are recomputed; a collapsed region stays collapsed while a region still starts on the same line (assumption).

## Tasks
- [x] T1 (S1, S2) zide-syntax: `fold_regions(language, source) -> Vec<FoldRegion { start_line, end_line, kind }>` for Rust, JS, TS, TSX; tests per kind. Route: inline. PR A.
- [x] T2 (S3, S4) viewer: map between list rows and buffer lines (visible lines), behavior-preserving with no folds; every row/line site uses it. Route: inline. PR B (with T3).
- [ ] T3 (S3, S4, S5, S7) viewer fold state, collapse/expand, placeholder rendering, cursor snapping, auto-unfold, `z` keys. Route: inline. PR B.
- [ ] T4 (S5, S6) gutter chevrons with click, IntelliJ keys (Ctrl+minus/plus), go/py regions if cheap. Route: inline. PR C.

Route declaration: all inline. No Writer trigger: T3 needs both T1's API and T2's mapping, so there are no independent units to launch together.

## Log
L1 (2026-11-06) "sigamos ahora con el folding"
L2 (2026-11-06) Asked what to fold; answer: "Todo lo de IntelliJ".
L3 (2026-11-06) Explorer map: viewer rows equal buffer lines at ~18 sites (uniform_list count at viewer.rs:1067, scroll_to_item at 256/530/853/955, view_metrics, glide_to, place_cursor_line, scroll_half_page, cursor_line and Selection::on_line in render, gutter number `(row + 1)`); all vim movement runs through one `vim.handle` call (viewer.rs:525) so a post-key snap away from hidden lines works; Ctrl-d/u bypass it via `move_to_line`; tree-sitter grammars live in zide-syntax (`grammars()`, `LanguageId`), each feature reparses on its own, so `fold_regions` goes there as a new `fold.rs`; render has no `cx` and the viewer has no click handling, so T4 needs a click closure plumbed in the way the tree does.
