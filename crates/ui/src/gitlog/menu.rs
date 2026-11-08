//! The context menu of a commit in the git log (right click): copy its hash, cherry-pick it,
//! reset the branch to it, revert it, start a branch or a tag on it.

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::commit_ops::{self, ResetMode};

use super::LogPanel;
use crate::branches::NameKind;
use crate::{Workspace, theme};

const ROW_HEIGHT: f32 = 26.;

/// What an entry of the menu does to the commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAction {
    CopyHash,
    CherryPick,
    Reset,
    Revert,
    NewBranch,
    NewTag,
}

pub const ENTRIES: [(CommitAction, &str); 6] = [
    (CommitAction::CopyHash, "Copy Revision Number"),
    (CommitAction::CherryPick, "Cherry-Pick"),
    (CommitAction::Reset, "Reset Current Branch to Here..."),
    (CommitAction::Revert, "Revert Commit"),
    (CommitAction::NewBranch, "New Branch..."),
    (CommitAction::NewTag, "New Tag..."),
];

/// An open menu: the commit it was asked on, where, and the entry the keyboard is on.
#[derive(Debug, Clone)]
pub struct CommitMenu {
    pub hash: String,
    pub subject: String,
    pub at: Point<Pixels>,
    pub row: usize,
}

impl CommitMenu {
    pub fn move_row(&mut self, delta: isize) {
        self.row = self.row.saturating_add_signed(delta).min(ENTRIES.len() - 1);
    }
}

/// How the commit is called in messages: its short hash and subject.
fn name(hash: &str, subject: &str) -> String {
    format!("{} {subject}", &hash[..hash.len().min(8)])
}

/// The content of the reset dialog: the four modes, the picked one explained.
pub struct ResetDialog {
    pub mode: ResetMode,
}

impl Render for ResetDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let rows = ResetMode::ALL.into_iter().map(|mode| {
            let pick = this.clone();
            div()
                .id(SharedString::from(format!("reset-{}", mode.label())))
                .flex()
                .items_center()
                .gap_2()
                .h(px(ROW_HEIGHT))
                .px_2()
                .cursor_pointer()
                .rounded_md()
                .when(mode == self.mode, |d| d.bg(rgb(theme::ACTIVE_BG)))
                .hover(|d| d.bg(rgb(theme::SELECTION)))
                .child(if mode == self.mode { "◉" } else { "○" })
                .child(mode.label())
                .on_click(move |_, _, cx| {
                    pick.update(cx, |dialog, cx| {
                        dialog.mode = mode;
                        cx.notify();
                    })
                })
        });
        div().flex().flex_col().gap_1().children(rows).child(
            div()
                .pt_1()
                .text_xs()
                .text_color(rgb(if self.mode == ResetMode::Hard {
                    theme::ERROR
                } else {
                    theme::MUTED
                }))
                .child(self.mode.describe()),
        )
    }
}

impl Workspace {
    /// Opens the menu of commit `index` at the mouse; the commit becomes the selected one.
    pub(super) fn open_commit_menu(
        &mut self,
        index: usize,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.select_log_commit(index, cx);
        if let Some(panel) = &mut self.log
            && let Some(entry) = panel.selected_entry()
        {
            panel.commit_menu = Some(CommitMenu {
                hash: entry.commit.hash.clone(),
                subject: entry.commit.subject.clone(),
                at,
                row: 0,
            });
        }
        cx.notify();
    }

    pub(super) fn close_commit_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.log {
            panel.commit_menu = None;
        }
        cx.notify();
    }

    /// Runs `action` on the commit of the open menu, and closes the menu.
    pub(super) fn commit_action(
        &mut self,
        action: CommitAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.log.as_mut().and_then(|panel| panel.commit_menu.take()) else {
            return;
        };
        let CommitMenu { hash, subject, .. } = menu;
        let called = name(&hash, &subject);
        match action {
            CommitAction::CopyHash => {
                cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                let note = Notification::success(format!("Copied {hash}"));
                self.notify(note, cx);
            }
            CommitAction::CherryPick => self.git_task(
                "Could not cherry-pick the commit",
                Some(format!("Cherry-picked {called}")),
                move |root| commit_ops::cherry_pick(&root, &hash),
                cx,
            ),
            CommitAction::Revert => self.git_task(
                "Could not revert the commit",
                Some(format!("Reverted {called}")),
                move |root| commit_ops::revert(&root, &hash),
                cx,
            ),
            CommitAction::Reset => self.confirm_reset(hash, called, window, cx),
            CommitAction::NewBranch => {
                let existing = self
                    .log
                    .as_ref()
                    .and_then(|panel| panel.refs.as_ref())
                    .map(|model| model.local_names())
                    .unwrap_or_default();
                self.ask_name(
                    NameKind::Branch,
                    "New Branch",
                    "Create",
                    String::new(),
                    existing,
                    move |this, branch, cx| {
                        let done = format!("Created '{branch}' from {called}");
                        let hash = hash.clone();
                        this.git_task(
                            "Could not create the branch",
                            Some(done),
                            move |root| zide_git::create_branch(&root, &branch, &hash),
                            cx,
                        );
                    },
                    window,
                    cx,
                );
            }
            CommitAction::NewTag => self.ask_name(
                NameKind::Tag,
                "New Tag",
                "Create",
                String::new(),
                Vec::new(),
                move |this, tag, cx| {
                    let done = format!("Tagged {called} as '{tag}'");
                    let hash = hash.clone();
                    this.git_task(
                        "Could not create the tag",
                        Some(done),
                        move |root| commit_ops::create_tag(&root, &tag, &hash),
                        cx,
                    );
                },
                window,
                cx,
            ),
        }
    }

    /// Moving the branch can drop work, so the mode is asked first.
    fn confirm_reset(
        &mut self,
        hash: String,
        called: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|_| ResetDialog {
            mode: ResetMode::default(),
        });
        let workspace = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (dialog, workspace) = (dialog.clone(), workspace.clone());
            let (hash, called) = (hash.clone(), called.clone());
            alert
                .title("Reset Current Branch")
                .description(format!("Move the current branch to {called}?"))
                .child(dialog.clone())
                .ok_text("Reset")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let mode = dialog.read(cx).mode;
                    let (hash, called) = (hash.clone(), called.clone());
                    workspace.update(cx, |this, cx| {
                        let done = format!("Reset the branch to {called} ({})", mode.label());
                        this.git_task(
                            "Could not reset the branch",
                            Some(done),
                            move |root| commit_ops::reset(&root, &hash, mode),
                            cx,
                        );
                    });
                    true
                })
        });
    }
}

/// The menu, drawn over everything at the mouse, with a layer under it that closes it on a click
/// anywhere else.
pub fn view(menu: &CommitMenu, workspace: &Entity<Workspace>) -> impl IntoElement {
    let backdrop = {
        let (left, right) = (workspace.clone(), workspace.clone());
        deferred(
            div()
                .id("commit-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    left.update(cx, |this, cx| this.close_commit_menu(cx))
                })
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    right.update(cx, |this, cx| this.close_commit_menu(cx))
                }),
        )
        .with_priority(1)
    };
    let rows = ENTRIES
        .iter()
        .enumerate()
        .map(|(position, (action, label))| {
            let (action, workspace) = (*action, workspace.clone());
            div()
                .id(("commit-menu", position))
                .h(px(ROW_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(position == menu.row, |d| d.bg(rgb(theme::TREE_FOCUSED)))
                .hover(|d| d.bg(rgb(theme::SELECTION)))
                .on_click(move |_, window, cx| {
                    workspace.update(cx, |this, cx| this.commit_action(action, window, cx))
                })
                .child(*label)
        });
    let list = deferred(
        anchored()
            .position(menu.at)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("commit-menu-list")
                    .occlude()
                    .min_w(px(260.))
                    .py_1()
                    .flex()
                    .flex_col()
                    .bg(rgb(theme::POPUP))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .children(rows),
            ),
    )
    .with_priority(2);
    // an absolute box over the tab, so the layer under the menu has its size to cover
    div().absolute().inset_0().child(backdrop).child(list)
}

impl LogPanel {
    /// The keys while the menu is open: it takes them all.
    pub(super) fn menu_key(&mut self, key: &str, down: bool, up: bool) -> MenuKey {
        let Some(menu) = &mut self.commit_menu else {
            return MenuKey::Ignored;
        };
        match key {
            "escape" => {
                self.commit_menu = None;
                MenuKey::Handled
            }
            "enter" => MenuKey::Run(ENTRIES[menu.row].0),
            _ if down => {
                menu.move_row(1);
                MenuKey::Handled
            }
            _ if up => {
                menu.move_row(-1);
                MenuKey::Handled
            }
            _ => MenuKey::Handled,
        }
    }
}

/// What a key did to the open menu.
pub enum MenuKey {
    Ignored,
    Handled,
    Run(CommitAction),
}

#[cfg(test)]
mod tests {
    use super::{CommitMenu, ENTRIES, name};
    use gpui_kit::{point, px};

    #[::core::prelude::v1::test]
    fn the_menu_has_the_six_entries_in_order() {
        let labels: Vec<&str> = ENTRIES.iter().map(|(_, label)| *label).collect();
        assert_eq!(
            labels,
            [
                "Copy Revision Number",
                "Cherry-Pick",
                "Reset Current Branch to Here...",
                "Revert Commit",
                "New Branch...",
                "New Tag...",
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_row_stays_inside_the_menu() {
        let mut menu = CommitMenu {
            hash: "abc".into(),
            subject: "s".into(),
            at: point(px(0.), px(0.)),
            row: 0,
        };
        menu.move_row(-1);
        assert_eq!(menu.row, 0);
        menu.move_row(99);
        assert_eq!(menu.row, ENTRIES.len() - 1);
    }

    #[::core::prelude::v1::test]
    fn a_commit_is_called_by_its_short_hash_and_subject() {
        assert_eq!(name("0123456789abcdef", "fix it"), "01234567 fix it");
        assert_eq!(name("abc", "x"), "abc x");
    }
}
