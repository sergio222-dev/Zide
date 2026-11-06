# Incremental parsing for big files
Objective: editing a 100k-line file must not block the UI. Branch perf/incremental-parse (from main). Delivery: single PR to main. Test runner: cargo test.

## Specs
- S1 "hay un pequeño delay cuando edito ... me preocupa este rendimiento en un archivo gigante, se puede optimizar?" Measured (release, 100k-line .rs): highlight 576 ms, fold_regions 503 ms, extract_nodes 324 ms, rebuild() 1.5 s per edit. Parse alone 372 ms, incremental reparse 15 ms.
- S2 The viewer parses a file once and reparses incrementally after each edit; highlight, folds and outline share that one tree.
- S3 Output is identical to a full parse: highlight spans, fold regions and outline nodes do not change (tests compare incremental vs full).
- S4 The compiled highlight query is built once, not per call.
- S5 Undo/redo and every other buffer mutation are covered (one mutation point: Buffer::apply).

## Tasks
- T1 (S5) Buffer records byte/point edits, `take_edits()` — inline — done
- T2 (S2,S3,S4) zide-syntax `Syntax` (parse, update, highlight, fold_regions on a tree), cached query — inline — done
- T3 (S2,S3) zide-stubs `extract_nodes` over an existing tree — inline — done
- T4 (S2) Viewer keeps a `Syntax` and uses it in rebuild — inline — done
- T5 (S1) Measure again; decide what is next — inline — done: 1.5 s -> ~0.6 s per keystroke

## Log
L1 (2026-11-06) user: "se puede optimizar?" then "dale" to: parse once, then incremental parsing, in a separate branch.
L2 Result (release, 100k-line .rs, per keystroke): rebuild 1.5 s -> ~0.6 s. No parse left; what remains walks the whole tree: highlight 227 ms, fold_regions 188 ms, build_lines 125 ms, outline 15 ms. Next PR: highlight only the visible range, lazy lines, folds only over changed ranges.
