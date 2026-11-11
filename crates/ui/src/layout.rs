//! How the project was left when it closed, and putting it back on the next open: the panels that
//! showed and, when the Claude chat did, the conversation it was on.

use gpui_kit::*;

use crate::{Center, Workspace, commit::CommitPanel, session};

impl Workspace {
    /// The panels showing now and the conversation the chat is on.
    fn current_layout(&self, cx: &App) -> session::Layout {
        session::Layout {
            tree: self.tree_visible,
            commit: self.commit_open,
            preview: self.md_preview,
            claude: self.center == Center::Chat,
            chat: self.claude_conversation(cx),
        }
    }

    /// Writes the layout when it differs from what was last written: called on every frame, it
    /// costs a comparison, and no path that changes a panel can forget to save.
    pub(crate) fn remember_layout_if_changed(&mut self, cx: &App) {
        let layout = self.current_layout(cx);
        if self.saved_layout.as_ref() != Some(&layout) {
            session::remember_layout(&self.root, &layout);
            self.saved_layout = Some(layout);
        }
    }

    /// Puts the panels back as they were left, once, right after the project opens. A project
    /// that was never left this way keeps the defaults.
    pub(crate) fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = session::layout(&self.root) else {
            return;
        };
        self.tree_visible = layout.tree;
        self.md_preview = layout.preview;
        if layout.commit {
            if self.commit.is_none() {
                self.commit = Some(CommitPanel::new(window, cx));
            }
            self.commit_open = true;
            self.refresh_commit(cx);
        }
        if layout.claude || layout.chat.is_some() {
            self.toggle_claude(window, cx);
            if let Some((id, title)) = layout.chat {
                self.restore_conversation(id, title, cx);
            }
            if !layout.claude {
                // the chat only had to be there for the conversation
                self.center = Center::Editor;
                self.focus.focus(window, cx);
            }
        }
        self.saved_layout = Some(self.current_layout(cx));
        cx.notify();
    }
}
