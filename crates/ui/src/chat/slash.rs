//! The `/` menu: while the message is a lone `/word`, the commands and skills Claude reported
//! for this project that match it. Tab or Enter completes, arrows move, Esc hides it.

use gpui_kit::*;

use crate::{Workspace, theme};

/// Rows the menu shows at most.
const MAX_ROWS: usize = 8;

/// The commands for what follows the `/`: those that start with it first, then those that
/// contain it, in the order Claude listed them.
fn matching(commands: &[String], query: &str) -> Vec<String> {
    let query = query.to_lowercase();
    let starts = commands
        .iter()
        .filter(|name| name.to_lowercase().starts_with(&query));
    let contains = commands.iter().filter(|name| {
        let name = name.to_lowercase();
        !name.starts_with(&query) && name.contains(&query)
    });
    starts.chain(contains).take(MAX_ROWS).cloned().collect()
}

/// What follows the `/` when the whole message is a single command word being typed.
fn typing(message: &str) -> Option<&str> {
    let word = message.strip_prefix('/')?;
    (!word.contains(char::is_whitespace)).then_some(word)
}

impl Workspace {
    /// The commands the menu offers for the active chat, `None` while it is closed.
    pub(super) fn slash_matches(&self, cx: &App) -> Option<Vec<String>> {
        let chats = self.claude.as_ref()?;
        let chat = chats.active();
        if chat.slash_dismissed || chats.sessions.is_some() {
            return None;
        }
        let message = chat.input.read(cx).value().to_string();
        let found = matching(&chats.commands, typing(&message)?);
        (!found.is_empty()).then_some(found)
    }

    pub(super) fn slash_move(&mut self, by: isize) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let chat = chats.active_mut();
        let last = MAX_ROWS - 1;
        chat.slash_selected = chat.slash_selected.saturating_add_signed(by).min(last);
    }

    /// Puts the highlighted command in the message of chat `id`. `false` when the menu is
    /// closed, so Enter goes on to send.
    pub(super) fn claude_complete(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.claude.as_ref().map(|c| c.active().id) != Some(id) {
            return false;
        }
        let Some(found) = self.slash_matches(cx) else {
            return false;
        };
        let Some(chats) = &mut self.claude else {
            return false;
        };
        let chat = chats.active_mut();
        let picked = &found[chat.slash_selected.min(found.len() - 1)];
        let text = format!("/{picked} ");
        chat.input
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
        true
    }

    /// The menu, to draw above the input box.
    pub(super) fn slash_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let found = self.slash_matches(cx)?;
        let selected = self.claude.as_ref()?.active().slash_selected;
        let selected = selected.min(found.len() - 1);
        let rows = found.into_iter().enumerate().map(|(index, name)| {
            div()
                .id(SharedString::from(format!("slash-{name}")))
                .px_3()
                .py_1()
                .cursor_pointer()
                .text_color(rgb(theme::TEXT))
                .bg(rgb(if index == selected {
                    theme::TREE_SELECTED
                } else {
                    theme::POPUP
                }))
                .child(format!("/{name}"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(chats) = &mut this.claude {
                        chats.active_mut().slash_selected = index;
                    }
                    let id = this.claude.as_ref().map(|c| c.active().id);
                    if let Some(id) = id {
                        this.claude_complete(id, window, cx);
                    }
                }))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(rgb(theme::CONTROL_BORDER))
                .bg(rgb(theme::POPUP))
                .py_1()
                .children(rows),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{matching, typing};

    fn commands() -> Vec<String> {
        ["compact", "review", "code-review", "clear"]
            .map(str::to_owned)
            .to_vec()
    }

    #[test]
    fn a_bare_slash_lists_everything_in_order() {
        assert_eq!(matching(&commands(), ""), commands());
    }

    #[test]
    fn prefix_matches_come_before_the_ones_that_only_contain_it() {
        assert_eq!(matching(&commands(), "re"), ["review", "code-review"]);
        assert_eq!(
            matching(&commands(), "C"),
            ["compact", "code-review", "clear"]
        );
        assert!(matching(&commands(), "zzz").is_empty());
    }

    #[test]
    fn only_a_lone_command_word_opens_the_menu() {
        assert_eq!(typing("/"), Some(""));
        assert_eq!(typing("/rev"), Some("rev"));
        assert_eq!(typing("/review the diff"), None);
        assert_eq!(typing("/review "), None);
        assert_eq!(typing("hello /rev"), None);
    }
}
