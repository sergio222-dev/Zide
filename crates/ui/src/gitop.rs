//! The pull, merge and rebase dialogs (Ctrl+Shift+U pulls; the others come from Find Action).
//! They share one shape: `git <command>`, the branch to take from a dropdown, and the options
//! git accepts under *Modify options*, picked from a menu and shown as chips. What cannot go
//! with what is already picked is dimmed in the menu. Enter (or the button) runs it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{
    BranchChoices, Chosen, OTHER_BRANCH, Op, PullChoices, Value, blocked, def, merge_args,
    pull_args, rebase_args,
};

use crate::{Center, Workspace, theme};

/// Rows a list shows before it scrolls.
const LIST_ROWS: f32 = 8.;
const ROW_HEIGHT: f32 = 26.;

/// The dropdowns of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    #[default]
    Remote,
    Branch,
    /// The branch to rebase, when it is not the current.
    Extra,
}

enum Targets {
    Loading,
    Pull(PullChoices),
    Branches(BranchChoices),
    Failed(String),
}

pub struct OpDialog {
    op: Op,
    targets: Targets,
    /// Pull: the remote. Not used by the others.
    pub remote: String,
    /// Pull: the branch of the remote. Merge and rebase: the branch, remote ones with theirs.
    pub branch: String,
    /// Rebase: the branch to rebase, when picked.
    pub extra: String,
    pub chosen: Vec<Chosen>,
    /// The options menu, when open, with the row the keyboard is on.
    pub menu: Option<usize>,
    /// The field the keyboard is on.
    pub field: Field,
    /// The list of `field`, when open, with the row the keyboard is on.
    open: Option<usize>,
    /// What was typed to narrow the open list down.
    filter: String,
    /// Where the text of the option that takes one is typed.
    input: Option<Entity<InputState>>,
}

impl OpDialog {
    fn new(op: Op, input: Option<Entity<InputState>>) -> Self {
        Self {
            op,
            targets: Targets::Loading,
            remote: String::new(),
            branch: String::new(),
            extra: String::new(),
            chosen: Vec::new(),
            menu: None,
            field: if op == Op::Pull {
                Field::Remote
            } else {
                Field::Branch
            },
            open: None,
            filter: String::new(),
            input,
        }
    }

    /// Takes what git said: what to take from, or why there is nothing to.
    fn set_pull(&mut self, found: Result<PullChoices, String>) {
        self.targets = match found {
            Ok(choices) => {
                self.remote = choices.remote.clone();
                self.branch = choices.branch.clone();
                Targets::Pull(choices)
            }
            Err(why) => Targets::Failed(why),
        };
    }

    fn set_branches(&mut self, found: Result<BranchChoices, String>) {
        self.targets = match found {
            Ok(choices) => {
                self.branch = choices.selected.clone();
                self.extra = choices.current.clone();
                Targets::Branches(choices)
            }
            Err(why) => Targets::Failed(why),
        };
    }

    /// What `field` offers.
    pub fn items(&self, field: Field) -> Vec<String> {
        match (&self.targets, field) {
            (Targets::Pull(c), Field::Remote) => c.remotes.clone(),
            (Targets::Pull(c), Field::Branch) => c.branches_of(&self.remote),
            (Targets::Branches(c), Field::Branch) => c.branches.clone(),
            (Targets::Branches(c), Field::Extra) => std::iter::once(c.current.clone())
                .chain(c.branches.iter().filter(|b| !b.contains('/')).cloned())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// What the open list of `field` shows: the items that hold what was typed, in any case.
    pub fn shown(&self, field: Field) -> Vec<String> {
        let items = self.items(field);
        if self.filter.is_empty() || field != self.field || self.open.is_none() {
            return items;
        }
        let wanted = self.filter.to_lowercase();
        items
            .into_iter()
            .filter(|item| item.to_lowercase().contains(&wanted))
            .collect()
    }

    /// A character typed while a dropdown has the keyboard: opens its list and narrows it down.
    pub fn type_char(&mut self, c: char) {
        if self.open.is_none() {
            self.toggle_list(self.field);
            if self.open.is_none() {
                return; // nothing to choose from
            }
        }
        self.filter.push(c);
        self.open = Some(0);
    }

    /// Backspace: takes the last typed character back; `false` when nothing was typed.
    pub fn backspace(&mut self) -> bool {
        if self.filter.pop().is_none() {
            return false;
        }
        self.open = Some(0);
        true
    }

    fn value(&self, field: Field) -> &str {
        match field {
            Field::Remote => &self.remote,
            Field::Branch => &self.branch,
            Field::Extra => &self.extra,
        }
    }

    /// The fields the keyboard goes through.
    fn fields(&self) -> Vec<Field> {
        match self.op {
            Op::Pull => vec![Field::Remote, Field::Branch],
            Op::Merge => vec![Field::Branch],
            Op::Rebase if self.is_chosen(OTHER_BRANCH) => vec![Field::Branch, Field::Extra],
            Op::Rebase => vec![Field::Branch],
        }
    }

    pub fn is_chosen(&self, id: &str) -> bool {
        self.chosen.iter().any(|c| c.id == id)
    }

    /// Picks the option `id`, or drops it when it is picked. `None` when it cannot go with what
    /// is picked.
    pub fn toggle_option(&mut self, id: &str) -> Option<bool> {
        if let Some(at) = self.chosen.iter().position(|c| c.id == id) {
            self.chosen.remove(at);
            if !self.fields().contains(&self.field) {
                self.field = Field::Branch;
            }
            return Some(false);
        }
        if blocked(self.op, &self.chosen, id) {
            return None;
        }
        self.chosen.push(Chosen::of(def(self.op, id)?));
        Some(true)
    }

    /// Opens the list of `field` on its current value, or closes it.
    pub fn toggle_list(&mut self, field: Field) {
        self.filter.clear();
        if self.open.is_some() && self.field == field {
            self.open = None;
            return;
        }
        self.field = field;
        let items = self.items(field);
        self.open = (!items.is_empty()).then(|| {
            items
                .iter()
                .position(|item| item == self.value(field))
                .unwrap_or(0)
        });
    }

    /// Tab: the keyboard goes to the next field.
    pub fn switch_field(&mut self) {
        self.open = None;
        self.filter.clear();
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = fields[(at + 1) % fields.len()];
    }

    /// Moves the row of the open list; when closed, opens it.
    pub fn move_row(&mut self, delta: isize) {
        let Some(row) = self.open else {
            return self.toggle_list(self.field);
        };
        let last = self.shown(self.field).len().saturating_sub(1);
        self.open = Some(row.saturating_add_signed(delta).min(last));
    }

    pub fn is_list_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close_list(&mut self) {
        self.open = None;
        self.filter.clear();
    }

    pub fn pick_highlighted(&mut self) {
        if let Some(row) = self.open {
            self.pick(self.field, row);
        }
    }

    /// Takes row `row` of `field` as its value and closes the list. A new remote brings its own
    /// branch: the one with the same name when it has it, else its first.
    pub fn pick(&mut self, field: Field, row: usize) {
        let item = self.shown(field).get(row).cloned();
        self.open = None;
        self.filter.clear();
        self.field = field;
        let Some(item) = item else {
            return;
        };
        match field {
            Field::Branch => self.branch = item,
            Field::Extra => self.extra = item,
            Field::Remote => {
                let kept = self.branch.clone();
                self.remote = item;
                let branches = self.items(Field::Branch);
                self.branch = if branches.contains(&kept) {
                    kept
                } else {
                    branches.first().cloned().unwrap_or_default()
                };
            }
        }
    }

    /// The text typed for the option that takes one.
    fn typed(&self, cx: &App) -> String {
        self.input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    /// What git is given, once everything it needs is known; `None` until then.
    pub fn arguments(&self, cx: &App) -> Option<Vec<String>> {
        let typed = self.typed(cx);
        let mut chosen = self.chosen.clone();
        for option in &mut chosen {
            option.value = match def(self.op, option.id)?.value {
                Value::None => String::new(),
                Value::Text(_) if typed.is_empty() => return None,
                Value::Text(_) => typed.clone(),
                Value::Branch => self.extra.clone(),
            };
        }
        let picked = |field| {
            let items = self.items(field);
            items.contains(&self.branch).then_some(())
        };
        picked(Field::Branch)?;
        Some(match self.op {
            Op::Pull => pull_args(&chosen, &self.remote, &self.branch),
            Op::Merge => merge_args(&chosen, &self.branch),
            Op::Rebase => rebase_args(&chosen, &self.branch),
        })
    }

    /// What the notification says once it went well.
    fn done(&self, into: Option<&str>) -> String {
        let into = into.map(|name| format!("'{name}'")).unwrap_or_default();
        match self.op {
            Op::Pull => format!("Pulled {}/{} into {into}", self.remote, self.branch),
            Op::Merge => format!("Merged '{}' into {into}", self.branch),
            Op::Rebase => format!("Rebased {into} onto '{}'", self.branch),
        }
    }
}

impl Workspace {
    /// Opens the dialog of `op`; from the dialog, closes it.
    pub(crate) fn toggle_git_op(&mut self, op: Op, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_op.is_some() {
            return self.close_git_op(window, cx);
        }
        self.popup = None;
        self.branches = None;
        self.conflicts = None;
        self.push = None;
        self.cancel_running_search();
        let input = op
            .options()
            .iter()
            .find_map(|option| match option.value {
                Value::Text(placeholder) => Some(placeholder),
                _ => None,
            })
            .map(|placeholder| cx.new(|cx| InputState::new(window, cx).placeholder(placeholder)));
        self.git_op = Some(OpDialog::new(op, input));
        self.focus.focus(window, cx); // the keys are read by the workspace
        let root = self.root.clone();
        self.begin_busy("Loading branches...", cx);
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    match op {
                        Op::Pull => Ok(zide_git::pull_choices(&root)),
                        _ => Err(zide_git::branch_choices(&root)),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.end_busy("Loading branches...", cx);
                if let Some(dialog) = &mut this.git_op {
                    match found {
                        Ok(found) => dialog.set_pull(found),
                        Err(found) => dialog.set_branches(found),
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_git_op(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_op = None;
        if self.center == Center::Chat {
            self.focus_active_chat(window, cx); // back to the message box
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Runs it and closes the dialog; the outcome comes as a notification.
    fn run_git_op(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &self.git_op else {
            return;
        };
        let Some(args) = dialog.arguments(cx) else {
            return;
        };
        let op = dialog.op;
        let done = dialog.done(self.branch.as_ref().map(|b| b.name.as_str()));
        self.close_git_op(window, cx);
        let failure = match op {
            Op::Pull => "Could not pull",
            Op::Merge => "Could not merge",
            Op::Rebase => "Could not rebase",
        };
        self.git_task(
            failure,
            Some(done),
            move |root| zide_git::integrate(&root, op, &args),
            cx,
        );
    }

    /// Picks or drops the option `id`; the one that takes text gets the keyboard.
    fn toggle_git_option(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.git_op else {
            return;
        };
        let input = dialog.input.clone();
        let takes_text = def(dialog.op, id).is_some_and(|d| matches!(d.value, Value::Text(_)));
        match (dialog.toggle_option(id), input) {
            (Some(true), Some(input)) if takes_text => {
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            (Some(false), _) if takes_text => self.focus.focus(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn git_op_typing(&self, window: &Window, cx: &App) -> bool {
        self.git_op
            .as_ref()
            .and_then(|d| d.input.as_ref())
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Tab: the keyboard goes to the next dropdown, or out of the text box.
    pub(crate) fn git_op_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_op_typing(window, cx) {
            self.focus.focus(window, cx);
        } else if let Some(dialog) = &mut self.git_op {
            dialog.switch_field();
        }
        cx.notify();
    }

    /// Keys while the dialog is open: it takes all of them, the editor reads none.
    pub(crate) fn git_op_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        // the text box types by itself
        if self.git_op_typing(window, cx) && !matches!(key, "escape" | "enter" | "tab") {
            return;
        }
        cx.stop_propagation();
        let modifiers = event.keystroke.modifiers;
        let alt = modifiers.alt;
        let Some(dialog) = &mut self.git_op else {
            return;
        };
        if dialog.menu.is_none() && !(modifiers.control || alt || modifiers.platform) {
            // typing narrows the dropdown that has the keyboard down to what matches
            let typed = match (key, event.keystroke.key_char.as_deref()) {
                ("backspace", _) => dialog.backspace().then_some(None),
                (_, Some(text)) if text.chars().count() == 1 && text != " " => {
                    Some(text.chars().next())
                }
                _ => None,
            };
            if let Some(c) = typed {
                if let Some(c) = c {
                    dialog.type_char(c);
                }
                return cx.notify();
            }
        }
        match (key, dialog.menu) {
            ("escape", Some(_)) => dialog.menu = None,
            ("escape", None) if dialog.is_list_open() => dialog.close_list(),
            ("escape", None) => return self.close_git_op(window, cx),
            ("enter", Some(row)) => {
                if let Some(option) = dialog.op.options().get(row) {
                    return self.toggle_git_option(option.id, window, cx);
                }
            }
            ("enter", None) if dialog.is_list_open() => dialog.pick_highlighted(),
            ("enter", None) => return self.run_git_op(window, cx),
            ("down", Some(row)) => {
                dialog.menu = Some((row + 1).min(dialog.op.options().len() - 1));
            }
            ("up", Some(row)) => dialog.menu = Some(row.saturating_sub(1)),
            ("down", None) => dialog.move_row(1),
            ("up", None) => dialog.move_row(-1),
            ("o", _) if alt => dialog.menu = dialog.menu.is_none().then_some(0),
            _ => return,
        }
        cx.notify();
    }

    pub(crate) fn git_op_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let dialog = self.git_op.as_ref().expect("the dialog is open");
        let workspace = cx.entity();
        let op = dialog.op;
        let (run_button, cancel_button, menu_toggle, outside) = (
            workspace.clone(),
            workspace.clone(),
            workspace.clone(),
            workspace.clone(),
        );

        let label = div()
            .px_2()
            .h(px(30.))
            .flex()
            .items_center()
            .flex_shrink_0()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::CONTROL_BORDER))
            .text_color(rgb(theme::MUTED))
            .child(op.command());
        let body = match &dialog.targets {
            Targets::Loading => note("Reading the branches", true).into_any_element(),
            Targets::Failed(why) => note(why.clone(), false)
                .text_color(rgb(theme::ERROR))
                .into_any_element(),
            _ => {
                let mut rows = div().flex().flex_col().gap_2();
                let mut first = div().flex().items_center().gap_2().child(label);
                if op == Op::Pull {
                    first = first.child(field(dialog, Field::Remote, 110., &workspace));
                }
                first = first.child(div().flex_1().child(field(
                    dialog,
                    Field::Branch,
                    0.,
                    &workspace,
                )));
                rows = rows.child(first);
                if dialog.fields().contains(&Field::Extra) {
                    rows = rows.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(110.))
                                    .flex_shrink_0()
                                    .text_sm()
                                    .text_color(rgb(theme::MUTED))
                                    .child("Branch to rebase"),
                            )
                            .child(div().flex_1().child(field(
                                dialog,
                                Field::Extra,
                                0.,
                                &workspace,
                            ))),
                    );
                }
                rows.into_any_element()
            }
        };

        let chips = dialog.chosen.iter().map(|chosen| {
            let workspace = workspace.clone();
            let id = chosen.id;
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(28.))
                .rounded_full()
                .bg(rgb(theme::SELECTION))
                .text_color(rgb(theme::STRONG))
                .child(if chosen.flag.is_empty() {
                    "another branch"
                } else {
                    chosen.flag
                })
                .child(
                    div()
                        .id(SharedString::from(format!("op-chip-{id}")))
                        .cursor_pointer()
                        .text_color(rgb(theme::MUTED))
                        .hover(|d| d.text_color(rgb(theme::STRONG)))
                        .child("×")
                        .on_click(move |_, window, cx| {
                            workspace.update(cx, |this, cx| this.toggle_git_option(id, window, cx))
                        }),
                )
        });
        let text_option = dialog
            .chosen
            .iter()
            .find(|c| matches!(def(op, c.id).map(|d| d.value), Some(Value::Text(_))))
            .map(|c| c.flag);
        let can_run = dialog.arguments(cx).is_some();

        div()
            .id("git-op-overlay")
            .occlude()
            // a click anywhere else closes the options menu; the menu and its link stop theirs
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                outside.update(cx, |this, cx| {
                    if let Some(d) = this.git_op.as_mut().filter(|d| d.menu.is_some()) {
                        d.menu = None;
                        cx.notify();
                    }
                })
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .pt(px(56.))
            .child(
                div()
                    .w(px(560.))
                    .flex()
                    .flex_col()
                    .self_start()
                    .gap_3()
                    .p_3()
                    .bg(rgb(theme::POPUP))
                    .text_color(rgb(theme::TEXT))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .child(body)
                    .when(!dialog.chosen.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap_2().children(chips))
                    })
                    .when_some(dialog.input.clone().zip(text_option), |d, (input, flag)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(110.))
                                        .flex_shrink_0()
                                        .text_sm()
                                        .text_color(rgb(theme::MUTED))
                                        .child(flag),
                                )
                                .child(div().flex_1().child(Input::new(&input))),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        div()
                                            .id("git-op-options")
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .text_sm()
                                            .text_color(rgb(theme::ICON_CODE))
                                            .child("Modify options ⌄")
                                            .on_click(move |_, _, cx| {
                                                menu_toggle.update(cx, |this, cx| {
                                                    if let Some(d) = &mut this.git_op {
                                                        d.menu = d.menu.is_none().then_some(0);
                                                        cx.notify();
                                                    }
                                                })
                                            }),
                                    )
                                    .when_some(dialog.menu, |d, row| {
                                        d.child(deferred(options_menu(dialog, row, &workspace)))
                                    }),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("git-op-go")
                                            .primary()
                                            .label(op.title())
                                            .disabled(!can_run)
                                            .on_click(move |_, window, cx| {
                                                run_button.update(cx, |this, cx| {
                                                    this.run_git_op(window, cx)
                                                })
                                            }),
                                    )
                                    .child(Button::new("git-op-cancel").label("Cancel").on_click(
                                        move |_, window, cx| {
                                            cancel_button.update(cx, |this, cx| {
                                                this.close_git_op(window, cx)
                                            })
                                        },
                                    )),
                            ),
                    ),
            )
    }
}

/// The menu of options: what is picked is checked, what cannot go with it is dimmed.
fn options_menu(
    dialog: &OpDialog,
    highlighted: usize,
    workspace: &Entity<Workspace>,
) -> Stateful<Div> {
    let rows = dialog.op.options().iter().enumerate().map(|(row, option)| {
        let workspace = workspace.clone();
        let id = option.id;
        let chosen = dialog.is_chosen(id);
        let dimmed = blocked(dialog.op, &dialog.chosen, id);
        div()
            .id(SharedString::from(format!("op-option-{id}")))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(ROW_HEIGHT))
            .text_sm()
            .text_color(rgb(if dimmed { theme::DIM } else { theme::TEXT }))
            .when(row == highlighted, |d| d.bg(rgb(theme::ACTIVE_BG)))
            .when(!dimmed, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(rgb(theme::SELECTION)))
                    .on_click(move |_, window, cx| {
                        workspace.update(cx, |this, cx| this.toggle_git_option(id, window, cx))
                    })
            })
            .child(
                div()
                    .w(px(14.))
                    .flex_shrink_0()
                    .child(if chosen { "✓" } else { "" }),
            )
            .child(div().flex_1().whitespace_nowrap().child(option.help))
            .child(div().text_color(rgb(theme::DIM)).child(option.flag))
    });
    div()
        .id("git-op-menu")
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .absolute()
        .top(px(24.))
        .left_0()
        .w(px(536.))
        .py_1()
        .flex()
        .flex_col()
        .bg(rgb(theme::POPUP))
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .rounded_md()
        .child(
            div()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(theme::STRONG))
                .child(format!("Add {} Options", dialog.op.title())),
        )
        .children(rows)
}

/// A line of text in the place of the fields that have nothing to show.
fn note(text: impl Into<SharedString>, loading: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(rgb(theme::MUTED))
        .children(loading.then(|| Spinner::new().small().color(rgb(theme::MUTED).into())))
        .child(text.into())
}

/// One dropdown: the value, and under it the list when open. `width` 0 fills the space.
fn field(dialog: &OpDialog, which: Field, width: f32, workspace: &Entity<Workspace>) -> Div {
    let focused = dialog.field == which;
    let open = focused.then_some(dialog.open).flatten();
    let items = if open.is_some() {
        dialog.shown(which)
    } else {
        Vec::new()
    };
    let toggle = workspace.clone();
    div()
        .relative()
        .when(width > 0., |d| d.w(px(width)).flex_shrink_0())
        .child(
            div()
                .id(SharedString::from(format!("op-field-{which:?}")))
                .px_2()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .rounded_md()
                .border_1()
                .border_color(rgb(if focused {
                    theme::ACTIVE_BORDER
                } else {
                    theme::CONTROL_BORDER
                }))
                .text_color(rgb(theme::STRONG))
                .child(
                    div()
                        .truncate()
                        .child(if open.is_some() && !dialog.filter.is_empty() {
                            format!("{}|", dialog.filter) // what is being typed, with its caret
                        } else {
                            dialog.value(which).to_string()
                        }),
                )
                .child(div().text_color(rgb(theme::DIM)).child("⌄"))
                .on_click(move |_, _, cx| {
                    toggle.update(cx, |this, cx| {
                        if let Some(dialog) = &mut this.git_op {
                            dialog.toggle_list(which);
                            cx.notify();
                        }
                    })
                }),
        )
        .when_some(open, |d, highlighted| {
            let rows = items.iter().enumerate().map(|(row, item)| {
                let pick = workspace.clone();
                div()
                    .id(SharedString::from(format!("op-{which:?}-{row}")))
                    .px_2()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .when(row == highlighted, |d| d.bg(rgb(theme::ACTIVE_BG)))
                    .hover(|d| d.bg(rgb(theme::SELECTION)))
                    .child(item.clone())
                    .on_click(move |_, _, cx| {
                        pick.update(cx, |this, cx| {
                            if let Some(dialog) = &mut this.git_op {
                                dialog.pick(which, row);
                                cx.notify();
                            }
                        })
                    })
            });
            // deferred: painted over what comes after it (the buttons, the chips)
            let rows: Vec<AnyElement> = if items.is_empty() {
                vec![note("No matches", false).into_any_element()]
            } else {
                rows.map(IntoElement::into_any_element).collect()
            };
            d.child(deferred(
                div()
                    .id(SharedString::from(format!("op-list-{which:?}")))
                    .occlude()
                    .absolute()
                    .top(px(34.))
                    .left_0()
                    .w_full()
                    .max_h(px(ROW_HEIGHT * LIST_ROWS))
                    .overflow_y_scroll()
                    .bg(rgb(theme::POPUP))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .children(rows),
            ))
        })
}

#[cfg(test)]
mod tests {
    use zide_git::{Op, PullChoices};

    use super::{Field, OpDialog};

    fn pulling() -> OpDialog {
        let mut dialog = OpDialog::new(Op::Pull, None);
        dialog.set_pull(Ok(PullChoices {
            current: "feat/x".into(),
            remotes: vec!["origin".into(), "fork".into()],
            branches: vec![
                "origin/main".into(),
                "origin/feat/x".into(),
                "fork/main".into(),
            ],
            remote: "origin".into(),
            branch: "feat/x".into(),
        }));
        dialog
    }

    #[test]
    fn another_remote_keeps_the_branch_when_it_has_it_else_takes_its_first() {
        let mut dialog = pulling();
        dialog.pick(Field::Remote, 1);
        assert_eq!(
            (dialog.remote.as_str(), dialog.branch.as_str()),
            ("fork", "main")
        );
        dialog.pick(Field::Remote, 0);
        assert_eq!(
            (dialog.remote.as_str(), dialog.branch.as_str()),
            ("origin", "main")
        );
    }

    #[test]
    fn the_list_opens_on_the_value_and_the_rows_stay_inside_it() {
        let mut dialog = pulling();
        dialog.field = Field::Branch;
        dialog.move_row(1); // closed: opens
        assert_eq!(dialog.open, Some(1));
        dialog.move_row(5);
        assert_eq!(dialog.open, Some(1)); // only two branches
        dialog.move_row(-5);
        dialog.pick_highlighted();
        assert_eq!(dialog.branch, "main");
        assert!(!dialog.is_list_open());
    }

    #[test]
    fn typing_narrows_the_open_list_and_a_pick_takes_the_matching_row() {
        let mut dialog = pulling();
        dialog.field = Field::Remote;
        dialog.type_char('F'); // closed: opens, any case
        assert!(dialog.is_list_open());
        assert_eq!(dialog.shown(Field::Remote), ["fork"]);
        dialog.pick_highlighted();
        assert_eq!(dialog.remote, "fork");
        assert!(dialog.filter.is_empty());
        assert_eq!(dialog.shown(Field::Remote), ["origin", "fork"]);
    }

    #[test]
    fn backspace_widens_the_list_again_and_nothing_matching_shows_no_rows() {
        let mut dialog = pulling();
        dialog.field = Field::Branch;
        for c in "zzz".chars() {
            dialog.type_char(c);
        }
        assert!(dialog.shown(Field::Branch).is_empty());
        assert!(dialog.backspace() && dialog.backspace() && dialog.backspace());
        assert_eq!(dialog.shown(Field::Branch).len(), 2);
        assert!(!dialog.backspace()); // nothing left to take back
    }

    #[test]
    fn closing_the_list_forgets_what_was_typed() {
        let mut dialog = pulling();
        dialog.field = Field::Branch;
        dialog.type_char('m');
        dialog.close_list();
        assert!(dialog.filter.is_empty() && !dialog.is_list_open());
    }

    #[test]
    fn an_option_that_goes_against_a_picked_one_is_refused_until_that_one_is_dropped() {
        let mut dialog = pulling();
        assert_eq!(dialog.toggle_option("--ff-only"), Some(true));
        assert_eq!(dialog.toggle_option("--no-commit"), Some(true));
        assert_eq!(dialog.toggle_option("--rebase"), None);
        assert_eq!(dialog.toggle_option("--ff-only"), Some(false));
        assert_eq!(dialog.toggle_option("--no-ff"), Some(true));
    }

    #[test]
    fn the_branch_to_rebase_is_a_field_only_while_it_is_picked() {
        let mut dialog = OpDialog::new(Op::Rebase, None);
        assert_eq!(dialog.fields(), [Field::Branch]);
        dialog.toggle_option("branch");
        assert_eq!(dialog.fields(), [Field::Branch, Field::Extra]);
        dialog.field = Field::Extra;
        dialog.toggle_option("branch");
        assert_eq!(dialog.field, Field::Branch);
    }
}
