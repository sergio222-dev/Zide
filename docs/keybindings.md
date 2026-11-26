# Keybindings

The shortcuts of Zide, IntelliJ-style for IDE actions; the Vim keys are in [Vim keymap](vim.md).

[Back to README](../README.md)

## Shortcuts

| Keys | Action |
| --- | --- |
| double-Shift | Search Everywhere |
| `Ctrl+Shift+N` | Go to File |
| `Ctrl+Alt+Shift+N` | Go to Symbol |
| `Ctrl+Shift+A` | Find Action |
| `Ctrl+E` | Recent files |
| `Ctrl+F` / `Ctrl+R` | Find / Replace in file |
| `F3` / `Shift+F3` | Next / previous match |
| `Ctrl+Shift+F` | Find in Files |
| ``Ctrl+Shift+` `` | Switch Branch |
| `Ctrl+Alt+A` | Add to Git: start tracking the selected file or folder in the tree (the open file elsewhere) |
| `Ctrl+Shift+U` | Pull dialog: remote and branch to pull into the current branch (`Tab` switches field, `Enter` pulls, `Alt+O` options) |
| `Ctrl+Shift+K` | Push dialog: commits to push, their files and diffs (`Tab` switches list, `Enter` opens the diff, `Ctrl+Enter` pushes) |
| `Alt+F7` | Find Usages |
| `gd` / `Ctrl+B` | Go to Definition |
| `Ctrl+Space` | Code completion at the caret (Insert mode) |
| `Alt+1` | Toggle project tree |
| `Alt+Shift+\`` | Scroll the project tree to the open file and select it |
| `Ctrl+K` | Commit panel: tick files, write a message, **Commit** or **Commit and Push…** (`Amend` folds them into the last commit) |
| Commit panel `j`/`k`, `Space`, `Enter` | Move over the files, tick or untick one, open its changes (`Tab` or `i` goes to the message, `Alt+Enter` commits, `Esc` back to the files or out) |
| Double click on a commit panel file | Its changes against the last commit, side by side, in a large dialog; long lines scroll sideways, both sides together (`Esc` closes, double `Space` opens search) |
| Right click on a commit panel file or folder | **Discard Changes…**: after confirming, puts its files back to the last commit (new files are deleted), so they leave the list |
| Commit panel **Stash** tab | The stashes of the repository (message and branch) and the files of the selected one; **Apply**, **Pop** and **Drop** (asks twice). `j`/`k` pick a stash, `a` applies, `p` pops; double click a file to see its changes |; **Stash…** (or Find Action → *Stash Changes*) asks for a message and whether to take untracked files or keep staged changes in place
| `a` / `m` / `d` / `y` / `p` | In the tree: new file (a name ending with `/` makes a folder), move or rename (a popup with the path, Enter applies, Esc closes), delete (asks first), copy, paste (into the selected folder; a taken name gets ` copy`) |
| `Ctrl+Shift+-` / `Ctrl+Shift+=` | With the tree focused: collapse / expand every folder (the buttons on top of the tree do the same) |
| `Ctrl+Alt+Left` / `Ctrl+Alt+Right` | Navigate back / forward (also the mouse's back and forward buttons) |
| `Ctrl+Shift+M` | Toggle Markdown preview (`.md` files) |
| `Ctrl+H` / `Ctrl+L` | Focus tree / editor |
| `Alt+Down` / `Alt+Up` | Next / previous method |
| `Ctrl+W` / `Ctrl+Shift+W` | Extend / shrink selection |
| `Ctrl+-` / `Ctrl+=` | Collapse / expand region (add `Shift` for all) |
| `Ctrl+Alt+L` | Reformat code |
| `Alt+C` / `Alt+W` / `Alt+R` | Toggle case / whole word / regex in the find panel |

## Project tree

In the project tree (`Alt+1` focuses it): `j` / `k` / `h` / `l` or the arrows move and expand,
`gg` / `G` go to the first / last row, `Ctrl+D` / `Ctrl+U` move half a page, `Enter` opens.

The Claude chat keys are in [Claude chat](chat.md), the Git log keys in [Git](git.md#git-log).

[Back to README](../README.md)
