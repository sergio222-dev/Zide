//! The Claude Code chat: a full-screen view (Alt+2) over one live `claude` session. The
//! conversation itself is `zide_claude::Transcript`; this module owns the process, turns
//! its events into UI state and draws the history and the input box.

mod asks;
mod changes;
mod composer;
mod gauges;
mod images;
mod menu;
mod options;
mod report;
mod sessions;
mod tools;

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::slider::SliderState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_claude::agents::{self, Agent, Kind};
use zide_claude::history::{self, SessionInfo};
use zide_claude::{
    Block, Defaults, Event, Limits, Options, Overall, Permission, Resolution, Session, Status,
    Transcript, Value,
};

use crate::markdown::{TextView, TextViewState};
use crate::{Workspace, theme};

/// The conversations that are open, one of them showing.
pub struct Chats {
    list: Vec<Chat>,
    active: usize,
    next_id: u64,
    /// The slash commands Claude last reported for this project.
    commands: Vec<String>,
    /// What `claude` uses for the project when no model or effort is picked.
    defaults: Defaults,
    /// The account's usage limits as the last turn of any chat reported them.
    limits: Option<Limits>,
    /// The list of earlier conversations, while it is open.
    sessions: Option<sessions::Sessions>,
    /// The live agents of the project (interactive and background), as `claude agents` last
    /// listed them.
    agents: Vec<Agent>,
    /// The picture shown large over the chat, if any.
    preview: Option<std::sync::Arc<Image>>,
    /// What each live agent's conversation is called, by session id: the title the CLI gave it
    /// or the first thing said in it, which says more than the agent's own name.
    titles: HashMap<String, String>,
}

impl Chats {
    /// What to call `agent`: its conversation's title when known, else the name `claude` gave it.
    fn name_of(&self, agent: &Agent) -> String {
        self.titles
            .get(&agent.session_id)
            .cloned()
            .unwrap_or_else(|| agent.name.clone())
    }

    /// What `chat` is doing. A chat that only follows an agent running elsewhere has no process
    /// here, so it shows the state `claude agents` reports for that agent.
    fn status_of(&self, chat: &Chat) -> Status {
        let followed = chat.tail && chat.session.is_none();
        let agent = followed
            .then(|| chat.transcript.session_id())
            .flatten()
            .and_then(|id| self.agents.iter().find(|agent| agent.session_id == id));
        match agent {
            Some(agent) => agent_status(agent),
            None => chat.status(),
        }
    }

    fn new(
        commands: Vec<String>,
        defaults: Defaults,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Chats {
        let mut chats = Chats {
            list: Vec::new(),
            active: 0,
            next_id: 0,
            commands,
            defaults,
            limits: None,
            sessions: None,
            agents: Vec::new(),
            preview: None,
            titles: HashMap::new(),
        };
        chats.add(window, cx);
        chats
    }

    /// Opens an empty conversation and shows it.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Workspace>) {
        let id = self.next_id;
        self.next_id += 1;
        let chat = Chat::new(id, &self.defaults, window, cx);
        self.list.push(chat);
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
        Overall::of(self.list.iter().map(Chat::status))
    }
}

pub struct Chat {
    /// Names the chat for the events of its process; ids are never reused.
    id: u64,
    transcript: Transcript,
    /// Which process the events on screen come from: the events of an earlier one are ignored.
    run: u64,
    /// The model and effort picked, `None` for the default.
    model: Option<String>,
    effort: Option<String>,
    effort_slider: Entity<SliderState>,
    /// The running process was started in bare mode.
    bare: bool,
    /// The effort slider shows something else than `effort`.
    options_stale: bool,
    _slider: Subscription,
    /// `None` until the first message, and again once the process has exited.
    session: Option<Session>,
    input: Entity<TextareaState>,
    /// The rendered Markdown of each assistant text block, by block index.
    markdown: HashMap<usize, Entity<TextViewState>>,
    /// The Markdown of the parts of a `/context` report, by block index and part: the head is
    /// part 0 and section `n` is part `n + 1`.
    report_md: HashMap<(usize, usize), Entity<TextViewState>>,
    scroll: ScrollHandle,
    /// How many of the last blocks are drawn; a long conversation is not drawn whole.
    shown: usize,
    /// Pictures pasted and not sent yet, with the number they are shown as.
    images: Vec<images::Numbered>,
    /// The number the next picture gets: they count up through the whole conversation.
    next_image: u32,
    /// The pictures sent with each message, by the index of its block.
    sent_images: HashMap<usize, Vec<images::Numbered>>,
    /// Tool cards the user opened or folded against their default, by tool call id.
    toggled: HashSet<String>,
    /// The diff of each edit, by tool call id.
    starts: HashMap<String, Vec<usize>>,
    /// The questionnaire of each question that waits for an answer, by request id.
    asks: asks::Asks,
    /// The highlighted entry of the `/` and `@` menu, among the matches.
    menu_selected: usize,
    /// Esc closed the menu; typing opens it again.
    menu_dismissed: bool,
    /// Why the last attempt to reach Claude failed.
    notice: Option<String>,
    /// The name of the conversation: the stored session's title, or the first thing said.
    title: Option<String>,
    /// The conversation belongs to another process (a background agent, another window): while
    /// this chat has no process of its own, it follows the stored session as it grows.
    tail: bool,
    /// When the file of the conversation this chat follows was last read.
    loaded_at: Option<std::time::SystemTime>,
    _submit: Subscription,
}

impl Chat {
    fn new(id: u64, defaults: &Defaults, window: &mut Window, cx: &mut Context<Workspace>) -> Chat {
        let effort_slider = Chat::new_effort_slider(defaults, window, cx);
        let slider = Chat::effort_events(id, &effort_slider, window, cx);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder("Describe a task or ask a question")
                .submit_on_enter(true)
        });
        let submit = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Workspace, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(chat) = this.claude.as_mut().and_then(|c| c.find(id)) {
                        chat.menu_selected = 0;
                        chat.menu_dismissed = false;
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
            run: 0,
            model: None,
            effort: None,
            effort_slider,
            options_stale: false,
            bare: false,
            _slider: slider,
            session: None,
            input,
            markdown: HashMap::new(),
            report_md: HashMap::new(),
            scroll: ScrollHandle::new(),
            shown: SHOWN_STEP,
            asks: HashMap::new(),
            toggled: HashSet::new(),
            starts: HashMap::new(),
            images: Vec::new(),
            next_image: 1,
            sent_images: HashMap::new(),
            menu_selected: 0,
            menu_dismissed: false,
            notice: None,
            title: None,
            tail: false,
            loaded_at: None,
            _submit: submit,
        }
    }

    /// What the chat is doing. A chat without a process is not running, whatever it shows.
    fn status(&self) -> Status {
        if self.session.is_none() {
            Status::Inactive
        } else {
            self.transcript.status()
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
        self.session.is_none()
            && self.transcript.session_id().is_none()
            && self.transcript.blocks().is_empty()
    }

    /// Gives every assistant text block its Markdown, after a stored conversation is loaded.
    fn rebuild_markdown(&mut self, cx: &mut App) {
        let texts: Vec<(usize, String)> = self
            .transcript
            .blocks()
            .iter()
            .enumerate()
            .filter_map(|(index, block)| match block {
                Block::Text(text) => Some((index, text.clone())),
                _ => None,
            })
            .collect();
        for (index, text) in texts {
            self.build_markdown(index, &text, cx);
        }
    }

    /// Makes the Markdown of a text block: one per part for a `/context` report.
    fn build_markdown(&mut self, index: usize, text: &str, cx: &mut App) {
        match report::parse(text) {
            Some(report) => {
                let parts =
                    std::iter::once(report.head).chain(report.sections.into_iter().map(|s| s.body));
                for (part, body) in parts.enumerate() {
                    let state = cx.new(|cx| TextViewState::markdown(&body, cx));
                    self.report_md.insert((index, part), state);
                }
            }
            None => {
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
            None if self.report_md.contains_key(&(last, 0)) => {}
            None => {
                let text = text.clone();
                self.build_markdown(last, &text, cx);
            }
        }
    }
}

impl Workspace {
    /// Alt+2: shows the chat; from the chat it stays there (Esc, or Alt+1 / Ctrl+K, leaves it).
    pub(crate) fn open_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.claude_open {
            self.focus_active_chat(window, cx);
            return cx.notify();
        }
        self.toggle_claude(window, cx);
    }

    pub(crate) fn toggle_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autosave(cx); // the chat may edit the files, and leaving the editor is a save
        if self.claude_open {
            self.claude_open = false;
            self.focus.focus(window, cx);
            return cx.notify();
        }
        if self.claude.is_none() {
            let commands = crate::session::claude_commands(&self.root);
            let defaults = zide_claude::defaults(&self.root);
            let mut chats = Chats::new(commands, defaults, window, cx);
            chats.titles = crate::session::claude_titles(&self.root);
            self.claude = Some(chats);
            self.watch_agents(cx);
        }
        self.log_open = false;
        self.claude_open = true;
        self.refresh_agents(cx);
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Keeps the list of agents current while the panel is showing: `claude` is asked in the
    /// background, every few seconds.
    fn watch_agents(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGENTS_EVERY).await;
                let shown = this.update(cx, |this, cx| {
                    if this.claude_open {
                        this.refresh_agents(cx);
                    }
                });
                if shown.is_err() {
                    break; // the workspace is gone
                }
            }
        })
        .detach();
    }

    /// Asks `claude agents` now; the list changes only when the answer does. The chats that
    /// follow another process re-read their session too.
    fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let followed: Vec<(u64, String, Option<std::time::SystemTime>)> = self
            .claude
            .iter()
            .flat_map(|chats| chats.list.iter())
            .filter(|chat| chat.tail && chat.session.is_none())
            .filter_map(|chat| {
                Some((
                    chat.id,
                    chat.transcript.session_id()?.to_owned(),
                    chat.loaded_at,
                ))
            })
            .collect();
        let project = self.root.clone();
        let known: HashSet<String> = self
            .claude
            .iter()
            .flat_map(|chats| chats.titles.keys().cloned())
            .collect();
        cx.spawn(async move |this, cx| {
            let (agents, stored, found) = cx
                .background_executor()
                .spawn(async move {
                    // a conversation is read again only when its file changed
                    let stored: Vec<_> = followed
                        .into_iter()
                        .filter_map(|(id, session, since)| {
                            let modified = history::modified_at(&project, &session);
                            if modified.is_some() && modified == since {
                                return None;
                            }
                            Some((id, modified, history::load_with_images(&project, &session)))
                        })
                        .collect();
                    let agents = agents::list(&root);
                    // reading a conversation for its title is done once per agent
                    let found: Vec<(String, String)> = agents
                        .iter()
                        .filter(|agent| !known.contains(&agent.session_id))
                        .filter_map(|agent| {
                            let title = history::title(&root, &agent.session_id)?;
                            Some((agent.session_id.clone(), title))
                        })
                        .collect();
                    (agents, stored, found)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(chats) = &mut this.claude else {
                    return;
                };
                // titles of agents that are gone are forgotten, so the cache stays small
                let before = chats.titles.clone();
                chats.titles.extend(found);
                chats
                    .titles
                    .retain(|id, _| agents.iter().any(|agent| &agent.session_id == id));
                if chats.titles != before {
                    crate::session::remember_claude_titles(&this.root, &chats.titles);
                    cx.notify();
                }
                if chats.agents != agents {
                    chats.agents = agents;
                    cx.notify();
                }
                for (id, modified, loaded) in stored {
                    let Some(chat) = chats.find(id) else {
                        continue;
                    };
                    // a message of ours started a process meanwhile: that one is the truth
                    let Ok((transcript, pictures)) = loaded else {
                        continue;
                    };
                    chat.loaded_at = modified;
                    if chat.session.is_none()
                        && transcript.blocks().len() != chat.transcript.blocks().len()
                    {
                        let follow = chat.at_bottom();
                        chat.transcript = transcript;
                        (chat.sent_images, chat.next_image) = images::from_stored(pictures);
                        chat.markdown.clear();
                        chat.rebuild_markdown(cx);
                        if follow {
                            chat.scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The session id and title of the conversation the active chat is on, if it is on one.
    pub(crate) fn claude_conversation(&self) -> Option<(String, String)> {
        let chat = self.claude.as_ref()?.active();
        let id = chat.transcript.session_id()?.to_owned();
        Some((id, chat.title.clone().unwrap_or_default()))
    }

    /// Puts the active chat back on a conversation from an earlier run. It follows the agent that
    /// may still be alive: its state comes from `claude agents` until a message starts a process.
    pub(crate) fn restore_conversation(
        &mut self,
        id: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let chat_id = chats.active().id;
        chats.active_mut().tail = true;
        let session = SessionInfo {
            id,
            title,
            modified: std::time::UNIX_EPOCH,
        };
        let untitled = session.title.is_empty();
        self.load_session(chat_id, session, cx);
        if untitled && let Some(chat) = self.claude.as_mut().and_then(|c| c.find(chat_id)) {
            chat.title = None; // the first message names it, as in any new chat
        }
    }

    /// Shows what an agent is doing: the chat that already follows its session, else a new one
    /// that reads the session from disk and keeps reading it. Sending a message there continues
    /// the same session with `--resume`.
    fn claude_open_agent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let Some(agent) = chats.agents.get(index).cloned() else {
            return;
        };
        if let Some(at) = chats
            .list
            .iter()
            .position(|chat| chat.transcript.session_id() == Some(agent.session_id.as_str()))
        {
            return self.claude_activate(at, window, cx);
        }
        chats.add(window, cx);
        let id = chats.active().id;
        chats.active_mut().tail = true;
        let title = chats.name_of(&agent);
        let session = SessionInfo {
            id: agent.session_id,
            title,
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_millis(agent.started_at),
        };
        self.load_session(id, session, cx);
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// The cross on a background agent: `claude rm`. What `claude` refuses (a worktree with
    /// unpushed commits, say) is shown and the agent stays.
    fn claude_remove_agent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self
            .claude
            .as_ref()
            .and_then(|c| c.agents.get(index))
            .cloned()
        else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let id = agent.id.clone();
            let removed = cx
                .background_executor()
                .spawn(async move { agents::remove(&id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match removed {
                    Ok(_) => {
                        let open = this.claude.as_mut().and_then(|chats| {
                            chats.agents.retain(|a| a.id != agent.id);
                            chats
                                .list
                                .iter()
                                .find(|chat| {
                                    chat.transcript.session_id() == Some(agent.session_id.as_str())
                                })
                                .map(|chat| chat.id)
                        });
                        if let Some(id) = open {
                            this.claude_close(id, window, cx);
                        }
                    }
                    Err(why) => this.notify(
                        Notification::error(why).title("Could not remove the agent"),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn focus_active_chat(&self, window: &mut Window, cx: &mut App) {
        if let Some(chats) = &self.claude {
            let chat = chats.active();
            chat.input.update(cx, |state, cx| state.focus(window, cx));
            chat.scroll.scroll_to_bottom();
        }
    }

    /// Keys while the chat is showing. The input box handles its own typing.
    pub(crate) fn claude_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = keystroke.key.as_str();
        let modifiers = &keystroke.modifiers;
        if key == "escape" && self.claude.as_ref().is_some_and(|c| c.preview.is_some()) {
            self.claude_preview(None, cx);
            return cx.stop_propagation();
        }
        if modifiers.alt && !modifiers.control && !modifiers.shift {
            match key {
                "up" | "down" => {
                    let step = if key == "down" { 1 } else { -1 };
                    self.claude_cycle(step, window, cx);
                    cx.stop_propagation();
                    return;
                }
                "h" => {
                    if self.claude.as_ref().is_some_and(|c| c.sessions.is_some()) {
                        self.close_claude_history(window, cx);
                    } else {
                        self.claude_history(window, cx);
                    }
                    cx.stop_propagation();
                    return cx.notify();
                }
                "i" => {
                    self.focus_active_chat(window, cx);
                    cx.stop_propagation();
                    return cx.notify();
                }
                _ => {}
            }
        }
        if key == "v" && keystroke.modifiers.secondary(cx) && self.claude_paste_image(cx) {
            cx.stop_propagation();
            return cx.notify();
        }
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
        if self.menu_matches(cx).is_some() {
            match key {
                "down" => self.menu_move(1, cx),
                "up" => self.menu_move(-1, cx),
                "tab" => {
                    let id = self.claude.as_ref().map(|c| c.active().id);
                    id.map(|id| self.claude_complete(id, window, cx));
                }
                "escape" => {
                    if let Some(chats) = &mut self.claude {
                        chats.active_mut().menu_dismissed = true;
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
        // Esc presses Stop while there is something to stop; otherwise it leaves the panel
        let busy = self.claude.as_ref().is_some_and(|c| c.active().busy());
        if busy {
            self.claude_interrupt();
        } else {
            self.toggle_claude(window, cx);
        }
        cx.stop_propagation();
    }

    /// Stops the turn in progress of the active chat. A message that waits starts right after.
    pub(super) fn claude_interrupt(&mut self) {
        if let Some(session) = self
            .claude
            .as_mut()
            .and_then(|c| c.active_mut().session.as_mut())
        {
            let _ = session.interrupt();
        }
    }

    fn claude_send(&mut self, id: u64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        // while Claude works the message is queued by the CLI; a pending question comes first
        if (text.is_empty() && chat.images.is_empty()) || chat.status() == Status::NeedsUser {
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
        let pictures = chat.pictures();
        let sent = chat
            .session
            .as_mut()
            .map(|session| session.send_with_images(&text, &pictures));
        match sent {
            Some(Ok(())) => {
                chat.notice = None;
                chat.title.get_or_insert_with(|| {
                    match text.lines().next().filter(|line| !line.is_empty()) {
                        Some(line) => line.chars().take(60).collect(),
                        None => "Image".to_owned(),
                    }
                });
                let at = chat.transcript.blocks().len();
                chat.transcript.user_message(&text);
                let sent = std::mem::take(&mut chat.images);
                if !sent.is_empty() {
                    chat.sent_images.insert(at, sent);
                }
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
        let bare = self.claude_bare;
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return Ok(());
        };
        let resume = chat.transcript.session_id().map(str::to_owned);
        let options = Options {
            resume: resume.as_deref(),
            model: chat.model.as_deref(),
            effort: chat.effort.as_deref(),
            minimal: bare,
            ..Options::default()
        };
        let (session, events) = Session::spawn(&self.root, options)?;
        chat.session = Some(session);
        chat.bare = bare;
        chat.run += 1;
        let run = chat.run;

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
                    .update(cx, |this, cx| this.claude_event(id, run, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }

    fn claude_event(&mut self, id: u64, run: u64, event: Event, cx: &mut Context<Self>) {
        if let Event::Init { commands, .. } = &event
            && !commands.is_empty()
            && let Some(chats) = &mut self.claude
            && chats.commands != *commands
        {
            chats.commands = commands.clone();
            crate::session::remember_claude_commands(&self.root, commands);
        }
        if let Event::Limits(limits) = &event
            && let Some(chats) = &mut self.claude
        {
            chats.limits = Some(*limits);
        }
        // a closed chat has no one to tell
        let Some(chat) = self
            .claude
            .as_mut()
            .and_then(|c| c.find(id))
            .filter(|chat| chat.run == run)
        else {
            return;
        };
        let follow = chat.at_bottom();
        if matches!(event, Event::Exit) {
            chat.session = None;
        }

        let was_working = chat.transcript.status() == Status::Working;
        chat.transcript.apply(event);
        // the turn is over: the agent has nothing left to do and is not waiting for the user
        if was_working && chat.transcript.status() == Status::Idle {
            crate::sound::done();
        }
        chat.sync_markdown(cx);
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

    /// Shows the next (or previous) chat, wrapping around at the ends.
    fn claude_cycle(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &self.claude else {
            return;
        };
        let count = chats.list.len() as isize;
        let index = (chats.active as isize + step).rem_euclid(count) as usize;
        self.claude_activate(index, window, cx);
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
        let id = chats.active().id;
        self.load_session(id, session, cx);
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Fills chat `id` with a stored conversation, read in the background. The chat knows the
    /// session at once, so what is sent next continues it even while it loads.
    fn load_session(&mut self, id: u64, session: SessionInfo, cx: &mut Context<Self>) {
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(id)) else {
            return;
        };
        chat.title = Some(session.title.clone());
        chat.transcript.apply(Event::Init {
            session_id: session.id.clone(),
            commands: Vec::new(),
            model: None,
        });
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let stored = session.id.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { history::load_with_images(&root, &stored) })
                .await;
            this.update(cx, |this, cx| {
                let Some(chat) = this.claude.as_mut().and_then(|c| c.find(id)) else {
                    return;
                };
                match loaded {
                    Ok((transcript, pictures)) => {
                        (chat.sent_images, chat.next_image) = images::from_stored(pictures);
                        // continue with what the conversation was made with
                        chat.model = transcript.model().map(str::to_owned);
                        chat.effort = transcript.effort().map(str::to_owned);
                        chat.options_stale = true;
                        chat.transcript = transcript;
                        chat.rebuild_markdown(cx);
                        chat.scroll.scroll_to_bottom();
                    }
                    // an agent that has not said anything has no file yet: an empty chat
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        chat.notice = Some(format!("Could not open the session: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let open = |agent: &Agent| {
            chats
                .list
                .iter()
                .any(|chat| chat.transcript.session_id() == Some(agent.session_id.as_str()))
        };
        let rows =
            chats
                .list
                .iter()
                .enumerate()
                .map(|(index, chat)| {
                    let status = chats.status_of(chat);
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
        let agent_row = |index: usize, agent: &Agent, cx: &mut Context<Self>| {
            let background = agent.kind == Kind::Background;
            let (color, label) = agent_style(agent);
            let shown = chats.active().transcript.session_id() == Some(agent.session_id.as_str());
            div()
                .id(SharedString::from(format!(
                    "agent-row-{}",
                    agent.session_id
                )))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                // a background agent takes no messages, so its row only reports its state
                .when(!background, |row| {
                    row.cursor_pointer()
                        .bg(rgb(if shown {
                            theme::TREE_SELECTED
                        } else {
                            theme::BACKGROUND
                        }))
                        .hover(|row| row.bg(rgb(theme::TREE_SELECTED)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.claude_open_agent(index, window, cx)
                        }))
                })
                .child(div().flex_none().size_2().rounded_full().bg(rgb(color)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_color(rgb(theme::STRONG))
                                .child(chats.name_of(agent)),
                        )
                        .child(div().text_xs().text_color(rgb(color)).child(label)),
                )
                .children(background.then(|| {
                    div()
                        .id(SharedString::from(format!("agent-rm-{}", agent.id)))
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
                                this.claude_remove_agent(index, window, cx)
                            }),
                        )
                }))
        };
        // live agents of other windows and terminals, that no chat here follows yet
        let others = chats
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| agent.kind == Kind::Interactive && !open(agent))
            .map(|(index, agent)| agent_row(index, agent, cx))
            .collect::<Vec<_>>();
        let background_count = chats
            .agents
            .iter()
            .filter(|agent| agent.kind == Kind::Background)
            .count();
        let backgrounds = chats
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| agent.kind == Kind::Background)
            .map(|(index, agent)| agent_row(index, agent, cx))
            .collect::<Vec<_>>();
        let background_section = (!backgrounds.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_b_1()
                        .border_color(rgb(theme::BORDER))
                        .text_color(rgb(theme::MUTED))
                        .child(format!("Background ({background_count})")),
                )
                .children(backgrounds)
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(240.))
            .min_h_0()
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
                    .children(rows)
                    .children(others)
                    .children(background_section),
            )
    }

    /// The chat, filling the area where the editor would be.
    pub(crate) fn claude_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_asks(window, cx);
        self.ensure_diffs();
        self.sync_options(window, cx);
        let popup = self.sessions_view(window, cx);
        let Some(chats) = &self.claude else {
            return div();
        };
        let chat = chats.active();
        let total = chat.transcript.blocks().len();
        let hidden = hidden_blocks(total, chat.shown);
        let blocks = chat
            .transcript
            .blocks()
            .iter()
            .enumerate()
            .skip(hidden)
            .map(|(i, block)| self.claude_block(chat, i, block, cx))
            .collect::<Vec<_>>();
        let menu = self.menu_view(cx);
        let images = self.images_view(cx);
        let panel = self.claude_panel(chats, cx);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let background_count = chats
            .agents
            .iter()
            .filter(|agent| agent.kind == Kind::Background)
            .count();
        let shown = chats.status_of(chat);
        let ended = shown == Status::Inactive && !chat.is_empty();
        // a running process keeps the mode it started with; any other starts with the toggle's
        let bare = if chat.session.is_some() {
            chat.bare
        } else {
            self.claude_bare
        };
        let main = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
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
                            .children((hidden > 0).then(|| {
                                Button::new("claude-earlier")
                                    .ghost()
                                    .label(format!("Show {} earlier", hidden.min(SHOWN_STEP)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(chats) = &mut this.claude {
                                            chats.active_mut().shown += SHOWN_STEP;
                                        }
                                        cx.notify();
                                    }))
                            }))
                            .children(blocks)
                            .children(
                                matches!(shown, Status::Working | Status::NeedsUser)
                                    .then(|| thinking(shown, chat.transcript.compacting())),
                            )
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
                    .child(self.composer(
                        chat,
                        chats,
                        menu,
                        images,
                        bare,
                        background_count,
                        now,
                        cx,
                    )),
            )
            .children(popup)
            .children(self.preview_view(cx));
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
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
                .flex()
                .flex_col()
                .gap_2()
                .rounded_md()
                .bg(rgb(theme::POPUP))
                .px_3()
                .py_2()
                .children((!text.is_empty()).then(|| text.clone()))
                .children(
                    chat.sent_images
                        .get(&index)
                        .map(|sent| self.sent_view(sent, cx)),
                )
                .into_any_element(),
            Block::Text(text) if chat.report_md.contains_key(&(index, 0)) => {
                self.report_view(chat, index, text, cx)
            }
            Block::Text(text) => match chat.markdown.get(&index) {
                Some(state) => div()
                    .child(
                        TextView::new(state)
                            .inline_code_style(crate::markdown::inline_code())
                            .selectable(true),
                    )
                    .into_any_element(),
                None => div().child(text.clone()).into_any_element(),
            },
            Block::Thinking(text) => div()
                .text_sm()
                .italic()
                .text_color(rgb(theme::DIM))
                .child(text.clone())
                .into_any_element(),
            Block::Tool(call) => self.tool_card(chat, call, cx).into_any_element(),
            Block::Permission(permission) => self.permission_card(chat.id, index, permission, cx),
            Block::Question(ask) => self.ask_card(chat, ask, cx),
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
            .child(div().text_sm().text_color(rgb(theme::MUTED)).child(summary))
            .children(tools::input_detail(
                &permission.tool_name,
                &permission.input,
                None,
            ));
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
/// How often the background agents are asked for while the panel shows.
const AGENTS_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// The dot color and words for a background agent: busy, waiting for the user, or the state
/// `claude` reports when it is idle.
fn agent_style(agent: &Agent) -> (u32, String) {
    match (agent.status.as_str(), agent.state.as_deref()) {
        (_, Some("blocked")) => (theme::NEEDS_USER, "Needs you".to_owned()),
        ("busy", _) => (theme::WORKING, "Working".to_owned()),
        ("idle", None) => (theme::OK, "Ready".to_owned()),
        (_, state) => (
            theme::DIM,
            capitalized(state.unwrap_or(agent.status.as_str())),
        ),
    }
}

/// The `Status` of an agent `claude` lists, so a chat that follows it reads like its row.
fn agent_status(agent: &Agent) -> Status {
    match (agent.status.as_str(), agent.state.as_deref()) {
        (_, Some("blocked")) => Status::NeedsUser,
        ("busy", _) => Status::Working,
        ("idle", None) => Status::Idle,
        _ => Status::Inactive,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn status_style(status: Status) -> (u32, &'static str) {
    match status {
        Status::Inactive => (theme::DIM, "Not running"),
        Status::Idle => (theme::OK, "Ready"),
        Status::Working => (theme::WORKING, "Working"),
        Status::NeedsUser => (theme::NEEDS_USER, "Needs you"),
    }
}

/// How many blocks a chat draws at first, and how many more each "Show earlier" adds.
const SHOWN_STEP: usize = 60;

/// How many of the first blocks are left out when the last `shown` are drawn.
fn hidden_blocks(total: usize, shown: usize) -> usize {
    total.saturating_sub(shown)
}

/// The sign that Claude has not answered yet: a pulsing orange dot and what it is doing.
fn thinking(status: Status, compacting: bool) -> impl IntoElement {
    let (label, color) = match status {
        Status::NeedsUser => ("Waiting for you", theme::NEEDS_USER),
        // what the CLI does by itself when the context is full, in blue like a question
        _ if compacting => ("Auto-compacting…", theme::NEEDS_USER),
        _ => ("Thinking…", theme::WORKING),
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(rgb(color))
        .child(div().size(px(8.)).rounded_full().bg(rgb(color)))
        .child(label)
        .with_animation(
            "claude-thinking",
            Animation::new(std::time::Duration::from_millis(1200))
                .repeat()
                .with_easing(pulsating_between(0.45, 1.0)),
            |element, delta| element.opacity(delta),
        )
}

pub(super) fn footer(status: Status, transcript: &Transcript, background: usize) -> String {
    let state = match status {
        Status::Inactive => "Not running",
        Status::Idle => "Ready",
        Status::Working => "Working. Esc stops it",
        Status::NeedsUser => "Waiting for you",
    };
    let mut parts = vec![state.to_owned()];
    if transcript.queued() > 0 {
        parts.push(format!("{} queued", transcript.queued()));
    }
    if background > 0 {
        parts.push(format!("{background} in background"));
    }
    if let Some(usage) = transcript.usage() {
        parts.push(format!("context {}k", usage.context_tokens / 1000));
    }
    if transcript.cost_usd() > 0.0 {
        parts.push(format!("${:.2}", transcript.cost_usd()));
    }
    parts.join("  ·  ")
}

#[cfg(test)]
mod tests {
    use zide_claude::agents::Agent;

    use super::{agent_style, hidden_blocks};
    use crate::theme;

    fn agent(status: &str, state: Option<&str>) -> Agent {
        Agent {
            kind: zide_claude::agents::Kind::Background,
            id: "i".into(),
            session_id: "s".into(),
            name: "n".into(),
            status: status.into(),
            state: state.map(str::to_owned),
            started_at: 0,
        }
    }

    #[test]
    fn an_agent_shows_what_it_does_or_waits_for() {
        assert_eq!(
            agent_style(&agent("busy", None)),
            (theme::WORKING, "Working".into())
        );
        assert_eq!(
            agent_style(&agent("idle", Some("blocked"))),
            (theme::NEEDS_USER, "Needs you".into())
        );
        assert_eq!(
            agent_style(&agent("idle", None)),
            (theme::OK, "Ready".into())
        );
        assert_eq!(
            agent_style(&agent("idle", Some("done"))),
            (theme::DIM, "Done".into())
        );
    }

    #[test]
    fn only_the_last_blocks_are_drawn() {
        assert_eq!(hidden_blocks(10, 60), 0);
        assert_eq!(hidden_blocks(60, 60), 0);
        assert_eq!(hidden_blocks(200, 60), 140);
    }
}
