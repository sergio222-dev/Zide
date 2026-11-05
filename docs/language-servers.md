# Language servers

Zide does not ship language servers. It starts the one of the language of the file you open
(in the background, once per project) and uses it for semantic features such as Find Usages
(`Alt+F7`). Without a server those features fall back to text search, and the popup says why.

Zide looks for each server in this order:

1. **Zide's own folder** (`~/.local/share/zide/servers` on Linux): `bin/<command>` for programs,
   `node_modules/.bin/<command>` for what npm installs with `--prefix`. Installing here touches
   nothing else on your machine.
2. The **`PATH`**, like any other program.

| Language | Files | Command | Install (pick one) |
| --- | --- | --- | --- |
| Rust | `.rs` | `rust-analyzer` | `rustup component add rust-analyzer` (on the `PATH`), or put the binary in `<folder>/bin/` |
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
