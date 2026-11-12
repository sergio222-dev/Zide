//! The Stash tab of the commit panel: the stashes of the repository, the files of the selected
//! one, and what can be done with it (Apply, Pop, Drop).

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{Disableable, Icon};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::stash::Stash;

use crate::commit::{Check, PanelTab, RowKind, StashAction, check_box, kind_color, panel_tabs};
use crate::{TREE_ROW_HEIGHT, Workspace, file_icon, theme};

/// The content of the Stash Changes dialog: the message and what else goes in the stash.
pub struct StashDialog {
    message: Entity<InputState>,
    keep_index: bool,
    untracked: bool,
}

impl StashDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            message: cx.new(|cx| InputState::new(window, cx).placeholder("Message (optional)")),
            keep_index: false,
            untracked: true,
        }
    }
}

impl Render for StashDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let option = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(check_box(if on { Check::On } else { Check::Off }))
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child("Message:")
                    .child(div().flex_1().child(Input::new(&self.message))),
            )
            .child(
                option("stash-untracked", "Include untracked files", self.untracked).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.untracked.toggle();
                        cx.notify();
                    }),
                ),
            )
            .child(
                option(
                    "stash-keep-index",
                    "Keep staged changes in place",
                    self.keep_index,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.keep_index.toggle();
                    cx.notify();
                })),
            )
    }
}

impl Workspace {
    /// Asks for a message and sets the changes aside (Find Action → *Stash Changes*).
    pub(crate) fn stash_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| StashDialog::new(window, cx));
        let workspace = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_alert_dialog(cx, move |alert, window, cx| {
            if first.replace(false) {
                let message = view.read(cx).message.clone();
                message.update(cx, |state, cx| state.focus(window, cx));
            }
            let (view, workspace) = (view.clone(), workspace.clone());
            alert
                .title("Stash Changes")
                .child(view.clone())
                .ok_text("Stash Changes")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let dialog = view.read(cx);
                    let message = dialog.message.read(cx).value().trim().to_string();
                    let (keep_index, untracked) = (dialog.keep_index, dialog.untracked);
                    workspace.update(cx, |this, cx| {
                        this.git_task(
                            "Could not stash the changes",
                            Some("Changes stashed".into()),
                            move |root| {
                                zide_git::stash::push(&root, &message, keep_index, untracked)
                            },
                            cx,
                        )
                    });
                    true
                })
        });
    }

    /// Switches the tab of the commit panel.
    pub(crate) fn set_panel_tab(
        &mut self,
        tab: PanelTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        panel.tab = tab;
        panel.stash.drop_armed = false;
        if tab == PanelTab::Stash {
            self.load_stash_files(cx);
        }
        self.focus_commit_list(window, cx);
        cx.notify();
    }

    /// Takes the stashes git listed, keeping the selected one when it is still there.
    pub(crate) fn set_stashes(&mut self, found: Vec<Stash>, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        let tab = &mut panel.stash;
        let kept = tab.stashes.get(tab.selected).map(|s| s.hash.clone());
        tab.selected = kept
            .and_then(|hash| found.iter().position(|s| s.hash == hash))
            .unwrap_or(0)
            .min(found.len().saturating_sub(1));
        tab.stashes = found;
        tab.drop_armed = false;
        self.load_stash_files(cx);
    }

    /// Selects the stash at `index` of the list.
    pub(crate) fn select_stash(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        let tab = &mut panel.stash;
        if index >= tab.stashes.len() || index == tab.selected {
            return;
        }
        tab.selected = index;
        tab.drop_armed = false;
        tab.list_scroll.scroll_to_item(index);
        self.load_stash_files(cx);
        cx.notify();
    }

    /// Reads the files of the selected stash off the UI thread, when the Stash tab is showing.
    fn load_stash_files(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        if panel.tab != PanelTab::Stash {
            return;
        }
        let tab = &mut panel.stash;
        let Some(stash) = tab.stashes.get(tab.selected) else {
            tab.files.set_changes(Vec::new());
            tab.files_of = None;
            return;
        };
        if tab.files_of.as_deref() == Some(stash.hash.as_str()) {
            return;
        }
        let hash = stash.hash.clone();
        tab.files_of = Some(hash.clone());
        tab.files.set_changes(Vec::new());
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let wanted = hash.clone();
            let files = cx
                .background_executor()
                .spawn(async move { zide_git::commit_files(&root, &hash) })
                .await;
            this.update(cx, |this, cx| {
                let Some(panel) = &mut this.commit else {
                    return;
                };
                // another stash was selected meanwhile
                if panel.stash.files_of.as_deref() != Some(wanted.as_str()) {
                    return;
                }
                match files {
                    Ok(files) => panel.stash.files.set_changes(files),
                    Err(why) => this.notify(
                        Notification::error(why).title("Could not read the stash"),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Applies, pops or drops the selected stash. Drop asks twice.
    pub(crate) fn stash_action(&mut self, action: StashAction, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.commit else {
            return;
        };
        let tab = &mut panel.stash;
        let Some(stash) = tab.stashes.get(tab.selected) else {
            return;
        };
        if panel.busy {
            return;
        }
        if action == StashAction::Drop && !std::mem::replace(&mut tab.drop_armed, true) {
            return cx.notify();
        }
        tab.drop_armed = false;
        panel.busy = true;
        let index = stash.index;
        let (failure, done): (&'static str, &str) = match action {
            StashAction::Apply => ("Could not apply the stash", "Stash applied"),
            StashAction::Pop => ("Could not pop the stash", "Stash popped"),
            StashAction::Drop => ("Could not drop the stash", "Stash dropped"),
        };
        self.git_task(
            failure,
            Some(done.to_string()),
            move |root| match action {
                StashAction::Apply => zide_git::stash::apply(&root, index),
                StashAction::Pop => zide_git::stash::pop(&root, index),
                StashAction::Drop => zide_git::stash::drop_stash(&root, index),
            },
            cx,
        );
    }

    /// Keys while the Stash tab has the keyboard: `j`/`k` pick the stash, `a` applies, `p` pops.
    pub(crate) fn stash_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zide_vim::Key;
        let Some(panel) = &self.commit else {
            return;
        };
        let at = panel.stash.selected;
        match (event.keystroke.key.as_str(), crate::vim_key(event)) {
            ("down", _) | (_, Some(Key::Char('j'))) => self.select_stash(at + 1, cx),
            ("up", _) | (_, Some(Key::Char('k'))) => self.select_stash(at.saturating_sub(1), cx),
            (_, Some(Key::Char('a'))) => self.stash_action(StashAction::Apply, cx),
            (_, Some(Key::Char('p'))) => self.stash_action(StashAction::Pop, cx),
            (_, Some(Key::Esc)) => {
                if let Some(panel) = &mut self.commit {
                    panel.list_focused = false;
                }
                self.focus.focus(window, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    pub(crate) fn stash_view(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let workspace = cx.entity();
        let project = self.root.file_name().map_or_else(
            || "Changes".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let panel = self.commit.as_ref().expect("the panel is open");
        let tab = &panel.stash;
        let (selected, focused, busy) = (tab.selected, panel.list_focused, panel.busy);
        let stashes = tab.stashes.clone();
        let rows = tab.files.rows();
        let chosen = tab.files.selected_row().map(|row| row.key);
        let files_scroll = tab.files_scroll.clone();
        let list_scroll = tab.list_scroll.clone();
        let hash = stashes.get(selected).map(|s| s.hash.clone());
        let none = stashes.is_empty();
        let drop_label = if tab.drop_armed { "Sure?" } else { "Drop" };
        let highlight = if focused {
            theme::TREE_FOCUSED
        } else {
            theme::TREE_SELECTED
        };

        let list = div()
            .id("stash-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&list_scroll)
            .children(stashes.iter().enumerate().map(|(index, stash)| {
                let select = workspace.clone();
                div()
                    .id(("stash-row", index))
                    .w_full()
                    .h(px(TREE_ROW_HEIGHT + 2.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(index == selected, |d| d.bg(rgb(highlight)))
                    .on_click(move |_, window, cx| {
                        select.update(cx, |this, cx| {
                            this.select_stash(index, cx);
                            this.focus_commit_list(window, cx);
                        })
                    })
                    .child(div().flex_1().truncate().child(stash.message.clone()))
                    .when(!stash.branch.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(rgb(theme::DIM))
                                .child(format!("⎇ {}", stash.branch)),
                        )
                    })
            }))
            .when(none, |d| {
                d.child(div().p_2().text_color(rgb(theme::DIM)).child("No stashes"))
            });

        let files = uniform_list("stash-files", rows.len(), {
            let workspace = workspace.clone();
            move |range, _, _| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let key = row.key.clone();
                        let foldable = !matches!(row.kind, RowKind::File(_));
                        let (on_click, on_fold) = (workspace.clone(), workspace.clone());
                        let (open_key, fold_key) = (key.clone(), key.clone());
                        let hash = hash.clone();
                        let name = match row.kind {
                            RowKind::Root => project.clone(),
                            _ => row.name.clone(),
                        };
                        div()
                            .id(("stash-file", index))
                            .w_full()
                            .h(px(TREE_ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8. + 14. * row.depth as f32))
                            .pr(px(8.))
                            .whitespace_nowrap()
                            .when(chosen.as_deref() == Some(key.as_str()), |d| {
                                d.bg(rgb(highlight))
                            })
                            .on_click(move |event, window, cx| {
                                on_click.update(cx, |this, cx| {
                                    let Some(panel) = &mut this.commit else {
                                        return;
                                    };
                                    panel.stash.files.select(&open_key);
                                    let change = panel.stash.files.change(&open_key).cloned();
                                    if let (Some(change), Some(hash), 2) =
                                        (change, &hash, event.click_count())
                                    {
                                        this.show_commit_diff(hash, change, window, cx);
                                    }
                                    cx.notify();
                                })
                            })
                            .child(
                                div()
                                    .id(("stash-fold", index))
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
                                                    panel.stash.files.toggle_fold(&fold_key);
                                                    cx.notify();
                                                }
                                            })
                                        })
                                    }),
                            )
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
                                    .child(name),
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
        .track_scroll(&files_scroll);

        let (apply, pop, drop) = (workspace.clone(), workspace.clone(), workspace.clone());
        let stash = workspace.clone();
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
            .child(panel_tabs(&workspace, PanelTab::Stash))
            .child(list)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .child(files)
                    .child(Scrollbar::new(&files_scroll)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        Button::new("stash-apply")
                            .primary()
                            .label("Apply")
                            .disabled(none || busy)
                            .on_click(move |_, _, cx| {
                                apply.update(cx, |this, cx| {
                                    this.stash_action(StashAction::Apply, cx)
                                })
                            }),
                    )
                    .child(
                        Button::new("stash-pop")
                            .label("Pop")
                            .disabled(none || busy)
                            .on_click(move |_, _, cx| {
                                pop.update(cx, |this, cx| this.stash_action(StashAction::Pop, cx))
                            }),
                    )
                    .child(
                        Button::new("stash-new")
                            .label("Stash…")
                            .ml_auto()
                            .disabled(busy)
                            .on_click(move |_, window, cx| {
                                stash.update(cx, |this, cx| this.stash_changes(window, cx))
                            }),
                    )
                    .child(
                        Button::new("stash-drop")
                            .label(drop_label)
                            .disabled(none || busy)
                            .on_click(move |_, _, cx| {
                                drop.update(cx, |this, cx| this.stash_action(StashAction::Drop, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::DIM))
                    .child("j/k: stash · a: apply · p: pop · Esc: editor"),
            )
    }
}
