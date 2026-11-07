//! The commit panel (Alt+2): the changed files as a tree with a check box on each, the message
//! and the buttons that commit what is ticked.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{Disableable, Icon};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{ChangeKind, FileChange};

use crate::{TREE_ROW_HEIGHT, Workspace, file_icon, theme};

/// Whether a row's files are ticked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    On,
    Off,
    Some,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Root,
    Dir,
    File(ChangeKind),
}

/// A line of the changes tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The path of a file or directory; empty for the root. Ticks and folds go by it.
    pub key: String,
    pub name: String,
    pub depth: usize,
    pub kind: RowKind,
    /// Files below it.
    pub count: usize,
    pub check: Check,
    pub expanded: bool,
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<usize>,
}

impl Node {
    fn insert(&mut self, parts: &[&str], index: usize) {
        match parts {
            [] => {}
            [_] => self.files.push(index),
            [dir, rest @ ..] => self
                .dirs
                .entry(dir.to_string())
                .or_default()
                .insert(rest, index),
        }
    }

    fn count(&self) -> usize {
        self.files.len() + self.dirs.values().map(Node::count).sum::<usize>()
    }
}

/// The changed files, what is ticked and what is folded.
#[derive(Default)]
pub struct Changes {
    changes: Vec<FileChange>,
    checked: HashSet<String>,
    /// Every path seen so far: a new one starts ticked, one the user unticked stays unticked.
    seen: HashSet<String>,
    collapsed: HashSet<String>,
}

impl Changes {
    /// Takes the repository's current changes, keeping what the user ticked.
    pub fn set_changes(&mut self, changes: Vec<FileChange>) {
        for change in &changes {
            if self.seen.insert(change.path.clone()) {
                self.checked.insert(change.path.clone());
            }
        }
        self.checked
            .retain(|path| changes.iter().any(|change| &change.path == path));
        self.changes = changes;
    }

    pub fn count(&self) -> usize {
        self.changes.len()
    }

    /// The change of the file at `path`.
    pub fn change(&self, path: &str) -> Option<&FileChange> {
        self.changes.iter().find(|change| change.path == path)
    }

    /// The tree: directories that hold a single directory and no file are shown as one row, as
    /// `apps/api/src`.
    pub fn rows(&self) -> Vec<Row> {
        let mut root = Node::default();
        for (index, change) in self.changes.iter().enumerate() {
            let parts: Vec<&str> = change.path.split('/').collect();
            root.insert(&parts, index);
        }
        let mut rows = Vec::new();
        if self.changes.is_empty() {
            return rows;
        }
        let expanded = !self.collapsed.contains("");
        rows.push(Row {
            key: String::new(),
            name: "Changes".into(),
            depth: 0,
            kind: RowKind::Root,
            count: self.changes.len(),
            check: self.check_of(""),
            expanded,
        });
        if expanded {
            self.walk(&root, "", 1, &mut rows);
        }
        rows
    }

    fn walk(&self, node: &Node, path: &str, depth: usize, rows: &mut Vec<Row>) {
        for (name, child) in &node.dirs {
            let (mut name, mut child, mut path) = (name.clone(), child, join(path, name));
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (next, grandchild) = child.dirs.iter().next().unwrap();
                name = format!("{name}/{next}");
                path = join(&path, next);
                child = grandchild;
            }
            let expanded = !self.collapsed.contains(&path);
            rows.push(Row {
                key: path.clone(),
                name,
                depth,
                kind: RowKind::Dir,
                count: child.count(),
                check: self.check_of(&path),
                expanded,
            });
            if expanded {
                self.walk(child, &path, depth + 1, rows);
            }
        }
        for &index in &node.files {
            let change = &self.changes[index];
            let name = change.path.rsplit('/').next().unwrap_or(&change.path);
            rows.push(Row {
                key: change.path.clone(),
                name: name.to_string(),
                depth,
                kind: RowKind::File(change.kind),
                count: 1,
                check: self.check_of(&change.path),
                expanded: false,
            });
        }
    }

    /// The changes under `key`: all of them for the root, one file, or a directory's files.
    fn under<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a FileChange> {
        self.changes.iter().filter(move |change| {
            key.is_empty()
                || change.path == key
                || change
                    .path
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    fn check_of(&self, key: &str) -> Check {
        let (mut on, mut total) = (0, 0);
        for change in self.under(key) {
            total += 1;
            on += self.checked.contains(&change.path) as usize;
        }
        match on {
            0 => Check::Off,
            n if n == total => Check::On,
            _ => Check::Some,
        }
    }

    /// Ticks everything under `key`, or unticks it when it was all ticked.
    pub fn toggle_check(&mut self, key: &str) {
        let untick = self.check_of(key) == Check::On;
        let paths: Vec<String> = self.under(key).map(|c| c.path.clone()).collect();
        for path in paths {
            if untick {
                self.checked.remove(&path);
            } else {
                self.checked.insert(path);
            }
        }
    }

    pub fn toggle_fold(&mut self, key: &str) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_string());
        }
    }

    pub fn ticked(&self) -> usize {
        self.checked.len()
    }

    /// What to hand git: the ticked files, and where a ticked rename came from.
    pub fn paths(&self) -> Vec<String> {
        self.changes
            .iter()
            .filter(|change| self.checked.contains(&change.path))
            .flat_map(|change| [Some(change.path.clone()), change.from.clone()])
            .flatten()
            .collect()
    }
}

pub struct CommitPanel {
    pub changes: Changes,
    pub amend: bool,
    /// A commit is running.
    pub busy: bool,
    pub message: Entity<TextareaState>,
    scroll: UniformListScrollHandle,
}

impl CommitPanel {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            changes: Changes::default(),
            amend: false,
            busy: false,
            message: cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message")),
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// What the notification says once a commit went through: how many files it holds.
fn done_message(files: usize, push: bool, amend: bool) -> String {
    let what = match (amend, files) {
        (true, 0) => "Amended the last commit".to_string(),
        (true, 1) => "Amended the last commit with 1 file".to_string(),
        (true, n) => format!("Amended the last commit with {n} files"),
        (false, 1) => "Committed 1 file".to_string(),
        (false, n) => format!("Committed {n} files"),
    };
    if push {
        format!("{what} and pushed")
    } else {
        what
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}/{name}")
    }
}

pub(crate) fn kind_color(kind: ChangeKind) -> u32 {
    match kind {
        // a file git does not track yet is a new file: green like the added ones
        ChangeKind::Added | ChangeKind::Untracked => theme::OK,
        ChangeKind::Modified | ChangeKind::Renamed => theme::ICON_CODE,
        ChangeKind::Deleted => theme::CHANGE_REMOVED,
        ChangeKind::Conflicted => theme::ERROR,
    }
}

fn check_box(check: Check) -> Div {
    let on = check != Check::Off;
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .size(px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(rgb(if on { theme::ACTIVE_BORDER } else { theme::DIM }))
        .when(on, |d| d.bg(rgb(theme::ACTIVE_BORDER)))
        .text_size(px(11.))
        .text_color(rgb(theme::STRONG))
        .child(match check {
            Check::On => "✓",
            Check::Some => "–",
            Check::Off => "",
        })
}

impl Workspace {
    /// Alt+2: opens the panel on the message; from the panel, closes it.
    pub(crate) fn toggle_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // the panel belongs to the editor: from the chat, Alt+2 goes back to it first (as Alt+1
        // does for the tree)
        if self.claude_open {
            return self.toggle_claude(window, cx);
        }
        self.autosave(cx); // what git is about to show is what is on disk
        let in_panel = self.commit_open && self.commit_focused(window, cx);
        if in_panel {
            self.commit_open = false;
            self.focus.focus(window, cx);
            return cx.notify();
        }
        if self.commit.is_none() {
            self.commit = Some(CommitPanel::new(window, cx));
        }
        self.commit_open = true;
        self.refresh_commit(cx);
        if let Some(panel) = &self.commit {
            panel
                .message
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// The message box has the keyboard (the editor must not read those keys).
    pub(crate) fn commit_focused(&self, window: &Window, cx: &App) -> bool {
        self.commit
            .as_ref()
            .is_some_and(|panel| panel.message.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Reads the repository's changes again, off the UI thread: the panel and the colors of the
    /// project tree show them.
    pub(crate) fn refresh_commit(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let changes = cx
                .background_executor()
                .spawn(async move { zide_git::changes(&root) })
                .await;
            this.update(cx, |this, cx| {
                this.git_status = Arc::new(
                    changes
                        .iter()
                        .map(|change| (change.path.clone(), change.kind))
                        .collect(),
                );
                if let Some(panel) = &mut this.commit {
                    panel.changes.set_changes(changes);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What a commit finished with: the panel reads the changes again and, when it worked, empties
    /// the message.
    pub(crate) fn commit_finished(&mut self, worked: bool, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return self.refresh_commit(cx);
        };
        let was_busy = std::mem::take(&mut panel.busy);
        panel.amend = panel.amend && !(was_busy && worked);
        if was_busy && worked {
            let message = panel.message.clone();
            self.window
                .update(cx, |_, window, cx| {
                    message.update(cx, |state, cx| state.set_value("", window, cx))
                })
                .ok();
        }
        self.refresh_commit(cx);
    }

    /// Commits the ticked files; with `push`, publishes the branch afterwards.
    fn commit_selected(&mut self, push: bool, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        if panel.busy {
            return;
        }
        let paths = panel.changes.paths();
        let files = panel.changes.ticked();
        let message = panel.message.read(cx).value().to_string();
        let amend = panel.amend;
        panel.busy = true;
        let done = done_message(files, push, amend);
        self.git_task(
            "Could not commit",
            Some(done),
            move |root| {
                zide_git::commit(&root, &paths, &message, amend)?;
                if push {
                    let branch = zide_git::branch_status(&root)
                        .ok_or("Could not tell the branch to push")?;
                    let remote = zide_git::push_remote(&root, &branch.name)?;
                    zide_git::push_branch(&root, &branch.name, &remote)?;
                }
                Ok(())
            },
            cx,
        );
    }

    pub(crate) fn commit_view(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let workspace = cx.entity();
        let panel = self.commit.as_ref().expect("the panel is open");
        let rows = panel.changes.rows();
        let (count, ticked, amend, busy) = (
            panel.changes.count(),
            panel.changes.ticked(),
            panel.amend,
            panel.busy,
        );
        let message = panel.message.clone();
        let scroll = panel.scroll.clone();
        let can_commit = !busy && ticked > 0 || amend;

        let list = uniform_list("commit-tree", rows.len(), {
            let workspace = workspace.clone();
            move |range, _, _| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let key = row.key.clone();
                        let foldable = !matches!(row.kind, RowKind::File(_));
                        let (on_check, on_fold) = (workspace.clone(), workspace.clone());
                        let on_open = workspace.clone();
                        let (check_key, fold_key, open_key) = (key.clone(), key.clone(), key);
                        div()
                            .id(("commit-row", index))
                            .w_full()
                            .h(px(TREE_ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8. + 14. * row.depth as f32))
                            .pr(px(8.))
                            .whitespace_nowrap()
                            .when(!foldable, |d| {
                                d.on_click(move |event, window, cx| {
                                    if event.click_count() == 2 {
                                        on_open.update(cx, |this, cx| {
                                            this.show_diff(&open_key, window, cx)
                                        });
                                    }
                                })
                            })
                            .child(
                                div()
                                    .id(("commit-fold", index))
                                    .w(px(12.))
                                    .flex_shrink_0()
                                    .child(match (foldable, row.expanded) {
                                        (false, _) => "",
                                        (true, true) => "▾",
                                        (true, false) => "▸",
                                    })
                                    .when(foldable, |d| {
                                        d.on_click(move |_, _, cx| {
                                            on_fold.update(cx, |this, cx| {
                                                if let Some(panel) = &mut this.commit {
                                                    panel.changes.toggle_fold(&fold_key);
                                                    cx.notify();
                                                }
                                            })
                                        })
                                    }),
                            )
                            .child(check_box(row.check).id(("commit-check", index)).on_click(
                                move |_, _, cx| {
                                    on_check.update(cx, |this, cx| {
                                        if let Some(panel) = &mut this.commit {
                                            panel.changes.toggle_check(&check_key);
                                            cx.notify();
                                        }
                                    })
                                },
                            ))
                            .child({
                                let (icon, color) =
                                    file_icon::for_row(&row.name, foldable, row.expanded);
                                Icon::new(icon)
                                    .size_4()
                                    .flex_shrink_0()
                                    .text_color(rgb(color))
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(rgb(match row.kind {
                                        RowKind::File(kind) => kind_color(kind),
                                        _ => theme::TEXT,
                                    }))
                                    .child(row.name.clone()),
                            )
                            .when(foldable, |d| {
                                d.child(div().text_color(rgb(theme::DIM)).child(format!(
                                    "{} file{}",
                                    row.count,
                                    if row.count == 1 { "" } else { "s" }
                                )))
                            })
                    })
                    .collect()
            }
        })
        .size_full()
        .track_scroll(&scroll);

        let amend_toggle = workspace.clone();
        let commit_button = workspace.clone();
        let push_button = workspace;
        div()
            .id("commit-panel")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(340.))
            .h_full()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .px_2()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .text_color(rgb(theme::STRONG))
                    .child("Commit"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .child(Scrollbar::new(&scroll)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(28.))
                    .child(
                        div()
                            .id("commit-amend")
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(check_box(if amend { Check::On } else { Check::Off }))
                            .child("Amend")
                            .on_click(move |_, _, cx| {
                                amend_toggle.update(cx, |this, cx| {
                                    if let Some(panel) = &mut this.commit {
                                        panel.amend = !panel.amend;
                                        cx.notify();
                                    }
                                })
                            }),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_color(rgb(theme::ICON_CODE))
                            .child(format!("{ticked} of {count} selected")),
                    ),
            )
            .child(
                div()
                    .h(px(120.))
                    .flex_shrink_0()
                    .child(Textarea::new(&message).h(px(120.))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .child(
                        Button::new("commit")
                            .primary()
                            .label("Commit")
                            .disabled(!can_commit)
                            .on_click(move |_, _, cx| {
                                commit_button.update(cx, |this, cx| this.commit_selected(false, cx))
                            }),
                    )
                    .child(
                        Button::new("commit-push")
                            .label("Commit and Push…")
                            .disabled(!can_commit)
                            .on_click(move |_, _, cx| {
                                push_button.update(cx, |this, cx| this.commit_selected(true, cx))
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            from: None,
            kind,
        }
    }

    fn sample() -> Changes {
        let mut changes = Changes::default();
        changes.set_changes(vec![
            change(".atl/a.json", ChangeKind::Modified),
            change(".atl/b.md", ChangeKind::Modified),
            change("apps/api/src/card/create.ts", ChangeKind::Modified),
            change("apps/api/src/card/infra/repo.ts", ChangeKind::Added),
        ]);
        changes
    }

    fn names(changes: &Changes) -> Vec<(String, usize, usize)> {
        changes
            .rows()
            .into_iter()
            .map(|r| (r.name, r.depth, r.count))
            .collect()
    }

    #[::core::prelude::v1::test]
    fn chains_of_single_directories_are_one_row() {
        let rows = names(&sample());
        assert_eq!(
            rows,
            [
                ("Changes".to_string(), 0, 4),
                (".atl".to_string(), 1, 2),
                ("a.json".to_string(), 2, 1),
                ("b.md".to_string(), 2, 1),
                ("apps/api/src/card".to_string(), 1, 2),
                ("infra".to_string(), 2, 1),
                ("repo.ts".to_string(), 3, 1),
                ("create.ts".to_string(), 2, 1),
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn a_directory_ticks_its_files_and_shows_a_mix() {
        let mut changes = sample();
        assert_eq!(changes.ticked(), 4);
        changes.toggle_check(".atl");
        assert_eq!(changes.ticked(), 2);
        let check = |changes: &Changes, key: &str| {
            changes
                .rows()
                .into_iter()
                .find(|r| r.key == key)
                .unwrap()
                .check
        };
        assert_eq!(check(&changes, ".atl"), Check::Off);
        assert_eq!(check(&changes, ""), Check::Some);
        changes.toggle_check("");
        assert_eq!(check(&changes, ""), Check::On);
        changes.toggle_check("");
        assert_eq!(changes.ticked(), 0);
    }

    #[::core::prelude::v1::test]
    fn folding_hides_the_rows_below() {
        let mut changes = sample();
        changes.toggle_fold(".atl");
        assert_eq!(changes.rows().len(), 6);
        changes.toggle_fold("");
        assert_eq!(changes.rows().len(), 1);
        changes.toggle_fold("");
        changes.toggle_fold(".atl");
        assert_eq!(changes.rows().len(), 8);
    }

    #[::core::prelude::v1::test]
    fn a_refresh_keeps_what_the_user_unticked_and_ticks_what_is_new() {
        let mut changes = sample();
        changes.toggle_check(".atl");
        changes.set_changes(vec![
            change(".atl/a.json", ChangeKind::Modified),
            change("new.rs", ChangeKind::Untracked),
        ]);
        assert_eq!(changes.paths(), ["new.rs"]);
    }

    #[::core::prelude::v1::test]
    fn the_notification_counts_the_files_of_the_commit() {
        assert_eq!(done_message(1, false, false), "Committed 1 file");
        assert_eq!(done_message(3, true, false), "Committed 3 files and pushed");
        assert_eq!(done_message(0, false, true), "Amended the last commit");
        assert_eq!(
            done_message(2, false, true),
            "Amended the last commit with 2 files"
        );
    }

    #[::core::prelude::v1::test]
    fn a_ticked_rename_hands_git_both_paths() {
        let mut changes = Changes::default();
        changes.set_changes(vec![FileChange {
            path: "new.rs".into(),
            from: Some("old.rs".into()),
            kind: ChangeKind::Renamed,
        }]);
        assert_eq!(changes.paths(), ["new.rs", "old.rs"]);
    }
}
