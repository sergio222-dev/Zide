# Language servers

Zide does not ship language servers. It starts the one of the language of the file you open
(in the background, once per project) and uses it for semantic features such as Find Usages
(`Alt+F7`) and Go to Definition (`gd`, `Ctrl+B`). Without a server Find Usages falls back to text
search, and the popup says why; Go to Definition falls back to the symbol index (declarations with
the same name, which may be approximate), and the status line says why.

Zide looks for each server in this order:

1. **Zide's own folder** (`~/.local/share/zide/servers` on Linux): `bin/<command>` for programs,
   `node_modules/.bin/<command>` for what npm installs with `--prefix`. Installing here touches
   nothing else on your machine.
2. The **`PATH`**, like any other program.

| Language | Files | Command | Install (pick one) |
| --- | --- | --- | --- |
| Rust | `.rs` | `rust-glancer lsp` (low memory), else `rust-analyzer` | `rust-glancer`: put the binary from its [releases](https://github.com/rust-glancer/rust-glancer/releases) in `~/.local/share/zide/servers/bin/` (Zide's folder only). Fallback when that is absent: `rustup component add rust-analyzer` (`PATH`), or the binary in `<folder>/bin/` |
| TypeScript / JavaScript | `.ts` `.tsx` `.js` `.jsx` `.mts` `.cts` `.mjs` `.cjs` | `tsc --lsp --stdio` (TypeScript 7, native) | `npm install --prefix ~/.local/share/zide/servers typescript` (Zide's folder). Fallback when that is absent: `typescript-language-server --stdio`, installed with `npm install --prefix ~/.local/share/zide/servers typescript-language-server typescript@5` or `npm install -g typescript-language-server typescript@5` (`PATH`) |
| Go | `.go` | `gopls` | `GOBIN=~/.local/share/zide/servers/bin go install golang.org/x/tools/gopls@latest` (Zide's folder), or `go install golang.org/x/tools/gopls@latest` (`$GOPATH/bin` on the `PATH`) |
| Python | `.py` `.pyi` | `pyright-langserver --stdio` | `npm install --prefix ~/.local/share/zide/servers pyright` (Zide's folder), or `npm install -g pyright` (`PATH`) |

The npm servers are Node scripts: `node` has to be on the `PATH` when Zide runs them.

## Checking it works

Run the command by itself: it should wait silently for input (stop it with Ctrl+C).
If it prints an error and exits, Zide reports the same problem.

- **rust-analyzer: "Unknown binary"**: `rustup` provides a launcher even when the component is
  missing. Run the install command above. It also needs `cargo` on the `PATH`.
- **typescript-language-server: not found**: check that npm's global bin directory is on the
  `PATH` (`npm bin -g` or `npm prefix -g`). With version managers such as `fnm` or `nvm`, the
  server belongs to the Node version that was active when you installed it.
- **Results are missing or slow the first time**: the server indexes the project before it
  answers. Zide waits for it (up to two minutes), and starts it when you open the first file
  of that language, so by the time you ask it has usually finished.

Restart Zide after installing a server.

## Rust: rust-glancer or rust-analyzer

On a workspace the size of Zide's, rust-analyzer grows to about 4 GB as soon as it answers any
semantic query (find usages, highlights, hints), because it loads every dependency. rust-glancer
stays around 100 MB and answered Find Usages in half a second.

It is used only when its binary is in Zide's folder, never from the `PATH`. Its limits: no
diagnostics, no proc-macro support (usages inside macro-generated code are missed), and it indexes
on save, so the buffers you have not saved are analysed less precisely. Remove the binary to go
back to rust-analyzer.

## TypeScript 7

`typescript@latest` is TypeScript 7, the native port. It ships its own language server
(`tsc --lsp --stdio`, a Go binary inside `@typescript/typescript-<os>-<arch>`): Zide uses it when
it is in its folder, which needs no Node at run time and starts in milliseconds.

`typescript-language-server` cannot use TypeScript 7 (no `tsserver.js`; it fails with *"Could not
find a valid TypeScript installation"*). If you use it as the fallback, install `typescript@5` or
`typescript@6` next to it. A project's own `typescript` in its `node_modules` is used first when
it is a version the server supports; the copy in Zide's folder is the fallback for projects that
have none, or where it is not at the folder you opened (for example a monorepo opened from its top
folder).
