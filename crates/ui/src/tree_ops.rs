//! The project tree's file keys: `m` moves or renames, `d` deletes, `y` copies and `p` pastes.

use std::path::PathBuf;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use crate::{Workspace, fileops, theme};

/// The content of the move and new dialogs: a path to edit, and why the one typed is not good.
pub struct PathDialog {
    input: Entity<InputState>,
    error: Option<String>,
    root: PathBuf,
    /// The entry being moved; `None` when the dialog makes a new one.
    from: Option<String>,
    _subscription: Subscription,
}

impl PathDialog {
    fn new(
        root: PathBuf,
        from: Option<String>,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| state.set_value(&value, window, cx));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.validate(cx);
            }
        });
        Self {
            input,
            error: None,
            root,
            from,
            _subscription: subscription,
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn validate(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let typed = self.input.read(cx).value().to_string();
        let checked = match &self.from {
            Some(from) => fileops::check_move(&self.root, from, &typed),
            None => fileops::check_new(&self.root, &typed).map(|_| typed),
        };
        self.error = checked.as_ref().err().cloned();
        cx.notify();
        checked.ok()
    }
}

impl Render for PathDialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child("Path:")
                    .child(div().flex_1().child(Input::new(&self.input))),
            )
            .children(self.from.is_none().then(|| {
                div()
                    .text_xs()
                    .text_color(rgb(theme::MUTED))
                    .child("A name ending with / creates a folder; any other creates a file.")
            }))
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_xs().text_color(rgb(theme::ERROR)).child(error)),
            )
    }
}

/// Whether `path` is `parent` or inside it.
fn is_under(path: &str, parent: &str) -> bool {
    fileops::follow(path, parent, "").is_some()
}

impl Workspace {
    /// The selected row of the tree: its path and whether it is a folder.
    fn selected_entry(&self) -> Option<(String, bool)> {
        let row = self.tree.rows().get(self.tree.selected()?)?;
        Some((row.path.clone(), row.dir))
    }

    /// `m`: asks for the new path of the selected entry, and moves it there.
    pub(crate) fn tree_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, _)) = self.selected_entry() else {
            return;
        };
        let view = cx.new(|cx| {
            PathDialog::new(
                self.root.clone(),
                Some(from.clone()),
                from.clone(),
                window,
                cx,
            )
        });
        let workspace = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_alert_dialog(cx, move |alert, window, cx| {
            if first.replace(false) {
                view.update(cx, |dialog, cx| dialog.focus(window, cx));
            }
            let (view, workspace, from) = (view.clone(), workspace.clone(), from.clone());
            alert
                .title("Move or Rename")
                .child(view.clone())
                .ok_text("Move")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    // a path that is not good keeps the dialog open, with the reason in it
                    let Some(to) = view.update(cx, |dialog, cx| dialog.validate(cx)) else {
                        return false;
                    };
                    let from = from.clone();
                    workspace.update(cx, |this, cx| this.move_entry(&from, &to, cx));
                    true
                })
        });
    }

    /// `a`: asks for the name of a new file, or folder when it ends with `/`, starting from the
    /// selected folder (or the folder of the selected file).
    pub(crate) fn tree_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = match self.selected_entry() {
            Some((path, true)) if !path.is_empty() => format!("{path}/"),
            Some((path, false)) => path
                .rsplit_once('/')
                .map(|(folder, _)| format!("{folder}/"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let view = cx.new(|cx| PathDialog::new(self.root.clone(), None, start, window, cx));
        let workspace = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_alert_dialog(cx, move |alert, window, cx| {
            if first.replace(false) {
                view.update(cx, |dialog, cx| dialog.focus(window, cx));
            }
            let (view, workspace) = (view.clone(), workspace.clone());
            alert
                .title("New File or Folder")
                .child(view.clone())
                .ok_text("Create")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    // a name that is not good keeps the dialog open, with the reason in it
                    let Some(typed) = view.update(cx, |dialog, cx| dialog.validate(cx)) else {
                        return false;
                    };
                    workspace.update(cx, |this, cx| this.create_entry(&typed, cx));
                    true
                })
        });
    }

    fn create_entry(&mut self, typed: &str, cx: &mut Context<Self>) {
        let (path, folder) = match fileops::create(&self.root, typed) {
            Ok(created) => created,
            Err(why) => {
                return self.notify(Notification::error(why).title("Could not create it"), cx);
            }
        };
        self.tree_reveal = Some(path.clone());
        if !folder {
            self.autosave(cx);
            self.open_file(&path, None, cx);
            self.tree_focused = false; // the new file is for editing
        }
        self.rescan(cx);
        self.refresh_commit(cx);
        cx.notify();
    }

    fn move_entry(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        if from == to {
            return;
        }
        self.autosave(cx); // the edits go with the file
        if let Err(why) = fileops::move_path(&self.root, from, to) {
            return self.notify(Notification::error(why).title("Could not move it"), cx);
        }
        let reopen = self
            .viewer
            .as_ref()
            .and_then(|viewer| fileops::follow(viewer.path(), from, to));
        let moved: Vec<String> = self
            .parked
            .keys()
            .filter(|path| is_under(path, from))
            .cloned()
            .collect();
        for path in moved {
            self.parked.remove(&path);
        }
        if let Some(path) = reopen {
            self.viewer = None;
            self.show_file(&path, None, cx);
        }
        self.tree_reveal = Some(to.to_string());
        self.rescan(cx);
        self.refresh_commit(cx);
        cx.notify();
    }

    /// `d`: asks before deleting the selected entry.
    pub(crate) fn tree_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, dir)) = self.selected_entry() else {
            return;
        };
        let workspace = cx.entity();
        let question = if dir {
            format!("Delete the folder '{path}' and everything in it?")
        } else {
            format!("Delete '{path}'?")
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (workspace, path) = (workspace.clone(), path.clone());
            alert
                .title("Delete")
                .description(question.clone())
                .ok_text("Delete")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let path = path.clone();
                    workspace.update(cx, |this, cx| this.delete_entry(path, cx));
                    true
                })
        });
    }

    fn delete_entry(&mut self, path: String, cx: &mut Context<Self>) {
        self.autosave(cx);
        // the editor lets go of it now: saving again must not bring the file back
        if self
            .viewer
            .as_ref()
            .is_some_and(|viewer| is_under(viewer.path(), &path))
        {
            self.viewer = None;
        }
        self.parked.retain(|parked, _| !is_under(parked, &path));
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let asked = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { fileops::delete(&root, &asked) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.tree_reveal = path.rsplit_once('/').map(|(parent, _)| parent.into());
                    }
                    Err(why) => this.notify(Notification::error(why).title("Could not delete"), cx),
                }
                this.rescan(cx);
                this.refresh_commit(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `y`: remembers the selected entry to paste.
    pub(crate) fn tree_copy(&mut self) {
        let Some((path, _)) = self.selected_entry() else {
            return;
        };
        self.tree_clipboard = Some(path);
    }

    /// `p`: pastes the copied entry into the selected folder, or the folder of the selected file.
    pub(crate) fn tree_paste(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.tree_clipboard.clone() else {
            return self.notify(Notification::info("Nothing copied: press y on a file"), cx);
        };
        let into = match self.selected_entry() {
            Some((path, true)) => path,
            Some((path, false)) => path
                .rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_default(),
            None => String::new(),
        };
        self.autosave(cx); // the copy is of what the editor shows
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fileops::copy_into(&root, &source, &into) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(copy) => this.tree_reveal = Some(copy),
                    Err(why) => this.notify(Notification::error(why).title("Could not paste"), cx),
                }
                this.rescan(cx);
                this.refresh_commit(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
