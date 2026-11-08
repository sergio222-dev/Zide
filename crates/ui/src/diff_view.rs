//! A double click on a file of the commit panel (or of a commit in the push dialog) opens its
//! changes, side by side, in a large dialog: lines that were added are green, lines that were
//! removed are gray and lines that replace others are blue, on both sides.

use gpui_kit::component::WindowExt;
use gpui_kit::component::diff::{
    Diff, DiffDecoration, DiffFile, DiffLineRange, DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use zide_git::FileChange;

use crate::{Workspace, theme};

/// A run of changed lines of a patch: where it is in the old file and in the new one (1-based,
/// inclusive), `None` when it has no lines there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    old: Option<(usize, usize)>,
    new: Option<(usize, usize)>,
}

impl Block {
    /// The color of its lines: blue when it replaces lines, else green (added) or gray (removed).
    fn color(&self) -> u32 {
        match (self.old, self.new) {
            (Some(_), Some(_)) => theme::DIFF_CHANGED,
            (None, _) => theme::DIFF_ADDED,
            (_, None) => theme::DIFF_REMOVED,
        }
    }
}

/// The runs of `-` and `+` lines between the unchanged lines of a unified patch.
fn blocks(patch: &str) -> Vec<Block> {
    #[derive(Default)]
    struct Run {
        old_start: usize,
        new_start: usize,
        removed: usize,
        added: usize,
    }
    fn flush(run: &mut Run, blocks: &mut Vec<Block>) {
        let range = |start: usize, count: usize| (count > 0).then(|| (start, start + count - 1));
        let (old, new) = (
            range(run.old_start, run.removed),
            range(run.new_start, run.added),
        );
        if old.is_some() || new.is_some() {
            blocks.push(Block { old, new });
        }
        *run = Run::default();
    }

    let mut blocks = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut run = Run::default();
    let mut in_hunk = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            flush(&mut run, &mut blocks);
            // `@@ -12,4 +12,6 @@`
            let start = |sign: char| {
                header
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix(sign))
                    .and_then(|part| part.split(',').next()?.parse().ok())
                    .unwrap_or(0)
            };
            (old, new) = (start('-'), start('+'));
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue; // the file's header lines
        }
        match line.as_bytes().first() {
            Some(b'-') => {
                if run.removed + run.added == 0 {
                    (run.old_start, run.new_start) = (old, new);
                }
                run.removed += 1;
                old += 1;
            }
            Some(b'+') => {
                if run.removed + run.added == 0 {
                    (run.old_start, run.new_start) = (old, new);
                }
                run.added += 1;
                new += 1;
            }
            // "\ No newline at end of file"
            Some(b'\\') => {}
            Some(b' ') | None => {
                flush(&mut run, &mut blocks);
                old += 1;
                new += 1;
            }
            // the header of the next file
            Some(_) => {
                flush(&mut run, &mut blocks);
                in_hunk = false;
            }
        }
    }
    flush(&mut run, &mut blocks);
    blocks
}

/// The backgrounds that paint the changed lines of `file` for the patch it came from.
fn decorations(file: &DiffFile, patch: &str) -> Vec<DiffDecoration> {
    let path = file.path().clone();
    let mut decorations = Vec::new();
    for block in blocks(patch) {
        let color: Hsla = rgb(block.color()).into();
        for (side, range) in [
            (DiffSide::Original, block.old),
            (DiffSide::Modified, block.new),
        ] {
            if let Some((start, end)) = range {
                decorations.push(
                    DiffDecoration::new(DiffLineRange::new(path.clone(), side, start, end))
                        .with_background(color),
                );
            }
        }
    }
    decorations
}

/// An empty file that was added or deleted has no hunk, and the viewer would show only its
/// header. Gives it the one empty line IntelliJ shows, so it is painted like any other addition.
fn with_empty_file_line(patch: &str) -> std::borrow::Cow<'_, str> {
    if patch.lines().any(|line| line.starts_with("@@")) {
        return patch.into();
    }
    let Some(head) = patch.lines().next() else {
        return patch.into();
    };
    let Some(path) = head
        .strip_prefix("diff --git a/")
        .and_then(|names| names.split_once(" b/"))
        .map(|(_, path)| path)
    else {
        return patch.into();
    };
    let has = |mode: &str| patch.lines().any(|line| line.starts_with(mode));
    if has("new file mode") {
        format!("{head}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+\n")
            .into()
    } else if has("deleted file mode") {
        format!("{head}\ndeleted file mode 100644\n--- a/{path}\n+++ /dev/null\n@@ -1 +0,0 @@\n-\n")
            .into()
    } else {
        patch.into()
    }
}

impl Workspace {
    /// Shows what changed in the file at `path`, as the commit panel lists it.
    pub(crate) fn show_diff(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(change) = self
            .commit
            .as_ref()
            .and_then(|panel| panel.changes.change(path))
            .cloned()
        else {
            return;
        };
        let root = self.root.clone();
        let title = change.path.clone();
        self.open_diff_of(title, move || zide_git::diff(&root, &change), window, cx);
    }

    /// Shows what the commit `hash` changed in `change`, as the push dialog lists it.
    pub(crate) fn show_commit_diff(
        &mut self,
        hash: &str,
        change: FileChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        let hash = hash.to_string();
        let title = format!("{}  ·  {}", change.path, hash.get(..8).unwrap_or(&hash));
        self.open_diff_of(
            title,
            move || zide_git::commit_diff(&root, &hash, &change),
            window,
            cx,
        );
    }

    /// Reads the patch off the UI thread, then opens it.
    fn open_diff_of(
        &mut self,
        title: String,
        patch: impl FnOnce() -> Result<String, String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx.background_executor().spawn(async move { patch() }).await;
            this.update_in(cx, |this, window, cx| match patch {
                Ok(patch) => this.open_diff(&title, &patch, window, cx),
                Err(why) => this.notify(Notification::error(why).title("Could not diff it"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn open_diff(&mut self, title: &str, patch: &str, window: &mut Window, cx: &mut Context<Self>) {
        let patch = &*with_empty_file_line(patch);
        let files = match DiffFile::parse(patch) {
            Ok(files) if !files.is_empty() => files,
            _ => {
                let note = Notification::info("Nothing to compare").title(title.to_string());
                return self.notify(note, cx);
            }
        };
        let decorations = files
            .first()
            .map(|file| decorations(file, patch))
            .unwrap_or_default();
        self.diff_open = true;
        let state = cx.new(|cx| DiffState::new(files, cx).with_mode(DiffMode::Split));
        let title = title.to_string();
        window.open_dialog(cx, move |dialog, window, _| {
            let size = window.viewport_size();
            dialog
                .title(title.clone())
                .width(size.width * 0.92)
                .margin_top(size.height * 0.04)
                .child(
                    div().h(size.height * 0.8).child(
                        Diff::new(&state)
                            .header_visible(false)
                            .decorations(decorations.clone()),
                    ),
                )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(old: Option<(usize, usize)>, new: Option<(usize, usize)>) -> Block {
        Block { old, new }
    }

    #[::core::prelude::v1::test]
    fn a_new_file_is_all_additions() {
        let patch = "diff --git a/t.js b/t.js\nnew file mode 100644\n--- /dev/null\n+++ b/t.js\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n";
        assert_eq!(blocks(patch), [block(None, Some((1, 3)))]);
        assert_eq!(blocks(patch)[0].color(), theme::DIFF_ADDED);
    }

    #[::core::prelude::v1::test]
    fn a_removal_is_gray_and_a_replacement_is_blue_on_both_sides() {
        let patch = "--- a/f\n+++ b/f\n@@ -1,7 +1,6 @@\n keep\n-gone\n keep\n-old 1\n-old 2\n+new\n keep\n keep\n keep\n";
        let found = blocks(patch);
        assert_eq!(
            found,
            [block(Some((2, 2)), None), block(Some((4, 5)), Some((3, 3)))]
        );
        assert_eq!(found[0].color(), theme::DIFF_REMOVED);
        assert_eq!(found[1].color(), theme::DIFF_CHANGED);
    }

    #[::core::prelude::v1::test]
    fn hunks_and_files_start_their_own_runs() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-x\n+y\n k\n@@ -10,2 +10,3 @@\n k\n+z\n k\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1,2 @@\n k\n+w\n\\ No newline at end of file\n";
        assert_eq!(
            blocks(patch),
            [
                block(Some((1, 1)), Some((1, 1))),
                block(None, Some((11, 11))),
                block(None, Some((2, 2))),
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn an_empty_added_file_gets_one_green_line() {
        let patch = "diff --git a/t.js b/t.js\nnew file mode 100644\nindex 0000000..e69de29\n";
        let filled = with_empty_file_line(patch);
        assert_eq!(blocks(&filled), [block(None, Some((1, 1)))]);
        assert_eq!(DiffFile::parse(&filled).unwrap()[0].additions(), 1);
        let gone = "diff --git a/t.js b/t.js\ndeleted file mode 100644\nindex e69de29..0000000\n";
        assert_eq!(
            blocks(&with_empty_file_line(gone)),
            [block(Some((1, 1)), None)]
        );
        // a patch with hunks, or a mode change alone, is left as it is
        let real = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(with_empty_file_line(real), real);
        let mode = "diff --git a/a b/a\nold mode 100644\nnew mode 100755\n";
        assert_eq!(with_empty_file_line(mode), mode);
    }

    #[::core::prelude::v1::test]
    fn a_patch_without_changes_has_no_blocks() {
        assert!(blocks("").is_empty());
        assert!(blocks("--- a/f\n+++ b/f\n").is_empty());
    }
}
