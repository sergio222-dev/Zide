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
| TypeScript / JavaScript | `.ts` `.tsx` `.js` `.jsx` `.mts` `.cts` `.mjs` `.cjs` | `typescript-language-server --stdio` | `npm install --prefix ~/.local/share/zide/servers typescript-language-server typescript` (Zide's folder), or `npm install -g typescript-language-server typescript` (`PATH`) |

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
