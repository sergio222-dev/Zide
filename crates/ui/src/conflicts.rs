//! The conflict resolver: when a merge, rebase, cherry-pick or revert stops on conflicts, a dialog
//! lists the files in conflict with what each side did to them. *Accept Yours* keeps the current
//! branch's version of the selected file, *Accept Theirs* the incoming one; *Merge…* opens
//! the file in the merge editor to settle it by hand. It is reached from Find Action, which offers it only while
//! there are conflicts.

use std::sync::Arc;

use gpui_kit::component::Disableable;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{Conflict, Conflicts, Edit, Operation, Side};

use crate::commit::{Check, check_box};
use crate::{Workspace, file_icon, theme};

const ROW_HEIGHT: f32 = 24.;

/// A line of the list: a folder that holds conflicted files (when grouped), or one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Dir {
        label: String,
        count: usize,
    },
    File {
        /// Position in the sorted list of files.
        index: usize,
        name: String,
        /// The folder of the file, shown beside its name when the list is not grouped.
        dir: String,
        yours: &'static str,
        theirs: &'static str,
    },
}

pub struct ConflictsPopup {
    data: Conflicts,
    selected: usize,
    /// Files are listed under their folder.
    pub grouped: bool,
    /// A file is being resolved.
    busy: bool,
    scroll: UniformListScrollHandle,
}

/// The folder a path is in, `""` at the top.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl ConflictsPopup {
    pub fn new(data: Conflicts) -> Self {
        let mut popup = Self {
            data: Conflicts {
                files: Vec::new(),
                ..data.clone()
            },
            selected: 0,
            grouped: false,
            busy: false,
            scroll: UniformListScrollHandle::new(),
        };
        popup.replace(data);
        popup
    }

    /// Takes the conflicts git still reports, keeping the selected file when it is still there.
    pub fn replace(&mut self, mut data: Conflicts) {
        let kept = self
            .selected_conflict()
            .map(|conflict| conflict.path.clone());
        // by folder, so the list reads the same grouped or not
        data.files
            .sort_by(|a, b| (folder(&a.path), &a.path).cmp(&(folder(&b.path), &b.path)));
        self.data = data;
        self.selected = kept
            .and_then(|path| self.data.files.iter().position(|c| c.path == path))
            .unwrap_or_else(|| self.selected.min(self.data.files.len().saturating_sub(1)));
        self.busy = false;
    }

    /// The names of your side and of theirs.
    pub fn names(&self) -> (String, String) {
        (self.data.yours.clone(), self.data.theirs.clone())
    }

    /// The file can be merged by hand: no side deleted it.
    pub fn can_merge(&self) -> bool {
        self.selected_conflict()
            .is_some_and(|c| c.yours != Edit::Deleted && c.theirs != Edit::Deleted)
    }

    pub fn selected_conflict(&self) -> Option<&Conflict> {
        self.data.files.get(self.selected)
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.data.files.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.data.files.len().saturating_sub(1));
    }

    /// The lines to show: the files, with a line for each folder when grouped. `root` names the
    /// folder of the files at the top of the repository.
    pub fn rows(&self, root: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut current: Option<&str> = None;
        for (index, conflict) in self.data.files.iter().enumerate() {
            let dir = folder(&conflict.path);
            if self.grouped && current != Some(dir) {
                current = Some(dir);
                let count = self
                    .data
                    .files
                    .iter()
                    .filter(|c| folder(&c.path) == dir)
                    .count();
                rows.push(Row::Dir {
                    label: if dir.is_empty() { root } else { dir }.to_string(),
                    count,
                });
            }
            let name = conflict.path.rsplit('/').next().unwrap_or(&conflict.path);
            rows.push(Row::File {
                index,
                name: name.to_string(),
                dir: dir.to_string(),
                yours: conflict.yours.label(),
                theirs: conflict.theirs.label(),
            });
        }
        rows
    }
}

impl Workspace {
    /// Find Action → *Resolve Conflicts…*: opens the dialog when git has conflicts to show.
    pub(crate) fn open_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { zide_git::conflicts(&root) })
                .await;
            this.update_in(cx, |this, window, cx| match found {
                Some(found) => {
                    this.popup = None;
                    this.branches = None;
                    this.push = None;
                    this.git_op = None;
                    this.conflicts = Some(ConflictsPopup::new(found));
                    this.focus.focus(window, cx); // the keys are read by the workspace
                    cx.notify();
                }
                None => this.notify(Notification::info("There are no conflicts to resolve"), cx),
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn close_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.conflicts = None;
        if self.claude_open {
            self.focus_active_chat(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Keeps `side`'s version of the selected file and marks it resolved. Once the last file is,
    /// the dialog closes.
    fn accept(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &mut self.conflicts else {
            return;
        };
        let Some(conflict) = popup.selected_conflict().cloned() else {
            return;
        };
        if std::mem::replace(&mut popup.busy, true) {
            return;
        }
        self.autosave(cx);
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let resolved = zide_git::resolve(&root, &conflict, side);
                    (resolved, zide_git::conflicts(&root))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.finish_resolve(outcome, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// What follows settling a file: shows the conflicts git still reports, or closes the dialog
    /// once there are none.
    pub(crate) fn finish_resolve(
        &mut self,
        outcome: (Result<(), String>, Option<Conflicts>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (resolved, remaining) = outcome;
        if let Err(why) = resolved {
            self.notify(Notification::error(why).title("Could not resolve it"), cx);
        }
        match (remaining, &mut self.conflicts) {
            (Some(remaining), Some(popup)) => popup.replace(remaining),
            (None, Some(_)) => {
                let finish = self.conflicts.as_ref().map(|popup| popup.data.operation);
                self.close_conflicts(window, cx);
                let done = match finish {
                    Some(Operation::Merge) => {
                        "Every conflict is resolved: commit to finish the merge"
                    }
                    _ => "Every conflict is resolved",
                };
                self.notify(Notification::success(done), cx);
            }
            _ => {}
        }
        self.rescan(cx); // the files changed on disk
        self.refresh_commit(cx);
        cx.notify();
    }

    /// Keys while the dialog is open: it takes all of them.
    pub(crate) fn conflicts_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zide_vim::Key;
        cx.stop_propagation();
        let Some(popup) = &mut self.conflicts else {
            return;
        };
        match (event.keystroke.key.as_str(), crate::vim_key(event)) {
            ("escape", _) => return self.close_conflicts(window, cx),
            ("down", _) | (_, Some(Key::Char('j'))) => popup.move_by(1),
            ("up", _) | (_, Some(Key::Char('k'))) => popup.move_by(-1),
            _ => return,
        }
        let root = self.root_name();
        if let Some(popup) = &self.conflicts {
            let at = popup
                .rows(&root)
                .iter()
                .position(|row| matches!(row, Row::File { index, .. } if *index == popup.selected));
            if let Some(at) = at {
                popup.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
            }
        }
        cx.notify();
    }

    pub(crate) fn conflicts_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let popup = self.conflicts.as_ref().expect("the dialog is open");
        let workspace = cx.entity();
        let rows = Arc::new(popup.rows(&self.root_name()));
        let (selected, grouped, busy) = (popup.selected, popup.grouped, popup.busy);
        let can_accept = !busy && popup.selected_conflict().is_some();
        let scroll = popup.scroll.clone();

        let name = |text: &str| {
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(theme::STRONG))
                .child(text.to_string())
        };
        let words = |text: &str| div().child(text.to_string());
        let (yours, theirs) = (popup.data.yours.clone(), popup.data.theirs.clone());
        let heading = div().flex().gap_1().whitespace_nowrap();
        let heading = match popup.data.operation {
            Operation::Merge => heading
                .child(words("Merging branch"))
                .child(name(&theirs))
                .child(words("into branch"))
                .child(name(&yours)),
            Operation::Rebase => heading
                .child(words("Rebasing branch"))
                .child(name(&theirs))
                .child(words("onto"))
                .child(name(&yours)),
            Operation::CherryPick => heading
                .child(words("Cherry-picking commit"))
                .child(name(&theirs))
                .child(words("onto branch"))
                .child(name(&yours)),
            Operation::Revert => heading
                .child(words("Reverting commit"))
                .child(name(&theirs))
                .child(words("on branch"))
                .child(name(&yours)),
        };

        let column = |label: String| {
            div()
                .w_1_3()
                .px_2()
                .flex_shrink_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label)
        };
        let header = div()
            .flex()
            .items_center()
            .h(px(ROW_HEIGHT))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::MUTED))
            .child(column("Name".into()))
            .child(column(format!("Yours ({yours})")))
            .child(column(format!("Theirs ({theirs})")));

        let list = uniform_list("conflict-rows", rows.len(), {
            let workspace = workspace.clone();
            move |range, _, _| {
                range
                    .map(|position| {
                        let line = div()
                            .id(("conflict-row", position))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center();
                        match &rows[position] {
                            Row::Dir { label, count } => {
                                let (icon, color) = file_icon::for_row(label, true, true);
                                line.px_2()
                                    .gap_1()
                                    .child(div().w(px(12.)).child("▾"))
                                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                    .child(label.clone())
                                    .child(div().text_color(rgb(theme::DIM)).child(format!(
                                        "{count} file{}",
                                        if *count == 1 { "" } else { "s" }
                                    )))
                                    .into_any_element()
                            }
                            Row::File {
                                index,
                                name,
                                dir,
                                yours,
                                theirs,
                            } => {
                                let (index, workspace) = (*index, workspace.clone());
                                let (icon, color) = file_icon::for_row(name, false, false);
                                line.cursor_pointer()
                                    .when(index == selected, |row| row.bg(rgb(theme::TREE_FOCUSED)))
                                    .on_click(move |event, window, cx| {
                                        let double = event.click_count() == 2;
                                        workspace.update(cx, |this, cx| {
                                            if let Some(popup) = &mut this.conflicts {
                                                popup.select(index);
                                                cx.notify();
                                            }
                                            // a double click settles the file by hand
                                            let can_merge =
                                                this.conflicts.as_ref().is_some_and(|popup| {
                                                    popup.can_merge() && !popup.busy
                                                });
                                            if double && can_merge {
                                                this.open_merge(window, cx);
                                            }
                                        })
                                    })
                                    .child(
                                        div()
                                            .w_1_3()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .px_2()
                                            .when(grouped, |d| d.pl(px(30.)))
                                            .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                            .child(name.clone())
                                            .when(!grouped && !dir.is_empty(), |d| {
                                                d.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(theme::DIM))
                                                        .child(dir.clone()),
                                                )
                                            }),
                                    )
                                    .child(column((*yours).to_string()))
                                    .child(column((*theirs).to_string()))
                                    .into_any_element()
                            }
                        }
                    })
                    .collect()
            }
        })
        .size_full()
        .track_scroll(&scroll);

        let can_merge = !busy && popup.can_merge();
        let (yours_button, theirs_button, close_button, group_toggle, merge_button) = (
            workspace.clone(),
            workspace.clone(),
            workspace.clone(),
            workspace.clone(),
            workspace,
        );
        div()
            .id("conflicts-overlay")
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .pt(px(56.))
            .child(
                div()
                    .w(px(900.))
                    .h(px(440.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .bg(rgb(theme::POPUP))
                    .text_color(rgb(theme::TEXT))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .child(heading)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .bg(rgb(theme::BACKGROUND))
                                    .border_1()
                                    .border_color(rgb(theme::BORDER))
                                    .child(header)
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .min_h_0()
                                            .child(list)
                                            .child(Scrollbar::new(&scroll)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .w(px(120.))
                                    .flex_shrink_0()
                                    .child(
                                        Button::new("accept-yours")
                                            .label("Accept Yours")
                                            .disabled(!can_accept)
                                            .on_click(move |_, window, cx| {
                                                yours_button.update(cx, |this, cx| {
                                                    this.accept(Side::Yours, window, cx)
                                                })
                                            }),
                                    )
                                    .child(
                                        Button::new("accept-theirs")
                                            .label("Accept Theirs")
                                            .disabled(!can_accept)
                                            .on_click(move |_, window, cx| {
                                                theirs_button.update(cx, |this, cx| {
                                                    this.accept(Side::Theirs, window, cx)
                                                })
                                            }),
                                    )
                                    .child(
                                        Button::new("merge-by-hand")
                                            .primary()
                                            .label("Merge…")
                                            .disabled(!can_merge)
                                            .on_click(move |_, window, cx| {
                                                merge_button.update(cx, |this, cx| {
                                                    this.open_merge(window, cx)
                                                })
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id("conflicts-group")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(check_box(if grouped { Check::On } else { Check::Off }))
                                    .child("Group files by directory")
                                    .on_click(move |_, _, cx| {
                                        group_toggle.update(cx, |this, cx| {
                                            if let Some(popup) = &mut this.conflicts {
                                                popup.grouped = !popup.grouped;
                                                cx.notify();
                                            }
                                        })
                                    }),
                            )
                            .child(div().ml_auto().child(
                                Button::new("conflicts-close").label("Close").on_click(
                                    move |_, window, cx| {
                                        close_button
                                            .update(cx, |this, cx| this.close_conflicts(window, cx))
                                    },
                                ),
                            )),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zide_git::Edit;

    fn conflict(path: &str) -> Conflict {
        Conflict {
            path: path.into(),
            yours: Edit::Modified,
            theirs: Edit::Deleted,
        }
    }

    fn conflicts(paths: &[&str]) -> Conflicts {
        Conflicts {
            operation: Operation::Merge,
            yours: "main".into(),
            theirs: "feature-branch".into(),
            files: paths.iter().map(|p| conflict(p)).collect(),
        }
    }

    #[::core::prelude::v1::test]
    fn files_are_listed_by_folder_and_grouped_on_demand() {
        let mut popup = ConflictsPopup::new(conflicts(&["src/b.rs", "top.txt", "src/a.rs"]));
        let flat: Vec<_> = popup
            .rows("zide")
            .into_iter()
            .map(|row| match row {
                Row::File { name, dir, .. } => format!("{dir}/{name}"),
                Row::Dir { .. } => unreachable!("not grouped"),
            })
            .collect();
        assert_eq!(flat, ["/top.txt", "src/a.rs", "src/b.rs"]);
        popup.grouped = true;
        let shown: Vec<_> = popup
            .rows("zide")
            .into_iter()
            .map(|row| match row {
                Row::Dir { label, count } => format!("{label}/ {count}"),
                Row::File { name, .. } => name,
            })
            .collect();
        assert_eq!(shown, ["zide/ 1", "top.txt", "src/ 2", "a.rs", "b.rs"]);
    }

    #[::core::prelude::v1::test]
    fn the_selection_stays_on_its_file_when_the_list_shrinks() {
        let mut popup = ConflictsPopup::new(conflicts(&["a", "b", "c"]));
        popup.move_by(1);
        popup.move_by(50);
        assert_eq!(popup.selected_conflict().unwrap().path, "c");
        popup.replace(conflicts(&["a", "c"]));
        assert_eq!(popup.selected_conflict().unwrap().path, "c");
        popup.replace(conflicts(&["a"]));
        assert_eq!(popup.selected_conflict().unwrap().path, "a");
        popup.replace(conflicts(&[]));
        assert!(popup.selected_conflict().is_none());
    }

    #[::core::prelude::v1::test]
    fn each_row_says_what_both_sides_did() {
        let popup = ConflictsPopup::new(conflicts(&["f.txt"]));
        assert!(matches!(
            &popup.rows("zide")[0],
            Row::File {
                yours: "Modified",
                theirs: "Deleted",
                ..
            }
        ));
    }
}
