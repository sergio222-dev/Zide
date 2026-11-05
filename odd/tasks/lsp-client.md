# LSP client (layer 2) — Find Usages first
Objective: semantic Find Usages through a language server; textual search stays as fallback.
Branch: master. Delivery: commit per task. Runner: `cargo test --workspace`.

## Specs
S1. "arrancá con el cliente LSP" — a client crate speaking JSON-RPC over stdio (Content-Length framing).
S2. Semantics only through LSP (ADR layer 2); no custom PSI. Servers: rust-analyzer (installed), typescript-language-server (not installed: tell the user, do not install).
S3. Find Usages (Alt+F7) uses `textDocument/references`; when the server is missing, not ready or fails, fall back to the textual search and say so.
S4. The UI never blocks on the server: start and requests run in the background.

## Tasks
- [x] T1 (S1) zide-lsp: framing, connection with request/response correlation, tests with in-memory pipes.
- [x] T2 (S1,S2) zide-lsp: client handshake, didOpen/didChange/didClose, references, server table by extension.
- [ ] T3 (S3,S4) UI: lazy server start, document sync before request, references into the Text list, fallback + status.

## Log
L1 (2026-11-05) user: "si, arrancá con el cliente LSP"
