//! The menu over the input box. A message that is a lone `/word` lists the commands and skills
//! Claude reported for the project; a trailing `@word` lists the project's files. Tab, Enter or
//! a click completes, arrows move, Esc hides it.

use gpui_kit::component::input::Position;
use gpui_kit::*;
use zide_search::{FileQuery, search_files};

use super::Chats;
use crate::theme;

/// Rows the menu shows at most.
const MAX_ROWS: usize = 8;

/// What the end of the message is asking for.
#[derive(Debug, PartialEq)]
enum Trigger<'a> {
    /// The whole message is `/word`.
    Command(&'a str),
    /// The message ends in `@word`, which starts at byte `start`.
    File { start: usize, word: &'a str },
}

fn trigger(message: &str) -> Option<Trigger<'_>> {
    if let Some(word) = message.strip_prefix('/') {
        return (!word.contains(char::is_whitespace)).then_some(Trigger::Command(word));
    }
    let start = message.rfind(char::is_whitespace).map_or(0, |at| {
        at + message[at..].chars().next().map_or(1, char::len_utf8)
    });
    let word = message[start..].strip_prefix('@')?;
    Some(Trigger::File { start, word })
}

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

/// `message` with the word that starts at `start` replaced by `insertion` and a space.
fn complete(message: &str, start: usize, insertion: &str) -> String {
    format!("{}{insertion} ", &message[..start])
}

/// Where the text ends: the last line, and the characters on it.
fn end_of(text: &str) -> Position {
    let line = text.matches('\n').count();
    let column = text
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count());
    Position::new(line as u32, column as u32)
}

/// One row of the menu: what it shows, and the message once it is picked.
struct Entry {
    label: String,
    message: String,
}

impl Chats {
    /// The rows the menu offers for the active chat, `None` while it is closed.
    fn menu_entries(&self, cx: &App) -> Option<Vec<Entry>> {
        let chat = self.active();
        if chat.menu_dismissed || self.sessions.is_some() {
            return None;
        }
        let message = chat.input.read(cx).value().to_string();
        let entries: Vec<Entry> = match trigger(&message)? {
            Trigger::Command(word) => matching(&self.commands, word)
                .into_iter()
                .map(|name| Entry {
                    message: complete(&message, 0, &format!("/{name}")),
                    label: format!("/{name}"),
                })
                .collect(),
            Trigger::File { start, word } => {
                let index = self.files.as_ref()?;
                let files: Vec<usize> = if word.is_empty() {
                    self.recent.iter().copied().take(MAX_ROWS).collect()
                } else {
                    search_files(index, &FileQuery::parse(word), MAX_ROWS)
                        .into_iter()
                        .map(|hit| hit.index)
                        .collect()
                };
                files
                    .into_iter()
                    .map(|file| {
                        let path = &index.paths()[file];
                        Entry {
                            message: complete(&message, start, &format!("@{path}")),
                            label: path.clone(),
                        }
                    })
                    .collect()
            }
        };
        (!entries.is_empty()).then_some(entries)
    }

    pub(super) fn menu_matches(&self, cx: &App) -> Option<usize> {
        self.menu_entries(cx).map(|entries| entries.len())
    }

    pub(super) fn menu_move(&mut self, by: isize, cx: &App) {
        let last = self.menu_matches(cx).unwrap_or(1).saturating_sub(1);
        let chat = self.active_mut();
        chat.menu_selected = chat.menu_selected.saturating_add_signed(by).min(last);
    }

    /// Puts the highlighted entry in the message of chat `id`. `false` when the menu is
    /// closed, so Enter goes on to send.
    pub(super) fn claude_complete(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active().id != id {
            return false;
        }
        let Some(mut entries) = self.menu_entries(cx) else {
            return false;
        };
        let chat = self.active_mut();
        let picked = entries.swap_remove(chat.menu_selected.min(entries.len() - 1));
        // `set_value` leaves the caret at the start; the user keeps typing after the mention
        chat.input.update(cx, |state, cx| {
            let at = end_of(&picked.message);
            state.set_value(picked.message, window, cx);
            state.set_cursor_position(at, window, cx);
        });
        cx.notify();
        true
    }

    /// The menu, to draw above the input box.
    pub(super) fn menu_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let entries = self.menu_entries(cx)?;
        let selected = self.active().menu_selected;
        let selected = selected.min(entries.len() - 1);
        let rows = entries.into_iter().enumerate().map(|(index, entry)| {
            div()
                .id(SharedString::from(format!("menu-{index}")))
                .px_3()
                .py_1()
                .cursor_pointer()
                .truncate()
                .text_color(rgb(theme::TEXT))
                .bg(rgb(if index == selected {
                    theme::TREE_SELECTED
                } else {
                    theme::POPUP
                }))
                .child(entry.label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.active_mut().menu_selected = index;
                    let id = this.active().id;
                    this.claude_complete(id, window, cx);
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
    use gpui_kit::component::input::Position;

    use super::{Trigger, complete, end_of, matching, trigger};

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
    fn only_a_lone_command_word_opens_the_command_menu() {
        assert_eq!(trigger("/"), Some(Trigger::Command("")));
        assert_eq!(trigger("/rev"), Some(Trigger::Command("rev")));
        assert_eq!(trigger("/review the diff"), None);
        assert_eq!(trigger("/review "), None);
        assert_eq!(trigger("hello /rev"), None);
    }

    #[test]
    fn a_trailing_at_word_opens_the_file_menu() {
        let file = |start, word| Some(Trigger::File { start, word });
        assert_eq!(trigger("@"), file(0, ""));
        assert_eq!(trigger("@src/ma"), file(0, "src/ma"));
        assert_eq!(trigger("explain @src/ma"), file(8, "src/ma"));
        assert_eq!(trigger("línea\n@a"), file(7, "a"));
        assert_eq!(trigger("explain @src/main.rs "), None);
        assert_eq!(trigger("mail me@home"), None);
        assert_eq!(trigger("plain"), None);
    }

    #[test]
    fn the_caret_goes_to_the_end_of_the_last_line() {
        assert_eq!(end_of("@src/main.rs "), Position::new(0, 13));
        assert_eq!(end_of("one\nñandú @a "), Position::new(1, 9));
    }

    #[test]
    fn completing_replaces_only_the_word_and_adds_a_space() {
        assert_eq!(
            complete("explain @sr", 8, "@src/main.rs"),
            "explain @src/main.rs "
        );
        assert_eq!(complete("/re", 0, "/review"), "/review ");
    }
}
