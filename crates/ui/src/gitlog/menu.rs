//! The context menus of the git log (right click). On a commit: copy its hash, cherry-pick it,
//! reset the branch to it, revert it, start a branch or a tag on it. On the files of a commit: show
//! their diff, revert or cherry-pick their changes into the working tree, or get them as they
//! were in that commit. On a branch or a tag of the tree: check it out, merge it into the current
//! branch, update a local branch from its upstream, push it or delete it once the remote has it;
//! only push it while the remote does not.

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::Op;
use zide_git::commit_ops::{self, ResetMode};
use zide_git::{FileChange, Switch};

use super::{LogEvent, LogPanel};
use crate::branches::NameKind;
use crate::changed_files::plural;
use crate::{Workspace, theme};

const ROW_HEIGHT: f32 = 26.;

/// What an entry of the menu does to the commit or to its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    CopyHash,
    CherryPick,
    Reset,
    Revert,
    NewBranch,
    NewTag,
    ShowDiff,
    RevertChanges,
    CherryPickChanges,
    GetFromRevision,
    Checkout,
    Merge,
    Update,
    Push,
    Delete,
}

/// What the menu was opened on.
#[derive(Debug, Clone)]
pub enum Subject {
    Commit,
    /// The files picked in the list of the commit.
    Files(Vec<FileChange>),
    /// A branch or a tag of the tree, with the remote it goes to and the current branch (or
    /// commit) it would be merged into.
    Ref {
        target: Switch,
        remote: String,
        into: String,
    },
}

const FILE_ENTRIES: [(MenuAction, &str); 4] = [
    (MenuAction::ShowDiff, "Show Diff"),
    (MenuAction::RevertChanges, "Revert Selected Changes"),
    (
        MenuAction::CherryPickChanges,
        "Cherry-Pick Selected Changes",
    ),
    (MenuAction::GetFromRevision, "Get from Revision"),
];

/// The entries of the menu of a commit or of its files.
fn table(files: bool) -> Vec<(MenuAction, String)> {
    let table: &[(MenuAction, &str)] = if files { &FILE_ENTRIES } else { &ENTRIES };
    table
        .iter()
        .map(|(action, label)| (*action, label.to_string()))
        .collect()
}

/// The name of a branch or tag, as git takes it.
fn ref_name(target: &Switch) -> &str {
    match target {
        Switch::Local(name) | Switch::Remote(name) | Switch::Tag(name) => name,
    }
}

/// The entries of the menu of a branch or tag. One the remote does not have yet can only be
/// pushed; one it has can be checked out, merged into `into`, updated (a local branch), pushed and
/// deleted. A remote branch is checked out or merged; the current branch is updated or pushed.
pub fn ref_entries(
    target: &Switch,
    current: bool,
    on_remote: bool,
    remote: &str,
    into: &str,
) -> Vec<(MenuAction, String)> {
    let name = ref_name(target);
    let push = (MenuAction::Push, format!("Push to {remote}"));
    let checkout = (MenuAction::Checkout, "Checkout".to_string());
    let merge = (MenuAction::Merge, format!("Merge '{name}' into '{into}'"));
    let update = (MenuAction::Update, "Update".to_string());
    let delete = (MenuAction::Delete, "Delete".to_string());
    match target {
        Switch::Remote(_) => vec![checkout, merge],
        _ if !on_remote => vec![push],
        Switch::Local(_) if current => vec![update, push],
        _ if current => vec![push],
        Switch::Local(_) => vec![checkout, merge, update, push, delete],
        _ => vec![checkout, merge, push, delete],
    }
}

pub const ENTRIES: [(MenuAction, &str); 6] = [
    (MenuAction::CopyHash, "Copy Revision Number"),
    (MenuAction::CherryPick, "Cherry-Pick"),
    (MenuAction::Reset, "Reset Current Branch to Here..."),
    (MenuAction::Revert, "Revert Commit"),
    (MenuAction::NewBranch, "New Branch..."),
    (MenuAction::NewTag, "New Tag..."),
];

/// An open menu: the commit (or ref) it was asked on, its entries, where, and the entry the
/// keyboard is on.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// Empty for a ref.
    pub hash: String,
    pub subject: String,
    pub on: Subject,
    pub entries: Vec<(MenuAction, String)>,
    pub at: Point<Pixels>,
    pub row: usize,
}

impl ContextMenu {
    pub fn move_row(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1);
        self.row = self.row.saturating_add_signed(delta).min(last);
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

impl LogPanel {
    /// Opens the menu of commit `index` at the mouse; the commit becomes the selected one.
    pub(super) fn open_context_menu(
        &mut self,
        index: usize,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.select_log_commit(index, cx);
        if let Some(entry) = self.selected_entry() {
            self.context_menu = Some(ContextMenu {
                hash: entry.commit.hash.clone(),
                subject: entry.commit.subject.clone(),
                on: Subject::Commit,
                entries: table(false),
                at,
                row: 0,
            });
        }
        cx.notify();
    }

    /// Opens the menu of the picked files at the mouse. A right click on a file that is not
    /// among them picks it alone first.
    pub(super) fn open_files_menu(
        &mut self,
        index: usize,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.picked_files().contains(&index) {
            self.click_file(index, false, false);
        }
        let files = self.selected_files();
        if let Some(entry) = self.selected_entry().filter(|_| !files.is_empty()) {
            self.context_menu = Some(ContextMenu {
                hash: entry.commit.hash.clone(),
                subject: entry.commit.subject.clone(),
                on: Subject::Files(files),
                entries: table(true),
                at,
                row: 0,
            });
        }
        cx.notify();
    }

    /// Opens the menu of the branch or tag on row `index` of the tree, which becomes the selected
    /// row; `into` is the current branch. Folders and sections have none.
    pub(super) fn open_ref_menu(
        &mut self,
        index: usize,
        at: Point<Pixels>,
        into: String,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = &mut self.refs else {
            return;
        };
        let Some(crate::branches::Kind::Branch {
            target, current, ..
        }) = model.rows().get(index).map(|row| row.kind.clone())
        else {
            return;
        };
        model.select(index);
        self.head_cursor = false;
        let remote = self.remote.clone().unwrap_or_else(|| "origin".into());
        let on_remote = match &target {
            Switch::Local(name) => model.remote_names().contains(&format!("{remote}/{name}")),
            Switch::Tag(name) => self
                .remote_tags
                .as_ref()
                .is_some_and(|(of, tags)| *of == remote && tags.contains(name)),
            Switch::Remote(_) => true,
        };
        self.context_menu = Some(ContextMenu {
            hash: String::new(),
            subject: String::new(),
            entries: ref_entries(&target, current, on_remote, &remote, &into),
            on: Subject::Ref {
                target,
                remote,
                into,
            },
            at,
            row: 0,
        });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    /// Closes the menu and asks the workspace to run `action` on what it was opened on.
    pub(super) fn menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu.take() {
            cx.emit(LogEvent::Menu { action, menu });
        }
        cx.notify();
    }
}

/// The actions of the menus: they are git's, run the way the workspace runs every git operation.
impl Workspace {
    /// Runs `action` on the branch or tag of the open menu.
    fn ref_action(
        &mut self,
        action: MenuAction,
        target: Switch,
        remote: String,
        into: String,
        cx: &mut Context<Self>,
    ) {
        let name = ref_name(&target).to_string();
        match (action, target) {
            (MenuAction::Checkout, target) => self.switch_branch(target, cx),
            (MenuAction::Update, Switch::Local(_)) => self.update_branch(name, cx),
            (MenuAction::Merge, _) => self.git_task(
                "Could not merge",
                Some(format!("Merged '{name}' into '{into}'")),
                move |root| zide_git::integrate(&root, Op::Merge, &[name]),
                cx,
            ),
            (MenuAction::Push, Switch::Tag(_)) => self.git_task(
                "Could not push the tag",
                Some(format!("Pushed '{name}' to {remote}")),
                move |root| {
                    zide_git::push_tag(&root, &remote, &name)?;
                    // the menu offers what the remote has: it has this tag now
                    if let Some((of, mut tags)) = crate::session::remote_tags(&root)
                        && of == remote
                    {
                        tags.push(name);
                        tags.sort();
                        tags.dedup();
                        crate::session::remember_remote_tags(&root, &remote, &tags);
                    }
                    Ok(())
                },
                cx,
            ),
            (MenuAction::Push, _) => self.git_task(
                "Could not push",
                Some(format!("Pushed '{name}' to {remote}")),
                move |root| zide_git::push_branch(&root, &name, &remote),
                cx,
            ),
            (MenuAction::Delete, Switch::Tag(_)) => self.git_task(
                "Could not delete the tag",
                Some(format!("Deleted the tag '{name}'")),
                move |root| zide_git::delete_tag(&root, &name),
                cx,
            ),
            (MenuAction::Delete, _) => self.git_task(
                "Could not delete the branch",
                Some(format!("Deleted the branch '{name}'")),
                move |root| zide_git::delete_branch(&root, &name),
                cx,
            ),
            _ => {}
        }
    }

    /// Runs `action` on what `menu` was opened on.
    pub(super) fn run_menu_action(
        &mut self,
        action: MenuAction,
        menu: ContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ContextMenu {
            hash, subject, on, ..
        } = menu;
        let called = name(&hash, &subject);
        let files = match on {
            Subject::Files(files) => files,
            Subject::Commit => Vec::new(),
            Subject::Ref {
                target,
                remote,
                into,
            } => return self.ref_action(action, target, remote, into, cx),
        };
        match action {
            MenuAction::CopyHash => {
                cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                let note = Notification::success(format!("Copied {hash}"));
                self.notify(note, cx);
            }
            MenuAction::ShowDiff => {
                if let Some(file) = files.into_iter().next() {
                    self.show_commit_diff(&hash, file, window, cx);
                }
            }
            MenuAction::RevertChanges => {
                let done = format!(
                    "Reverted the changes of {} from {called}",
                    plural(files.len(), "file")
                );
                self.git_task(
                    "Could not revert the changes",
                    Some(done),
                    move |root| commit_ops::apply_changes(&root, &hash, &files, true),
                    cx,
                );
            }
            MenuAction::CherryPickChanges => {
                let done = format!(
                    "Applied the changes of {} from {called}",
                    plural(files.len(), "file")
                );
                self.git_task(
                    "Could not apply the changes",
                    Some(done),
                    move |root| commit_ops::apply_changes(&root, &hash, &files, false),
                    cx,
                );
            }
            MenuAction::GetFromRevision => {
                let done = format!("Got {} as in {called}", plural(files.len(), "file"));
                self.git_task(
                    "Could not get the files from that revision",
                    Some(done),
                    move |root| commit_ops::get_from_revision(&root, &hash, &files),
                    cx,
                );
            }
            MenuAction::CherryPick => self.git_task(
                "Could not cherry-pick the commit",
                Some(format!("Cherry-picked {called}")),
                move |root| commit_ops::cherry_pick(&root, &hash),
                cx,
            ),
            MenuAction::Revert => self.git_task(
                "Could not revert the commit",
                Some(format!("Reverted {called}")),
                move |root| commit_ops::revert(&root, &hash),
                cx,
            ),
            MenuAction::Reset => self.confirm_reset(hash, called, window, cx),
            MenuAction::NewBranch => {
                let existing = self
                    .log
                    .as_ref()
                    .and_then(|log| log.read(cx).refs.as_ref().map(|model| model.local_names()))
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
            MenuAction::Checkout
            | MenuAction::Merge
            | MenuAction::Update
            | MenuAction::Push
            | MenuAction::Delete => {}
            MenuAction::NewTag => self.ask_name(
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
pub fn view(menu: &ContextMenu, entity: &Entity<LogPanel>) -> impl IntoElement {
    let backdrop = {
        let (left, right) = (entity.clone(), entity.clone());
        deferred(
            div()
                .id("commit-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    left.update(cx, |this, cx| this.close_context_menu(cx))
                })
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    right.update(cx, |this, cx| this.close_context_menu(cx))
                }),
        )
        .with_priority(1)
    };
    let rows = menu
        .entries
        .iter()
        .enumerate()
        .map(|(position, (action, label))| {
            let (action, label, entity) = (*action, label.clone(), entity.clone());
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
                .on_click(move |_, _, cx| {
                    entity.update(cx, |this, cx| this.menu_action(action, cx))
                })
                .child(label)
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
        let Some(menu) = &mut self.context_menu else {
            return MenuKey::Ignored;
        };
        match key {
            "escape" => {
                self.context_menu = None;
                MenuKey::Handled
            }
            "enter" => match menu.entries.get(menu.row) {
                Some((action, _)) => MenuKey::Run(*action),
                None => MenuKey::Handled,
            },
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
    Run(MenuAction),
}

#[cfg(test)]
mod tests {
    use super::{ContextMenu, ENTRIES, MenuAction, name, ref_entries, table};
    use gpui_kit::{point, px};
    use zide_git::Switch;

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
        let mut menu = ContextMenu {
            hash: "abc".into(),
            subject: "s".into(),
            on: super::Subject::Commit,
            entries: table(false),
            at: point(px(0.), px(0.)),
            row: 0,
        };
        menu.move_row(-1);
        assert_eq!(menu.row, 0);
        menu.move_row(99);
        assert_eq!(menu.row, ENTRIES.len() - 1);
    }

    fn labels(entries: Vec<(MenuAction, String)>) -> Vec<String> {
        entries.into_iter().map(|(_, label)| label).collect()
    }

    #[::core::prelude::v1::test]
    fn a_ref_the_remote_does_not_have_can_only_be_pushed() {
        for target in [
            Switch::Tag("v0.1.15".into()),
            Switch::Local("feat/x".into()),
        ] {
            assert_eq!(
                labels(ref_entries(&target, false, false, "origin", "main")),
                ["Push to origin"]
            );
        }
    }

    #[::core::prelude::v1::test]
    fn a_ref_on_the_remote_is_checked_out_merged_pushed_or_deleted() {
        let tag = Switch::Tag("v0.1.13".into());
        assert_eq!(
            labels(ref_entries(&tag, false, true, "origin", "f90e90b7")),
            [
                "Checkout",
                "Merge 'v0.1.13' into 'f90e90b7'",
                "Push to origin",
                "Delete"
            ]
        );
        let branch = Switch::Local("feat/x".into());
        let actions: Vec<_> = ref_entries(&branch, false, true, "upstream", "main")
            .into_iter()
            .map(|(action, _)| action)
            .collect();
        assert_eq!(
            actions,
            [
                MenuAction::Checkout,
                MenuAction::Merge,
                MenuAction::Update,
                MenuAction::Push,
                MenuAction::Delete
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_current_branch_is_updated_or_pushed_and_a_remote_one_checked_out_or_merged() {
        let current = Switch::Local("main".into());
        assert_eq!(
            labels(ref_entries(&current, true, true, "origin", "main")),
            ["Update", "Push to origin"]
        );
        let remote = Switch::Remote("origin/feat/x".into());
        assert_eq!(
            labels(ref_entries(&remote, false, true, "origin", "main")),
            ["Checkout", "Merge 'origin/feat/x' into 'main'"]
        );
    }

    #[::core::prelude::v1::test]
    fn a_commit_is_called_by_its_short_hash_and_subject() {
        assert_eq!(name("0123456789abcdef", "fix it"), "01234567 fix it");
        assert_eq!(name("abc", "x"), "abc x");
    }
}
