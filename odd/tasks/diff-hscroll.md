# diff-hscroll

Objective: synchronized horizontal scroll in the side-by-side diff dialog. Branches: zide `feat/diff-hscroll` (stacked on `feat/commit-discard`), gpui-kit fork `feat/diff-split-scroll` (from tag v0.9.1, local clone `../gpui-kit`). Delivery: single-pr per repo. Runner: `cargo test`.

## Specs

- S1: "cuando se visualizan los cambios que estos tengan un scroll horizontal, hoy en dia no tienen" — the diff dialog (double click on a commit panel file, or a push dialog file) scrolls horizontally instead of wrapping long lines.
- S2: "scroll horizontal sincronizado" — in Split mode both sides scroll by the same offset; each side keeps its half of the dialog and its line-number gutter fixed (user chose: patch gpui-kit's `Diff`, "Patch en gpui-kit (Recommended)").
- S3: Unified mode and soft-wrap behavior of gpui-kit's `Diff` stay as they are.
- S4: zide consumes the patched gpui-kit through `[patch.crates-io]` until upstream releases it. (Assumption: how the fork is published — local path vs pushed git fork — is the user's decision before the zide PR.)

## Tasks

- T1 [S2,S3] gpui-kit: per-column synchronized horizontal scroll in Split mode, with test. Route: inline (explorer mapped first: Mapping trigger, 9k-line module). Status: done, fork commit 229b814b (RED: canvas offset -120 != 0; GREEN: 762 component tests, clippy -D warnings clean; hit-test clipping mutation caught). Verifier blocker B1 (press on a short line scrolled out of sight picked the next line) fixed in fork commit after 229b814b, pinned by `a_short_line_scrolled_out_of_sight_still_takes_presses_on_its_row`.
- T2 [S1,S4] zide: `[patch.crates-io]` to the fork (local path `../gpui-kit`), soft wrap removed in `crates/ui/src/diff_view.rs`, README. Route: inline. Status: done locally (workspace tests + clippy green); manual app check pending (user); not pushable while the patch is a local path.

## Log

- L1 (2026-11-20, user): "ok, dejemos esto de de lado, ahora para el siguiente feature vamos a hacer que cuando se visualizan los cambios que estos tengan un scroll horizontal, hoy en dia no tienen, es posible agregar un scroll horizontal sincronizado para esto?"
- L2 (2026-11-20): finding — gpui-kit `Diff` in Split without soft-wrap scrolls the whole two-column canvas with one `ScrollHandle`; the left column leaves the screen. Options offered; user chose "Patch en gpui-kit (Recommended)".
- L3: read-only explorer mapping `crates/component/src/diff` for the design.
- L4 (2026-11-20): design — split columns keep half the viewport; max shift = (content_width - viewport)/2; `SplitScroll` implements `ScrollbarHandle` so `ScrollableMask`/`Scrollbar` reuse it; `#source` wrapped in an `overflow_hidden` clip with `left(offset)`; selection hit-testing uses run bounds intersected with the content mask.
- L5 (2026-11-20): independent verifier (Risk item 5): S3 PASS; S2 FAIL on B1 (run_at filtered hidden runs for line choice). Fix: visible bounds pick the side only, full bounds pick the line. Recheck: RED (Original,1) -> GREEN; 763 component tests, clippy clean. Advisories kept: 1px border not in code_width (pre-existing), single-column split files can over-scroll (pre-existing), no auto-scroll on drag to column edge (missing feature).
- L6 (2026-11-20, user): "no veo ningun scroll para poder hacer scroll horizontal en el diff de un commit" / "pero si apreto las flechas si se desplaza". Cause: `SplitScroll` took its viewport from the paint canvas, absolute without insets, laid out below the list (y=199 in a 200px host), so the horizontal scrollbar sat out of sight. Fix ef4c5b10: `SplitScroll` measures the body's `diff-horizontal` ScrollHandle bounds. Pinned by `the_split_scrollbar_sits_on_the_bottom_of_the_body` (RED offset 0 -> GREEN). Shift+wheel maps to horizontal on Linux (gpui wayland/x11).
