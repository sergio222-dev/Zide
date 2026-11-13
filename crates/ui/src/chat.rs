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
mod workers;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::settings::Protocol;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::slider::SliderState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_claude::agents::{self, Agent, Kind};
use zide_claude::history::{self, SessionInfo};
use zide_claude::{
    AgentSession, Block, Defaults, Event, Limits, Options, Overall, Permission, Resolution,
    Session, Status, Transcript, Value,
};

use crate::markdown::{TextView, TextViewState};
use crate::{Center, Workspace, theme};
use zide_index::FileIndex;

/// The conversations that are open, one of them showing.
pub struct Chats {
    /// The project the chats work in.
    root: PathBuf,
    /// New processes start in bare mode (Claude: Bare Mode).
    bare: bool,
    /// The project's files and the recently opened ones, for the `@` menu: the workspace's,
    /// handed over whenever they change.
    files: Option<Arc<FileIndex>>,
    recent: Vec<usize>,
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
    /// The worker whose view replaces the chat area, as (its chat's id, its tool use id).
    viewing: Option<(u64, String)>,
    /// Where the worker view is scrolled.
    worker_scroll: ScrollHandle,
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

    fn new(root: PathBuf, bare: bool, window: &mut Window, cx: &mut Context<Chats>) -> Chats {
        let mut chats = Chats {
            commands: crate::session::claude_commands(&root),
            defaults: zide_claude::defaults(&root),
            titles: crate::session::claude_titles(&root),
            root,
            bare,
            files: None,
            recent: Vec::new(),
            list: Vec::new(),
            active: 0,
            next_id: 0,
            limits: None,
            sessions: None,
            agents: Vec::new(),
            preview: None,
            viewing: None,
            worker_scroll: ScrollHandle::new(),
        };
        chats.add(window, cx);
        chats
    }

    /// Opens an empty conversation and shows it.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Chats>) {
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
    fn overall(&self) -> Overall {
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
    session: Option<Box<dyn AgentSession>>,
    /// The agent the chat was opened with, kept for its whole life: changing the agent in
    /// the settings is for new chats, and a transcript belongs to one protocol.
    launcher: zide_claude::Launcher,
    protocol: Protocol,
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
    /// The subagents this chat's Claude launched, each with its own conversation.
    workers: workers::Workers,
    /// The rendered Markdown of the workers' text blocks, by worker and block index.
    worker_md: HashMap<(String, usize), Entity<TextViewState>>,
    _submit: Subscription,
}

impl Chat {
    fn new(id: u64, defaults: &Defaults, window: &mut Window, cx: &mut Context<Chats>) -> Chat {
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
            move |this: &mut Chats, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(chat) = this.find(id) {
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
            launcher: zide_claude::launcher(),
            protocol: crate::settings::protocol(),
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
            workers: workers::Workers::default(),
            worker_md: HashMap::new(),
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

    /// Keeps the Markdown of the last block of worker `id` in step with its text.
    fn sync_worker_markdown(&mut self, id: &str, cx: &mut App) {
        let Some(worker) = self.workers.get(id) else {
            return;
        };
        let last = worker.transcript.blocks().len().saturating_sub(1);
        let Some(Block::Text(text)) = worker.transcript.blocks().get(last) else {
            return;
        };
        match self.worker_md.get(&(id.to_owned(), last)) {
            Some(state) => state.update(cx, |state, cx| state.set_text(text, cx)),
            None => {
                let state = cx.new(|cx| TextViewState::markdown(text, cx));
                self.worker_md.insert((id.to_owned(), last), state);
            }
        }
    }

    /// Forgets the Markdown of the workers that are gone.
    fn prune_worker_markdown(&mut self) {
        let workers = &self.workers;
        self.worker_md
            .retain(|(id, _), _| workers.get(id).is_some());
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

/// What the chats ask of the workspace that shows them.
pub enum ChatEvent {
    /// Leave the chat for the editor (Esc with nothing to stop).
    Hide,
    /// Tell the user something, as an info toast.
    Info(&'static str),
    /// Something failed: an error toast.
    Error {
        title: &'static str,
        message: String,
    },
}

impl EventEmitter<ChatEvent> for Chats {}

impl Render for Chats {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.claude_view(window, cx)
    }
}

/// The chat as a panel of the workspace: when it fills the center, where the focus goes, and
/// the keys the workspace sees first.
impl Workspace {
    /// Alt+2: shows the chat; from the chat it stays there (Esc, or Alt+1 / Ctrl+K, leaves it).
    pub(crate) fn open_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.center == Center::Chat {
            self.focus_active_chat(window, cx);
            return cx.notify();
        }
        self.toggle_claude(window, cx);
    }

    pub(crate) fn toggle_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autosave(cx); // the chat may edit the files, and leaving the editor is a save
        if self.center == Center::Chat {
            self.center = Center::Editor;
            self.focus.focus(window, cx);
            return cx.notify();
        }
        let chats = self.chats(window, cx);
        self.center = Center::Chat;
        chats.update(cx, |chats, cx| chats.refresh_agents(cx));
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// The chats, made the first time they are needed.
    fn chats(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Chats> {
        if let Some(chats) = &self.claude {
            return chats.clone();
        }
        let (root, bare) = (self.root.clone(), self.claude_bare);
        let chats = cx.new(|cx| Chats::new(root, bare, window, cx));
        // the status bar shows how the chats stand, also while the editor is showing
        cx.observe(&chats, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &chats,
            window,
            |this, _, event: &ChatEvent, window, cx| match event {
                ChatEvent::Hide => {
                    if this.center == Center::Chat {
                        this.toggle_claude(window, cx);
                    }
                }
                ChatEvent::Info(note) => this.notify(Notification::info(*note), cx),
                ChatEvent::Error { title, message } => {
                    this.notify(Notification::error(message.clone()).title(*title), cx)
                }
            },
        )
        .detach();
        self.claude = Some(chats.clone());
        self.sync_chat_files(cx);
        self.watch_agents(cx);
        chats
    }

    /// Hands the chats the project's files and the recent ones, after either changed.
    pub(crate) fn sync_chat_files(&self, cx: &mut App) {
        if let Some(chats) = &self.claude {
            let (files, recent) = (self.index.clone(), self.recent.clone());
            chats.update(cx, |chats, _| (chats.files, chats.recent) = (files, recent));
        }
    }

    /// Claude: Bare Mode, for the processes started from now on.
    pub(crate) fn toggle_claude_bare(&mut self, cx: &mut Context<Self>) {
        self.claude_bare.toggle();
        if let Some(chats) = &self.claude {
            let bare = self.claude_bare;
            chats.update(cx, |chats, _| chats.bare = bare);
        }
        cx.notify();
    }

    /// Claude: Confirm Before Stopping, saved in the settings.
    pub(crate) fn toggle_confirm_stop(&mut self, cx: &mut Context<Self>) {
        if let Err(why) = crate::settings::toggle_confirm_stop() {
            let note = Notification::error(why).title("Could not save the setting");
            self.notify(note, cx);
        }
        cx.notify();
    }

    /// Keeps the list of agents current while the panel is showing: `claude` is asked in the
    /// background, every few seconds.
    fn watch_agents(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGENTS_EVERY).await;
                let shown = this.update(cx, |this, cx| {
                    if this.center == Center::Chat
                        && let Some(chats) = &this.claude
                    {
                        chats.update(cx, |chats, cx| chats.refresh_agents(cx));
                    }
                });
                if shown.is_err() {
                    break; // the workspace is gone
                }
            }
        })
        .detach();
    }

    /// Opens an empty conversation next to the others.
    pub(crate) fn claude_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.center != Center::Chat {
            self.toggle_claude(window, cx);
        }
        if let Some(chats) = &self.claude {
            chats.update(cx, |chats, cx| chats.new_chat(window, cx));
        }
    }

    /// Opens the list of earlier conversations over the chat (showing the chat first).
    pub(crate) fn claude_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // the earlier conversations are Claude Code's; they would resume through the ACP agent
        if crate::settings::protocol() == Protocol::Acp {
            return self.notify(Notification::info(CLAUDE_HISTORY_ONLY), cx);
        }
        if self.center != Center::Chat {
            self.toggle_claude(window, cx);
        }
        if let Some(chats) = &self.claude {
            chats.update(cx, |chats, cx| chats.claude_history(window, cx));
        }
    }

    /// The keys while the chat is showing, before the input box's own bindings see them.
    pub(crate) fn claude_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(chats) = &self.claude {
            chats.update(cx, |chats, cx| chats.claude_key(keystroke, window, cx));
        }
    }

    /// Puts the cursor in the active chat's input box.
    pub(crate) fn focus_active_chat(&self, window: &mut Window, cx: &mut App) {
        if let Some(chats) = &self.claude {
            chats.update(cx, |chats, cx| chats.focus_active_chat(window, cx));
        }
    }

    /// The session id and title of the conversation the active chat is on, if it is on one.
    pub(crate) fn claude_conversation(&self, cx: &App) -> Option<(String, String)> {
        self.claude.as_ref()?.read(cx).conversation()
    }

    /// Puts the active chat back on a conversation from an earlier run.
    pub(crate) fn restore_conversation(
        &mut self,
        id: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(chats) = &self.claude {
            chats.update(cx, |chats, cx| chats.restore_conversation(id, title, cx));
        }
    }

    /// How the chats stand together, for the status bar.
    pub(crate) fn claude_overall(&self, cx: &App) -> Overall {
        self.claude
            .as_ref()
            .map_or(Overall::Off, |chats| chats.read(cx).overall())
    }
}

/// Why Claude's earlier conversations cannot be resumed with another agent.
const CLAUDE_HISTORY_ONLY: &str =
    "Earlier conversations are Claude's: pick a Claude agent in Configure Agent to resume one";

impl Chats {
    /// Asks `claude agents` now; the list changes only when the answer does. The chats that
    /// follow another process re-read their session too.
    fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        // background agents are Claude Code's: an ACP agent's command does not know `agents`
        if crate::settings::protocol() == Protocol::Acp {
            return;
        }
        let root = self.root.clone();
        let followed: Vec<(u64, String, Option<std::time::SystemTime>)> = self
            .list
            .iter()
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
        let known: HashSet<String> = self.titles.keys().cloned().collect();
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
                // titles of agents that are gone are forgotten, so the cache stays small
                let before = this.titles.clone();
                this.titles.extend(found);
                this.titles
                    .retain(|id, _| agents.iter().any(|agent| &agent.session_id == id));
                if this.titles != before {
                    crate::session::remember_claude_titles(&this.root, &this.titles);
                    cx.notify();
                }
                if this.agents != agents {
                    this.agents = agents;
                    cx.notify();
                }
                for (id, modified, loaded) in stored {
                    let Some(chat) = this.find(id) else {
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
    fn conversation(&self) -> Option<(String, String)> {
        let chat = self.active();
        // only Claude's conversations can be resumed; an ACP session id would reach `--resume`
        if chat.protocol != Protocol::Claude {
            return None;
        }
        let id = chat.transcript.session_id()?.to_owned();
        Some((id, chat.title.clone().unwrap_or_default()))
    }

    /// Puts the active chat back on a conversation from an earlier run. It follows the agent that
    /// may still be alive: its state comes from `claude agents` until a message starts a process.
    fn restore_conversation(&mut self, id: String, title: String, cx: &mut Context<Self>) {
        // the saved conversation is Claude's: an ACP chat would start a fresh session instead
        if self.active().protocol != Protocol::Claude {
            return;
        }
        let chat_id = self.active().id;
        self.active_mut().tail = true;
        let session = SessionInfo {
            id,
            title,
            modified: std::time::UNIX_EPOCH,
        };
        let untitled = session.title.is_empty();
        self.load_session(chat_id, session, cx);
        if untitled && let Some(chat) = self.find(chat_id) {
            chat.title = None; // the first message names it, as in any new chat
        }
    }

    /// Shows what an agent is doing: the chat that already follows its session, else a new one
    /// that reads the session from disk and keeps reading it. Sending a message there continues
    /// the same session with `--resume`.
    fn claude_open_agent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agents.get(index).cloned() else {
            return;
        };
        if let Some(at) = self
            .list
            .iter()
            .position(|chat| chat.transcript.session_id() == Some(agent.session_id.as_str()))
        {
            return self.claude_activate(at, window, cx);
        }
        self.add(window, cx);
        let id = self.active().id;
        self.active_mut().tail = true;
        let title = self.name_of(&agent);
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
        let Some(agent) = self.agents.get(index).cloned() else {
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
                        this.agents.retain(|a| a.id != agent.id);
                        let open = this
                            .list
                            .iter()
                            .find(|chat| {
                                chat.transcript.session_id() == Some(agent.session_id.as_str())
                            })
                            .map(|chat| chat.id);
                        if let Some(id) = open {
                            this.claude_close(id, window, cx);
                        }
                    }
                    Err(why) => cx.emit(ChatEvent::Error {
                        title: "Could not remove the agent",
                        message: why.to_string(),
                    }),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn focus_active_chat(&self, window: &mut Window, cx: &mut App) {
        let chat = self.active();
        chat.input.update(cx, |state, cx| state.focus(window, cx));
        chat.scroll.scroll_to_bottom();
    }

    /// Keys while the chat is showing. The input box handles its own typing.
    fn claude_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let key = keystroke.key.as_str();
        let modifiers = &keystroke.modifiers;
        if key == "escape" && self.preview.is_some() {
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
                    if self.sessions.is_some() {
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
        if let Some(sessions) = self.sessions.as_mut() {
            match key {
                "down" => sessions.move_selection(1, cx),
                "up" => sessions.move_selection(-1, cx),
                "escape" => self.close_claude_history(window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return cx.notify();
        }
        // the worker view has no input: Esc goes back to the chat
        if self.viewing.is_some() {
            if key == "escape" {
                self.claude_leave_worker(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        // Tab outside any text box puts the cursor in the chat, like Alt+I
        if key == "tab"
            && !modifiers.modified()
            && !window
                .context_stack()
                .iter()
                .any(|context| context.contains("Input"))
        {
            self.focus_active_chat(window, cx);
            cx.stop_propagation();
            return cx.notify();
        }
        if self.menu_matches(cx).is_some() {
            match key {
                "down" => self.menu_move(1, cx),
                "up" => self.menu_move(-1, cx),
                "tab" => {
                    let id = self.active().id;
                    self.claude_complete(id, window, cx);
                }
                "escape" => self.active_mut().menu_dismissed = true,
                _ => return,
            }
            cx.stop_propagation();
            return cx.notify();
        }
        if key != "escape" {
            return;
        }
        // Esc presses Stop while there is something to stop (asking first, unless that is
        // turned off); otherwise it leaves the panel
        if self.active().busy() {
            if crate::settings::confirm_stop() {
                self.confirm_interrupt(window, cx);
            } else {
                self.claude_interrupt();
            }
        } else {
            cx.emit(ChatEvent::Hide);
        }
        cx.stop_propagation();
    }

    /// Esc while Claude works: a slip of the key would throw the turn away, so it is asked first.
    fn confirm_interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chats = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (stop, keep) = (chats.clone(), chats.clone());
            alert
                .title("Stop Claude")
                .description("Stop the turn Claude is working on?")
                .ok_text("Yes")
                .cancel_text("No")
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    stop.update(cx, |chats, cx| {
                        chats.claude_interrupt();
                        chats.focus_active_chat(window, cx);
                    });
                    true
                })
                .on_cancel(move |_, window, cx| {
                    keep.update(cx, |chats, cx| chats.focus_active_chat(window, cx));
                    true
                })
        });
    }

    /// Stops the turn in progress of the active chat. A message that waits starts right after.
    pub(super) fn claude_interrupt(&mut self) {
        if let Some(session) = self.active_mut().session.as_mut() {
            let _ = session.interrupt();
        }
    }

    fn claude_send(&mut self, id: u64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        let Some(chat) = self.find(id) else {
            return;
        };
        // while Claude works the message is queued by the CLI; a pending question comes first
        if (text.is_empty() && chat.images.is_empty()) || chat.status() == Status::NeedsUser {
            return;
        }
        if chat.session.is_none()
            && let Err(error) = self.start_claude(id, cx)
        {
            if let Some(chat) = self.find(id) {
                chat.notice = Some(format!("Could not start `claude`: {error}"));
            }
            return cx.notify();
        }
        let Some(chat) = self.find(id) else {
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
        let (bare, root) = (self.bare, self.root.clone());
        let Some(chat) = self.find(id) else {
            return Ok(());
        };
        let (session, events): (Box<dyn AgentSession>, _) = match chat.protocol {
            Protocol::Claude => {
                let resume = chat.transcript.session_id().map(str::to_owned);
                let options = Options {
                    launcher: Some(&chat.launcher),
                    resume: resume.as_deref(),
                    model: chat.model.as_deref(),
                    effort: chat.effort.as_deref(),
                    minimal: bare,
                    ..Options::default()
                };
                let (session, events) = Session::spawn(&root, options)?;
                (Box::new(session), events)
            }
            // a new session each time: resuming one is for a later version (`session/load`)
            Protocol::Acp => {
                let (session, events) = zide_acp::spawn(&chat.launcher, &root)?;
                (Box::new(session), events)
            }
        };
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
            && self.commands != *commands
        {
            self.commands = commands.clone();
            crate::session::remember_claude_commands(&self.root, commands);
        }
        if let Event::Limits(limits) = &event {
            self.limits = Some(*limits);
        }
        let viewed = self
            .viewing
            .as_ref()
            .filter(|(chat, _)| *chat == id)
            .map(|(_, worker)| worker.clone());
        let follow_worker =
            self.worker_scroll.max_offset().y + self.worker_scroll.offset().y < px(8.);
        let worker_scroll = self.worker_scroll.clone();
        let Some(chat) = self.find(id).filter(|chat| chat.run == run) else {
            return;
        };
        // a worker's events are its own: the chat's conversation never sees them
        if event.is_worker() {
            if let Some(grew) = chat.workers.apply(event, viewed.as_deref()) {
                chat.sync_worker_markdown(&grew, cx);
                if follow_worker && viewed.as_deref() == Some(grew.as_str()) {
                    worker_scroll.scroll_to_bottom();
                }
            }
            chat.prune_worker_markdown();
            return cx.notify();
        }
        let follow = chat.at_bottom();
        if matches!(event, Event::Exit) {
            chat.session = None;
            chat.workers.clear();
            chat.prune_worker_markdown();
            if viewed.is_some() {
                self.viewing = None;
            }
        }
        let Some(chat) = self.find(id) else {
            return;
        };

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
    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.claude_leave_worker(window, cx);
        self.add(window, cx);
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Shows the next (or previous) chat, wrapping around at the ends.
    fn claude_cycle(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.list.len() as isize;
        let index = (self.active as isize + step).rem_euclid(count) as usize;
        self.claude_activate(index, window, cx);
    }

    fn claude_activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.claude_leave_worker(window, cx);
        if index < self.list.len() {
            self.active = index;
        }
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Shows what worker `tool_use_id` of chat `chat_id` does, in place of the chat. Its chat
    /// becomes the active one, so leaving the view goes back to it.
    fn claude_open_worker(
        &mut self,
        chat_id: u64,
        tool_use_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.claude_leave_worker(window, cx);
        let Some(index) = self.list.iter().position(|chat| chat.id == chat_id) else {
            return;
        };
        self.active = index;
        self.viewing = Some((chat_id, tool_use_id));
        self.worker_scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Closes the worker view, if it is open, and goes back to the chat. A worker that
    /// finished while it was watched leaves the list now. Says whether a view was open.
    fn claude_leave_worker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((chat_id, tool_use_id)) = self.viewing.take() else {
            return false;
        };
        if let Some(chat) = self.find(chat_id) {
            chat.workers.left(&tool_use_id);
            chat.prune_worker_markdown();
        }
        self.focus_active_chat(window, cx);
        cx.notify();
        true
    }

    /// Closes a chat; its process, if any, stops. The last one is replaced by an empty chat.
    fn claude_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.list.iter().position(|chat| chat.id == id) else {
            return;
        };
        self.list.remove(index);
        // its workers went with it
        if self.viewing.as_ref().is_some_and(|(chat, _)| *chat == id) {
            self.viewing = None;
        }
        if self.list.is_empty() {
            self.add(window, cx);
        } else if index < self.active || self.active >= self.list.len() {
            self.active = self.active.saturating_sub(1);
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
        // a new chat takes the agent in use, and Claude's history needs a Claude chat
        if crate::settings::protocol() != Protocol::Claude {
            return cx.emit(ChatEvent::Info(CLAUDE_HISTORY_ONLY));
        }
        if let Some(index) = self
            .list
            .iter()
            .position(|chat| chat.transcript.session_id() == Some(session.id.as_str()))
        {
            return self.claude_activate(index, window, cx);
        }
        // an empty ACP chat would send the resumed conversation's next message to its agent;
        // a new chat is Claude's, as `claude_resume` refuses while an ACP agent is in use
        if !self.active().is_empty() || self.active().protocol != Protocol::Claude {
            self.add(window, cx);
        }
        let id = self.active().id;
        self.load_session(id, session, cx);
        self.focus_active_chat(window, cx);
        cx.notify();
    }

    /// Fills chat `id` with a stored conversation, read in the background. The chat knows the
    /// session at once, so what is sent next continues it even while it loads.
    fn load_session(&mut self, id: u64, session: SessionInfo, cx: &mut Context<Self>) {
        let Some(chat) = self.find(id) else {
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
                let Some(chat) = this.find(id) else {
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
        let Some(chat) = self.find(id) else {
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
        let worker_rows = chats
            .list
            .iter()
            .flat_map(|chat| {
                chat.workers
                    .list()
                    .iter()
                    .map(move |worker| (chat.id, worker))
            })
            .map(|(chat_id, worker)| {
                let viewed = chats
                    .viewing
                    .as_ref()
                    .is_some_and(|(chat, tool)| *chat == chat_id && *tool == worker.tool_use_id);
                let color = if worker.finished {
                    theme::DIM
                } else {
                    theme::WORKING
                };
                let detail = match &worker.activity {
                    Some(activity) => format!("{} · {activity}", worker.kind),
                    None => worker.kind.clone(),
                };
                let tool_use_id = worker.tool_use_id.clone();
                div()
                    .id(SharedString::from(format!(
                        "worker-row-{}",
                        worker.tool_use_id
                    )))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .bg(rgb(if viewed {
                        theme::TREE_SELECTED
                    } else {
                        theme::BACKGROUND
                    }))
                    .hover(|row| row.bg(rgb(theme::TREE_SELECTED)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.claude_open_worker(chat_id, tool_use_id.clone(), window, cx)
                    }))
                    .child(div().flex_none().size_2().rounded_full().bg(rgb(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_color(rgb(theme::STRONG))
                                    .child(worker.description.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(theme::MUTED))
                                    .child(detail),
                            ),
                    )
            })
            .collect::<Vec<_>>();
        let worker_section = (!worker_rows.is_empty()).then(|| {
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
                        .child(format!("Workers ({})", worker_rows.len())),
                )
                .children(worker_rows)
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
                        Button::new("claude-new")
                            .ghost()
                            .label("New")
                            .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
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
                    .children(background_section)
                    .children(worker_section),
            )
    }

    /// What a worker does, read-only, in place of the chat: its blocks drawn as the chat
    /// draws its own, under a header that says what it is and a way back.
    fn worker_view(&self, chat: &Chat, worker: &workers::Worker, cx: &mut Context<Self>) -> Div {
        let blocks = worker
            .transcript
            .blocks()
            .iter()
            .enumerate()
            .filter_map(|(index, block)| match block {
                Block::Text(text) => Some(text_view(
                    chat.worker_md.get(&(worker.tool_use_id.clone(), index)),
                    text,
                )),
                Block::Thinking(text) => Some(thinking_view(text)),
                Block::Tool(call) => Some(self.tool_card(chat, call, cx).into_any_element()),
                // a worker asks nothing of the user: the CLI decides for it
                _ => None,
            })
            .collect::<Vec<_>>();
        let (color, state) = if worker.finished {
            (theme::DIM, "Finished")
        } else {
            (theme::WORKING, "Working")
        };
        div()
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
                    .child(Button::new("worker-back").ghost().label("← Back").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.claude_leave_worker(window, cx);
                        }),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(theme::STRONG))
                            .child(worker.description.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(theme::MUTED))
                            .child(worker.kind.clone()),
                    )
                    .child(div().text_sm().text_color(rgb(color)).child(state)),
            )
            .child(
                div()
                    .id("worker-history")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.worker_scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_4()
                            .max_w(px(900.))
                            .mx_auto()
                            .children(blocks)
                            .children(worker.activity.iter().filter(|_| !worker.finished).map(
                                |activity| {
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme::WORKING))
                                        .child(activity.clone())
                                },
                            )),
                    ),
            )
    }

    /// The chat, filling the area where the editor would be.
    fn claude_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_asks(window, cx);
        self.ensure_diffs();
        self.sync_options(window, cx);
        let popup = self.sessions_view(window, cx);
        let chat = self.active();
        let worker = self
            .viewing
            .as_ref()
            .filter(|(id, _)| *id == chat.id)
            .and_then(|(_, tool)| chat.workers.get(tool));
        if let Some(worker) = worker {
            let view = self.worker_view(chat, worker, cx);
            let panel = self.claude_panel(self, cx);
            return div()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .text_color(rgb(theme::TEXT))
                .child(panel)
                .child(
                    view.relative()
                        .children(popup)
                        .children(self.preview_view(cx)),
                );
        }
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
        let panel = self.claude_panel(self, cx);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let background_count = self
            .agents
            .iter()
            .filter(|agent| agent.kind == Kind::Background)
            .count();
        let shown = self.status_of(chat);
        let ended = shown == Status::Inactive && !chat.is_empty();
        // a running process keeps the mode it started with; any other starts with the toggle's
        let bare = if chat.session.is_some() {
            chat.bare
        } else {
            self.bare
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
                                        this.active_mut().shown += SHOWN_STEP;
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
                        self,
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
            Block::Text(text) => text_view(chat.markdown.get(&index), text),
            Block::Thinking(text) => thinking_view(text),
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

/// An assistant text block: its Markdown once made, the plain text until then.
fn text_view(markdown: Option<&Entity<TextViewState>>, text: &str) -> AnyElement {
    match markdown {
        Some(state) => div()
            .child(
                TextView::new(state)
                    .inline_code_style(crate::markdown::inline_code())
                    .selectable(true),
            )
            .into_any_element(),
        None => div().child(text.to_owned()).into_any_element(),
    }
}

fn thinking_view(text: &str) -> AnyElement {
    div()
        .text_sm()
        .italic()
        .text_color(rgb(theme::DIM))
        .child(text.to_owned())
        .into_any_element()
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
