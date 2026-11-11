//! The Git log tab (Alt+3): the refs on the left (HEAD, Local, Remote), the commits in the middle
//! with their graph, and on the right the files of the selected commit and its details. The box
//! on top searches the message, the author or the start of a hash; a ref picked in the tree
//! narrows the history to it, and picking it again goes back to every branch.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::calendar::Date;
use gpui_kit::component::date_picker::{DatePicker, DatePickerEvent, DatePickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::log::{self, Author, GraphRow, LogEntry, LogQuery, RefKind, RefLabel, Scope};
use zide_git::{Branches, FileChange, Switch};

mod menu;

use menu::{ContextMenu, MenuAction};

use crate::branches::{Kind, Model, Row};
use crate::changed_files::{self, Picks};
use crate::{Center, Workspace, file_icon, statusbar, theme};

const ROW_HEIGHT: f32 = 24.;
const LANE_WIDTH: f32 = 14.;
/// Lanes drawn at most; the ones beyond share the last column.
const MAX_LANES: usize = 10;
/// Commits read at first, and added each time the list reaches its end.
const PAGE: usize = 600;
/// How long a pause in typing waits before the search runs.
const TYPING_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
/// The sections of the refs tree.
const SECTIONS: [(&str, &str); 3] = [("local", "Local"), ("remote", "Remote"), ("tags", "Tags")];
const LANE_COLORS: [u32; 8] = [
    0x61afef, 0xe5c07b, 0x98c379, 0xc678dd, 0xe06c75, 0x56b6c2, 0xd19a66, 0xabb2bf,
];

/// The filters that open a popup under their button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    User,
    /// The list of ways to narrow by date.
    Date,
    /// The popup with the After and Before pickers, opened by the last row of the date list.
    Range,
}

/// The dates the list offers besides the range, with how git reads each.
const PRESETS: [(&str, &str); 3] = [
    ("Last 24 hours", "24 hours ago"),
    ("Last 7 days", "7 days ago"),
    ("Last 30 days", "30 days ago"),
];

/// What a row of the date list asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatePick {
    Any,
    /// The preset, counted from 1.
    Preset(usize),
    /// Choose the days with the pickers.
    Select,
}

fn date_items() -> Vec<String> {
    std::iter::once("Any time".to_string())
        .chain(PRESETS.iter().map(|(label, _)| label.to_string()))
        .chain(std::iter::once("Select...".to_string()))
        .collect()
}

fn date_pick(row: usize) -> DatePick {
    match row {
        0 => DatePick::Any,
        row if row <= PRESETS.len() => DatePick::Preset(row),
        _ => DatePick::Select,
    }
}

/// What the keyboard moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    Refs,
    #[default]
    Commits,
    Files,
}

enum Entries {
    Loading,
    Failed(String),
    Ready {
        list: Arc<Vec<LogEntry>>,
        graph: Arc<Vec<GraphRow>>,
        /// There may be older commits than the ones read.
        more: bool,
    },
}

enum Files {
    Loading,
    Failed(String),
    /// Sorted by folder, then path.
    Ready(Vec<FileChange>),
}

pub struct LogPanel {
    /// The repository.
    root: PathBuf,
    /// The workspace's: the keys of the lists are read by the workspace, which forwards them.
    focus: FocusHandle,
    query: Entity<InputState>,
    /// The paths the commits must touch, separated by spaces.
    paths: Entity<InputState>,
    _typing: Vec<Subscription>,
    /// Everyone who made commits, for the user filter.
    authors: Vec<Author>,
    /// The email of the user the history is narrowed to; empty: everyone.
    user: String,
    /// The pickers of the date filter, and the days they hold (`2026-11-08`; empty: none).
    after: Entity<DatePickerState>,
    before: Entity<DatePickerState>,
    after_day: String,
    before_day: String,
    /// The preset of the date list in use, counted from 1; 0 when there is none.
    preset: usize,
    /// The popup of a filter, when open, with the row the keyboard is on (the user list; the
    /// date popup is for the mouse).
    menu: Option<(Filter, usize)>,
    /// The context menu of a commit, of its files or of a ref, when open.
    context_menu: Option<menu::ContextMenu>,
    /// The remote the current branch pushes to, for the menu of a ref.
    remote: Option<String>,
    /// The tags a remote had at the last fetch: its name and the tags.
    remote_tags: Option<(String, HashSet<String>)>,
    /// Counts the pauses in typing: a search waits for its own to end.
    typed: u64,
    refs: Option<Model>,
    /// The cursor is on the HEAD line, above the tree.
    head_cursor: bool,
    scope: Scope,
    entries: Entries,
    /// Commits asked for; grows when the list reaches its end.
    limit: usize,
    /// Counts the reads of the list: an older answer is dropped.
    generation: u64,
    loading: bool,
    selected: usize,
    pub pane: Pane,
    files: Files,
    /// The files picked, and the one the cursor is on.
    picks: Picks,
    /// The branches that hold the selected commit, once read.
    in_branches: Option<Vec<String>>,
    commit_scroll: UniformListScrollHandle,
    file_scroll: UniformListScrollHandle,
    ref_scroll: UniformListScrollHandle,
}

/// The ref that narrows the history, as the tree's row key (`local/main`).
fn scope_key(scope: &Scope) -> Option<String> {
    let Scope::Ref(name) = scope else {
        return None;
    };
    name.strip_prefix("refs/heads/")
        .map(|n| format!("local/{n}"))
        .or_else(|| {
            name.strip_prefix("refs/remotes/")
                .map(|n| format!("remote/{n}"))
        })
}

/// What picking `target` in the tree asks for: that ref, or every branch when it already is the
/// one.
/// The rows of the user filter: everyone, then each author with how many commits they made.
fn user_items(authors: &[Author]) -> Vec<String> {
    std::iter::once("All users".to_string())
        .chain(
            authors
                .iter()
                .map(|a| format!("{} <{}>  ({})", a.name, a.email, a.commits)),
        )
        .collect()
}

/// The email the user filter takes for `row` of [`user_items`]; empty for everyone.
fn user_at(authors: &[Author], row: usize) -> String {
    row.checked_sub(1)
        .and_then(|at| authors.get(at))
        .map(|a| a.email.clone())
        .unwrap_or_default()
}

/// The limits git is given for the days picked: from the start of the first to the end of the
/// last.
fn day_limits(after: &str, before: &str) -> (String, String) {
    let at = |day: &str, time: &str| {
        if day.is_empty() {
            String::new()
        } else {
            format!("{day} {time}")
        }
    };
    (at(after, "00:00:00"), at(before, "23:59:59"))
}

fn scope_after_pick(current: &Scope, target: &Switch) -> Scope {
    let wanted = match target {
        Switch::Local(name) => Scope::Ref(format!("refs/heads/{name}")),
        Switch::Remote(full) => Scope::Ref(format!("refs/remotes/{full}")),
        Switch::Tag(name) => Scope::Ref(format!("refs/tags/{name}")),
    };
    if *current == wanted {
        Scope::All
    } else {
        wanted
    }
}

impl LogPanel {
    fn new(root: PathBuf, focus: FocusHandle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Text or hash"));
        let paths = cx.new(|cx| InputState::new(window, cx).placeholder("Paths"));
        let picker = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| DatePickerState::new(window, cx).date_format("%Y-%m-%d"))
        };
        let [after, before] = [picker(window, cx), picker(window, cx)];
        let mut typing: Vec<Subscription> = [&query, &paths]
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.log_typed(cx);
                    }
                })
            })
            .into();
        for (which, picker) in [&after, &before].into_iter().enumerate() {
            typing.push(
                cx.subscribe(picker, move |this, _, event: &DatePickerEvent, cx| {
                    let DatePickerEvent::Change(value) = event;
                    let day = value.format("%Y-%m-%d").map(|d| d.to_string());
                    this.log_day(which == 0, day.unwrap_or_default(), cx);
                }),
            );
        }
        Self {
            root,
            focus,
            query,
            paths,
            _typing: typing,
            authors: Vec::new(),
            user: String::new(),
            after,
            before,
            after_day: String::new(),
            before_day: String::new(),
            preset: 0,
            context_menu: None,
            remote: None,
            remote_tags: None,
            menu: None,
            typed: 0,
            refs: None,
            head_cursor: false,
            scope: Scope::All,
            entries: Entries::Loading,
            limit: PAGE,
            generation: 0,
            loading: false,
            selected: 0,
            pane: Pane::default(),
            files: Files::Loading,
            picks: Picks::default(),
            in_branches: None,
            commit_scroll: UniformListScrollHandle::new(),
            file_scroll: UniformListScrollHandle::new(),
            ref_scroll: UniformListScrollHandle::new(),
        }
    }

    /// What the search box, the filters and the scope ask for.
    fn log_query(&self, cx: &App) -> LogQuery {
        let (mut since, until) = day_limits(&self.after_day, &self.before_day);
        if let Some((_, git)) = self.preset.checked_sub(1).and_then(|at| PRESETS.get(at)) {
            since = git.to_string();
        }
        LogQuery {
            scope: self.scope.clone(),
            text: self.query.read(cx).value().trim().to_string(),
            user: self.user.clone(),
            since,
            until,
            paths: self
                .paths
                .read(cx)
                .value()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        }
    }

    fn menu_items(&self, filter: Filter) -> Vec<String> {
        match filter {
            Filter::User => user_items(&self.authors),
            Filter::Date => date_items(),
            Filter::Range => Vec::new(),
        }
    }

    /// Opens the list of `filter` on its current value, or closes it.
    fn toggle_menu(&mut self, filter: Filter) {
        if self.menu.is_some_and(|(open, _)| open == filter) {
            self.menu = None;
            return;
        }
        let row = match filter {
            Filter::User => self
                .authors
                .iter()
                .position(|a| a.email == self.user)
                .map_or(0, |at| at + 1),
            Filter::Date if self.preset > 0 => self.preset,
            Filter::Date if !self.after_day.is_empty() || !self.before_day.is_empty() => {
                PRESETS.len() + 1
            }
            Filter::Date | Filter::Range => 0,
        };
        self.menu = Some((filter, row));
    }

    /// Takes row `row` of the list of `filter` and closes it.
    fn pick_menu(&mut self, filter: Filter, row: usize) {
        self.menu = None;
        match filter {
            Filter::User => self.user = user_at(&self.authors, row),
            Filter::Date | Filter::Range => {}
        }
    }

    fn list(&self) -> &[LogEntry] {
        match &self.entries {
            Entries::Ready { list, .. } => list,
            _ => &[],
        }
    }

    fn selected_entry(&self) -> Option<&LogEntry> {
        self.list().get(self.selected)
    }

    /// Selects commit `index` (kept inside the list); says whether it changed.
    fn select(&mut self, index: usize) -> bool {
        let last = self.list().len().saturating_sub(1);
        let index = index.min(last);
        let moved = index != self.selected;
        self.selected = index;
        moved
    }

    /// Takes the commits read, keeping the selected one when it is still listed.
    fn set_entries(
        &mut self,
        generation: u64,
        found: Result<Vec<LogEntry>, String>,
        filtered: bool,
    ) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        let kept = self.selected_entry().map(|e| e.commit.hash.clone());
        self.entries = match found {
            Err(why) => Entries::Failed(why),
            Ok(list) => {
                let graph = if filtered {
                    // matches are not a connected history: only their dots
                    vec![GraphRow::default(); list.len()]
                } else {
                    log::graph(&list)
                };
                let more = list.len() >= self.limit;
                self.selected = kept
                    .and_then(|hash| list.iter().position(|e| e.commit.hash == hash))
                    .unwrap_or(0);
                Entries::Ready {
                    list: Arc::new(list),
                    graph: Arc::new(graph),
                    more,
                }
            }
        };
    }

    fn set_refs(&mut self, found: Branches) {
        match &mut self.refs {
            Some(model) => model.replace(found),
            None => self.refs = Some(Model::with_sections(found, &SECTIONS)),
        }
    }

    /// Marks the files of the selected commit as being read; its hash, to read them.
    fn start_files_load(&mut self) -> Option<String> {
        let hash = self.selected_entry()?.commit.hash.clone();
        self.files = Files::Loading;
        self.in_branches = None;
        self.picks = Picks::default();
        Some(hash)
    }

    /// Takes the files read for `hash`, dropped if another commit was selected meanwhile.
    fn set_files(&mut self, hash: &str, found: Result<Vec<FileChange>, String>) {
        if self.selected_entry().map(|e| e.commit.hash.as_str()) != Some(hash) {
            return;
        }
        self.picks = Picks::default();
        self.files = match found {
            Ok(mut list) => {
                changed_files::sort(&mut list);
                Files::Ready(list)
            }
            Err(why) => Files::Failed(why),
        };
    }

    fn files(&self) -> &[FileChange] {
        match &self.files {
            Files::Ready(list) => list,
            _ => &[],
        }
    }

    fn picked_files(&self) -> Arc<HashSet<usize>> {
        Arc::new(self.picks.picked())
    }

    fn selected_files(&self) -> Vec<FileChange> {
        let mut picked: Vec<usize> = self.picks.picked().into_iter().collect();
        picked.sort_unstable();
        picked
            .into_iter()
            .filter_map(|at| self.files().get(at).cloned())
            .collect()
    }

    /// A click on file `index`, as [`Picks::click`] reads it.
    fn click_file(&mut self, index: usize, ctrl: bool, shift: bool) {
        self.pane = Pane::Files;
        self.picks.click(index, ctrl, shift);
    }

    fn selected_file(&self) -> Option<&FileChange> {
        self.files().get(self.picks.cursor)
    }

    fn scope_key(&self) -> Option<String> {
        scope_key(&self.scope)
    }

    fn scope_for(&self, target: &Switch) -> Scope {
        scope_after_pick(&self.scope, target)
    }

    /// A short name for what the history is narrowed to, `None` for every branch.
    fn scope_label(&self) -> Option<String> {
        match &self.scope {
            Scope::All => None,
            Scope::Head => Some("HEAD".into()),
            Scope::Ref(name) => Some(
                name.strip_prefix("refs/heads/")
                    .or_else(|| name.strip_prefix("refs/remotes/"))
                    .unwrap_or(name)
                    .to_string(),
            ),
        }
    }

    /// The cursor of the tree is on a section or folder that is closed.
    fn closed_ref_row(&self) -> bool {
        let Some(model) = &self.refs else {
            return false;
        };
        model.rows().get(model.selected()).is_some_and(|row| {
            matches!(
                row.kind,
                Kind::Section { expanded: false } | Kind::Folder { expanded: false }
            )
        })
    }

    /// Moves the cursor of the tree; `true` when it moved.
    fn move_ref(&mut self, delta: isize) -> bool {
        let Some(model) = &mut self.refs else {
            return false;
        };
        if self.head_cursor {
            if delta > 0 {
                self.head_cursor = false;
                model.select(0);
            }
            return true;
        }
        if delta < 0 && model.selected() == 0 {
            self.head_cursor = true;
            return true;
        }
        model.move_by(delta);
        true
    }
}

/// What the log asks of the workspace that shows it.
pub enum LogEvent {
    /// Leave the tab for the editor (Esc).
    Hide,
    /// Run `action` of the context menu `menu`, which is closed now.
    Menu {
        action: MenuAction,
        menu: ContextMenu,
    },
    /// Show the diff of `file` in commit `hash`.
    ShowDiff { hash: String, file: FileChange },
    /// Open the menu of the branch or tag on row `row` of the tree at `at`: the workspace knows
    /// the current branch it would be merged into.
    RefMenu { row: usize, at: Point<Pixels> },
}

impl EventEmitter<LogEvent> for LogPanel {}

impl Render for LogPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.log_view(cx)
    }
}

/// The git log as a tab of the workspace: when it fills the center, and the keys the workspace
/// reads for it.
impl Workspace {
    /// Alt+3: shows the tab; from the tab it stays there (Esc, or Alt+1 / Ctrl+K, leaves it).
    pub(crate) fn open_git_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.center == Center::Log {
            self.focus.focus(window, cx);
            return cx.notify();
        }
        self.toggle_git_log(window, cx);
    }

    /// Leaves the chat or the git log, whichever covers the editor; `false` when none does.
    pub(crate) fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.center == Center::Chat {
            self.toggle_claude(window, cx);
        } else if self.center == Center::Log {
            self.toggle_git_log(window, cx);
        } else {
            return false;
        }
        true
    }

    pub(crate) fn toggle_git_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.center == Center::Log {
            self.center = Center::Editor;
            self.focus.focus(window, cx);
            return cx.notify();
        }
        let log = self.log_panel(window, cx);
        self.center = Center::Log; // the tab takes the place of the chat, as the chat does the editor
        self.focus.focus(window, cx); // the keys are read by the workspace
        log.update(cx, |log, cx| log.reload(cx));
        cx.notify();
    }

    /// The log, made the first time it is opened.
    fn log_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<LogPanel> {
        if let Some(log) = &self.log {
            return log.clone();
        }
        let (root, focus) = (self.root.clone(), self.focus.clone());
        let log = cx.new(|cx| LogPanel::new(root, focus, window, cx));
        cx.subscribe_in(
            &log,
            window,
            |this, _, event: &LogEvent, window, cx| match event {
                LogEvent::Hide => {
                    if this.center == Center::Log {
                        this.toggle_git_log(window, cx);
                    }
                }
                LogEvent::Menu { action, menu } => {
                    this.run_menu_action(*action, menu.clone(), window, cx)
                }
                LogEvent::RefMenu { row, at } => {
                    let into = this
                        .branch
                        .as_ref()
                        .map(|b| b.name.clone())
                        .unwrap_or_default();
                    let (row, at) = (*row, *at);
                    if let Some(log) = &this.log {
                        log.update(cx, |log, cx| log.open_ref_menu(row, at, into, cx));
                    }
                }
                LogEvent::ShowDiff { hash, file } => {
                    this.show_commit_diff(hash, file.clone(), window, cx)
                }
            },
        )
        .detach();
        self.log = Some(log.clone());
        log
    }

    /// Reads the history again after git changed it, when the tab is showing.
    pub(crate) fn reload_log(&mut self, cx: &mut Context<Self>) {
        if self.center == Center::Log
            && let Some(log) = &self.log
        {
            log.update(cx, |log, cx| log.reload(cx));
        }
    }

    /// Tab while the tab is showing.
    pub(crate) fn log_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(log) = &self.log {
            log.update(cx, |log, cx| log.log_tab(window, cx));
        }
    }

    /// Keys while the tab is showing.
    pub(crate) fn log_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(log) = &self.log {
            log.update(cx, |log, cx| log.log_key(event, window, cx));
        }
    }
}

impl LogPanel {
    /// The name of the project's folder.
    fn root_name(&self) -> String {
        self.root
            .file_name()
            .map_or_else(|| ".".into(), |name| name.to_string_lossy().into_owned())
    }

    /// The history and the refs read again.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load_log_refs(cx);
        self.load_log(cx);
    }

    /// Reads the branches for the tree and the authors for the user filter again, off the UI
    /// thread.
    fn load_log_refs(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (found, authors, remote, remote_tags) = cx
                .background_executor()
                .spawn(async move {
                    let found = zide_git::branches(&root);
                    let current = found.as_ref().and_then(|b| b.current.clone());
                    let remote = zide_git::push_remote(&root, &current.unwrap_or_default()).ok();
                    let remote_tags = crate::session::remote_tags(&root)
                        .map(|(remote, tags)| (remote, tags.into_iter().collect()));
                    (found, log::authors(&root), remote, remote_tags)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(found) = found {
                    this.set_refs(found);
                }
                this.remote = remote;
                this.remote_tags = remote_tags;
                if let Ok(authors) = authors {
                    this.authors = authors;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the commits for the scope, the search and the number asked for, off the UI thread.
    fn load_log(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.loading = true;
        let generation = self.generation;
        let query = self.log_query(cx);
        let filtered = query.is_filtered();
        let limit = self.limit;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { log::read(&root, &query, limit) })
                .await;
            this.update(cx, |this, cx| {
                this.set_entries(generation, found, filtered);
                this.load_log_files(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The list reached its end: asks for another page.
    fn log_more(&mut self, cx: &mut Context<Self>) {
        let more = matches!(self.entries, Entries::Ready { more: true, .. });
        if !more || self.loading {
            return;
        }
        self.limit += PAGE;
        self.load_log(cx);
    }

    /// The text changed: the search runs when typing pauses.
    fn log_typed(&mut self, cx: &mut Context<Self>) {
        self.typed += 1;
        self.limit = PAGE;
        let typed = self.typed;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_PAUSE).await;
            this.update(cx, |this, cx| {
                if this.typed == typed {
                    this.load_log(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads, off the UI thread, the files and the branches of the selected commit.
    fn load_log_files(&mut self, cx: &mut Context<Self>) {
        let Some(hash) = self.start_files_load() else {
            return;
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let wanted = hash.clone();
            let (files, branches) = cx
                .background_executor()
                .spawn(async move {
                    (
                        zide_git::commit_files(&root, &hash),
                        log::branches_with(&root, &hash),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.set_files(&wanted, files);
                if this.selected_entry().map(|e| e.commit.hash.as_str()) == Some(&wanted) {
                    this.in_branches = branches.ok();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_log_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        self.pane = Pane::Commits;
        if self.select(index) {
            self.load_log_files(cx);
        }
        cx.notify();
    }

    /// Narrows the history to what the tree row `row` is, or opens/closes it when it is a folder.
    fn activate_log_ref(&mut self, row: usize, cx: &mut Context<Self>) {
        self.pane = Pane::Refs;
        self.head_cursor = false;
        let Some(model) = &mut self.refs else {
            return;
        };
        model.select(row);
        if let Some(target) = model.activate() {
            self.scope = self.scope_for(&target);
            self.limit = PAGE;
            self.load_log(cx);
        }
        cx.notify();
    }

    /// The HEAD line: the history of the checked-out branch, or every branch when it already is.
    fn pick_log_head(&mut self, cx: &mut Context<Self>) {
        self.pane = Pane::Refs;
        self.head_cursor = true;
        self.scope = if self.scope == Scope::Head {
            Scope::All
        } else {
            Scope::Head
        };
        self.limit = PAGE;
        self.load_log(cx);
    }

    fn show_log_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(entry), Some(file)) = (self.selected_entry(), self.selected_file()) else {
            return;
        };
        let (hash, file) = (entry.commit.hash.clone(), file.clone());
        cx.emit(LogEvent::ShowDiff { hash, file });
    }

    fn log_typing(&self, window: &Window, cx: &App) -> bool {
        [&self.query, &self.paths]
            .iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Opens or closes the list of a filter.
    fn toggle_log_menu(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.toggle_menu(filter);

        cx.notify();
    }

    /// The date filter changed: `after` tells which of the two days, and an empty one is cleared.
    fn log_day(&mut self, after: bool, day: String, cx: &mut Context<Self>) {
        self.preset = 0; // the days picked take the place of a preset
        if after {
            self.after_day = day;
        } else {
            self.before_day = day;
        }
        self.limit = PAGE;

        self.load_log(cx);
    }

    /// Narrows the history by row `row` of the list of `filter`.
    fn pick_log_filter(
        &mut self,
        filter: Filter,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.limit = PAGE;
        match filter {
            Filter::User => self.pick_menu(filter, row),
            Filter::Range => return,
            Filter::Date => match date_pick(row) {
                DatePick::Select => {
                    // the pickers are for the days; nothing is asked of git until one is picked
                    self.menu = Some((Filter::Range, 0));
                    return cx.notify();
                }
                pick => {
                    self.menu = None;
                    self.preset = match pick {
                        DatePick::Preset(at) => at,
                        _ => 0,
                    };
                    self.after_day.clear();
                    self.before_day.clear();
                    for picker in [self.after.clone(), self.before.clone()] {
                        picker.update(cx, |picker, cx| {
                            picker.set_date(Date::Single(None), window, cx)
                        });
                    }
                }
            },
        }
        self.load_log(cx);
    }

    /// Tab: the keyboard goes to the next list, or out of the search box.
    fn log_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.log_typing(window, cx) {
            self.focus.focus(window, cx);
        } else {
            self.pane = match self.pane {
                Pane::Refs => Pane::Commits,
                Pane::Commits => Pane::Files,
                Pane::Files => Pane::Refs,
            };
        }
        cx.notify();
    }

    /// Keys while the tab is showing: what is typed in the search box is the box's.
    fn log_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        use zide_vim::Key;
        let key = event.keystroke.key.as_str();
        if self.log_typing(window, cx) {
            if key == "escape" {
                self.focus.focus(window, cx); // back to the lists
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        let vim = crate::vim_key(event);
        if self.context_menu.is_some() {
            // the context menu of a commit takes every key
            let down = key == "down" || vim == Some(Key::Char('j'));
            let up = key == "up" || vim == Some(Key::Char('k'));
            cx.stop_propagation();
            return match self.menu_key(key, down, up) {
                menu::MenuKey::Run(action) => self.menu_action(action, cx),
                _ => cx.notify(),
            };
        }
        if matches!(self.menu, Some((Filter::Range, _))) {
            // the pickers read their own keys; Esc closes the popup, and not the tab
            if key == "escape" {
                self.menu = None;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        cx.stop_propagation();
        let pane = self.pane;
        let down = key == "down" || vim == Some(Key::Char('j'));
        let up = key == "up" || vim == Some(Key::Char('k'));
        if let Some((filter, row)) = self.menu {
            // an open filter list takes the keys
            let last = self.menu_items(filter).len().saturating_sub(1);
            match key {
                "escape" => self.menu = None,
                "enter" => return self.pick_log_filter(filter, row, window, cx),
                _ if down => self.menu = Some((filter, (row + 1).min(last))),
                _ if up => self.menu = Some((filter, row.saturating_sub(1))),
                _ => {}
            }
            return cx.notify();
        }
        match key {
            "escape" => return cx.emit(LogEvent::Hide),
            "/" | "p" => {
                let input = if key == "/" { &self.query } else { &self.paths };
                let input = input.clone();
                input.update(cx, |state, cx| state.focus(window, cx));
                return cx.notify();
            }
            "u" => return self.toggle_log_menu(Filter::User, cx),
            "d" => return self.toggle_log_menu(Filter::Date, cx),
            _ if down || up => {
                let delta = if down { 1 } else { -1 };
                match pane {
                    Pane::Refs => {
                        self.move_ref(delta);
                    }
                    Pane::Commits => {
                        let at = self.selected.saturating_add_signed(delta);
                        self.select_log_commit(at, cx);
                        return self.scroll_log();
                    }
                    Pane::Files => {
                        let to = self.picks.cursor.saturating_add_signed(delta);
                        let last = self.files().len().saturating_sub(1);
                        self.picks.move_to(to.min(last), modifiers.shift);
                    }
                }
            }
            "pagedown" | "pageup" if pane == Pane::Commits => {
                let delta = if key == "pagedown" { 15 } else { -15 };
                let at = self.selected.saturating_add_signed(delta);
                self.select_log_commit(at, cx);
                return self.scroll_log();
            }
            "space" if pane == Pane::Files => self.picks.toggle(),
            "enter" => match pane {
                Pane::Refs if self.head_cursor => return self.pick_log_head(cx),
                Pane::Refs => {
                    let row = self.refs.as_ref().map_or(0, Model::selected);
                    return self.activate_log_ref(row, cx);
                }
                Pane::Commits => self.pane = Pane::Files,
                Pane::Files => return self.show_log_diff(cx),
            },
            "left" | "right" => {
                let open = key == "right";
                // Right opens a closed folder, and only when there is nothing to open does it
                // leave the tree for the commits.
                let closed = self.closed_ref_row();
                match (pane, &mut self.refs) {
                    (Pane::Refs, Some(model)) if !self.head_cursor && (!open || closed) => {
                        model.fold(open)
                    }
                    (Pane::Refs, _) if open => self.pane = Pane::Commits,
                    (Pane::Commits, _) if open => self.pane = Pane::Files,
                    (Pane::Commits, _) => self.pane = Pane::Refs,
                    (Pane::Files, _) if !open => self.pane = Pane::Commits,
                    _ => {}
                }
            }
            _ => {}
        }
        self.scroll_log();
        cx.notify();
    }

    /// Brings the selected rows of the three lists into view.
    fn scroll_log(&self) {
        self.commit_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        if let Some(model) = &self.refs {
            self.ref_scroll
                .scroll_to_item(model.selected(), ScrollStrategy::Nearest);
        }
        let root = self.root_name();
        let at = changed_files::rows(self.files(), &root).iter().position(
            |row| matches!(row, changed_files::FileRow::File { index, .. } if *index == self.picks.cursor),
        );
        if let Some(at) = at {
            self.file_scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        }
    }

    fn log_view(&self, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let root = self.root_name();

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(40.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .child(div().w(px(300.)).child(Input::new(&self.query)))
            .child(filter_menu(self, Filter::User, &entity))
            .child(filter_menu(self, Filter::Date, &entity))
            .child(div().w(px(200.)).child(Input::new(&self.paths)))
            .children(self.scope_label().map(|label| {
                let clear = entity.clone();
                div()
                    .id("log-scope")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(26.))
                    .rounded_md()
                    .bg(rgb(theme::SELECTION))
                    .child(format!("Branch: {label}"))
                    .child(
                        div()
                            .id("log-scope-clear")
                            .cursor_pointer()
                            .text_color(rgb(theme::MUTED))
                            .child("×")
                            .on_click(move |_, _, cx| {
                                clear.update(cx, |this, cx| {
                                    this.scope = Scope::All;
                                    this.limit = PAGE;

                                    this.load_log(cx);
                                })
                            }),
                    )
            }))
            .children(
                self.loading
                    .then(|| Spinner::new().small().color(rgb(theme::MUTED).into())),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .text_color(rgb(theme::TEXT))
            .children(
                self.context_menu
                    .as_ref()
                    .map(|open| menu::view(open, &entity)),
            )
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(refs_pane(self, &entity))
                    .child(commits_pane(self, &entity))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(380.))
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(rgb(theme::BORDER))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .child(files_pane(self, &root, &entity)),
                            )
                            .child(details(self)),
                    ),
            )
    }
}

/// A button that opens the list of `filter`; it says what the history is narrowed to.
fn filter_menu(panel: &LogPanel, filter: Filter, entity: &Entity<LogPanel>) -> Div {
    let (label, active) = match filter {
        Filter::User => match panel.authors.iter().find(|a| a.email == panel.user) {
            Some(author) => (format!("User: {}", author.name), true),
            None if !panel.user.is_empty() => (format!("User: {}", panel.user), true),
            None => ("User".to_string(), false),
        },
        Filter::Date | Filter::Range => match (panel.after_day.as_str(), panel.before_day.as_str())
        {
            ("", "") => match panel.preset.checked_sub(1).and_then(|at| PRESETS.get(at)) {
                Some((label, _)) => (format!("Date: {label}"), true),
                None => ("Date".to_string(), false),
            },
            (after, "") => (format!("Date: after {after}"), true),
            ("", before) => (format!("Date: before {before}"), true),
            (after, before) => (format!("Date: {after} to {before}"), true),
        },
    };
    // the range popup belongs to the Date button
    let shown = |menu: Filter| panel.menu.filter(|(open, _)| *open == menu);
    let open = shown(filter);
    let range = if filter == Filter::Date {
        shown(Filter::Range)
    } else {
        None
    };
    let toggle = entity.clone();
    div()
        .relative()
        .flex_shrink_0()
        .child(
            div()
                .id(SharedString::from(format!("log-filter-{filter:?}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(26.))
                .rounded_md()
                .cursor_pointer()
                .whitespace_nowrap()
                .border_1()
                .border_color(rgb(if open.is_some() {
                    theme::ACTIVE_BORDER
                } else {
                    theme::CONTROL_BORDER
                }))
                .when(active, |d| d.bg(rgb(theme::SELECTION)))
                .child(label)
                .child(div().text_color(rgb(theme::DIM)).child("⌄"))
                .on_click(move |_, _, cx| {
                    toggle.update(cx, |this, cx| this.toggle_log_menu(filter, cx))
                }),
        )
        .when_some(range, |d, _| {
            let row = |label: &'static str, picker: &Entity<DatePickerState>| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(50.)).child(label))
                    .child(
                        div().w(px(190.)).child(
                            DatePicker::new(picker)
                                .placeholder("Any day")
                                .cleanable(true),
                        ),
                    )
            };
            // deferred: painted over the lists under the header
            d.child(deferred(
                div()
                    .id("log-menu-date")
                    .occlude()
                    .absolute()
                    .top(px(30.))
                    .left_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .bg(rgb(theme::POPUP))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .child(row("After", &panel.after))
                    .child(row("Before", &panel.before)),
            ))
        })
        .when_some(open, |d, (_, highlighted)| {
            let rows = panel
                .menu_items(filter)
                .into_iter()
                .enumerate()
                .map(|(row, item)| {
                    let pick = entity.clone();
                    div()
                        .id(SharedString::from(format!("log-{filter:?}-{row}")))
                        .px_2()
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .cursor_pointer()
                        .whitespace_nowrap()
                        .when(row == highlighted, |d| d.bg(rgb(theme::ACTIVE_BG)))
                        .hover(|d| d.bg(rgb(theme::SELECTION)))
                        .child(item)
                        .on_click(move |_, window, cx| {
                            pick.update(cx, |this, cx| {
                                this.pick_log_filter(filter, row, window, cx)
                            })
                        })
                });
            // deferred: painted over the lists under the header
            d.child(deferred(
                div()
                    .id(SharedString::from(format!("log-menu-{filter:?}")))
                    .occlude()
                    .absolute()
                    .top(px(30.))
                    .left_0()
                    .min_w(px(180.))
                    .max_h(px(ROW_HEIGHT * 10.))
                    .overflow_y_scroll()
                    .bg(rgb(theme::POPUP))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .children(rows),
            ))
        })
}

/// A line of text in the place of a list that has nothing to show.
fn note(text: impl Into<SharedString>, loading: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .p_3()
        .text_sm()
        .text_color(rgb(theme::MUTED))
        .children(loading.then(|| Spinner::new().small().color(rgb(theme::MUTED).into())))
        .child(text.into())
}

fn focus_bg(focused: bool) -> u32 {
    if focused {
        theme::TREE_FOCUSED
    } else {
        theme::TREE_SELECTED
    }
}

fn refs_pane(panel: &LogPanel, entity: &Entity<LogPanel>) -> Div {
    let focused = panel.pane == Pane::Refs;
    let head = {
        let entity = entity.clone();
        let active = panel.scope == Scope::Head;
        div()
            .id("log-head")
            .flex()
            .items_center()
            .gap_1()
            .h(px(ROW_HEIGHT))
            .px_2()
            .flex_shrink_0()
            .cursor_pointer()
            .font_weight(FontWeight::BOLD)
            .when(panel.head_cursor && focused, |d| d.bg(rgb(focus_bg(true))))
            .when(active && !(panel.head_cursor && focused), |d| {
                d.bg(rgb(focus_bg(false)))
            })
            .on_click(move |_, _, cx| entity.update(cx, |this, cx| this.pick_log_head(cx)))
            .child("HEAD (Current Branch)")
    };
    let body = match &panel.refs {
        None => note("", true).into_any_element(),
        Some(model) => {
            let rows: Arc<Vec<Row>> = model.rows();
            let (cursor, scope_key) = (model.selected(), panel.scope_key());
            let cursor_in_tree = focused && !panel.head_cursor;
            let entity = entity.clone();
            let scroll = panel.ref_scroll.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list("log-refs", rows.len(), move |range, _, _| {
                        range
                            .map(|index| {
                                let row = &rows[index];
                                let entity = entity.clone();
                                let line = div()
                                    .id(("log-ref", index))
                                    .w_full()
                                    .h(px(ROW_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .pl(px(8. + 14. * row.depth as f32))
                                    .pr_2()
                                    .whitespace_nowrap()
                                    .cursor_pointer()
                                    .on_click({
                                        let entity = entity.clone();
                                        move |_, _, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.activate_log_ref(index, cx)
                                            })
                                        }
                                    })
                                    .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                                        let at = event.position;
                                        entity.update(cx, |_, cx| {
                                            cx.emit(LogEvent::RefMenu { row: index, at })
                                        })
                                    });
                                let picked = scope_key.as_deref() == Some(row.key.as_str());
                                let line = match &row.kind {
                                    Kind::Section { expanded } => line
                                        .font_weight(FontWeight::BOLD)
                                        .child(div().w(px(12.)).child(if *expanded {
                                            "▾"
                                        } else {
                                            "▸"
                                        }))
                                        .child(row.label.clone()),
                                    Kind::Folder { expanded } => {
                                        let (icon, color) =
                                            file_icon::for_row(&row.label, true, *expanded);
                                        line.child(div().w(px(12.)).child(if *expanded {
                                            "▾"
                                        } else {
                                            "▸"
                                        }))
                                        .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                        .child(row.label.clone())
                                    }
                                    Kind::Branch { current, .. } => line
                                        .child(div().w(px(12.)))
                                        .child(
                                            Icon::new(Icon::empty().path(statusbar::BRANCH_ICON))
                                                .size_4()
                                                .text_color(rgb(theme::ICON_DATA)),
                                        )
                                        .child(
                                            div()
                                                .when(*current, |d| d.font_weight(FontWeight::BOLD))
                                                .child(row.label.clone()),
                                        ),
                                };
                                let line = if index == cursor && cursor_in_tree {
                                    line.bg(rgb(focus_bg(true)))
                                } else if picked {
                                    line.bg(rgb(focus_bg(false)))
                                } else {
                                    line
                                };
                                line.into_any_element()
                            })
                            .collect()
                    })
                    .size_full()
                    .track_scroll(&scroll),
                )
                .child(Scrollbar::new(&panel.ref_scroll))
                .into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .w(px(240.))
        .flex_shrink_0()
        .border_r_1()
        .border_color(rgb(theme::BORDER))
        .child(head)
        .child(body)
}

fn lane_color(index: usize) -> Hsla {
    rgb(LANE_COLORS[index % LANE_COLORS.len()]).into()
}

fn stroke(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla) {
    let mut line = PathBuilder::stroke(px(1.5));
    line.move_to(from);
    line.line_to(to);
    if let Ok(path) = line.build() {
        window.paint_path(path, color);
    }
}

/// The lanes of one row: the lines that run past, those that end or start at the dot, and the dot.
fn graph_cell(row: &GraphRow, width: f32) -> Stateful<Div> {
    let row = row.clone();
    let canvas = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let column = |lane: usize| {
                bounds.origin.x + px(LANE_WIDTH * lane.min(MAX_LANES - 1) as f32 + LANE_WIDTH / 2.)
            };
            let top = bounds.origin.y;
            let middle = top + bounds.size.height / 2.;
            let bottom = top + bounds.size.height;
            for &(lane, color) in &row.through {
                let (from, to) = (point(column(lane), top), point(column(lane), bottom));
                stroke(window, from, to, lane_color(color));
            }
            for &(lane, color) in &row.top {
                let (from, to) = (point(column(lane), top), point(column(row.node), middle));
                stroke(window, from, to, lane_color(color));
            }
            for &(lane, color) in &row.bottom {
                let (from, to) = (point(column(row.node), middle), point(column(lane), bottom));
                stroke(window, from, to, lane_color(color));
            }
            let radius = px(4.);
            let dot = Bounds::new(
                point(column(row.node) - radius, middle - radius),
                size(radius * 2., radius * 2.),
            );
            window.paint_quad(fill(dot, lane_color(row.color)).corner_radii(radius));
        },
    );
    div()
        .id("log-graph")
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .child(canvas.size_full())
}

fn ref_chip(label: &RefLabel) -> Div {
    let color = match label.kind {
        RefKind::Local | RefKind::Head => theme::ICON_DATA,
        RefKind::Remote => theme::ICON_CODE,
        RefKind::Tag => theme::ICON_FOLDER,
    };
    div()
        .px_1()
        .h(px(18.))
        .flex()
        .items_center()
        .flex_shrink_0()
        .rounded(px(3.))
        .border_1()
        .border_color(rgb(color))
        .text_xs()
        .text_color(rgb(color))
        .when(label.current, |d| d.font_weight(FontWeight::BOLD))
        .child(label.name.clone())
}

fn commits_pane(panel: &LogPanel, entity: &Entity<LogPanel>) -> Div {
    let body = match &panel.entries {
        Entries::Loading => note("Reading the history", true).into_any_element(),
        Entries::Failed(why) => note(why.clone(), false)
            .text_color(rgb(theme::ERROR))
            .into_any_element(),
        Entries::Ready { list, .. } if list.is_empty() => {
            note("No commits found", false).into_any_element()
        }
        Entries::Ready { list, graph, more } => {
            let (list, graph, more) = (list.clone(), graph.clone(), *more);
            let lanes = graph
                .iter()
                .map(GraphRow::width)
                .max()
                .unwrap_or(1)
                .min(MAX_LANES);
            let width = LANE_WIDTH * lanes as f32 + 6.;
            let (selected, focused) = (panel.selected, panel.pane == Pane::Commits);
            let entity = entity.clone();
            div()
                .relative()
                .size_full()
                .child(
                    uniform_list("log-commits", list.len(), move |range, window, cx| {
                        if more && range.end + 20 >= list.len() {
                            let entity = entity.clone();
                            window.defer(cx, move |_, cx| {
                                entity.update(cx, |this, cx| this.log_more(cx))
                            });
                        }
                        range
                            .map(|index| {
                                let entry = &list[index];
                                let entity = entity.clone();
                                let (date, clock) = zide_git::date_and_time(
                                    entry.commit.time,
                                    entry.commit.tz_minutes,
                                );
                                div()
                                    .id(("log-commit", index))
                                    .w_full()
                                    .h(px(ROW_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .cursor_pointer()
                                    .when(index == selected, |d| d.bg(rgb(focus_bg(focused))))
                                    .on_click({
                                        let entity = entity.clone();
                                        move |_, _, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.select_log_commit(index, cx)
                                            })
                                        }
                                    })
                                    .on_mouse_down(MouseButton::Right, move |event, _, cx| {
                                        let at = event.position;
                                        entity.update(cx, |this, cx| {
                                            this.open_context_menu(index, at, cx)
                                        })
                                    })
                                    .child(graph_cell(&graph[index], width))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .when(entry.parents.len() > 1, |d| {
                                                d.text_color(rgb(theme::MUTED))
                                            })
                                            .child(entry.commit.subject.clone()),
                                    )
                                    .children(entry.refs.iter().map(ref_chip))
                                    .child(
                                        div()
                                            .w(px(120.))
                                            .flex_shrink_0()
                                            .truncate()
                                            .font_weight(FontWeight::BOLD)
                                            .child(entry.commit.author.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(130.))
                                            .flex_shrink_0()
                                            .whitespace_nowrap()
                                            .text_color(rgb(theme::MUTED))
                                            .child(format!("{date}, {clock}")),
                                    )
                                    .into_any_element()
                            })
                            .collect()
                    })
                    .size_full()
                    .track_scroll(&panel.commit_scroll),
                )
                .child(Scrollbar::new(&panel.commit_scroll))
                .into_any_element()
        }
    };
    div().flex_1().min_w_0().h_full().child(body)
}

fn files_pane(panel: &LogPanel, root: &str, entity: &Entity<LogPanel>) -> AnyElement {
    match &panel.files {
        Files::Loading => note(
            if panel.selected_entry().is_some() {
                "Loading"
            } else {
                ""
            },
            panel.selected_entry().is_some(),
        )
        .into_any_element(),
        Files::Failed(why) => note(why.clone(), false).into_any_element(),
        Files::Ready(list) if list.is_empty() => note("No files changed", false).into_any_element(),
        Files::Ready(list) => {
            let entity = entity.clone();
            changed_files::list(
                "log-files",
                Arc::new(changed_files::rows(list, root)),
                panel.picked_files(),
                panel.pane == Pane::Files,
                &panel.file_scroll,
                {
                    let entity = entity.clone();
                    move |index, clicks, modifiers, _, cx| {
                        entity.update(cx, |this, cx| {
                            let ctrl = modifiers.control || modifiers.platform;
                            this.click_file(index, ctrl, modifiers.shift);

                            if clicks == 2 && !ctrl && !modifiers.shift {
                                this.show_log_diff(cx);
                            }
                            cx.notify();
                        })
                    }
                },
                Some(Rc::new(move |index, at, _, cx| {
                    entity.update(cx, |this, cx| this.open_files_menu(index, at, cx))
                })),
            )
        }
    }
}

/// Under the files: the message of the selected commit, who made it and when, and where it is.
fn details(panel: &LogPanel) -> Stateful<Div> {
    let frame = div()
        .id("log-details")
        .flex_shrink_0()
        .h(px(220.))
        .p_3()
        .overflow_y_scroll()
        .border_t_1()
        .border_color(rgb(theme::BORDER));
    let Some(entry) = panel.selected_entry() else {
        return frame;
    };
    // the content keeps its height, so it scrolls inside the frame instead of squeezing
    let block = div().flex().flex_col().gap_2().flex_shrink_0();
    let commit = &entry.commit;
    let (date, clock) = zide_git::date_and_time(commit.time, commit.tz_minutes);
    let content = block
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(theme::STRONG))
                .child(commit.subject.clone()),
        )
        .children((!commit.body.is_empty()).then(|| {
            div()
                .text_sm()
                .text_color(rgb(theme::MUTED))
                .child(commit.body.clone())
        }))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .text_sm()
                .text_color(rgb(theme::MUTED))
                .child(format!("{} {}", commit.short_hash(), commit.author))
                .child(
                    div()
                        .text_color(rgb(theme::ICON_CODE))
                        .child(format!("<{}>", commit.email)),
                )
                .child(format!("on {date} at {clock}")),
        )
        .children((!entry.refs.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(entry.refs.iter().map(ref_chip))
        }))
        .children(panel.in_branches.as_ref().map(|branches| {
            let count = changed_files::plural(branches.len(), "branch");
            div()
                .text_xs()
                .text_color(rgb(theme::DIM))
                .child(format!("In {count}: {}", branches.join(", ")))
        }));
    frame.child(content)
}

#[cfg(test)]
mod tests {
    use zide_git::Switch;
    use zide_git::log::Scope;

    use zide_git::log::Author;

    use super::{
        DatePick, date_items, date_pick, day_limits, scope_after_pick, scope_key, user_at,
        user_items,
    };

    fn authors() -> Vec<Author> {
        let author = |name: &str, email: &str, commits| Author {
            name: name.into(),
            email: email.into(),
            commits,
        };
        vec![author("Ana", "ana@x.io", 7), author("Luis", "l@x.io", 2)]
    }

    #[::core::prelude::v1::test]
    fn the_user_list_starts_with_everyone_and_a_row_gives_the_email() {
        let who = authors();
        assert_eq!(
            user_items(&who),
            ["All users", "Ana <ana@x.io>  (7)", "Luis <l@x.io>  (2)"]
        );
        assert_eq!(user_at(&who, 0), "");
        assert_eq!(user_at(&who, 2), "l@x.io");
        assert_eq!(user_at(&who, 9), ""); // past the list: everyone
    }

    #[::core::prelude::v1::test]
    fn the_date_list_has_the_presets_and_ends_in_select() {
        assert_eq!(
            date_items(),
            [
                "Any time",
                "Last 24 hours",
                "Last 7 days",
                "Last 30 days",
                "Select..."
            ]
        );
        assert_eq!(date_pick(0), DatePick::Any);
        assert_eq!(date_pick(3), DatePick::Preset(3));
        assert_eq!(date_pick(4), DatePick::Select);
    }

    #[::core::prelude::v1::test]
    fn the_days_picked_cover_the_whole_first_and_last_day() {
        assert_eq!(day_limits("", ""), (String::new(), String::new()));
        assert_eq!(
            day_limits("2026-11-01", "2026-11-08"),
            (
                "2026-11-01 00:00:00".to_string(),
                "2026-11-08 23:59:59".to_string()
            )
        );
        assert_eq!(day_limits("", "2026-11-08").0, "");
    }

    #[::core::prelude::v1::test]
    fn picking_a_ref_narrows_the_history_and_picking_it_again_goes_back_to_all() {
        let main = Switch::Local("main".into());
        let narrowed = scope_after_pick(&Scope::All, &main);
        assert_eq!(narrowed, Scope::Ref("refs/heads/main".into()));
        assert_eq!(scope_after_pick(&narrowed, &main), Scope::All);
        let other = Switch::Remote("origin/dev".into());
        assert_eq!(
            scope_after_pick(&narrowed, &other),
            Scope::Ref("refs/remotes/origin/dev".into())
        );
    }

    #[::core::prelude::v1::test]
    fn the_scope_maps_back_to_the_row_of_the_tree() {
        assert_eq!(
            scope_key(&Scope::Ref("refs/heads/a/b".into())).as_deref(),
            Some("local/a/b")
        );
        assert_eq!(
            scope_key(&Scope::Ref("refs/remotes/origin/x".into())).as_deref(),
            Some("remote/origin/x")
        );
        assert_eq!(scope_key(&Scope::All), None);
        assert_eq!(scope_key(&Scope::Head), None);
    }
}
