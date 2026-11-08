# Git log tab (IntelliJ-style)
Objective: Alt+4 tab with the commit history (merges included), a branch selector and a search box. Branch: feat/git-log (from main). Delivery: ask-on-risk. Runner: cargo test.

## Specs
- S1: "hacer una tab nueva, alt+4, la que muestra el historial de commits, merge, con seleccion de ramas y buscador" — Alt+4 opens/closes a new full-area tab (like the chat), listed in Find Action.
- S2: Left, the refs tree as in the reference image: `HEAD (Current Branch)`, `Local`, `Remote` (remotes as folders, `/` prefixes as folders). Selecting a ref filters the history to it; selecting it again goes back to every branch.
- S3: Center, the commit list with a graph (lanes, merge joins), the subject, ref labels (branches/tags) on the row, author and date. Merge commits are listed.
- S4: A search box, placeholder "Text or hash": filters by message text, author or a hash prefix.
- S5: Right, the files the selected commit changed (grouped by folder, colored by kind) and under them its details (subject, body, hash, author, email, date, the refs it is on). Double click on a file opens its diff with the existing commit diff.
- S6: Option 1 chosen by the user: "vamos con la 1 entonces" — User, Date and Paths filters are NOT in this version.
- S7 (assumed): history loads in pages (the list asks for more at its end); the graph is drawn only while unfiltered (a filtered list is flat).

## Tasks
- T1 (S3,S4) zide_git::log: entries (hash, parents, refs, author, date, subject), filter (all / ref / query), graph lanes — inline — done
- T2 (S5) zide_git: details of a commit (refs it is on) — inline — done (log::branches_with, with T1)
- T3 (S2-S5) ui/gitlog.rs: state, render, search input, keys — inline — done
- T4 (S1) Alt+4, action, Find Action entry, README — inline — done

## Log
L1 (2026-11-08): "siguiente feature grande: hacer una tab nueva, alt+4, la que muestra el historial de commits, merge, con seleccion de ramas y buscador" (with a screenshot of IntelliJ's Git tool window)
L2 (2026-11-08): scope question answered: "vamos con la 1 entonces" (core: refs tree + graph list + search + details with files; filters later). The engram mirror is pending (the engram MCP failed to connect this session).
L3 (2026-11-08): "es agregar esos filtros" — the User, Date and Paths filters left out by S6 are now in (feat/git-log-filters): user list from `git shortlog`, date presets (24h, 7d, 30d, year), paths box; no custom date range.
L4 (2026-11-08): "podriamos hacer un selector de fechas al hacer click en select cuando se hace click en Date?" (IntelliJ's After/Before pickers) — the Date button now opens a popup with an After and a Before date picker instead of the presets; git gets `--since`/`--until` covering the whole first and last day.
L5 (2026-11-08): "quiero los presets de 24 horas, 7 dias y last 30 days, hay que agregarle uno mas que diga select... y eso abre el selector de fechas" — the Date button lists Any time, the three presets and Select...; the last one opens the After/Before pickers.
