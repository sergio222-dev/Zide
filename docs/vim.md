# Vim keymap

The Vim keys of Zide and how to rebind them from `~/.config/zide/settings.json`.

[Back to README](../README.md)

## Fold and scroll keys

Vim fold keys (`za`, `zo`, `zc`, `zM`, `zR`) and scroll keys (`zt`, `zz`, `zb`, `Ctrl+D`,
`Ctrl+U`) work in normal mode.

## Customizing the keymap

The Normal and Visual mode keys are data: [`crates/vim/src/default_keymap.json`](../crates/vim/src/default_keymap.json)
binds each key to an *opcode*, and groups keys into trees (`d` waits for a motion, `g` for `g` or `d`). The
`keymap` key of `~/.config/zide/settings.json` is merged over it key by key, in the same shape:

```json
{
  "keymap": {
    "lists": { "motions": { "L": "motion.line_end" } },
    "normal": {
      "keys": {
        "x": null,
        "g": { "keys": { "h": { "code": "editor.go_to_definition", "name": "Hop to definition" } } },
        "<C-f>": "editor.find"
      }
    }
  }
}
```

- `normal` and `visual` are the root groups; `lists` holds key lists groups share through `include` (the default
  ones are `motions`, `objects`, `targets` and `ctrl`). A key added to a list reaches every group that includes it.
- A group is `{ "code", "name", "include", "keys" }` (no `code` makes a plain prefix such as `g` or `z`); a key
  is one character or `<C-x>`, `<CR>`, `<BS>`, `<Space>`. An opcode alone replaces the default key whole, an object
  refines it (its keys merge into the default ones), `null` unbinds it.
- Each entry is checked: an unknown opcode, a malformed key, or an opcode where it cannot work (`visual.*` in
  normal, `command.*` after an operator, a text object outside `object.inside` / `object.around`, `line` anywhere but
  right after an operator, digits `1`-`9` where they are a count, `<Esc>`) is left out, the default kept, and a
  notification lists why when a file opens.
- The key hints panel reads the same trees, so new keys show up there with their `name` (or the opcode's own
  description).

Opcodes: `operator.change` `operator.delete` `operator.yank` `line` `object.inside` `object.around`
`motion.line_start` `motion.first_non_blank` `motion.line_end` `motion.left` `motion.down` `motion.up`
`motion.right` `motion.word` `motion.big_word` `motion.word_back` `motion.word_end` `motion.big_word_end`
`motion.first_line` `motion.last_line` `object.word` `object.paren` `object.brace` `object.bracket` `object.angle`
`object.double_quote` `object.single_quote` `object.backtick` `command.insert` `command.insert_line_start`
`command.append` `command.append_line_end` `command.open_below` `command.open_above` `command.delete_char`
`command.delete_line_end` `command.paste_after` `command.paste_before` `command.undo` `command.repeat` `redo`
`number.increment` `number.decrement` `visual.char` `visual.line` `visual.other_end` `visual.change`
`visual.delete` `visual.yank` `editor.go_to_definition` `editor.find` `editor.find_next` `editor.find_previous`
`editor.half_page_down` `editor.half_page_up` `editor.fold_toggle` `editor.fold_open` `editor.fold_close`
`editor.fold_open_all` `editor.fold_close_all` `editor.scroll_top` `editor.scroll_center` `editor.scroll_bottom`
`editor.scroll_top_first_non_blank` `editor.scroll_center_first_non_blank` `editor.scroll_bottom_first_non_blank`.

[Back to README](../README.md)
