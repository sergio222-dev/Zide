# Git

Branches, push, pull, merge, rebase, conflict resolution and the Git log.

[Back to README](../README.md)

## Dialogs

- **Branch switcher** (``Ctrl+Shift+` ``, click the branch in the status bar, or Find Action → *Switch Branch*): Recent, Local, Remote and
  Tags as folder trees (`feat/` holds `feat/*`), searchable; Enter checks out, a remote branch gets a
  tracking local one and a tag is checked out detached.
  Right-click a branch for its menu: *New Branch from…*, *Update* (fast-forward only), *Push…* (asks first, never forced)
  and *Rename* (the dialog refuses a name that already exists).
- **Push dialog** (`Ctrl+Shift+K`, or Find Action → *Push Commits*): the commits the remote lacks on the left, the files of the
  selected commit on the right with its message, author and date under them. `Enter` or a double click on a file opens what
  the commit changed in it, side by side (green added, gray removed, blue replaced); `Ctrl+Enter` or **Push** pushes (never
  forced), even with no new commits (it still sets the branch's upstream), and *Push tags* sends the tags too. The header says
  where it goes: `branch → remote : branch`.
- **Pull, Merge and Rebase dialogs** (`Ctrl+Shift+U` pulls; Find Action → *Pull*, *Merge*, *Rebase*): `git <command>` with the
  branch taken from a dropdown (`Tab` switches, `Up`/`Down` open and move, `Enter` picks, then `Enter` runs; typing in a dropdown narrows its list down). *Modify options*
  (`Alt+O`) lists what git accepts, shown as chips once picked; what cannot go with a picked option is dimmed. Options that take
  text (`-m`, `--onto`) show a box for it; rebase can also pick another branch to rebase.
- **Conflict resolver** (Find Action → *Resolve Conflicts*, offered only while git reports conflicts): when a merge, rebase,
  cherry-pick or revert stops on conflicts, a dialog (it opens by itself when the operation was started from Zide) lists each file with what both sides did to it. *Accept Yours* keeps
  the current branch's version of the selected file, *Accept Theirs* the incoming one (a side that deleted the file deletes it);
  the file is then marked resolved. Once the last file is resolved, a rebase, cherry-pick or revert continues by itself
  (`git <operation> --continue`, keeping each commit's message); when a later commit clashes too the dialog opens again
  with its files. A merge is finished by committing it in the commit panel. *Merge…* (or a double click on the file) opens the merge editor: yours on the left, theirs on the right and the result
  (it starts as the common ancestor) in the middle. The arrow of a change (`»` / `«`) puts it in the result — both sides go one
  under the other — and the cross (`✕`) leaves it out; additions are green, removals gray, replacements blue and changes both
  sides made red. Long lines scroll sideways, the three panes together (scrollbar at the bottom, or `Shift`+wheel). The toolbar applies every non-conflicting change (`F7` / `Shift+F7` move between changes); *Apply* is enabled
  once no conflict is left, and writes the result and marks the file resolved. The result can be typed over: click or use the
  arrows, `Home`/`End`, type, `Enter`, `Backspace`, `Delete`, `Ctrl+V` to paste, `Ctrl+Z` to undo. Typing does not settle a
  change: it keeps its color and arrows until its sides are applied or discarded (an arrow puts the side's lines over what was
  typed). A line break cannot be removed where it would join two parts of the file (an empty line at the edge of a part can).
  While a merge, rebase, cherry-pick or revert is unfinished the status bar shows it (`Merging feature-branch`); with a merge, the
  commit panel (`Ctrl+K`) opens with git's message (`Merge branch 'feature-branch'`) and commits everything staged to conclude it.

The commit panel (`Ctrl+K`) and its keys are listed in [Keybindings](keybindings.md).

## Git log

In the Git log (`Alt+3` opens it): the refs tree on the left (HEAD, Local, Remote, Tags) narrows the history to
the ref you pick, and picking it again goes back to every branch. `/` (or `Ctrl+F`) focuses the search box (message,
author or the start of a hash), `Tab` moves between refs, commits and files, `j` / `k` or the arrows move,
`Enter` on a file (or a double click) opens that commit's diff, and `Esc` closes the tab.
The **User** button (`u`; `Up`/`Down` move, `Enter` picks, `Esc` closes) narrows the history to one author. **Date**
(`d`) lists *Last 24 hours*, *Last 7 days* and *Last 30 days*, and *Select...* opens an *After* and a *Before* date
picker (the clear button of each one drops it); and the **Paths**
box (`p`) keeps the commits that touch any of the paths typed, separated by spaces. The filters combine with the search
and the branch; while any is on, the graph is drawn as plain dots because the commits shown are no longer a connected history.
Right click on a commit opens its menu (`Up`/`Down` and `Enter` work too, `Esc` closes it): *Copy Revision Number*,
*Cherry-Pick*, *Reset Current Branch to Here...* (asks for Soft, Mixed, Hard or Keep), *Revert Commit*, *New Branch...*
(created from that commit and checked out) and *New Tag...*. A cherry-pick or revert that clashes leaves its conflicts
for the resolver, as a merge does.

Right click on a branch or tag of the tree opens its menu. One the remote does not have yet only offers *Push to origin*
(the remote the current branch pushes to); one it has offers *Checkout*, *Merge 'x' into 'current'*, *Push to origin* and
*Delete* (a branch with unmerged commits is kept; a tag is deleted here only, the remote keeps it). A remote branch offers
*Checkout* and *Merge*, the current branch only *Push*. Find Action → *Fetch* brings the branches and tags of every remote
(dropping the remote branches that are gone) and reads which tags the remote has, which is how the menu of a tag knows.

The files of the selected commit take `Ctrl+click` to add or drop a file, `Shift+click` (or `Shift+Up`/`Down`) for a
range and `Space` to toggle the one under the cursor. Right click on them opens *Show Diff*, *Revert Selected Changes*
(undoes them in the working tree), *Cherry-Pick Selected Changes* (applies them) and *Get from Revision* (overwrites the
files with their version in that commit). None of them commits or stages anything.

[Back to README](../README.md)
