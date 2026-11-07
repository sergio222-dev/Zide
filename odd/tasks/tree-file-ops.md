# Project tree file operations (m / d / y / p)
Objective: manage files from the project tree with Vim-style keys. Branch: feat/tree-file-ops (from feat/commit-panel). Delivery: ask-on-risk. Runner: cargo test.

## Specs
- S1: "m -> renombrar o mover una carpeta con el formato "dir/*/file" se abre un popup en el file explorer con un input y la dir absoluta en relacion al project, al darle enter se actualiza el nombre/mueve el archivo, esc lo cierra" — `m` on the selected row opens a popup with an input holding its path relative to the project; Enter renames/moves (missing folders are created), Esc closes.
- S2: "d -> sale un popyo preguntando si queres borra el archivo" — `d` asks for confirmation, then deletes.
- S3: "y -> copia el archivo" — `y` copies the selected file (or folder).
- S4: "p -> pega el archivo" — `p` pastes the copied entry (assumed: into the selected folder, or the folder of the selected file; a taken name gets " copy" appended).
- S5 (assumed): paths stay inside the project; the target must not exist; a folder cannot go into itself; an open file follows its rename and closes on delete.

## Tasks
- T1 (S1-S5) ui/fileops.rs: path checks, move, delete, copy with tests — inline — done
- T2 (S1,S2) dialogs: PathDialog with error, delete confirmation — inline — done
- T3 (S1-S4) keys in the tree, clipboard, viewer follow-up, README — inline — done

## Log
L1 (2026-11-07): "estamos por ahora, el file explorer hay que agregar algunas feature / m -> renombrar o mover una carpeta con el formato "dir/*/file" se abre un popup en el file explorer con un input y la dir absoluta en relacion al project, al darle enter se actualiza el nombre/mueve el archivo, esc lo cierra / d -> sale un popyo preguntando si queres borra el archivo / y -> copia el archivo / p -> pega el archivo"
L2 (2026-11-07): build, clippy, 163 tests pass, app runs 8s. Not observed: the keys and dialogs on screen. Tests were written with the code, no RED observed.
L3 (2026-11-07): "centrado en horizontal y que cuando se aprete alt+shift+`(alt+~) se scrollie al archivo abierto en el editor" -> tree row content centered in the panel width (names wider than the panel overflow and scroll sideways); RevealOpenFile action (Alt+Shift+`, Alt+~) shows the tree, selects and scrolls to the open file. Not observed on screen.
L4 (2026-11-07): "no me explique bien, los items del file explorer no se centran, sino que el scroll en horizontal se mueve hasta quedar centrado el icono del archivo que esta seleccionado en el file explorer" -> corrects L3: rows are left-aligned again; scroll_tree_sideways now centers the selected row's icon in the panel (clamped to the scrollable width, so a tree that fits does not move).
