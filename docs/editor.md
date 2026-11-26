# Editor features

What the editor does beyond plain text editing: Vim editing, search, language server features, AI suggestions and the parts of the window around the code.

[Back to README](../README.md)

## Features

- **Vim editing**: motions, operators (`d`, `c`, `y`), visual modes, text objects, registers,
  dot repeat, undo/redo, `Ctrl+A` / `Ctrl+X` to increment / decrement the number under the cursor, `Ctrl+/` to comment or uncomment the line or the selection
  with the file's language markers (`//`, `#`, `<!-- -->`).
- **Key hints** (LazyVim's which-key): a command left half typed (`d`, `c`, `y`, `g`, `z`, `ci`,
  `da`...) shows a small panel with the keys that can finish it after a short pause; Esc closes it.
- **Search Everywhere** (double-Shift): files, text in the project, recent files and actions in
  one popup. Go to File, Go to Symbol and Find Action have their own shortcuts.
- **Find and replace** in the file (with regex, case and whole-word toggles) and Find in Files.
- **Find Usages** (`Alt+F7`): semantic through a language server, text search as a fallback.
- **Go to Definition** (`gd` or `Ctrl+B`): through a language server; without one it jumps by name in the
  symbol index and the status line says so.
- **Code completion** while inserting: the file's language server offers what fits as you type a word or after a character the server lists (`.`, `:`),
  in a popup at the caret narrowed down as you keep typing (IntelliJ matching: `gBN` finds `getByName`).
  It opens on what is typed, not on the key that enters Insert mode.
  `Up`/`Down` or `Ctrl+N`/`Ctrl+P` pick, `Enter` or a click accepts (with the import it needs, when the server
  offers one), `Esc` leaves Insert and the popup; `Ctrl+Space` asks by hand. `.` repeats the completed word.
  The selected item's documentation shows beside the list (Markdown, as the server writes it). An import TypeScript
  only sends once the item is looked at still goes in when the item was accepted before it arrived.
  Without a language server (or when it has nothing for a word) the project's symbols of the file's language and the
  words of the file are offered instead; typing offers them in code files, `Ctrl+Space` in any file.
- **AI suggestions** (off by default): click `Qwen` in the status bar to use a local `qwen2.5-coder:1.5b-base`
  through [Ollama](https://ollama.com) (reused when one already runs, otherwise started, and the model downloaded the
  first time). Once typing pauses with no popup open, what fits at the caret shows dimmed; `Tab` takes it, typing it
  narrows it, anything else drops it. The dot is gray when off, orange while starting or writing, green when ready,
  red when it failed. Once a suggestion came, it says where Ollama runs the model: `GPU`, `CPU` (no usable GPU:
  slow; on Arch Linux install `ollama-cuda`) or the share on the GPU when it did not fit.
- **Signature help** while typing a call: after `(` or `,` the signature shows over the line, the parameter being
  typed in bold, and follows the cursor until the call is closed.
- **Status bar**: messages on the left; on the right the open file's language server (gray when not running,
  orange while starting, green when ready, red when it cannot run; hover it for why and how to install it),
  the branch's state against its upstream (`↑` to push, `↓` to pull,
  `Δ` when diverged), the branch name, the cursor position and the vim mode.
- **Minimap** at the right of the editor: every line as colored bars (folded lines are left out), the visible part
  marked, click or drag to jump. Lines added or modified since the last commit are painted green or brown, removed ones
  gray, errors red and the cursor's line blue.
  Turn it off with Find Action → *Show Minimap* (Enter flips it, the popup stays open; the project remembers it).
- **Code folding** for blocks, declarations, imports and long comments (Python docstrings too), with gutter chevrons.
  In Python a block folds from its header line (`def`, `class`, `if`...), so the header stays visible.
- **Git blame** in gray after the cursor line: author's last name, date, time and commit summary.
- **Matching pairs**: the caret on a bracket highlights its partner; on a JSX tag name, the other tag; on the `:` of a Python block, the block's last line.
- **Sticky scroll**: the class and function declarations that scrolled out of view stay pinned at the top of the editor (up to five, outermost first).
- **Project tree** panel, with IntelliJ ordering and reveal of the open file.
- **Markdown preview** (`Ctrl+Shift+M`): the rendered file beside the editor for `.md` files, updated as you type.
- **Navigation history**: the mouse's back and forward buttons (and `Ctrl+Alt+Left` / `Ctrl+Alt+Right`) return to where the cursor was before each jump (opening a file, go to line, Find Usages, a link).
- **Syntax highlighting** with tree-sitter (Rust, JavaScript, TypeScript, TSX, Go, Python, Markdown).
- **Breadcrumb** with the enclosing symbols at the cursor.
- **Whitespace dots and rainbow indent guides**, per-project font size (Ctrl+wheel to zoom).
- **Fonts**: run **Fonts** in Find Action to pick the code font (monospace families only) and the interface font; saved as `editor_font` and `ui_font` in `~/.config/zide/settings.json`.
- **Auto-indent**: Enter, `o` and `O` keep the line's indentation, one level deeper after `{`, `[`, `(` or `:`; Enter between a pair puts the closer on its own line, and a closer typed first on a line goes back one level. Tab and Backspace work by whole levels. The level is the one the file already uses; files without indentation take `indent` from `~/.config/zide/settings.json` (`"tab"` or a number of spaces, 4 by default).
- **Navigation**: next/previous method (`Alt+Down` / `Alt+Up`), extend/shrink selection.
- **Autosave** and live reload of the project tree.
- **Self-updater**: checks GitHub Releases at startup and verifies the minisign signature
  before installing.

Git features are described in [Git](git.md), shortcuts in [Keybindings](keybindings.md).

[Back to README](../README.md)
