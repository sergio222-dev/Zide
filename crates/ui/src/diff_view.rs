//! A double click on a file of the commit panel opens its changes, side by side, in a large
//! dialog.

use gpui_kit::component::WindowExt;
use gpui_kit::component::diff::{Diff, DiffFile, DiffMode, DiffState};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use crate::Workspace;

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
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx
                .background_executor()
                .spawn(async move { zide_git::diff(&root, &change).map(|patch| (change, patch)) })
                .await;
            this.update_in(cx, |this, window, cx| match patch {
                Ok((change, patch)) => this.open_diff(&change.path, &patch, window, cx),
                Err(why) => this.notify(Notification::error(why).title("Could not diff it"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn open_diff(&mut self, path: &str, patch: &str, window: &mut Window, cx: &mut Context<Self>) {
        let files = match DiffFile::parse(patch) {
            Ok(files) if !files.is_empty() => files,
            _ => {
                let note = Notification::info("Nothing to compare").title(path.to_string());
                return self.notify(note, cx);
            }
        };
        self.diff_open = true;
        let state = cx.new(|cx| DiffState::new(files, cx).with_mode(DiffMode::Split));
        let title = path.to_string();
        window.open_dialog(cx, move |dialog, window, _| {
            let size = window.viewport_size();
            dialog
                .title(title.clone())
                .width(size.width * 0.92)
                .margin_top(size.height * 0.04)
                .child(
                    div()
                        .h(size.height * 0.8)
                        .child(Diff::new(&state).header_visible(false)),
                )
        });
    }
}
