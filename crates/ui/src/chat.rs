//! The Claude Code chat: a full-screen view (Alt+2) over one live `claude` session. The
//! conversation itself is `zide_claude::Transcript`; this module owns the process, turns
//! its events into UI state and draws the history and the input box.

mod sessions;
mod slash;

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::*;
use zide_claude::history::{self, SessionInfo};
use zide_claude::{
    Block, Event, Options, Overall, Permission, Resolution, Session, Status, ToolCall, Transcript,
    Value,
};

use crate::markdown::{TextView, TextViewState};
use crate::{Workspace, theme};

/// What a question tells Claude while the chat cannot show questions yet.
const NO_QUESTIONS: &str =
    "This chat cannot show multiple-choice questions yet. Ask in plain text instead.";

/// The conversations that are open, one of them showing.
pub struct Chats {
    list: Vec<Chat>,
    active: usize,
    next_id: u64,
    /// The slash commands Claude last reported for this project.
    commands: Vec<String>,
    /// The list of earlier conversations, while it is open.
    sessions: Option<sessions::Sessions>,
}

impl Chats {
    fn new(commands: Vec<String>, window: &mut Window, cx: &mut Context<Workspace>) -> Chats {
        let mut chats = Chats {
            list: Vec::new(),
            active: 0,
            next_id: 0,
            commands,
            sessions: None,
        };
        chats.add(window, cx);
        chats
    }

    /// Opens an empty conversation and shows it.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Workspace>) {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Chat::new(id, window, cx));
        self.active = self.list.len() - 1;
    }

    fn active(&self) -> &Chat {
        &self.list[self.active]
    }

    fn active_mut(&mut self) -> &mut Chat {
        &mut self.list[self.active]
    }

    fn find(&mut self, id: u64) -> Option<&mut Chat> {
        self.list.iter_mut().find(|chat| chat.id == id)
    }

    /// How the chats stand together, for the status bar.
    pub fn overall(&self) -> Overall {
        Overall::of(self.list.iter().map(|chat| chat.transcript.status()))
    }
}

pub struct Chat {
    /// Names the chat for the events of its process; ids are never reused.
    id: u64,
    transcript: Transcript,
    /// `None` until the first message, and again once the process has exited.
    session: Option<Session>,
    input: Entity<TextareaState>,
    /// The rendered Markdown of each assistant text block, by block index.
    markdown: HashMap<usize, Entity<TextViewState>>,
    scroll: ScrollHandle,
    /// The highlighted command of the `/` menu, among the matches.
    slash_selected: usize,
    /// Esc closed the `/` menu; typing opens it again.
    slash_dismissed: bool,
    /// Why the last attempt to reach Claude failed.
    notice: Option<String>,
    /// The name of the conversation: the stored session's title, or the first thing said.
    title: Option<String>,
    _submit: Subscription,
}

impl Chat {
    fn new(id: u64, window: &mut Window, cx: &mut Context<Workspace>) -> Chat {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder(
                    "Message Claude. Enter sends, Shift+Enter adds a line, Esc interrupts.",
                )
                .submit_on_enter(true)
        });
        let submit = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Workspace, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(chat) = this.claude.as_mut().and_then(|c| c.find(id)) {
                        chat.slash_selected = 0;
                        chat.slash_dismissed = false;
                    }
                    cx.notify();
                }
                // with the `/` menu open, Enter completes instead of sending
                InputEvent::PressEnter { shift: false, .. }
                    if !this.claude_complete(id, window, cx) =>
                {
                    let text = input.read(cx).value().to_string();
                    this.claude_send(id, text, window, cx);
                }
                _ => {}
            },
        );
        Chat {
            id,
            transcript: Transcript::new(),
            session: None,
            input,
            markdown: HashMap::new(),
            scroll: ScrollHandle::new(),
            slash_selected: 0,
            slash_dismissed: false,
            notice: None,
            title: None,
            _submit: submit,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self.transcript.status(),
            Status::Working | Status::NeedsUser
        )
    }

    /// Whether the history is scrolled to its end, so new text should keep it there.
    fn at_bottom(&self) -> bool {
        self.scroll.max_offset().y + self.scroll.offset().y < px(8.)
    }

    /// Whether nothing was said here: a stored conversation can take its place.
    fn is_empty(&self) -> bool {
        self.session.is_none() && self.transcript.blocks().is_empty()
    }

    /// Gives every assistant text block its Markdown, after a stored conversation is loaded.
    fn rebuild_markdown(&mut self, cx: &mut App) {
        for (index, block) in self.transcript.blocks().iter().enumerate() {
            if let Block::Text(text) = block {
                let state = cx.new(|cx| TextViewState::markdown(text, cx));
                self.markdown.insert(index, state);
            }
        }
    }

    /// Keeps the Markdown of the block that is growing in step with its text.
    fn sync_markdown(&mut self, cx: &mut App) {
        let last = self.transcript.blocks().len().saturating_sub(1);
        let Some(Block::Text(text)) = self.transcript.blocks().get(last) else {
            return;
        };
        match self.markdown.get(&last) {
            Some(state) => state.update(cx, |state, cx| state.set_text(text, cx)),
            None => {
                let state = cx.new(|cx| TextViewState::markdown(text, cx));
                self.markdown.insert(last, state);
            }
        }
    }
}

impl Workspace {
    pub(crate) fn toggle_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.claude_open {
            self.claude_open = false;
            self.focus.focus(window, cx);
            return cx.notify();
        }
        if self.claude.is_none() {
            let commands = crate::session::claude_commands(&self.root);
            self.claude = Some(Chats::new(commands, window, cx));
        }
        self.claude_open = true;
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    fn focus_active_chat(&self, window: &mut Window, cx: &mut App) {
        if let Some(chats) = &self.claude {
            let chat = chats.active();
            chat.input.update(cx, |state, cx| state.focus(window, cx));
            chat.scroll.scroll_to_bottom();
        }
    }

    /// Keys while the chat is showing. The input box handles its own typing.
    pub(crate) fn claude_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if let Some(sessions) = self.claude.as_mut().and_then(|c| c.sessions.as_mut()) {
            match key {
                "down" => sessions.move_selection(1, cx),
                "up" => sessions.move_selection(-1, cx),
                "escape" => self.close_claude_history(window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return cx.notify();
        }
        if self.slash_matches(cx).is_some() {
            match key {
                "down" => self.slash_move(1),
                "up" => self.slash_move(-1),
                "tab" => {
                    let id = self.claude.as_ref().map(|c| c.active().id);
                    id.map(|id| self.claude_complete(id, window, cx));
                }
                "escape" => {
                    if let Some(chats) = &mut self.claude {
                        chats.active_mut().slash_dismissed = true;
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            return cx.notify();
        }
        if key != "escape" {
            return;
        }
        let busy = self.claude.as_ref().is_some_and(|c| c.active().busy());
        if busy {
            if let Some(session) = self
                .claude
                .as_mut()
                .and_then(|c| c.active_mut().session.as_mut())
            {
                let _ = session.interrupt();
            }
        } else {
            self.toggle_claude(window, cx);
        }
        cx.stop_propagation();
    }

    fn claude_send(&mut self, id: u64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        if text.is_empty() || chat.busy() {
            return;
        }
        if chat.session.is_none()
            && let Err(error) = self.start_claude(id, cx)
        {
            if let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) {
                chat.notice = Some(format!("Could not start `claude`: {error}"));
            }
            return cx.notify();
        }
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        let sent = chat.session.as_mut().map(|session| session.send(&text));
        match sent {
            Some(Ok(())) => {
                chat.notice = None;
                chat.title.get_or_insert_with(|| {
                    text.lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect()
                });
                chat.transcript.user_message(&text);
                chat.input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                chat.scroll.scroll_to_bottom();
            }
            Some(Err(error)) => {
                chat.session = None;
                chat.notice = Some(format!("Could not send the message: {error}"));
            }
            None => {}
        }
        cx.notify();
    }

    /// Starts `claude` for a chat, continuing its conversation if one was running before.
    fn start_claude(&mut self, id: u64, cx: &mut Context<Self>) -> std::io::Result<()> {
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return Ok(());
        };
        let resume = chat.transcript.session_id().map(str::to_owned);
        let options = Options {
            resume: resume.as_deref(),
            ..Options::default()
        };
        let (session, events) = Session::spawn(&self.root, options)?;
        chat.session = Some(session);

        // The session reports on a std channel; a thread forwards it to the UI's executor.
        let (forward, incoming) = async_channel::unbounded::<Event>();
        std::thread::spawn(move || {
            for event in events {
                if forward.send_blocking(event).is_err() {
                    break;
                }
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok(event) = incoming.recv().await {
                if this
                    .update(cx, |this, cx| this.claude_event(id, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }

    fn claude_event(&mut self, id: u64, event: Event, cx: &mut Context<Self>) {
        if let Event::Init { commands, .. } = &event
            && !commands.is_empty()
            && let Some(chats) = &mut self.claude
            && chats.commands != *commands
        {
            chats.commands = commands.clone();
            crate::session::remember_claude_commands(&self.root, commands);
        }
        // a closed chat has no one to tell
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        let follow = chat.at_bottom();
        let question = match &event {
            Event::Question { request_id, .. } => Some(request_id.clone()),
            _ => None,
        };
        if matches!(event, Event::Exit) {
            chat.session = None;
        }

        chat.transcript.apply(event);
        chat.sync_markdown(cx);
        if let Some(request_id) = question {
            if let Some(session) = &mut chat.session {
                let _ = session.deny(&request_id, NO_QUESTIONS);
            }
            chat.transcript.resolve(&request_id, Resolution::Denied);
        }
        if follow {
            chat.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Opens an empty conversation next to the others.
    pub(crate) fn claude_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.claude_open {
            self.toggle_claude(window, cx);
        }
        if let Some(chats) = &mut self.claude {
            chats.add(window, cx);
        }
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    fn claude_activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(chats) = self.claude.as_mut().filter(|c| index < c.list.len()) {
            chats.active = index;
        }
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Closes a chat; its process, if any, stops. The last one is replaced by an empty chat.
    fn claude_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let Some(index) = chats.list.iter().position(|chat| chat.id == id) else {
            return;
        };
        chats.list.remove(index);
        if chats.list.is_empty() {
            chats.add(window, cx);
        } else if index < chats.active || chats.active >= chats.list.len() {
            chats.active = chats.active.saturating_sub(1);
        }
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Shows a stored conversation: the chat that already has it, else an empty chat. The next
    /// message continues it with `--resume`; nothing runs until then.
    pub(super) fn claude_resume(
        &mut self,
        session: SessionInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        if let Some(index) = chats
            .list
            .iter()
            .position(|chat| chat.transcript.session_id() == Some(session.id.as_str()))
        {
            return self.claude_activate(index, window, cx);
        }
        if !chats.active().is_empty() {
            chats.add(window, cx);
        }
        let chat = chats.active_mut();
        chat.title = Some(session.title.clone());
        let id = chat.id;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let stored = session.id.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { history::load(&root, &stored) })
                .await;
            this.update(cx, |this, cx| {
                let Some(chat) = this.claude.as_mut().and_then(|c| c.find(id)) else {
                    return;
                };
                match loaded {
                    Ok(transcript) => {
                        chat.transcript = transcript;
                        chat.rebuild_markdown(cx);
                        chat.scroll.scroll_to_bottom();
                    }
                    Err(error) => {
                        chat.notice = Some(format!("Could not open the session: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    fn claude_decide(&mut self, id: u64, request_id: &str, allow: bool, cx: &mut Context<Self>) {
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        let input = chat
            .transcript
            .blocks()
            .iter()
            .find_map(|block| match block {
                Block::Permission(p) if p.request_id == request_id => Some(p.input.clone()),
                _ => None,
            });
        let (Some(input), Some(session)) = (input, chat.session.as_mut()) else {
            return;
        };
        let sent = if allow {
            session.allow(request_id, &input)
        } else {
            session.deny(request_id, "Denied by the user")
        };
        if sent.is_ok() {
            let resolution = if allow {
                Resolution::Allowed
            } else {
                Resolution::Denied
            };
            chat.transcript.resolve(request_id, resolution);
        }
        cx.notify();
    }

    /// The panel with the open chats and how each stands.
    fn claude_panel(&self, chats: &Chats, cx: &mut Context<Self>) -> Div {
        let rows =
            chats
                .list
                .iter()
                .enumerate()
                .map(|(index, chat)| {
                    let status = chat.transcript.status();
                    let (color, label) = status_style(status);
                    let id = chat.id;
                    div()
                        .id(SharedString::from(format!("chat-row-{id}")))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .cursor_pointer()
                        .bg(rgb(if index == chats.active {
                            theme::TREE_SELECTED
                        } else {
                            theme::BACKGROUND
                        }))
                        .hover(|row| row.bg(rgb(theme::TREE_SELECTED)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.claude_activate(index, window, cx)
                        }))
                        .child(div().flex_none().size_2().rounded_full().bg(rgb(color)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().text_color(rgb(theme::STRONG)).child(
                                    chat.title.clone().unwrap_or_else(|| "New chat".to_owned()),
                                ))
                                .child(div().text_xs().text_color(rgb(color)).child(label)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("chat-close-{id}")))
                                .flex_none()
                                .px_1()
                                .text_color(rgb(theme::DIM))
                                .hover(|close| close.text_color(rgb(theme::STRONG)))
                                .child("×")
                                // on mouse down so the row under it never sees a click
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.claude_close(id, window, cx)
                                    }),
                                ),
                        )
                })
                .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(240.))
            .h_full()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(div().text_color(rgb(theme::MUTED)).child("Chats"))
                    .child(
                        Button::new("claude-new").ghost().label("New").on_click(
                            cx.listener(|this, _, window, cx| this.claude_new(window, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("claude-chats")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    /// The chat, filling the area where the editor would be.
    pub(crate) fn claude_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let popup = self.sessions_view(window, cx);
        let Some(chats) = &self.claude else {
            return div();
        };
        let chat = chats.active();
        let blocks = chat
            .transcript
            .blocks()
            .iter()
            .enumerate()
            .map(|(i, block)| self.claude_block(chat, i, block, cx))
            .collect::<Vec<_>>();
        let slash_menu = self.slash_view(cx);
        let panel = self.claude_panel(chats, cx);
        let ended = chat.transcript.status() == Status::Inactive && !chat.is_empty();
        let main = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(theme::STRONG))
                            .child(chat.title.clone().unwrap_or_else(|| "New chat".to_owned())),
                    )
                    .child(
                        Button::new("claude-history-open")
                            .ghost()
                            .label("History")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.claude_history(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("claude-history")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&chat.scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_4()
                            .max_w(px(900.))
                            .mx_auto()
                            .children(blocks)
                            .children(chat.notice.iter().map(|notice| {
                                div().text_color(rgb(theme::ERROR)).child(notice.clone())
                            }))
                            .children(ended.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme::DIM))
                                    .child("Not running. Send a message to start it again.")
                            })),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .p_2()
                    .child(
                        div()
                            .max_w(px(900.))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(slash_menu)
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(theme::CONTROL_BORDER))
                                    .bg(rgb(theme::POPUP))
                                    .px_3()
                                    .py_2()
                                    .child(Textarea::new(&chat.input)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme::DIM))
                                    .child(footer(&chat.transcript)),
                            ),
                    ),
            )
            .children(popup);
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .text_color(rgb(theme::TEXT))
            .child(panel)
            .child(main)
    }

    fn claude_block(
        &self,
        chat: &Chat,
        index: usize,
        block: &Block,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match block {
            Block::User(text) => div()
                .rounded_md()
                .bg(rgb(theme::POPUP))
                .px_3()
                .py_2()
                .child(text.clone())
                .into_any_element(),
            Block::Text(text) => match chat.markdown.get(&index) {
                Some(state) => div()
                    .child(TextView::new(state).selectable(true))
                    .into_any_element(),
                None => div().child(text.clone()).into_any_element(),
            },
            Block::Thinking(text) => div()
                .text_sm()
                .italic()
                .text_color(rgb(theme::DIM))
                .child(text.clone())
                .into_any_element(),
            Block::Tool(call) => tool_line(call).into_any_element(),
            Block::Permission(permission) => self.permission_card(chat.id, index, permission, cx),
            // Questions are denied on arrival until the chat can show them.
            Block::Question(_) => div()
                .text_sm()
                .text_color(rgb(theme::DIM))
                .child("Claude asked a question the chat cannot show yet.")
                .into_any_element(),
            Block::Interrupted => div()
                .text_sm()
                .text_color(rgb(theme::DIM))
                .child("Interrupted")
                .into_any_element(),
            Block::Error(message) => div()
                .text_color(rgb(theme::ERROR))
                .child(message.clone())
                .into_any_element(),
        }
    }

    fn permission_card(
        &self,
        chat_id: u64,
        index: usize,
        permission: &Permission,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = summary(&permission.input);
        let card = div()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::ACTIVE_BORDER))
            .p_3()
            .child(format!("Claude wants to use {}", permission.tool_name))
            .child(div().text_sm().text_color(rgb(theme::MUTED)).child(summary));
        match &permission.resolution {
            None => {
                let (allow_id, deny_id) =
                    (permission.request_id.clone(), permission.request_id.clone());
                card.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("allow-{index}")))
                                .primary()
                                .label("Allow")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.claude_decide(chat_id, &allow_id, true, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("deny-{index}")))
                                .label("Deny")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.claude_decide(chat_id, &deny_id, false, cx)
                                })),
                        ),
                )
            }
            Some(resolution) => card.child(div().text_sm().text_color(rgb(theme::DIM)).child(
                match resolution {
                    Resolution::Allowed => "Allowed",
                    Resolution::Denied => "Denied",
                    Resolution::Answered(_) => "Answered",
                    Resolution::Cancelled => "Cancelled",
                },
            )),
        }
        .into_any_element()
    }
}

/// One line for a tool call: a mark for how it went, its name and what it works on.
fn tool_line(call: &ToolCall) -> Div {
    let (mark, color) = match &call.output {
        None => ("…", theme::DIM),
        Some(output) if output.is_error => ("✗", theme::ERROR),
        Some(_) => ("✓", theme::OK),
    };
    div()
        .flex()
        .gap_2()
        .text_sm()
        .child(div().text_color(rgb(color)).child(mark))
        .child(
            div()
                .text_color(rgb(theme::STRONG))
                .child(call.name.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(theme::MUTED))
                .child(summary(&call.input)),
        )
}

/// The argument of a tool that says most about what it does, on one line.
fn summary(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "pattern",
        "path",
        "url",
        "query",
        "description",
    ] {
        if let Some(text) = input[key].as_str() {
            let line = text.lines().next().unwrap_or_default();
            return match line.char_indices().nth(120) {
                Some((end, _)) => format!("{}…", &line[..end]),
                None => line.to_owned(),
            };
        }
    }
    String::new()
}

/// The color and the words a chat's state is shown with.
fn status_style(status: Status) -> (u32, &'static str) {
    match status {
        Status::Inactive => (theme::DIM, "Not running"),
        Status::Idle => (theme::OK, "Ready"),
        Status::Working => (theme::WORKING, "Working"),
        Status::NeedsUser => (theme::NEEDS_USER, "Needs you"),
    }
}

fn footer(transcript: &Transcript) -> String {
    let state = match transcript.status() {
        Status::Inactive => "Not running",
        Status::Idle => "Ready",
        Status::Working => "Working. Esc interrupts",
        Status::NeedsUser => "Waiting for you",
    };
    let mut parts = vec![state.to_owned()];
    if let Some(usage) = transcript.usage() {
        parts.push(format!("context {}k", usage.context_tokens / 1000));
    }
    if transcript.cost_usd() > 0.0 {
        parts.push(format!("${:.2}", transcript.cost_usd()));
    }
    parts.join("  ·  ")
}
