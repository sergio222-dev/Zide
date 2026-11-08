# Interactive conflict resolver (three-pane merge)
Objective: resolve a conflicted file with three panes (yours | result | theirs) from the *Merge…* button of the conflict dialog. Branch: feat/merge-editor (from feat/merge-in-progress). Delivery: ask-on-risk. Runner: cargo test.

## Specs
- S1: "en el medio el resultado y a los costados los cambios, las flechas >> indican la direccion y al hacer click sobre las flechas se agrega los cambios, en casos donde ambos cambios se agreguen se agregan uno abajo del otro" — result in the middle (starts as the base), changes at the sides; the arrow of a change applies it to the result; applying both puts one under the other.
- S2: "con la cruz se descartan esos cambios, en al segunda imagen se muestra como se ve cuando se descarta un cambio" — the cross discards a change (it stays shown, dimmed, without controls).
- S3: "las adiciones se ven en verde con la flecha y la cruz, las remociones en gris tambien con flecha y cruz" — additions green, removals gray, replacements blue (as the diff dialog), conflicts reddish; each change has arrow and cross.
- S4: "En lo posible se podria editar a mano el panel del medio el cual seria el resultado final" — hand editing of the result (best effort; see Log).
- S5 (from the screenshot): toolbar counter "No changes. 1 conflict.", bottom buttons Accept Left, Accept Right, Apply, Cancel; left is "Changes from <yours>", right "Changes from <theirs>".

## Tasks
- T1 (S1,S2,S5) git: stages, line-level three-way merge model with per-change state, resolve_with — inline — done (commit 'three-way merge model')
- T2 (S1-S3,S5) ui: merge editor overlay, Merge… button, apply writes the file and stages it — inline — done
- T3 (S4) hand edit of the result: caret, typing, paste, undo, click — inline — done

## Log
L1 (2026-11-08): "ahora vamos a agregar la ventana de resolve conflict interactiva, en la imagen se muestran 3 paneles, en el medio el resultado y a los costados los cambios, las flechas >> indican la direccion y al hacer click sobre las flechas se agrega los cambios, en casos donde ambos cambios se agreguen se agregan uno abajo del otro, con la cruz se descartan esos cambios, en al segunda imagen se muestra como se ve cuando se descarta un cambio. las adiciones se ven en verde con la flecha y la cruz, las remociones en gris tambien con flecha y cruz. En lo posible se podria editar a mano el panel del medio el cual seria el resultado final"
L2 (2026-11-08): build, clippy, 49 git + 199 ui tests pass. Not observed: the editor on screen. S4 (hand editing of the result) not done: the panes are aligned rows of a custom list, there is no text editor in the middle; it needs its own task.
L3 (2026-11-08): "dale que salga como el #91 y encaramos el T3" -> PR #91 opened; T3 done: typed lines override the result of a segment (Merge::set_result), caret over the aligned rows, Ctrl+Z history of merge states. No selection or copy; Delete/Backspace never join two segments. 49+ git and 205 ui tests pass, clippy clean. Not observed on screen (caret position from measured cell width and a canvas-measured origin).
L4 (2026-11-08): "cuando editamos el coloreado de la linea se va, deberia quedar en verde/roja si todavia hay conflictos o adiciones" -> typing no longer settles a change (set_result leaves sides pending); the result keeps red while the conflict is open, green (addition) or blue (replacement) once something is applied or typed; Apply stays disabled until each conflict has its sides applied or discarded.
