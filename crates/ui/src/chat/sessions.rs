//! The list of earlier conversations: an overlay with a search box over a gpui-kit
//! `Command` list. Picking one shows it again and the next message continues it.

use std::time::{Duration, SystemTime};

use gpui_kit::component::IndexPath;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use zide_claude::history::{self, SessionInfo};

use crate::{Workspace, theme};

pub struct Sessions {
    input: Entity<InputState>,
    commands: Entity<CommandState>,
    /// `None` while the folder is being read.
    list: Option<Vec<SessionInfo>>,
    /// The highlighted match, as a position among the sessions that match the search.
    selected: usize,
    _subscription: Subscription,
}

impl Sessions {
    fn matching(&self, cx: &App) -> Vec<&SessionInfo> {
        let query = self.input.read(cx).value().to_string();
        let sessions = self.list.as_deref().unwrap_or_default();
        matching(sessions, &query)
    }

    pub(super) fn move_selection(&mut self, by: isize, cx: &App) {
        let last = self.matching(cx).len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(by).min(last);
    }
}

/// The sessions whose title has every word of `query`, in any case.
fn matching<'a>(sessions: &'a [SessionInfo], query: &str) -> Vec<&'a SessionInfo> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    sessions
        .iter()
        .filter(|session| {
            let title = session.title.to_lowercase();
            words.iter().all(|word| title.contains(word))
        })
        .collect()
}

/// How long ago, the way a list shows it: "just now", "5 min ago", "3 h ago", "2 d ago".
fn ago(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => "just now".to_owned(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    }
}

impl Workspace {
    /// Opens the list of earlier conversations over the chat (showing the chat first).
    pub(crate) fn claude_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // the earlier conversations are Claude Code's; they would resume through the ACP agent
        if crate::settings::protocol() == crate::settings::Protocol::Acp {
            let note = gpui_kit::component::notification::Notification::info(
                "Earlier conversations are Claude's: pick a Claude agent in Configure Agent to resume one",
            );
            return self.notify(note, cx);
        }
        if !self.claude_open {
            self.toggle_claude(window, cx);
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Workspace, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(sessions) = this.claude.as_mut().and_then(|c| c.sessions.as_mut()) {
                        sessions.selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.claude_pick(None, window, cx),
                _ => {}
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        let Some(chats) = &mut self.claude else {
            return;
        };
        chats.sessions = Some(Sessions {
            input,
            commands: cx.new(|cx| CommandState::new(window, cx)),
            list: None,
            selected: 0,
            _subscription: subscription,
        });

        // Reading the files is for the background: a long conversation is megabytes.
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { history::list(&root) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(sessions) = this.claude.as_mut().and_then(|c| c.sessions.as_mut()) {
                    sessions.list = Some(list);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_claude_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        chats.sessions = None;
        chats
            .active()
            .input
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Continues the session at `row` among the matches (the highlighted one for `None`).
    pub(super) fn claude_pick(
        &mut self,
        row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sessions) = self.claude.as_ref().and_then(|c| c.sessions.as_ref()) else {
            return;
        };
        let row = row.unwrap_or(sessions.selected);
        let Some(picked) = sessions.matching(cx).get(row).map(|s| (*s).clone()) else {
            return;
        };
        self.close_claude_history(window, cx);
        self.claude_resume(picked, window, cx);
    }

    /// The overlay, when the list is open.
    pub(super) fn sessions_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let chats = self.claude.as_mut()?;
        let current = chats.active().transcript.session_id().map(str::to_owned);
        let sessions = chats.sessions.as_mut()?;
        let found = sessions.matching(cx).len();
        sessions.selected = sessions.selected.min(found.saturating_sub(1));
        let (state, selected) = (sessions.commands.clone(), sessions.selected);
        window.defer(cx, move |window, cx| {
            state.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(selected)), window, cx)
            });
        });

        let now = SystemTime::now();
        let items: Vec<_> = sessions
            .matching(cx)
            .into_iter()
            .map(|session| {
                let title = session.title.clone();
                let when = ago(now.duration_since(session.modified).unwrap_or_default());
                let is_current = current.as_deref() == Some(session.id.as_str());
                CommandItem::new().child(move |_, _| {
                    div()
                        .flex()
                        .gap_3()
                        .justify_between()
                        .child(div().min_w_0().truncate().child(title.clone()))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(rgb(theme::DIM))
                                .child(if is_current {
                                    format!("current · {when}")
                                } else {
                                    when.clone()
                                }),
                        )
                })
            })
            .collect();
        let loading = sessions.list.is_none();
        let (hovered, confirmed) = (cx.entity(), cx.entity());
        let list = Command::new(&sessions.commands)
            .items(items)
            .searchable(false)
            .bordered(false)
            .max_h(px(360.))
            .empty(move |_, _, _| {
                div().p_3().text_color(rgb(theme::DIM)).child(if loading {
                    "Loading…"
                } else {
                    "No sessions"
                })
            })
            .on_select(move |path, _, cx| {
                hovered.update(cx, |this, cx| {
                    if let Some(sessions) = this
                        .claude
                        .as_mut()
                        .and_then(|c| c.sessions.as_mut())
                        .filter(|s| s.selected != path.row)
                    {
                        sessions.selected = path.row;
                        cx.notify();
                    }
                });
            })
            .on_confirm(move |path, window, cx| {
                confirmed.update(cx, |this, cx| this.claude_pick(Some(path.row), window, cx));
            });
        Some(
            div()
                .absolute()
                .top(px(80.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(560.))
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .bg(rgb(theme::POPUP))
                        .child(Input::new(&sessions.input))
                        .child(list),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    // not `super::*`: it would bring in gpui's own `test` macro through the kit's glob
    use std::time::{Duration, SystemTime};

    use zide_claude::history::SessionInfo;

    use super::{ago, matching};

    fn session(title: &str) -> SessionInfo {
        SessionInfo {
            id: title.to_owned(),
            title: title.to_owned(),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn every_word_of_the_search_must_be_in_the_title() {
        let all = [
            session("Fix the login bug"),
            session("Login page design"),
            session("Refactor tree"),
        ];
        let titles = |query| -> Vec<_> {
            matching(&all, query)
                .into_iter()
                .map(|s| s.title.as_str())
                .collect()
        };
        assert_eq!(
            titles(""),
            ["Fix the login bug", "Login page design", "Refactor tree"]
        );
        assert_eq!(titles("LOGIN"), ["Fix the login bug", "Login page design"]);
        assert_eq!(titles("login bug"), ["Fix the login bug"]);
        assert!(titles("nothing").is_empty());
    }

    #[test]
    fn ago_picks_the_largest_unit() {
        let secs = Duration::from_secs;
        assert_eq!(ago(secs(20)), "just now");
        assert_eq!(ago(secs(5 * 60)), "5 min ago");
        assert_eq!(ago(secs(3 * 3600 + 10)), "3 h ago");
        assert_eq!(ago(secs(2 * 86400)), "2 d ago");
    }
}
