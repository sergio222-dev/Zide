//! The push dialog (Ctrl+Shift+K): the commits the remote lacks on the left, the files of the
//! selected commit on the right with the commit's details under them. Enter or a double click on
//! a file shows what the commit changed in it; Ctrl+Enter (or the button) pushes.

use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{ChangeKind, Commit, FileChange, Outgoing};

use crate::commit::{Check, check_box, kind_color};
use crate::{Workspace, file_icon, theme};

const ROW_HEIGHT: f32 = 24.;

/// What the keyboard moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Commits,
    Files,
}

/// What the dialog knows of the commits to push.
enum Commits {
    Loading,
    Failed(String),
    Ready(Outgoing),
}

/// The files of the selected commit.
enum Files {
    Loading,
    Failed(String),
    /// Sorted by folder, then path.
    Ready(Vec<FileChange>),
}

/// A line of the files list: a folder that holds files, or one of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRow {
    Dir {
        label: String,
        count: usize,
    },
    File {
        /// Position in the sorted list of files.
        index: usize,
        name: String,
        kind: ChangeKind,
    },
}

pub struct PushPopup {
    commits: Commits,
    commit: usize,
    files: Files,
    file: usize,
    pub pane: Pane,
    /// Push the tags too.
    pub tags: bool,
    commit_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
}

impl Default for PushPopup {
    fn default() -> Self {
        Self {
            commits: Commits::Loading,
            commit: 0,
            files: Files::Loading,
            file: 0,
            pane: Pane::default(),
            tags: false,
            commit_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
        }
    }
}

impl PushPopup {
    /// Takes what git said of the branch: its commits, or why there are none to show.
    pub fn set_outgoing(&mut self, found: Result<Outgoing, String>) {
        self.commit = 0;
        self.files = Files::Loading;
        self.commits = match found {
            Ok(outgoing) => Commits::Ready(outgoing),
            Err(why) => Commits::Failed(why),
        };
    }

    pub fn outgoing(&self) -> Option<&Outgoing> {
        match &self.commits {
            Commits::Ready(outgoing) => Some(outgoing),
            _ => None,
        }
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.outgoing()?.commits.get(self.commit)
    }

    pub fn can_push(&self) -> bool {
        self.outgoing()
            .is_some_and(|outgoing| !outgoing.commits.is_empty())
    }

    /// Selects the commit `index`; true when that is another one than before.
    pub fn select_commit(&mut self, index: usize) -> bool {
        let last = self
            .outgoing()
            .map_or(0, |outgoing| outgoing.commits.len().saturating_sub(1));
        let index = index.min(last);
        let changed = index != self.commit;
        self.commit = index;
        if changed {
            self.files = Files::Loading;
        }
        changed
    }

    pub fn move_commit(&mut self, delta: isize) -> bool {
        self.select_commit(self.commit.saturating_add_signed(delta))
    }

    /// Marks the files of the selected commit as being read; its hash, to read them.
    pub fn start_files_load(&mut self) -> Option<String> {
        let hash = self.selected_commit()?.hash.clone();
        self.files = Files::Loading;
        self.file = 0;
        Some(hash)
    }

    /// Takes the files read for the commit `hash`; they are dropped if another commit was
    /// selected meanwhile.
    pub fn set_files(&mut self, hash: &str, found: Result<Vec<FileChange>, String>) {
        if self.selected_commit().map(|commit| commit.hash.as_str()) != Some(hash) {
            return;
        }
        self.file = 0;
        self.files = match found {
            Ok(mut list) => {
                list.sort_by(|a, b| (folder(&a.path), &a.path).cmp(&(folder(&b.path), &b.path)));
                Files::Ready(list)
            }
            Err(why) => Files::Failed(why),
        };
    }

    fn file_list(&self) -> &[FileChange] {
        match &self.files {
            Files::Ready(list) => list,
            _ => &[],
        }
    }

    pub fn move_file(&mut self, delta: isize) {
        let last = self.file_list().len().saturating_sub(1);
        self.file = self.file.saturating_add_signed(delta).min(last);
    }

    pub fn select_file(&mut self, index: usize) {
        self.file = index.min(self.file_list().len().saturating_sub(1));
    }

    pub fn selected_file(&self) -> Option<&FileChange> {
        self.file_list().get(self.file)
    }

    /// The files under a line for each folder that holds some. `root` names the folder of the
    /// files that sit at the top of the repository.
    pub fn file_rows(&self, root: &str) -> Vec<FileRow> {
        let list = self.file_list();
        let mut rows: Vec<FileRow> = Vec::new();
        let mut current: Option<&str> = None;
        for (index, change) in list.iter().enumerate() {
            let dir = folder(&change.path);
            if current != Some(dir) {
                current = Some(dir);
                let count = list.iter().filter(|c| folder(&c.path) == dir).count();
                let label = if dir.is_empty() { root } else { dir };
                rows.push(FileRow::Dir {
                    label: label.to_string(),
                    count,
                });
            }
            let name = change.path.rsplit('/').next().unwrap_or(&change.path);
            rows.push(FileRow::File {
                index,
                name: name.to_string(),
                kind: change.kind,
            });
        }
        rows
    }
}

/// The folder a path is in, `""` at the top.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn plural(count: usize, one: &str) -> String {
    format!("{count} {one}{}", if count == 1 { "" } else { "s" })
}

impl Workspace {
    /// Ctrl+Shift+K: shows what a push would publish; from the dialog, closes it.
    pub(crate) fn toggle_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.push.is_some() {
            return self.close_push(window, cx);
        }
        self.popup = None;
        self.branches = None;
        self.cancel_running_search();
        self.push = Some(PushPopup::default());
        self.focus.focus(window, cx); // the keys are read by the workspace
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { zide_git::outgoing(&root) })
                .await;
            this.update(cx, |this, cx| {
                let Some(popup) = &mut this.push else {
                    return; // closed before git answered
                };
                popup.set_outgoing(found);
                this.load_push_files(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push = None;
        if self.claude_open {
            self.focus_active_chat(window, cx); // back to the message box
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Reads, off the UI thread, the files of the selected commit.
    fn load_push_files(&mut self, cx: &mut Context<Self>) {
        let Some(hash) = self.push.as_mut().and_then(PushPopup::start_files_load) else {
            return;
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let wanted = hash.clone();
            let files = cx
                .background_executor()
                .spawn(async move { zide_git::commit_files(&root, &hash) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(popup) = &mut this.push {
                    popup.set_files(&wanted, files);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn select_push_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(popup) = &mut self.push else {
            return;
        };
        popup.pane = Pane::Commits;
        if popup.select_commit(index) {
            self.load_push_files(cx);
        }
        cx.notify();
    }

    /// Shows what the selected commit changed in the selected file.
    fn show_push_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.push else {
            return;
        };
        let (Some(commit), Some(change)) = (popup.selected_commit(), popup.selected_file()) else {
            return;
        };
        let (hash, change) = (commit.hash.clone(), change.clone());
        self.show_commit_diff(&hash, change, window, cx);
    }

    /// Pushes the branch (and the tags, when asked) and closes the dialog; the outcome comes as a
    /// notification.
    fn run_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.push else {
            return;
        };
        let Some(outgoing) = popup.outgoing().filter(|_| popup.can_push()).cloned() else {
            return;
        };
        let tags = popup.tags;
        let done = format!(
            "Pushed {} to {}/{}",
            plural(outgoing.commits.len(), "commit"),
            outgoing.remote,
            outgoing.branch
        );
        self.close_push(window, cx);
        self.git_task(
            "Could not push",
            Some(done),
            move |root| {
                zide_git::push_branch(&root, &outgoing.branch, &outgoing.remote)?;
                if tags {
                    zide_git::push_tags(&root, &outgoing.remote)?;
                }
                Ok(())
            },
            cx,
        );
    }

    /// Tab: the keyboard goes to the other list.
    pub(crate) fn push_switch_pane(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.push {
            popup.pane = match popup.pane {
                Pane::Commits => Pane::Files,
                Pane::Files => Pane::Commits,
            };
            cx.notify();
        }
    }

    /// Keys while the dialog is open: it takes all of them, the editor reads none.
    pub(crate) fn push_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zide_vim::Key;
        cx.stop_propagation();
        let Some(popup) = &mut self.push else {
            return;
        };
        let key = crate::vim_key(event);
        let pane = popup.pane;
        let modifiers = event.keystroke.modifiers;
        match (event.keystroke.key.as_str(), key) {
            ("escape", _) => return self.close_push(window, cx),
            ("enter", _) if modifiers.control => return self.run_push(window, cx),
            ("enter", _) => match pane {
                Pane::Commits => popup.pane = Pane::Files,
                Pane::Files => return self.show_push_diff(window, cx),
            },
            ("down", _) | (_, Some(Key::Char('j'))) => match pane {
                Pane::Commits => {
                    if popup.move_commit(1) {
                        self.load_push_files(cx);
                    }
                }
                Pane::Files => popup.move_file(1),
            },
            ("up", _) | (_, Some(Key::Char('k'))) => match pane {
                Pane::Commits => {
                    if popup.move_commit(-1) {
                        self.load_push_files(cx);
                    }
                }
                Pane::Files => popup.move_file(-1),
            },
            ("left", _) | (_, Some(Key::Char('h'))) => popup.pane = Pane::Commits,
            ("right", _) | (_, Some(Key::Char('l'))) => popup.pane = Pane::Files,
            ("t", _) if modifiers.alt => popup.tags = !popup.tags,
            _ => return,
        }
        if let Some(popup) = &self.push {
            popup
                .commit_scroll
                .scroll_to_item(popup.commit, ScrollStrategy::Nearest);
            let root = self.root_name();
            let at = popup
                .file_rows(&root)
                .iter()
                .position(|row| matches!(row, FileRow::File { index, .. } if *index == popup.file));
            if let Some(at) = at {
                popup
                    .file_scroll
                    .scroll_to_item(at, ScrollStrategy::Nearest);
            }
        }
        cx.notify();
    }

    /// The name of the project's folder.
    fn root_name(&self) -> String {
        self.root
            .file_name()
            .map_or_else(|| ".".into(), |name| name.to_string_lossy().into_owned())
    }

    pub(crate) fn push_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let popup = self.push.as_ref().expect("the dialog is open");
        let workspace = cx.entity();
        let root = self.root_name();

        let commits = commit_list(popup, &workspace);
        let files = file_list(popup, &root, &workspace);
        let (can_push, tags, pane) = (popup.can_push(), popup.tags, popup.pane);
        let (push_button, cancel_button, tags_toggle) =
            (workspace.clone(), workspace.clone(), workspace);

        let heading = match &popup.commits {
            Commits::Ready(outgoing) => div()
                .flex()
                .gap_1()
                .whitespace_nowrap()
                .child(
                    div()
                        .text_color(rgb(theme::STRONG))
                        .child(outgoing.branch.clone()),
                )
                .child(div().text_color(rgb(theme::DIM)).child("→"))
                .child(div().text_color(rgb(theme::ICON_CODE)).child(format!(
                    "{} : {}{}",
                    outgoing.remote,
                    outgoing.branch,
                    if outgoing.on_remote { "" } else { " (new)" }
                ))),
            _ => div().text_color(rgb(theme::DIM)).child("Commits to push"),
        };

        div()
            .id("push-overlay")
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
                    .w(px(860.))
                    .h(px(520.))
                    .flex()
                    .flex_col()
                    .bg(rgb(theme::POPUP))
                    .text_color(rgb(theme::TEXT))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .child(
                        div()
                            .px_3()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .border_b_1()
                            .border_color(rgb(theme::BORDER))
                            .child(heading),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w(px(340.))
                                    .flex_shrink_0()
                                    .border_r_1()
                                    .border_color(rgb(theme::BORDER))
                                    .child(div().relative().flex_1().min_h_0().child(commits)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().flex_1().min_h_0().child(files))
                                    .child(commit_details(popup)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .h(px(48.))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id("push-tags")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .child(check_box(if tags { Check::On } else { Check::Off }))
                                    .child("Push tags")
                                    .on_click(move |_, _, cx| {
                                        tags_toggle.update(cx, |this, cx| {
                                            if let Some(popup) = &mut this.push {
                                                popup.tags = !popup.tags;
                                                cx.notify();
                                            }
                                        })
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme::DIM))
                                    .child(match pane {
                                        Pane::Commits => {
                                            "Enter: files · Tab: switch · Ctrl+Enter: push"
                                        }
                                        Pane::Files => {
                                            "Enter: diff · Tab: switch · Ctrl+Enter: push"
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("push-go")
                                            .primary()
                                            .label("Push")
                                            .disabled(!can_push)
                                            .on_click(move |_, window, cx| {
                                                push_button.update(cx, |this, cx| {
                                                    this.run_push(window, cx)
                                                })
                                            }),
                                    )
                                    .child(Button::new("push-cancel").label("Cancel").on_click(
                                        move |_, window, cx| {
                                            cancel_button
                                                .update(cx, |this, cx| this.close_push(window, cx))
                                        },
                                    )),
                            ),
                    ),
            )
    }
}

/// A line of text in the place of a list that has nothing to show.
fn note(text: impl Into<SharedString>, loading: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .p_3()
        .text_sm()
        .text_color(rgb(theme::MUTED))
        .children(loading.then(|| Spinner::new().small().color(rgb(theme::MUTED).into())))
        .child(text.into())
}

fn commit_list(popup: &PushPopup, workspace: &Entity<Workspace>) -> AnyElement {
    let outgoing = match &popup.commits {
        Commits::Loading => return note("Loading", true).into_any_element(),
        Commits::Failed(why) => return note(why.clone(), false).into_any_element(),
        Commits::Ready(outgoing) if outgoing.commits.is_empty() => {
            return note("Nothing to push: the remote has every commit", false).into_any_element();
        }
        Commits::Ready(outgoing) => outgoing,
    };
    let subjects: Arc<Vec<String>> =
        Arc::new(outgoing.commits.iter().map(|c| c.subject.clone()).collect());
    let (selected, focused) = (popup.commit, popup.pane == Pane::Commits);
    let workspace = workspace.clone();
    let scroll = popup.commit_scroll.clone();
    div()
        .relative()
        .size_full()
        .child(
            uniform_list("push-commits", subjects.len(), move |range, _, _| {
                range
                    .map(|index| {
                        let workspace = workspace.clone();
                        div()
                            .id(("push-commit", index))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .px_2()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .cursor_pointer()
                            .when(index == selected, |row| {
                                row.bg(rgb(if focused {
                                    theme::TREE_FOCUSED
                                } else {
                                    theme::TREE_SELECTED
                                }))
                            })
                            .on_click(move |_, _, cx| {
                                workspace.update(cx, |this, cx| this.select_push_commit(index, cx))
                            })
                            .child(subjects[index].clone())
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(&scroll),
        )
        .child(Scrollbar::new(&popup.commit_scroll))
        .into_any_element()
}

fn file_list(popup: &PushPopup, root: &str, workspace: &Entity<Workspace>) -> AnyElement {
    match &popup.files {
        Files::Loading => {
            return note(
                if popup.can_push() { "Loading" } else { "" },
                popup.can_push(),
            )
            .into_any_element();
        }
        Files::Failed(why) => return note(why.clone(), false).into_any_element(),
        Files::Ready(_) => {}
    }
    let rows = Arc::new(popup.file_rows(root));
    let (selected, focused) = (popup.file, popup.pane == Pane::Files);
    let workspace = workspace.clone();
    let scroll = popup.file_scroll.clone();
    div()
        .relative()
        .size_full()
        .child(
            uniform_list("push-files", rows.len(), move |range, _, _| {
                range
                    .map(|position| {
                        let row = div()
                            .id(("push-file", position))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .whitespace_nowrap();
                        match &rows[position] {
                            FileRow::Dir { label, count } => {
                                let (icon, color) = file_icon::for_row(label, true, true);
                                row.child(div().w(px(12.)).child("▾"))
                                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                    .child(label.clone())
                                    .child(
                                        div()
                                            .text_color(rgb(theme::DIM))
                                            .child(plural(*count, "file")),
                                    )
                                    .into_any_element()
                            }
                            FileRow::File { index, name, kind } => {
                                let index = *index;
                                let workspace = workspace.clone();
                                let (icon, color) = file_icon::for_row(name, false, false);
                                row.pl(px(32.))
                                    .when(index == selected, |row| {
                                        row.bg(rgb(if focused {
                                            theme::TREE_FOCUSED
                                        } else {
                                            theme::TREE_SELECTED
                                        }))
                                    })
                                    .cursor_pointer()
                                    .on_click(move |event, window, cx| {
                                        workspace.update(cx, |this, cx| {
                                            if let Some(popup) = &mut this.push {
                                                popup.pane = Pane::Files;
                                                popup.select_file(index);
                                            }
                                            if event.click_count() == 2 {
                                                this.show_push_diff(window, cx);
                                            }
                                            cx.notify();
                                        })
                                    })
                                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                    .child(
                                        div()
                                            .text_color(rgb(kind_color(*kind)))
                                            .child(name.clone()),
                                    )
                                    .into_any_element()
                            }
                        }
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(&scroll),
        )
        .child(Scrollbar::new(&popup.file_scroll))
        .into_any_element()
}

/// Under the files: the message of the selected commit and who made it, when.
fn commit_details(popup: &PushPopup) -> Div {
    let panel = div()
        .flex()
        .flex_col()
        .gap_2()
        .flex_shrink_0()
        .h(px(150.))
        .p_3()
        .overflow_hidden()
        .border_t_1()
        .border_color(rgb(theme::BORDER));
    let Some(commit) = popup.selected_commit() else {
        return panel;
    };
    let (date, clock) = zide_git::date_and_time(commit.time, commit.tz_minutes);
    panel
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(theme::STRONG))
                .child(commit.subject.clone()),
        )
        .children((!commit.body.is_empty()).then(|| {
            div()
                .text_sm()
                .text_color(rgb(theme::MUTED))
                .child(commit.body.clone())
        }))
        .child(
            div()
                .flex()
                .gap_1()
                .text_sm()
                .text_color(rgb(theme::MUTED))
                .child(format!("{} {}", commit.short_hash(), commit.author))
                .child(
                    div()
                        .text_color(rgb(theme::ICON_CODE))
                        .child(format!("<{}>", commit.email)),
                )
                .child(format!("on {date} at {clock}")),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, subject: &str) -> Commit {
        Commit {
            hash: hash.into(),
            author: "Ada".into(),
            email: "ada@x.org".into(),
            time: 0,
            tz_minutes: 0,
            subject: subject.into(),
            body: String::new(),
        }
    }

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            from: None,
            kind,
        }
    }

    fn popup_with(commits: &[(&str, &str)]) -> PushPopup {
        let mut popup = PushPopup::default();
        popup.set_outgoing(Ok(Outgoing {
            branch: "feat/x".into(),
            remote: "origin".into(),
            on_remote: true,
            commits: commits.iter().map(|(h, s)| commit(h, s)).collect(),
        }));
        popup
    }

    #[::core::prelude::v1::test]
    fn only_a_branch_with_commits_can_be_pushed() {
        assert!(!PushPopup::default().can_push());
        assert!(!popup_with(&[]).can_push());
        assert!(popup_with(&[("a1", "one")]).can_push());
        let mut failed = PushPopup::default();
        failed.set_outgoing(Err("HEAD is detached".into()));
        assert!(!failed.can_push());
    }

    #[::core::prelude::v1::test]
    fn the_selection_stays_inside_the_commits_and_says_when_it_moved() {
        let mut popup = popup_with(&[("a1", "one"), ("b2", "two")]);
        assert!(!popup.move_commit(-1), "already on the first");
        assert!(popup.move_commit(1));
        assert!(!popup.move_commit(5), "stops on the last");
        assert_eq!(popup.selected_commit().unwrap().subject, "two");
    }

    #[::core::prelude::v1::test]
    fn files_of_another_commit_than_the_selected_are_dropped() {
        let mut popup = popup_with(&[("a1", "one"), ("b2", "two")]);
        popup.move_commit(1);
        popup.set_files("a1", Ok(vec![change("old.rs", ChangeKind::Added)]));
        assert!(popup.selected_file().is_none());
        popup.set_files("b2", Ok(vec![change("new.rs", ChangeKind::Added)]));
        assert_eq!(popup.selected_file().unwrap().path, "new.rs");
    }

    #[::core::prelude::v1::test]
    fn files_are_grouped_under_their_folder_and_the_selection_walks_them_in_order() {
        let mut popup = popup_with(&[("a1", "one")]);
        popup.set_files(
            "a1",
            Ok(vec![
                change("src/b.rs", ChangeKind::Modified),
                change("src/a/deep.rs", ChangeKind::Added),
                change("top.js", ChangeKind::Added),
                change("src/a.rs", ChangeKind::Deleted),
            ]),
        );
        let rows = popup.file_rows("zide");
        let shown: Vec<String> = rows
            .iter()
            .map(|row| match row {
                FileRow::Dir { label, count } => format!("{label}/ {count}"),
                FileRow::File { index, name, .. } => format!("{index}:{name}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "zide/ 1",
                "0:top.js",
                "src/ 2",
                "1:a.rs",
                "2:b.rs",
                "src/a/ 1",
                "3:deep.rs"
            ]
        );
        popup.move_file(1);
        assert_eq!(popup.selected_file().unwrap().path, "src/a.rs");
        popup.move_file(10);
        assert_eq!(popup.selected_file().unwrap().path, "src/a/deep.rs");
    }

    #[::core::prelude::v1::test]
    fn the_counts_read_in_singular_and_plural() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(3, "commit"), "3 commits");
    }
}
