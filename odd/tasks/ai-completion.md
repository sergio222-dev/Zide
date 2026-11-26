# AI completion (Qwen2.5-coder through Ollama) — inline ghost text
Objective: local LLM fill-in-the-middle suggestions as ghost text; language servers keep the popup and are what remains when the model is off or fails.
Branch: feat/ai-completion (on chore/gpui-kit-0.11). Delivery: ask-on-risk. Runner: `cargo test --workspace`.

## Specs
S1. "se quiere implementar Qwen2.5-coder para el autocompletado/suggestions, y usar como fallback los lsp que tenemos hoy integrados" — the model suggests at the caret (FIM: text before and after the caret); the LSP popup keeps working as today and wins while it is open; with the model off or failing, behavior is exactly today's.
S2. "ghost text en línea" — the suggestion shows dimmed at the caret, inside the line (first line), the rest of a multi-line suggestion under it; Tab accepts it when the popup is closed; typing what it shows narrows it; anything else drops it.
S3. "que este apagado por defecto pero que se puede inicializar" — off at every launch; turning it on starts or reuses an Ollama server and pulls the model when missing.
S4. "un indicador de que esta funcionando" — a status bar indicator: off, starting/pulling, ready, working, failed (with reason); clicking it turns the model on or off.
S5. "cuando se abren 2 instancias de zide, entiendo que deberia siempre haber un solo serve de ollama asi no saturamos la ram/gpu" — an instance reuses a server already answering on the Ollama port and only starts `ollama serve` when none answers; it stops only the server it started; an instance whose server went away starts one again on its next question. The model is loaded once by the shared server (`keep_alive` bounds how long it stays in VRAM).

Assumptions: model `qwen2.5-coder:1.5b-base` (base models do FIM), host `127.0.0.1:11434` (or `OLLAMA_HOST`).

## Tasks
- [x] T1 (S3,S5) zide-ai crate: probe/reuse/spawn server, model check + pull, FIM generate. Route: inline. 4339f05
- [x] T2 (S2) viewer: ghost text injected at the caret, rest under it, narrowing, accept. Route: inline. 380374c
- [x] T3 (S1,S3,S4) UI: on/off from the status bar indicator, ask after a pause while typing, Tab accepts. Route: inline. 65035a3
- [ ] T4 manual app check by the user (indicator, ghost text, Tab, two instances).

## Log
L1 (2026-11-25) user: "vamos a hacer una nueva feature, se quiere implementar Qwen2.5-coder para el autocompletado/suggestions, y usar como fallback los lsp que tenemos hoy integrados, habria que analizar de ver como levantarlo, que este apago por defecto pero que se puede inicializar y un indicador de que esta funcionando"
L2 (2026-11-25) user: "ghost text en línea, dale. otra cosa, habria que ver que pasa cuando se abren 2 instancias de zide, entiendo que deberia siempre haber un solo serve de ollama asi no saturamos la ram/gpu"
L3 Ollama docs: `/api/generate` takes `suffix` (FIM) and `keep_alive` (default 5m). Ollama installed here, service inactive, no models; RTX 3070 8 GB.
L4 Verification: `cargo test --workspace` green (zide-ai 11, zide-ui 288), clippy clean. Real Ollama 0.40.15: pull 35 s; FIM on `fn add(a,b) -> u32 { | }` answered "a + b" in 139 ms warm (1.6 s cold load). A second `ollama serve` fails with "bind: address already in use" (S5 premise holds). `ollama ps` shows 100% CPU: the Arch `ollama` package has no CUDA (`ollama-cuda` gives GPU).
L5 Known limits: an instance killed hard (SIGKILL) leaves its `ollama serve` running; the next instance reuses it. Ghost asked only when no popup is open (the LSP/local popup opens on most word characters), so suggestions mostly come after spaces, punctuation and new lines.
L6 (2026-11-25) user: "hice la prueba, como puedo ejecutar el autocompletado? veo el texto fantasma pero no se como autocompletarlo"
L7 Cause: Tab is bound to the `SwitchTab` action (Workspace context); gpui runs actions before key listeners, so `completion_key` never saw Tab and `commit_tab` ran. Fix: the SwitchTab handler accepts the ghost (or the open completion popup) first while the editor has the keys.
L8 (2026-11-25) user: "tengo una situacion, cuando entro en insert mode el auto suggest me muestra el popup, pero si quiero hacer una new line me toma el suggest, como resuelvo esto?"
L9 Fix: the completion popup takes Enter only after an item was picked on purpose (arrows, Ctrl+N/P, click); otherwise Enter is a new line and the popup closes. Tab always accepts.
L10 (2026-11-25) user: "hagamos una cosa, el enter acepta la primera suggest, tab es para el ghost suggest, el popup no se abre hasta que el usuario escriba, antes al entrar al insert mode ya se mostraba el popup, tiene que esperar a que el usuario haga input, para qwen esto no hace falta tiene que seguir funcionando como hoy"
L11 Supersedes L9. Enter accepts the selected popup item (the first unless moved); Tab only takes the ghost. Cause of the popup on entering Insert: the entering key (`i`, `a`, `o`) reached `complete_as_typed` with the viewer already inserting and counted as a typed letter; now only keys typed while already inserting ask. The ghost is still asked after that key.
L12 (2026-11-25) user: "el punto 2 se ve interesante, podemos aplicar eso"
L13 Prompt window: the text before the caret starts on a multiple of 2048 chars (then the next line start), at most 6000 back, so it stays put while typing and Ollama reuses its cached prompt start. Measured on crates/ui/src/viewer.rs deep in the file, CPU, 9 keys: old 9081 ms average prompt read per key (cache 1 token), new 2680 ms (cache 1038 of 1497 tokens). What is still read each key is mostly the 2000 chars after the caret, which follow the edit.
L14 (2026-11-25) user: "si, indicalo" (show where Ollama runs the model in the indicator; no own CPU fallback: Ollama already falls back to CPU or splits).
L15 Indicator reads /api/ps after each suggestion: `Qwen · GPU`, `Qwen · CPU`, `Qwen · N% GPU`. Real Ollama here: before any question None, after one `Cpu`, matching `ollama ps` "100% CPU".
L16 (2026-11-25) user: "ya instale ollama-cuda" / "ahi actualice ollama". With ollama 0.40.15 + ollama-cuda 0.40.17 the GPU was not used (versions must match); after updating ollama to 0.40.17: "inference compute library=CUDA RTX 3070", `ollama ps` 100% GPU. Same typing measure on viewer.rs: prompt read 130 ms cold, ~45 ms cached (CPU: 7.3 s / 2.7 s); generation ~240 tok/s, 96 tok in ~400 ms; wall 87-456 ms per suggestion (CPU: 3-9 s).
