# Themes

Zide's colors come from a theme. Five themes are built in, and you can add your own as JSON files.

## Built-in themes

| Name | Character |
| --- | --- |
| Zide Dark | The default: a dark scheme in the spirit of One Dark. |
| Zide Light | A light scheme, with overlays retuned for a white background. |
| Zide Ember | Warm and low-contrast: browns, amber and sage. |
| Zide Fjord | Cool blues and teals on a deep navy. |
| Zide High Contrast | Bright, saturated colors on black. |

Their files live in [`crates/ui/themes/`](../crates/ui/themes/); copy one as a starting point for your own.

## Your themes

Put one file per theme in `~/.config/zide/themes/` (the `themes` folder next to `settings.json`), named
`<anything>.json`. Files with another extension are ignored. The folder is read once, when Zide first
lists the themes.

- Your themes come after the built-in ones, sorted by name.
- A theme with the same name as a built-in one replaces it.

## File format

```json
{
  "name": "My Theme",
  "light": false,
  "colors": {
    "background": "#1c1d20",
    "text": "#abb2bf",
    "caret_block": "#ff7a0099",
    "indent_bands": ["#e5c07b1c", "#98c3791c", "#61afef1c", "#c678dd1c"]
  }
}
```

- `name` (required): the name shown in the theme list.
- `light` (optional, default `false`): `true` switches dialogs, selects and other components to their
  light mode.
- `colors` (optional): the colors, by key (see below).

Colors are written `"#rrggbb"`. The keys marked **RGBA** also take `"#rrggbbaa"`, where `aa` is the
opacity (`00` transparent, `ff` opaque); `"#rrggbb"` there means opaque.

### Fallback and errors

- A file may give only some colors: every key it leaves out takes the color of **Zide Dark**,
  whatever `light` says. A light theme should therefore give every color.
- A wrong entry (an unknown key, a color that is not `#rrggbb`, a value of the wrong type, an
  `indent_bands` list without exactly 4 colors) is skipped, and the rest of the file still applies.
- A file that is not valid JSON, or has no `name`, is skipped.
- Each problem is reported once, naming the file and the key; Zide never fails to start because of a
  theme file.

## Color keys

### Editor and panels

| Key | Meaning |
| --- | --- |
| `background` | Background of the editor and the panels. |
| `text` | Plain text. |
| `dim` | Line numbers, separators. |
| `muted` | Status line, breadcrumb path. |
| `strong` | The current file in the breadcrumb. |
| `border` | Borders between panels. |
| `symbol` | Accents: the kind of a completion item, keys in the which-key popup. |
| `selection` | Selected text. |
| `word` | Other occurrences of the word under the caret. |
| `bracket` | The bracket under the caret and its partner. |
| `find_match` | Matches of a search. |
| `find_current` | The current match of a search. |
| `caret_bar` | The bar caret (Insert mode). |
| `caret_block` | The block caret of Vim's Normal mode. **RGBA** |
| `tree_selected` | The selected row of the project tree. |
| `tree_focused` | The selected row when the tree has focus. |
| `indent_bands` | Rainbow indent: a list of exactly 4 colors, one translucent band per indent level, cycling. **RGBA** |
| `whitespace` | The dots that mark whitespace. **RGBA** |

### Popups and controls

| Key | Meaning |
| --- | --- |
| `popup` | The popup, the find panel and their controls. |
| `control_border` | Borders of controls (inputs, buttons). |
| `active_bg` | Background of the active item (a selected list row, a pressed toggle). |
| `active_border` | Border of the active item. |
| `error` | Errors. |

### Git and diffs

| Key | Meaning |
| --- | --- |
| `change_added` | Gutter bar: lines added since the last commit. |
| `change_modified` | Gutter bar: lines changed since the last commit. |
| `change_removed` | Gutter bar: lines removed since the last commit. |
| `diff_added` | Background of added lines in a diff. |
| `diff_removed` | Background of removed lines in a diff. |
| `diff_changed` | Background of replaced lines in a diff (on both sides). |
| `diff_conflict` | Lines both sides changed, in the merge editor. |
| `tint_added` | Lines a chat's edit added, over the text. **RGBA** |
| `tint_removed` | Lines a chat's edit removed, over the text. **RGBA** |

### Minimap, hints, icons and status

| Key | Meaning |
| --- | --- |
| `minimap_cursor` | The minimap: the line of the cursor. |
| `minimap_view` | The minimap: the part of the file the editor shows. **RGBA** |
| `hint_bg` | Background of the chip with a parameter's name before an argument. |
| `hint_text` | Text of that chip. |
| `icon_folder` | Project tree icon: folders. |
| `icon_code` | Project tree icon: source files. |
| `icon_data` | Project tree icon: data and config files. |
| `icon_media` | Project tree icon: images and other media. |
| `ok` | Success marks (a tool that finished well). |
| `working` | A chat that is thinking. |
| `needs_user` | A chat that waits for you to allow something. |

### Syntax

| Key | Meaning |
| --- | --- |
| `syntax_keyword` | Keywords, list markers. |
| `syntax_string` | Strings and characters. |
| `syntax_number` | Numbers, constants, bold text in Markdown. |
| `syntax_comment` | Comments. |
| `syntax_function` | Functions and methods. |
| `syntax_type` | Types, tags. |
| `syntax_property` | Fields, properties, attributes. |
| `syntax_variable` | Variables, Markdown headings. |
| `syntax_operator` | Operators and punctuation. |
| `syntax_emphasis` | Emphasis (italic) in Markdown. |
| `syntax_link` | Links. |
