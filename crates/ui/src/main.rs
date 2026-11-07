//! Search Everywhere prototype: double-Shift popup with file and text search over a real project.
//!
//! `zide-ui [root]` indexes `root` (default: the current directory) in the background;
//! the window shows up before the index is ready (ADR-009). Tab switches between the Files and
//! Text tabs. Timings go to stderr.

use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod definition;
mod file_icon;
mod indent;
mod markdown;
mod meter;
mod nav;
mod projects;
mod rows;
mod session;
mod statusbar;
mod sticky;
mod theme;
mod usages;
mod viewer;
mod welcome;

use gpui_kit::component::Icon;
use gpui_kit::component::IndexPath;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::*;
use viewer::{Effect, FindStatus, FormatRequest, HighlightKey, Viewer};
use welcome::{OpenProject, Welcome};
use zide_core::{Change, DoubleShiftDetector, ProjectWatcher, Scan, scan_project, watch_project};
use zide_index::FileIndex;
use zide_lsp::{Client, Error as LspError, server_for};
use zide_search::{
    ActionHit, ActionText, FileHit, FileQuery, TextHit, TextQuery, TextSearchError, search_actions,
    search_files, search_files_with_recent, search_text,
};
use zide_stubs::{StubCache, StubKind, StubStore, SymbolHit, search_symbols};
use zide_text::{FindQuery, Revision};
use zide_tree::{Outcome as TreeOutcome, Tree};

actions!(
    zide,
    [
        SwitchTab,
        ToggleRegex,
        ToggleCase,
        ToggleWord,
        FindInFile,
        ReplaceInFile,
        ReplaceAll,
        FindNext,
        FindPrevious,
        GoToFile,
        GoToSymbol,
        FindAction,
        FindInFiles,
        RecentFiles,
        ToggleProjectTree,
        ToggleMarkdownPreview,
        NavigateBack,
        NavigateForward,
        FindUsages,
        GoToDefinition,
        ReformatCode,
        ExtendSelection,
        FocusTree,
        FocusEditor,
        ShrinkSelection,
        NextMethod,
        CollapseRegion,
        ExpandRegion,
        CollapseAll,
        ExpandAll,
        PreviousMethod,
    ]
);

/// Text search toggles, like IntelliJ's Regex / Match Case / Words buttons.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct TextOptions {
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
}

/// GitHub repository releases are published to.
const UPDATE_REPO: &str = "sergio222-dev/Zide";
/// How often a running Zide looks for a newer release.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_FILE_RESULTS: usize = 200;
const MAX_SYMBOL_RESULTS: usize = 200;
const MAX_RECENT: usize = 50;
/// Height of a row of the project tree, in pixels.
const TREE_ROW_HEIGHT: f32 = 22.;
const MAX_TEXT_RESULTS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Files,
    Symbols,
    Text,
    Actions,
    /// Files opened in this session (Ctrl+E); not part of the Tab cycle.
    Recent,
}

impl Tab {
    fn next(self) -> Self {
        match self {
            Tab::Files => Tab::Symbols,
            Tab::Symbols => Tab::Text,
            Tab::Text => Tab::Actions,
            Tab::Actions | Tab::Recent => Tab::Files,
        }
    }
}

/// A command of the IDE (ADR-011): searched by name and synonyms, run from the Actions tab.
struct ActionDef {
    name: &'static str,
    synonyms: &'static [&'static str],
    /// The GPUI action behind the command, when it has one: its keymap binding is shown.
    action: Option<Box<dyn Action>>,
    /// Commands that act on the popup itself keep it open; the others close it first.
    closes_popup: bool,
    run: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
}

fn action_registry() -> Vec<ActionDef> {
    vec![
        ActionDef {
            name: "Go to File",
            synonyms: &["find files", "open file"],
            action: Some(Box::new(GoToFile)),
            closes_popup: false,
            run: |ws, _, cx| ws.show_tab(Tab::Files, cx),
        },
        ActionDef {
            name: "Reformat Code",
            synonyms: &["format", "beautify", "prettify", "indent"],
            action: Some(Box::new(ReformatCode)),
            closes_popup: true,
            run: |ws, _, cx| ws.reformat(cx),
        },
        ActionDef {
            name: "Go to Definition",
            synonyms: &["declaration", "gd", "jump to definition", "goto definition"],
            action: Some(Box::new(GoToDefinition)),
            closes_popup: true,
            run: |ws, _, cx| ws.go_to_definition(cx),
        },
        ActionDef {
            name: "Find Usages",
            synonyms: &["references", "occurrences", "where used"],
            action: Some(Box::new(FindUsages)),
            closes_popup: false,
            run: |ws, window, cx| ws.find_usages(window, cx),
        },
        ActionDef {
            name: "Toggle Project Tree",
            synonyms: &["file explorer", "project view", "sidebar"],
            action: Some(Box::new(ToggleProjectTree)),
            closes_popup: true,
            run: |ws, _, cx| ws.toggle_tree(cx),
        },
        ActionDef {
            name: "Toggle Markdown Preview",
            synonyms: &["markdown", "md", "render"],
            action: Some(Box::new(ToggleMarkdownPreview)),
            closes_popup: true,
            run: |ws, _, cx| ws.toggle_md_preview(cx),
        },
        ActionDef {
            name: "Navigate Back",
            synonyms: &["previous location", "go back", "history"],
            action: Some(Box::new(NavigateBack)),
            closes_popup: true,
            run: |ws, _, cx| ws.navigate(true, cx),
        },
        ActionDef {
            name: "Navigate Forward",
            synonyms: &["next location", "go forward", "history"],
            action: Some(Box::new(NavigateForward)),
            closes_popup: true,
            run: |ws, _, cx| ws.navigate(false, cx),
        },
        ActionDef {
            name: "Recent Files",
            synonyms: &["recent", "history", "switcher"],
            action: Some(Box::new(RecentFiles)),
            closes_popup: false,
            run: |ws, _, cx| ws.show_tab(Tab::Recent, cx),
        },
        ActionDef {
            name: "Go to Symbol",
            synonyms: &["symbols", "function", "class", "method"],
            action: Some(Box::new(GoToSymbol)),
            closes_popup: false,
            run: |ws, _, cx| ws.show_tab(Tab::Symbols, cx),
        },
        ActionDef {
            name: "Find Action",
            synonyms: &["commands", "actions"],
            action: Some(Box::new(FindAction)),
            closes_popup: false,
            run: |ws, _, cx| ws.show_tab(Tab::Actions, cx),
        },
        ActionDef {
            name: "Find in File",
            synonyms: &["search in file", "find in editor"],
            action: Some(Box::new(FindInFile)),
            closes_popup: true,
            run: |ws, window, cx| ws.open_find(false, window, cx),
        },
        ActionDef {
            name: "Replace in File",
            synonyms: &["find and replace", "substitute"],
            action: Some(Box::new(ReplaceInFile)),
            closes_popup: true,
            run: |ws, window, cx| ws.open_find(true, window, cx),
        },
        ActionDef {
            name: "Find in Files",
            synonyms: &["grep", "ripgrep", "search text"],
            action: Some(Box::new(FindInFiles)),
            closes_popup: false,
            run: |ws, _, cx| ws.show_tab(Tab::Text, cx),
        },
        ActionDef {
            name: "Switch Tab",
            synonyms: &["next tab"],
            action: Some(Box::new(SwitchTab)),
            closes_popup: false,
            run: |ws, _, cx| ws.switch_tab(cx),
        },
        ActionDef {
            name: "Toggle Regex",
            synonyms: &["regular expression"],
            action: Some(Box::new(ToggleRegex)),
            closes_popup: false,
            run: |ws, _, cx| ws.toggle_text_option(|o| o.regex = !o.regex, cx),
        },
        ActionDef {
            name: "Toggle Match Case",
            synonyms: &["case sensitive"],
            action: Some(Box::new(ToggleCase)),
            closes_popup: false,
            run: |ws, _, cx| ws.toggle_text_option(|o| o.case_sensitive = !o.case_sensitive, cx),
        },
        ActionDef {
            name: "Toggle Whole Word",
            synonyms: &["words"],
            action: Some(Box::new(ToggleWord)),
            closes_popup: false,
            run: |ws, _, cx| ws.toggle_text_option(|o| o.whole_word = !o.whole_word, cx),
        },
        ActionDef {
            name: "Reload Project",
            synonyms: &["rescan", "refresh", "reindex"],
            action: None,
            closes_popup: true,
            run: |ws, _, cx| ws.rescan(cx),
        },
        ActionDef {
            name: "Update Zide",
            synonyms: &["upgrade", "check for updates", "new version"],
            action: None,
            closes_popup: true,
            run: |_, _, cx| cx.emit(UpdateRequested),
        },
        ActionDef {
            name: "Open",
            synonyms: &["open project", "open folder", "switch project"],
            action: None,
            closes_popup: true,
            run: |_, _, cx| cx.emit(OpenRequested),
        },
        ActionDef {
            name: "Quit",
            synonyms: &["exit", "close ide"],
            action: None,
            closes_popup: true,
            run: |_, _, cx| cx.quit(),
        },
    ]
}

/// The find / replace bar of the editor (not a popup: it lives in the window layout).
struct FindPanel {
    find: Entity<InputState>,
    replace: Entity<InputState>,
    show_replace: bool,
    options: TextOptions,
    _subscriptions: Vec<Subscription>,
}

/// What the background text search tells the UI.
enum TextMessage {
    Hits(Vec<TextHit>),
    Done(Result<usize, TextSearchError>),
}

struct Popup {
    input: Entity<InputState>,
    query: String,
    tab: Tab,
    files: Arc<Vec<FileHit>>,
    text: Arc<Vec<TextHit>>,
    text_status: String,
    /// Where the Text rows came from, beside the count (e.g. a language server).
    note: String,
    actions: Arc<Vec<ActionHit>>,
    /// Draws the Actions rows; the query, matching and selection stay Zide's.
    commands: Entity<CommandState>,
    symbols: Arc<Vec<SymbolHit>>,
    options: TextOptions,
    selected: usize,
    /// Text tab filters: file mask (`*.rs`) and directory (`crates/ui`).
    mask_input: Entity<InputState>,
    dir_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    fn len(&self) -> usize {
        match self.tab {
            Tab::Files => self.files.len(),
            Tab::Symbols => self.symbols.len(),
            Tab::Text => self.text.len(),
            Tab::Actions => self.actions.len(),
            Tab::Recent => self.files.len(),
        }
    }

    fn select(&mut self, row: usize) {
        self.selected = row;
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.len().saturating_sub(1);
        self.select(self.selected.saturating_add_signed(delta).min(last));
    }
}

/// What is known of the language server of a language.
enum LspState {
    Starting,
    Ready(Arc<Client>),
    /// It could not run (not installed, crashed on startup): the reason, to show once.
    Unavailable(String),
}

/// What the Markdown preview shows and where it was last scrolled to.
struct MdPreview {
    state: Entity<markdown::TextViewState>,
    /// The file and revision whose text `state` holds.
    path: String,
    revision: Revision,
    /// The editor's first visible line when the preview was last scrolled to follow it.
    synced_top: Option<usize>,
}

/// The user asked for "Update Zide"; the [`Shell`] owns updates, so the workspace only asks.
struct UpdateRequested;

impl EventEmitter<UpdateRequested> for Workspace {}

/// The user asked for "Open": the [`Shell`] asks for a folder and swaps the project in.
struct OpenRequested;

impl EventEmitter<OpenRequested> for Workspace {}

struct Workspace {
    focus: FocusHandle,
    /// Editor font size, changed with Ctrl+wheel and shared by every file.
    font_size: f32,
    /// When the last Space was pressed in Normal mode: two in a row open the popup.
    last_space: Option<Instant>,
    /// The frame-rate overlay, when started with `ZIDE_FPS=1`.
    meter: Option<meter::FrameMeter>,
    /// The project tree on the left.
    tree: Tree,
    tree_visible: bool,
    /// The checked-out branch and how far it is from its upstream, `None` outside a repository.
    branch: Option<zide_git::BranchStatus>,
    /// The rendered Markdown beside the editor (Ctrl+Shift+M), for `.md` files.
    md_preview: bool,
    md: Option<MdPreview>,
    /// Where the cursor was before each jump, for the mouse's back and forward buttons.
    nav: nav::NavHistory,
    /// Keys go to the tree instead of the editor.
    tree_focused: bool,
    /// A `g` was typed in the tree (`gg` goes to the top).
    tree_g: bool,
    /// The file and word whose usages were last asked of a language server.
    highlight_requested: Option<(String, HighlightKey)>,
    tree_scroll: UniformListScrollHandle,
    /// Language servers by `ServerSpec::name`, started the first time they are needed.
    lsp: std::collections::HashMap<&'static str, LspState>,
    root: PathBuf,
    /// `None` while the project is still being scanned.
    index: Option<Arc<FileIndex>>,
    popup: Option<Popup>,
    viewer: Option<Viewer>,
    /// Files opened earlier in the session, with their cursor, undo history and search.
    parked: std::collections::HashMap<String, Viewer>,
    /// The find / replace panel above the editor.
    find: Option<FindPanel>,
    /// Bumped on every search; results of an older search are dropped.
    search_generation: u64,
    cancel_search: Option<Arc<AtomicBool>>,
    /// Files opened from the popup (indices into `index`), most recent first.
    recent: Vec<usize>,
    /// Bumped on every scan; the result of an older scan is dropped.
    scan_generation: u64,
    changes_tx: async_channel::Sender<Change>,
    /// A language server published diagnostics (one pending wake-up is enough).
    diagnostics_tx: async_channel::Sender<()>,
    /// The file and text revision the language server was last sent, to diagnose.
    diagnosed: Option<(String, Revision)>,
    /// Bumped on every symbol indexing; the result of an older one is dropped.
    symbols_generation: u64,
    _watcher: Option<ProjectWatcher>,
    /// `None` until the symbols of the current index are parsed.
    symbols: Option<Arc<StubStore>>,
    registry: std::rc::Rc<Vec<ActionDef>>,
    action_texts: Vec<ActionText>,
    /// Where notifications are shown.
    window: AnyWindowHandle,
}

impl Workspace {
    fn new(root: PathBuf, window: &Window, cx: &mut Context<Self>) -> Self {
        // file-system changes arrive on a watcher thread; a foreground task reacts to them
        let (changes_tx, changes_rx) = async_channel::unbounded::<Change>();
        // the branch moves with commits, checkouts and fetches, none of which the watcher reports
        let git_root = root.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let root = git_root.clone();
                let status = cx
                    .background_executor()
                    .spawn(async move { zide_git::branch_status(&root) })
                    .await;
                let updated = this.update(cx, |this, cx| {
                    if this.branch != status {
                        this.branch = status;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(change) = changes_rx.recv().await {
                let reacted = this.update(cx, |this, cx| match change {
                    Change::Files => this.rescan(cx),
                    // same file list: the symbol indices stay valid and unchanged files are reused
                    Change::Contents => {
                        let previous = this.symbols.clone();
                        this.index_symbols(previous, cx);
                    }
                });
                if reacted.is_err() {
                    break;
                }
            }
        })
        .detach();

        let (diagnostics_tx, diagnostics_rx) = async_channel::bounded::<()>(1);
        cx.spawn(async move |this, cx| {
            while diagnostics_rx.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| this.apply_diagnostics(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let registry = action_registry();
        let mut workspace = Self {
            focus: cx.focus_handle(),
            font_size: viewer::DEFAULT_FONT_SIZE,
            last_space: None,
            meter: meter::FrameMeter::from_env(),
            tree: Tree::new(),
            tree_visible: false,
            branch: None,
            md_preview: false,
            md: None,
            nav: nav::NavHistory::default(),
            tree_focused: false,
            tree_g: false,
            highlight_requested: None,
            tree_scroll: UniformListScrollHandle::new(),
            lsp: std::collections::HashMap::new(),
            root,
            index: None,
            popup: None,
            viewer: None,
            parked: std::collections::HashMap::new(),
            find: None,
            search_generation: 0,
            cancel_search: None,
            recent: Vec::new(),
            scan_generation: 0,
            changes_tx,
            diagnostics_tx,
            diagnosed: None,
            symbols_generation: 0,
            _watcher: None,
            symbols: None,
            action_texts: registry
                .iter()
                .map(|def| ActionText {
                    name: def.name.to_string(),
                    synonyms: def.synonyms.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
            registry: std::rc::Rc::new(registry),
            window: window.window_handle(),
        };
        workspace.rescan(cx);
        if let Some(size) = session::font_size(&workspace.root) {
            workspace.font_size = size.clamp(viewer::MIN_FONT_SIZE, viewer::MAX_FONT_SIZE);
        }
        if let Some(last) = session::last_file(&workspace.root) {
            workspace.open_file(&last, None, cx);
        }
        workspace
    }

    /// Scans the project in the background and swaps the index in when done. Used at startup
    /// and whenever the watcher reports changes; only the latest scan is applied.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        let root = self.root.clone();
        let changes = self.changes_tx.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            // scan, then watch the directories of that scan (a new directory means another
            // rescan); adding thousands of watches must not block the UI thread either
            let (paths, dirs, watcher) = cx
                .background_executor()
                .spawn(async move {
                    let scan = scan_project(&root).unwrap_or_else(|e| {
                        eprintln!("scan failed: {e}");
                        Scan::default()
                    });
                    let watcher = watch_project(&root, &scan.dirs, move |change| {
                        let _ = changes.send_blocking(change);
                    });
                    (scan.files, scan.dirs, watcher)
                })
                .await;
            match &watcher {
                Ok(w) if w.unwatched > 0 => eprintln!("{} directories not watched", w.unwatched),
                Err(e) => eprintln!("watching failed: {e}"),
                _ => {}
            }
            eprintln!("indexed {} files in {:?}", paths.len(), started.elapsed());
            this.update(cx, |this, cx| {
                if this.scan_generation != generation {
                    return;
                }
                this.tree.set_entries(&paths, &dirs);
                if let Some(viewer) = &this.viewer {
                    this.tree.reveal(viewer.path()); // the file restored at startup
                }
                this.replace_index(FileIndex::new(paths));
                this._watcher = watcher.ok();
                // the old store indexes the old file list: keep it only to reuse its stubs
                let previous = this.symbols.take();
                this.index_symbols(previous, cx);
                this.search_again(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Parses the symbols of the current files in the background; stale results are dropped.
    fn index_symbols(&mut self, previous: Option<Arc<StubStore>>, cx: &mut Context<Self>) {
        let Some(index) = self.index.clone() else {
            return;
        };
        self.symbols_generation += 1;
        let generation = self.symbols_generation;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let symbols = cx
                .background_executor()
                .spawn(async move {
                    // reuse the running store, or else what the last run left on disk
                    let cache_path = StubCache::path_for(&root);
                    let cache = match &previous {
                        Some(previous) => Some(previous.cache()),
                        None => cache_path.as_deref().and_then(StubCache::load),
                    };
                    let store =
                        StubStore::build(index, &root, cache.as_ref(), &AtomicBool::new(false))?;
                    if let Some(path) = cache_path
                        && let Err(e) = store.save(&path)
                    {
                        eprintln!("saving the stub cache failed: {e}");
                    }
                    Some(store)
                })
                .await;
            let Some(symbols) = symbols else {
                return;
            };
            eprintln!(
                "indexed {} symbols in {:?} ({} files reused)",
                symbols.len(),
                started.elapsed(),
                symbols.reused()
            );
            this.update(cx, |this, cx| {
                if this.symbols_generation == generation {
                    this.symbols = Some(Arc::new(symbols));
                    this.search_again(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Swaps in a fresh index, keeping the recent files that still exist.
    fn replace_index(&mut self, index: FileIndex) {
        if let Some(old) = &self.index {
            let wanted: Vec<&String> = self.recent.iter().map(|&i| &old.paths()[i]).collect();
            let found: std::collections::HashMap<&str, usize> = index
                .paths()
                .iter()
                .enumerate()
                .filter(|(_, path)| wanted.contains(path))
                .map(|(i, path)| (path.as_str(), i))
                .collect();
            self.recent = wanted
                .iter()
                .filter_map(|path| found.get(path.as_str()).copied())
                .collect();
        }
        self.index = Some(Arc::new(index));
    }

    /// Saves the open file; called whenever the user's attention leaves the editor.
    fn autosave(&mut self, cx: &mut Context<Self>) {
        self.remember_view(); // it runs whenever the user leaves: another window, a popup, closing
        if self.viewer.as_mut().is_some_and(|viewer| viewer.save()) {
            self.refresh_git(cx);
        }
    }

    /// Keeps where the open file is scrolled to and its caret, for the next time it is opened.
    fn remember_view(&self) {
        if let Some(viewer) = &self.viewer
            && let Some(view) = viewer.view()
        {
            session::remember_view(&self.root, viewer.path(), view);
        }
    }

    /// Asks git, off the UI thread, who last changed each line of the open file and what the last
    /// commit had, to mark the lines that differ.
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.viewer.as_ref().map(|viewer| viewer.path().to_string()) else {
            return;
        };
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let asked = path.clone();
            let (blame, on_disk, head) = cx
                .background_executor()
                .spawn(async move {
                    // the text git blames: the file on disk
                    let on_disk = std::fs::read_to_string(root.join(&asked)).unwrap_or_default();
                    (
                        zide_git::blame(&root, &asked),
                        on_disk,
                        zide_git::head_content(&root, &asked),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                // The viewer may have moved on to another file meanwhile.
                if let Some(viewer) = this.viewer.as_mut().filter(|v| v.path() == path) {
                    viewer.set_blame(blame, on_disk);
                    viewer.set_head(head);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autosave(cx);
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().to_string();
                if let Some(popup) = &mut this.popup {
                    popup.query = query;
                    popup.note.clear();
                }
                this.search_again(cx);
            }
        });
        let mask_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("File mask, e.g. *.rs"));
        let dir_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Directory, e.g. crates/ui"));
        let filter_subscriptions: Vec<_> = [&mask_input, &dir_input]
            .into_iter()
            .map(|filter| {
                cx.subscribe(filter, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change)
                        && this.popup.as_ref().is_some_and(|p| p.tab == Tab::Text)
                    {
                        this.search_again(cx);
                    }
                })
            })
            .collect();
        input.update(cx, |state, cx| state.focus(window, cx));
        let commands = cx.new(|cx| CommandState::new(window, cx));
        self.popup = Some(Popup {
            input,
            query: String::new(),
            tab: Tab::Files,
            files: Arc::default(),
            text: Arc::default(),
            text_status: String::new(),
            note: String::new(),
            actions: Arc::default(),
            commands,
            symbols: Arc::default(),
            options: TextOptions::default(),
            selected: 0,
            mask_input,
            dir_input,
            _subscriptions: [subscription]
                .into_iter()
                .chain(filter_subscriptions)
                .collect(),
        });
        self.search_again(cx);
    }

    fn toggle_option(&mut self, toggle: fn(&mut TextOptions), cx: &mut Context<Self>) {
        let Some(popup) = &mut self.popup else {
            return self.toggle_find_option(toggle, cx);
        };
        if popup.tab != Tab::Text {
            return;
        }
        toggle(&mut popup.options);
        self.search_again(cx);
    }

    /// Opens the find panel (with the replace row when `replace`) for the open file.
    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        self.tree_focused = false; // the search box takes the keys
        let selected = viewer.selected_text();
        if self.popup.is_some() {
            self.close_popup(window, cx);
        }
        if self.find.is_none() {
            let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
            let replace_input = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
            let on_find = cx.subscribe(&find, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => this.run_find(cx),
                InputEvent::PressEnter { shift, .. } => this.find_step(!*shift, cx),
                _ => {}
            });
            let on_replace = cx.subscribe(&replace_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.replace_one(cx);
                }
            });
            self.find = Some(FindPanel {
                find,
                replace: replace_input,
                show_replace: false,
                options: TextOptions::default(),
                _subscriptions: vec![on_find, on_replace],
            });
        }
        if let Some(panel) = &mut self.find {
            panel.show_replace |= replace;
            if let Some(text) = selected {
                panel
                    .find
                    .update(cx, |state, cx| state.set_value(text, window, cx));
            }
            panel.find.update(cx, |state, cx| state.focus(window, cx));
        }
        self.run_find(cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.viewer {
            viewer.hide_find();
        }
        self.find = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Searches the open file for what the panel says.
    fn run_find(&mut self, cx: &mut Context<Self>) {
        let (Some(panel), Some(viewer)) = (&self.find, &mut self.viewer) else {
            return;
        };
        viewer.set_find(&FindQuery {
            pattern: panel.find.read(cx).value().to_string(),
            case_sensitive: panel.options.case_sensitive,
            whole_word: panel.options.whole_word,
            regex: panel.options.regex,
        });
        cx.notify();
    }

    /// F3 / Shift+F3: the next match of what is being searched. With the caret on a word that
    /// is not a match of the current search, that word is searched instead, without opening the
    /// panel. With neither, the panel opens.
    fn step_or_open_find(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let panel_open = self.find.is_some();
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        if viewer.has_find() && (panel_open || viewer.cursor_on_match()) {
            return self.find_step(forward, cx);
        }
        match viewer.word_to_search() {
            Some(word) => {
                viewer.search_word(&word, forward);
                cx.notify();
            }
            None if viewer.has_find() => self.find_step(forward, cx),
            None => self.open_find(false, window, cx),
        }
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.viewer {
            viewer.jump(forward, false);
            cx.notify();
        }
    }

    fn replace_one(&mut self, cx: &mut Context<Self>) {
        let (Some(panel), Some(viewer)) = (&self.find, &mut self.viewer) else {
            return;
        };
        viewer.replace_current(panel.replace.read(cx).value().as_ref());
        cx.notify();
    }

    fn replace_everywhere(&mut self, cx: &mut Context<Self>) {
        let (Some(panel), Some(viewer)) = (&self.find, &mut self.viewer) else {
            return;
        };
        viewer.replace_all(panel.replace.read(cx).value().as_ref());
        cx.notify();
    }

    fn toggle_find_option(&mut self, toggle: fn(&mut TextOptions), cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.find {
            toggle(&mut panel.options);
            self.run_find(cx);
        }
    }

    /// Opens the popup on `tab` (Go to File, Go to Symbol, Find Action, Find in Files).
    /// Alt+F7: every whole-word occurrence of the identifier under the cursor, in the Text tab.
    /// Textual for now: a language server will say which ones really are usages.
    fn find_usages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(word) = self.viewer.as_ref().and_then(Viewer::word_at_cursor) else {
            return;
        };
        if self.popup.is_none() {
            self.open_popup(window, cx);
        }
        let Some(popup) = &mut self.popup else {
            return;
        };
        popup.query = word.clone();
        popup.options = TextOptions {
            regex: false,
            case_sensitive: true,
            whole_word: true,
        };
        let input = popup.input.clone();
        input.update(cx, |state, cx| state.set_value(word.clone(), window, cx));
        self.show_tab(Tab::Text, cx);
        // the text matches above are shown at once; a language server refines them
        self.semantic_usages(word, cx);
    }

    /// The running language server of the open file, with the file's path and language.
    fn open_file_server(
        &self,
    ) -> Option<(
        &'static zide_lsp::ServerSpec,
        &'static str,
        PathBuf,
        Arc<Client>,
    )> {
        let relative = self.viewer.as_ref()?.path();
        let (spec, language) = server_for(relative)?;
        let Some(LspState::Ready(client)) = self.lsp.get(spec.name) else {
            return None;
        };
        Some((spec, language, self.root.join(relative), client.clone()))
    }

    /// Shows what the language server last published about the open file.
    fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some((_, _, path, client)) = self.open_file_server() else {
            return;
        };
        if let Some(viewer) = &mut self.viewer
            && viewer.set_diagnostics(client.diagnostics(&path))
        {
            cx.notify();
        }
    }

    /// Sends the open file's text to its language server, a moment after the last edit, so the
    /// server diagnoses it; the answer arrives as a publish (`apply_diagnostics`).
    fn request_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some((spec, language, path, client)) = self.open_file_server() else {
            return; // not started yet: tried again at the next frame
        };
        let Some(viewer) = &self.viewer else {
            return;
        };
        let current = (viewer.path().to_string(), viewer.revision());
        if self.diagnosed.as_ref() == Some(&current) {
            return;
        }
        let opened = self.diagnosed.as_ref().map(|(file, _)| file) != Some(&current.0);
        self.diagnosed = Some(current.clone());
        let text = viewer.text();
        let lines = text.lines().count() as u32 + 1;
        if opened {
            self.apply_diagnostics(cx); // what the server already said about this file
        }
        cx.spawn(async move |this, cx| {
            if !opened {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
            let wanted = this
                .update(cx, |this, _| this.diagnosed.as_ref() == Some(&current))
                .unwrap_or(false);
            if !wanted {
                return; // edited again while waiting: the newer text is sent instead
            }
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    client.sync(&path, language, &text)?;
                    client.pull_diagnostics(&path)?;
                    // a hint that fails to come is not worth losing the server for
                    let hints = client.parameter_hints(&path, lines).unwrap_or_default();
                    Ok::<_, LspError>(hints)
                })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Err(LspError::Closed | LspError::Io(_)) => {
                        this.lsp.remove(spec.name);
                        this.diagnosed = None; // a new server has not been sent the file
                    }
                    Err(_) => {}
                    Ok(hints) => {
                        // what a server that does not publish answered
                        this.apply_diagnostics(cx);
                        if let Some(viewer) = this.viewer.as_mut().filter(|v| v.path() == current.0)
                            && viewer.set_hints(hints, current.1)
                        {
                            cx.notify();
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks the language server where the word under the caret is used, once the caret rests on
    /// it for a moment; until the answer arrives the viewer highlights by text.
    fn request_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        let Some(target) = viewer.highlight_target() else {
            self.highlight_requested = None;
            return;
        };
        let relative = viewer.path().to_string();
        let Some((spec, language)) = server_for(&relative) else {
            return;
        };
        let Some(LspState::Ready(client)) = self.lsp.get(spec.name) else {
            return; // not started yet: ask again at the next frame
        };
        let request = (relative, target.key);
        if self.highlight_requested.as_ref() == Some(&request) {
            return;
        }
        self.highlight_requested = Some(request.clone());
        let client = client.clone();
        let text = viewer.text();
        let path = self.root.join(&request.0);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let wanted = this
                .update(cx, |this, _| {
                    this.highlight_requested.as_ref() == Some(&request)
                })
                .unwrap_or(false);
            if !wanted {
                return; // the caret moved on while waiting
            }
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    client.sync(&path, language, &text)?;
                    client.document_highlight(&path, target.line, target.character)
                })
                .await;
            this.update(cx, |this, cx| {
                if matches!(outcome, Err(LspError::Closed | LspError::Io(_))) {
                    this.lsp.remove(spec.name);
                }
                if this.highlight_requested.as_ref() != Some(&request) {
                    return;
                }
                if let (Ok(ranges), Some(viewer)) = (
                    outcome,
                    this.viewer.as_mut().filter(|v| v.path() == request.0),
                ) {
                    viewer.set_semantic(request.1, ranges);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Ctrl+W / Ctrl+Shift+W in the editor.
    fn change_selection(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.popup.is_some() || self.tree_focused {
            return;
        }
        if let Some(viewer) = &mut self.viewer {
            if extend {
                viewer.extend_selection();
            } else {
                viewer.shrink_selection();
            }
            cx.notify();
        }
    }

    /// Alt+Down / Alt+Up in the editor.
    /// Ctrl+minus / plus (Ctrl+Shift for all of them): `command` is a `z` fold command.
    fn fold(&mut self, command: char, cx: &mut Context<Self>) {
        if self.popup.is_some() || self.tree_focused {
            cx.propagate();
            return;
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.fold_command(command);
            cx.notify();
        }
    }

    fn jump_to_method(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.popup.is_some() || self.tree_focused {
            cx.propagate(); // the popup and the tree move their selection with Down / Up
            return;
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.next_method(forward);
            cx.notify();
        }
    }

    /// Shows `text` in the editor's status line (when a file is open).
    fn set_notice(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.viewer {
            viewer.set_notice(Some(text));
        }
        cx.notify();
    }

    fn notify(&self, notification: Notification, cx: &mut Context<Self>) {
        self.window
            .update(cx, |_, window, cx| {
                window.push_notification(notification.placement(Anchor::BottomRight), cx)
            })
            .ok();
    }

    /// Reformat Code (IntelliJ's Ctrl+Alt+L): the Visual selection if there is one, else the
    /// whole file, as its language server formats it.
    fn reformat(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        let relative = viewer.path().to_string();
        let Some((spec, language)) = server_for(&relative) else {
            viewer.set_notice(Some("no formatter for this file".into()));
            return cx.notify();
        };
        let Some(request) = viewer.format_request() else {
            return;
        };
        let client = match self.lsp.get(spec.name) {
            Some(LspState::Ready(client)) => client.clone(),
            Some(LspState::Unavailable(why)) => {
                viewer.set_notice(Some(format!("not formatted: {why}")));
                return cx.notify();
            }
            Some(LspState::Starting) | None => {
                viewer.set_notice(Some("not formatted: the formatter is starting".into()));
                self.warm_server(&relative, cx);
                return cx.notify();
            }
        };
        let path = self.root.join(&relative);
        cx.spawn(async move |this, cx| {
            let FormatRequest {
                revision,
                text,
                range,
            } = request;
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    client.sync(&path, language, &text)?;
                    client.format(&path, range)
                })
                .await;
            this.update(cx, |this, cx| {
                if matches!(outcome, Err(LspError::Closed | LspError::Io(_))) {
                    this.lsp.remove(spec.name);
                }
                let Some(viewer) = this.viewer.as_mut().filter(|v| v.path() == relative) else {
                    return;
                };
                match outcome {
                    Ok(edits) => {
                        // false: typed meanwhile, the edits no longer fit
                        if viewer.apply_format(revision, edits) {
                            viewer.set_notice(None);
                        }
                    }
                    Err(e) => viewer.set_notice(Some(format!("not formatted: {e}"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts the language server of `path`'s language in the background, once, so it has
    /// loaded the project by the time it is asked something.
    fn warm_server(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some((spec, _)) = server_for(path) else {
            return;
        };
        if self.lsp.contains_key(spec.name) {
            return;
        }
        self.lsp.insert(spec.name, LspState::Starting);
        let root = self.root.clone();
        let published = self.diagnostics_tx.clone();
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    Client::start(spec, &root, move || {
                        let _ = published.try_send(());
                    })
                    .map(Arc::new)
                })
                .await;
            this.update(cx, |this, cx| {
                match started {
                    Ok(client) => {
                        this.lsp.insert(spec.name, LspState::Ready(client));
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", spec.command);
                        this.lsp
                            .insert(spec.name, LspState::Unavailable(setup_hint(spec, &e)));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the language server of the open file for the real usages of the symbol under the
    /// cursor and swaps them in for the text matches when they arrive.
    fn semantic_usages(&mut self, word: String, cx: &mut Context<Self>) {
        let (Some(viewer), Some(index)) = (&self.viewer, self.index.clone()) else {
            return;
        };
        let Some((spec, language)) = server_for(viewer.path()) else {
            return; // no server for this language: the text matches stand
        };
        let client = match self.lsp.get(spec.name) {
            Some(LspState::Ready(client)) => client.clone(),
            Some(LspState::Unavailable(why)) => {
                return self.set_usages_note(&word, format!("{why} · text matches"), cx);
            }
            Some(LspState::Starting) | None => {
                let path = viewer.path().to_string();
                self.warm_server(&path, cx); // it died, or was never asked for: start it
                let note = format!("{} is starting · text matches", spec.command);
                return self.set_usages_note(&word, note, cx);
            }
        };
        let label = client.label().to_string();
        let (line, character) = viewer.lsp_position();
        let text = viewer.text();
        let path = self.root.join(viewer.path());
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    client.sync(&path, language, &text)?;
                    let found = client.references(&path, line, character)?;
                    Ok::<_, LspError>(usages::hits(&root, &index, &found))
                })
                .await;
            eprintln!("usages from {label} in {:?}", started.elapsed());
            this.update(cx, |this, cx| {
                // a dead server is started anew by the next request
                if matches!(outcome, Err(LspError::Closed | LspError::Io(_))) {
                    this.lsp.remove(spec.name);
                }
                this.apply_usages(&word, &label, outcome, cx);
            })
            .ok();
        })
        .detach();
    }

    fn set_usages_note(&mut self, word: &str, note: String, cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.popup
            && popup.tab == Tab::Text
            && popup.query == word
        {
            popup.note = note;
            cx.notify();
        }
    }

    /// Shows what the language server found, unless the popup moved on to another query.
    fn apply_usages(
        &mut self,
        word: &str,
        command: &str,
        result: Result<Vec<TextHit>, LspError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(hits) if !hits.is_empty() => {
                if !self
                    .popup
                    .as_ref()
                    .is_some_and(|p| p.tab == Tab::Text && p.query == word)
                {
                    return;
                }
                self.cancel_running_search(); // the text matches no longer matter
                if let Some(popup) = &mut self.popup {
                    popup.text_status = format!("{} usages", hits.len());
                    popup.note = format!("by {command}");
                    popup.text = Arc::new(hits);
                    popup.select(0);
                }
                cx.notify();
            }
            Ok(_) => self.set_usages_note(word, format!("{command} found none · text matches"), cx),
            Err(e) => self.set_usages_note(word, format!("{e} · text matches"), cx),
        }
    }

    /// `gd` / Ctrl+B: jumps to the definition of the word under the cursor. The language server
    /// answers when it can; otherwise the symbol index does, by name, and the status line says so.
    fn go_to_definition(&mut self, cx: &mut Context<Self>) {
        let (Some(viewer), Some(index)) = (&self.viewer, self.index.clone()) else {
            return;
        };
        let Some(word) = viewer.word_at_cursor() else {
            return self.set_notice(
                "go to definition: no identifier under the cursor".into(),
                cx,
            );
        };
        let Some((spec, language)) = server_for(viewer.path()) else {
            let why = "there is no language server for this file".to_string();
            return self.definition_by_name(&word, &why, cx);
        };
        let client = match self.lsp.get(spec.name) {
            Some(LspState::Ready(client)) => client.clone(),
            Some(LspState::Unavailable(why)) => {
                let why = why.clone();
                return self.definition_by_name(&word, &why, cx);
            }
            Some(LspState::Starting) | None => {
                let path = viewer.path().to_string();
                self.warm_server(&path, cx);
                let why = format!("{} is starting", spec.command);
                return self.definition_by_name(&word, &why, cx);
            }
        };
        let label = client.label().to_string();
        let (line, character) = viewer.lsp_position();
        let text = viewer.text();
        let path = self.root.join(viewer.path());
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    client.sync(&path, language, &text)?;
                    let found = client.definition(&path, line, character)?;
                    Ok::<_, LspError>(definition::in_project(&root, &index, &found))
                })
                .await;
            this.update(cx, |this, cx| {
                // a dead server is started anew by the next request
                if matches!(outcome, Err(LspError::Closed | LspError::Io(_))) {
                    this.lsp.remove(spec.name);
                }
                match outcome {
                    Ok(targets) if !targets.is_empty() => this.jump_to(&targets, None, cx),
                    Ok(_) => {
                        let why = format!("{label} found no definition in the project");
                        this.definition_by_name(&word, &why, cx);
                    }
                    Err(e) => this.definition_by_name(&word, &e.to_string(), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// The fallback of Go to Definition: the declarations of the symbol index named `word`.
    /// `why` is what kept the language server from answering.
    fn definition_by_name(&mut self, word: &str, why: &str, cx: &mut Context<Self>) {
        let (Some(index), Some(symbols), Some(viewer)) =
            (self.index.clone(), self.symbols.clone(), &self.viewer)
        else {
            let note = format!("go to definition: {why} · the symbol index is not ready yet");
            return self.set_notice(note, cx);
        };
        let targets = definition::by_name(&self.root, &index, &symbols, word, viewer.path());
        if targets.is_empty() {
            let note = format!(
                "go to definition: {why} · no declaration named `{word}` in the symbol index"
            );
            return self.set_notice(note, cx);
        }
        let via = format!("{why} · jumped by name in the symbol index, which may be approximate");
        self.jump_to(&targets, Some(via), cx);
    }

    /// Moves the cursor to the first of `targets`, recording where it was for Back. `fallback`
    /// says why the language server did not answer, when it did not.
    fn jump_to(
        &mut self,
        targets: &[definition::Target],
        fallback: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = targets.first() else {
            return;
        };
        self.record_jump();
        if self.viewer.as_ref().is_none_or(|v| v.path() != target.path) {
            self.show_file(&target.path, None, cx);
        }
        let mut note = fallback.map(|why| format!("go to definition: {why}"));
        if targets.len() > 1 {
            let several = format!("{} definitions, showing the first", targets.len());
            note = Some(match note {
                Some(note) => format!("{note} · {several}"),
                None => several,
            });
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.go_to_spot(target.line, target.column);
            viewer.set_notice(note);
        }
        cx.notify();
    }

    fn open_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() {
            self.open_popup(window, cx);
        }
        self.show_tab(tab, cx);
    }

    /// A key typed while no popup is open: the find panel or the editor gets it.
    fn scroll_tree_to_selection(&self) {
        if let Some(row) = self.tree.selected() {
            self.tree_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
            self.scroll_tree_sideways(row);
        }
    }

    /// A row whose name is cut off on the right scrolls sideways so its icon sits at the left
    /// edge; a row that fits shows the tree from the left again. Text width is estimated.
    fn scroll_tree_sideways(&self, row: usize) {
        const CHAR: f32 = 7.5; // approximate advance of one character
        const ICON_AT: f32 = 24.; // chevron column + gap, after the indent
        let rows = self.tree.shared_rows();
        let Some(row) = rows.get(row) else { return };
        let handle = self.tree_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.width);
        let viewport = if viewport > 0. { viewport } else { 260. };
        let icon_x = ICON_AT + 14. * row.depth as f32;
        let right = icon_x + 20. + CHAR * row.name.chars().count() as f32 + 8.;
        let max = f32::from(handle.max_offset().x);
        let x = if right > viewport {
            -icon_x.min(max)
        } else {
            0.
        };
        let mut offset = handle.offset();
        offset.x = px(x);
        handle.set_offset(offset);
    }

    /// Alt+1: shows the tree and focuses it; from the tree, hides it again (like IntelliJ).
    fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        match (self.tree_visible, self.tree_focused) {
            (true, true) => {
                self.tree_visible = false;
                self.tree_focused = false;
            }
            _ => {
                self.tree_visible = true;
                self.tree_focused = true;
                self.scroll_tree_to_selection();
            }
        }
        cx.notify();
    }

    /// Ctrl+Shift+M: shows or hides the rendered Markdown beside the editor.
    fn toggle_md_preview(&mut self, cx: &mut Context<Self>) {
        self.md_preview = !self.md_preview;
        cx.notify();
    }

    /// The preview of the open file, when it is Markdown and the preview is on. The text goes
    /// from the buffer to the preview only when its revision changed, and the preview follows
    /// the editor's scroll.
    fn markdown_preview(&mut self, cx: &mut Context<Self>) -> Option<markdown::TextView> {
        let viewer = self
            .viewer
            .as_ref()
            .filter(|v| markdown::is_markdown(v.path()))?;
        if !self.md_preview {
            return None;
        }
        let (path, revision) = (viewer.path(), viewer.revision());
        let md = match &mut self.md {
            Some(md) => {
                if md.path != path || md.revision != revision {
                    let text = viewer.text();
                    md.state.update(cx, |state, cx| state.set_text(&text, cx));
                    md.path = path.to_string();
                    md.revision = revision;
                    md.synced_top = None;
                }
                md
            }
            None => self.md.insert(MdPreview {
                state: cx.new(|cx| markdown::TextViewState::markdown(&viewer.text(), cx)),
                path: path.to_string(),
                revision,
                synced_top: None,
            }),
        };
        if let Some((top, bottom)) = viewer.visible_lines()
            && md.synced_top != Some(top)
        {
            // Revealing only scrolls as far as needed to show the line: going down, that is the
            // line at the bottom edge, so aim at the editor's last line to keep both aligned.
            let down = md.synced_top.is_some_and(|previous| top > previous);
            let lines: Vec<usize> = if down {
                (top..=bottom).rev().collect()
            } else {
                (top..=bottom).collect()
            };
            md.synced_top = Some(top);
            for range in lines
                .into_iter()
                .filter_map(|line| viewer.line_source_range(line))
            {
                if md
                    .state
                    .update(cx, |state, cx| state.reveal_range(range, cx).is_ok())
                {
                    break;
                }
            }
        }
        let state = md.state.clone();
        let workspace = cx.entity();
        Some(markdown::preview(
            &state,
            self.root.clone(),
            path.to_string(),
            move |path, cx| {
                workspace.update(cx, |this, cx| this.open_link(&path, cx));
            },
        ))
    }

    /// A click on a link of the Markdown preview: opens the project file it names, if any.
    fn open_link(&mut self, path: &str, cx: &mut Context<Self>) {
        let known = self
            .index
            .as_ref()
            .is_some_and(|index| index.paths().iter().any(|p| p == path));
        if known {
            self.open_file(path, None, cx);
            self.tree_focused = false;
            cx.notify();
        }
    }

    /// Ctrl+H / Ctrl+L: moves the focus to the project tree (showing it if hidden) or back to
    /// the editor.
    fn focus_tree(&mut self, tree: bool, cx: &mut Context<Self>) {
        if self.popup.is_some() || (!tree && self.viewer.is_none()) {
            return;
        }
        if tree {
            self.tree_visible = true;
            self.scroll_tree_to_selection();
        }
        self.tree_focused = tree;
        cx.notify();
    }

    /// Keys while the tree has the focus: Vim motions, arrows and Enter.
    fn tree_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        use zide_vim::Key;
        let pending_g = std::mem::take(&mut self.tree_g);
        let outcome = match (event.keystroke.key.as_str(), vim_key(event)) {
            ("down", _) | (_, Some(Key::Char('j'))) => {
                self.tree.move_by(1);
                TreeOutcome::None
            }
            ("up", _) | (_, Some(Key::Char('k'))) => {
                self.tree.move_by(-1);
                TreeOutcome::None
            }
            ("right", _) | (_, Some(Key::Char('l'))) => self.tree.expand(),
            ("left", _) | (_, Some(Key::Char('h'))) => {
                self.tree.collapse();
                TreeOutcome::None
            }
            (_, Some(Key::Ctrl(c @ ('d' | 'u')))) => {
                self.tree.move_half_page(self.tree_visible_rows(), c == 'd');
                TreeOutcome::None
            }
            (_, Some(Key::Enter)) => self.tree.activate(),
            (_, Some(Key::Char('G'))) => {
                self.tree.select_last();
                TreeOutcome::None
            }
            (_, Some(Key::Char('g'))) => {
                if pending_g {
                    self.tree.select_first();
                } else {
                    self.tree_g = true;
                }
                TreeOutcome::None
            }
            (_, Some(Key::Esc)) => {
                self.tree_focused = false;
                TreeOutcome::None
            }
            _ => TreeOutcome::None,
        };
        self.apply_tree_outcome(outcome, cx);
    }

    /// How many tree rows fit on screen, as of the last layout.
    fn tree_visible_rows(&self) -> usize {
        self.tree_scroll
            .0
            .borrow()
            .last_item_size
            .map_or(0, |size| {
                (f32::from(size.item.height) / TREE_ROW_HEIGHT) as usize
            })
    }

    fn apply_tree_outcome(&mut self, outcome: TreeOutcome, cx: &mut Context<Self>) {
        if let TreeOutcome::Open(path) = outcome {
            self.open_file(&path, None, cx);
            self.tree_focused = false; // the file is for editing
        }
        self.scroll_tree_to_selection();
        cx.notify();
    }

    fn project_tree(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let rows = self.tree.shared_rows();
        let selected = self.tree.selected();
        let focused = self.tree_focused;
        let workspace = cx.entity();
        // the widest row sets the scrollable width (an indent step is about two characters)
        let widest = rows
            .iter()
            .enumerate()
            .max_by_key(|(_, row)| row.depth * 2 + row.name.chars().count())
            .map(|(index, _)| index);
        let list = uniform_list("project-tree", rows.len(), move |range, _, _| {
            range
                .map(|index| {
                    let row = &rows[index];
                    let workspace = workspace.clone();
                    let background = match (selected == Some(index), focused) {
                        (true, true) => Some(theme::TREE_FOCUSED),
                        (true, false) => Some(theme::TREE_SELECTED),
                        _ => None,
                    };
                    let line = div()
                        .id(("tree-row", index))
                        .w_full()
                        .h(px(TREE_ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(8. + 14. * row.depth as f32))
                        .pr(px(16.))
                        .whitespace_nowrap()
                        .child(div().w(px(12.)).flex_shrink_0().child(
                            match (row.dir, row.expanded) {
                                (false, _) => "",
                                (true, true) => "▾",
                                (true, false) => "▸",
                            },
                        ))
                        .child({
                            let (icon, color) =
                                file_icon::for_row(&row.name, row.dir, row.expanded);
                            Icon::new(icon)
                                .size_4()
                                .flex_shrink_0()
                                .text_color(rgb(color))
                        })
                        .child(div().flex_shrink_0().child(row.name.clone()))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            workspace.update(cx, |this, cx| {
                                this.tree_focused = true;
                                let outcome = this.tree.click(index);
                                this.apply_tree_outcome(outcome, cx);
                            });
                        });
                    match background {
                        Some(color) => line.bg(rgb(color)),
                        None => line,
                    }
                })
                .collect()
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(widest)
        .track_scroll(&self.tree_scroll)
        .size_full();
        div()
            .id("project-tree")
            .flex_shrink_0()
            .w(px(260.))
            .h_full()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .relative()
                    .size_full()
                    .child(list)
                    .child(Scrollbar::new(&self.tree_scroll)),
            )
    }

    fn editor_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let in_panel = self.find.as_ref().is_some_and(|panel| {
            [&panel.find, &panel.replace]
                .iter()
                .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
        });
        if in_panel {
            if event.keystroke.key == "escape" {
                self.close_find(window, cx);
                cx.stop_propagation();
            }
            return; // the input handles its own keys
        }
        let m = event.keystroke.modifiers;
        let plain = !(m.control || m.alt || m.platform || m.shift);
        let in_tree = self.tree_focused && self.tree_visible;
        let idle = in_tree || self.viewer.as_ref().is_none_or(Viewer::awaits_command);
        if event.keystroke.key == "space" && plain && idle {
            let now = Instant::now();
            let double = self
                .last_space
                .take()
                .is_some_and(|t| now.duration_since(t) <= DoubleShiftDetector::DEFAULT_INTERVAL);
            if double {
                self.open_popup(window, cx);
            } else {
                self.last_space = Some(now);
            }
            cx.stop_propagation();
            return;
        }
        self.last_space = None;
        if in_tree {
            self.tree_key(event, cx);
            cx.stop_propagation();
            return;
        }
        let (Some(viewer), Some(key)) = (&mut self.viewer, vim_key(event)) else {
            return;
        };
        let effect = viewer.handle_key(key);
        if key == zide_vim::Key::Esc && self.find.is_some() {
            self.close_find(window, cx);
        }
        if effect == Effect::OpenFind {
            self.open_find(false, window, cx);
        }
        if effect == Effect::GoToDefinition {
            self.go_to_definition(cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn switch_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.popup {
            let next = popup.tab.next();
            self.show_tab(next, cx);
        }
    }

    fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.popup {
            popup.tab = tab;
            popup.selected = 0;
        }
        self.search_again(cx);
    }

    /// Flips a text option from the Actions tab: shows the Text tab and clears the query typed
    /// to find the action, which would otherwise become the text pattern.
    fn toggle_text_option(&mut self, toggle: fn(&mut TextOptions), cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.popup {
            popup.tab = Tab::Text;
            popup.selected = 0;
        }
        self.toggle_option(toggle, cx);
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.as_ref().is_some_and(|p| p.tab == Tab::Actions) {
            return self.run_selected_action(window, cx);
        }
        let (Some(popup), Some(index)) = (&self.popup, &self.index) else {
            return;
        };
        let target = match popup.tab {
            Tab::Files | Tab::Recent => popup
                .files
                .get(popup.selected)
                .map(|hit| (hit.index, FileQuery::parse(&popup.query).line)),
            Tab::Text => popup
                .text
                .get(popup.selected)
                .map(|hit| (hit.file, Some(hit.line as u32))),
            Tab::Symbols => popup.symbols.get(popup.selected).and_then(|hit| {
                let symbol = self.symbols.as_ref()?.symbol(hit.entry);
                Some((symbol.file, Some(symbol.node.line)))
            }),
            Tab::Actions => None,
        };
        if let Some((file, line)) = target {
            let path = index.paths()[file].clone();
            self.open_file(&path, line, cx);
            self.close_popup(window, cx);
        }
    }

    /// Shows `path`, parking the open file with its cursor and undo history. A parked file
    /// comes back as it was, unless it changed on disk in the meantime.
    fn open_file(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        // opening the file already shown is no jump, unless it goes to a line
        if self
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.path() != path || line.is_some())
        {
            self.record_jump();
        }
        self.show_file(path, line, cx);
    }

    /// The place of the cursor in the open file.
    fn current_place(&self) -> Option<nav::Place> {
        let viewer = self.viewer.as_ref()?;
        let (line, column) = viewer.cursor_spot();
        Some(nav::Place {
            path: viewer.path().to_string(),
            line,
            column,
        })
    }

    /// The cursor is about to jump: Back will return to where it is now.
    fn record_jump(&mut self) {
        if let Some(place) = self.current_place() {
            self.nav.record(place);
        }
    }

    /// The mouse's back and forward buttons: moves the cursor to the previous or next place of
    /// the history, opening its file if needed. Places in files that no longer exist are passed.
    fn navigate(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            return;
        }
        let Some(current) = self.current_place() else {
            return;
        };
        loop {
            let step = if back {
                self.nav.back(current.clone())
            } else {
                self.nav.forward(current.clone())
            };
            let Some(place) = step else {
                return;
            };
            if place.path != current.path && !self.root.join(&place.path).is_file() {
                continue;
            }
            if place.path != current.path {
                self.show_file(&place.path, None, cx);
            }
            if let Some(viewer) = &mut self.viewer {
                viewer.go_to_spot(place.line, place.column);
            }
            cx.notify();
            return;
        }
    }

    /// Shows `path` without touching the navigation history.
    fn show_file(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        self.autosave(cx);
        self.warm_server(path, cx);
        // every way of opening a file (popup, tree) counts as a recent one
        if let Some(file) = self
            .index
            .as_ref()
            .and_then(|index| index.paths().iter().position(|p| p == path))
        {
            self.recent.retain(|&i| i != file);
            self.recent.insert(0, file);
            self.recent.truncate(MAX_RECENT);
        }
        self.remember_view();
        if let Some(old) = self.viewer.take() {
            self.parked.insert(old.path().to_string(), old);
            if self.parked.len() > MAX_RECENT
                && let Some(oldest) = self.parked.keys().find(|k| k.as_str() != path).cloned()
            {
                self.parked.remove(&oldest);
            }
        }
        let (mut viewer, reopened) = match self.parked.remove(path) {
            Some(parked) if !parked.is_stale() => (parked, false),
            _ => (Viewer::open(&self.root, path), true),
        };
        if let Some(line) = line {
            viewer.go_to_line(line);
        }
        viewer.set_font_size(self.font_size);
        // a file read again starts where it was left, unless it was opened at a line
        if line.is_none()
            && reopened
            && let Some(view) = session::view(&self.root, path)
        {
            viewer.restore_view(view);
        }
        let large = reopened && viewer.is_plain();
        self.viewer = Some(viewer);
        if large {
            self.notify(
                Notification::warning(format!(
                    "This file has more than {} lines. Syntax highlighting, folding and the \
                     outline are off to keep editing fast.",
                    viewer::SYNTAX_LINE_LIMIT
                ))
                .title("Large file"),
                cx,
            );
        }
        self.refresh_git(cx);
        session::remember_file(&self.root, path);
        self.tree.reveal(path);
        self.tree_focused = false; // a file was opened: the keys are the editor's
        self.scroll_tree_to_selection();
        if self.find.is_some() {
            self.run_find(cx); // the panel stays open: search the new file
        }
    }

    /// Ctrl+wheel: a notch is a point of font size; a touchpad scrolls by pixels, so it
    /// zooms in proportion.
    fn zoom(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if dy == 0. {
            return;
        }
        let step = if event.delta.precise() {
            dy / 40.
        } else {
            dy.signum()
        };
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        self.font_size =
            (self.font_size + step).clamp(viewer::MIN_FONT_SIZE, viewer::MAX_FONT_SIZE);
        viewer.set_font_size(self.font_size);
        session::remember_font_size(&self.root, self.font_size);
        cx.notify();
    }

    fn run_selected_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        let Some(hit) = popup.actions.get(popup.selected) else {
            return;
        };
        let (run, closes_popup) = {
            let def = &self.registry[hit.index];
            (def.run, def.closes_popup)
        };
        if closes_popup {
            self.close_popup(window, cx);
        } else {
            // the query was typed to find the action, not to search with
            popup.query.clear();
            let input = popup.input.clone();
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        run(self, window, cx);
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_running_search();
        self.popup = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_running_search(&mut self) {
        if let Some(cancel) = self.cancel_search.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.search_generation += 1;
    }

    /// Searches again for the popup's current query and tab, on background threads.
    /// Typing again cancels the running search and drops its results, so the UI thread
    /// never waits and never shows stale hits.
    fn search_again(&mut self, cx: &mut Context<Self>) {
        self.cancel_running_search();
        let (Some(popup), Some(index)) = (&mut self.popup, self.index.clone()) else {
            cx.notify();
            return;
        };
        let query = popup.query.clone();
        let generation = self.search_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_search = Some(cancel.clone());

        match popup.tab {
            Tab::Files if query.trim().is_empty() => {
                // nothing typed yet: recent files first, then the first files
                popup.files = Arc::new(
                    self.recent
                        .iter()
                        .copied()
                        .chain((0..index.len()).filter(|i| !self.recent.contains(i)))
                        .take(MAX_FILE_RESULTS)
                        .map(|index| FileHit {
                            index,
                            score: 0,
                            fragments: Vec::new(),
                        })
                        .collect(),
                );
                popup.select(0);
                cx.notify();
            }
            Tab::Recent => {
                // at most MAX_RECENT files: matched here, synchronously, in a small index of their own
                let paths = self
                    .recent
                    .iter()
                    .map(|&i| index.paths()[i].clone())
                    .collect();
                let recent_index = FileIndex::new(paths);
                let hits = if query.trim().is_empty() {
                    (0..recent_index.len())
                        .map(|index| FileHit {
                            index,
                            score: 0,
                            fragments: Vec::new(),
                        })
                        .collect()
                } else {
                    search_files(&recent_index, &FileQuery::parse(&query), MAX_RECENT)
                };
                popup.files = Arc::new(
                    hits.into_iter()
                        .map(|hit| FileHit {
                            index: self.recent[hit.index],
                            ..hit
                        })
                        .collect(),
                );
                // like IntelliJ: the first row is the open file, so Enter goes back to the one
                // before. Decided here, not in `show_tab`, because background work (indexing)
                // searches again later and would reset the selection to the first row.
                let keep = if query.trim().is_empty() { 1 } else { 0 };
                popup.select(keep.min(popup.len().saturating_sub(1)));
                cx.notify();
            }
            Tab::Files => {
                let recent = self.recent.clone();
                self.search_files(query, recent, index, generation, cancel, cx)
            }
            Tab::Symbols => {
                popup.symbols = Arc::default();
                popup.select(0);
                cx.notify();
                if let (false, Some(symbols)) = (query.trim().is_empty(), self.symbols.clone()) {
                    self.search_symbols(query, symbols, generation, cancel, cx);
                }
            }
            Tab::Actions => {
                popup.actions = Arc::new(search_actions(&self.action_texts, &query, usize::MAX));
                popup.select(0);
                cx.notify();
            }
            Tab::Text => {
                popup.text = Arc::default();
                popup.text_status = String::new();
                popup.select(0);
                cx.notify();
                if !query.is_empty() {
                    let non_empty = |input: &Entity<InputState>| {
                        let value = input.read(cx).value().trim().to_string();
                        (!value.is_empty()).then_some(value)
                    };
                    let text_query = TextQuery {
                        pattern: query,
                        regex: popup.options.regex,
                        case_sensitive: popup.options.case_sensitive,
                        whole_word: popup.options.whole_word,
                        file_mask: non_empty(&popup.mask_input),
                        directory: non_empty(&popup.dir_input),
                    };
                    self.search_text(text_query, index, generation, cancel, cx);
                }
            }
        }
    }

    fn search_symbols(
        &mut self,
        query: String,
        symbols: Arc<StubStore>,
        generation: u64,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let label = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { search_symbols(&symbols, &query, MAX_SYMBOL_RESULTS, &cancel) })
                .await;
            let Some(hits) = hits else {
                return; // cancelled by a newer search
            };
            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                eprintln!(
                    "symbols {label:?}: {} hits in {:?}",
                    hits.len(),
                    started.elapsed()
                );
                if let Some(popup) = &mut this.popup {
                    popup.symbols = Arc::new(hits);
                    popup.select(0);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn search_files(
        &mut self,
        query: String,
        recent: Vec<usize>,
        index: Arc<FileIndex>,
        generation: u64,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let label = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move {
                    let query = FileQuery::parse(&query);
                    search_files_with_recent(&index, &query, MAX_FILE_RESULTS, &cancel, &recent)
                })
                .await;
            let Some(hits) = hits else {
                return; // cancelled by a newer search
            };
            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                eprintln!(
                    "files {label:?}: {} hits in {:?}",
                    hits.len(),
                    started.elapsed()
                );
                if let Some(popup) = &mut this.popup {
                    popup.files = Arc::new(hits);
                    popup.select(0);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Streams hits: the background thread sends them in batches and the UI appends them
    /// as they arrive.
    fn search_text(
        &mut self,
        query: TextQuery,
        index: Arc<FileIndex>,
        generation: u64,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        let (tx, rx) = async_channel::unbounded::<TextMessage>();
        let root = self.root.clone();
        let label = query.pattern.clone();
        cx.background_executor()
            .spawn(async move {
                let result =
                    search_text(&index, &root, &query, MAX_TEXT_RESULTS, &cancel, |hits| {
                        let _ = tx.send_blocking(TextMessage::Hits(hits));
                    });
                let _ = tx.send_blocking(TextMessage::Done(result));
            })
            .detach();

        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let mut first_batch = true;
            while let Ok(message) = rx.recv().await {
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.search_generation != generation {
                            return false;
                        }
                        let Some(popup) = &mut this.popup else {
                            return false;
                        };
                        match message {
                            TextMessage::Hits(hits) => {
                                if first_batch {
                                    eprintln!(
                                        "text {label:?}: first batch after {:?}",
                                        started.elapsed()
                                    );
                                    first_batch = false;
                                }
                                Arc::make_mut(&mut popup.text).extend(hits);
                                popup.text_status =
                                    format!("{} hits · searching…", popup.text.len());
                            }
                            TextMessage::Done(Ok(total)) => {
                                eprintln!(
                                    "text {label:?}: {total} hits in {:?}",
                                    started.elapsed()
                                );
                                popup.text_status = format!("{total} hits");
                            }
                            TextMessage::Done(Err(e)) => popup.text_status = e.to_string(),
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn status(&self, popup: &Popup) -> String {
        match (&self.index, popup.tab) {
            (_, Tab::Actions) => format!("{} actions", popup.actions.len()),
            (_, Tab::Recent) => format!("{} recent files", popup.files.len()),
            (None, _) => format!("Indexing {}…", self.root.display()),
            (Some(_), Tab::Symbols) => match &self.symbols {
                None => "Indexing symbols…".into(),
                Some(symbols) => format!("{} symbols", symbols.len()),
            },
            (Some(index), Tab::Files) => format!("{} files · {}", index.len(), self.root.display()),
            (Some(_), Tab::Text) if popup.query.is_empty() => "Type text to search in files".into(),
            (Some(_), Tab::Text) if popup.note.is_empty() => popup.text_status.clone(),
            (Some(_), Tab::Text) => format!("{} · {}", popup.text_status, popup.note),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        let root = self.view(window, cx);
        let Some(meter) = &mut self.meter else {
            return root;
        };
        meter.frame(started, started.elapsed());
        root.child(
            div()
                .absolute()
                .top_1()
                .right_3()
                .px_2()
                .rounded_sm()
                .bg(rgba(0x000000b0))
                .text_xs()
                .text_color(rgb(theme::STRONG))
                .child(meter.label()),
        )
    }
}

impl Workspace {
    fn view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        if let Some(viewer) = &mut self.viewer {
            viewer.tick(window);
        }
        self.request_highlights(cx);
        self.request_diagnostics(cx);

        let preview = self.markdown_preview(cx);
        let mut root = div()
            .id("workspace")
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme::BACKGROUND))
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleRegex, _, cx| {
                this.toggle_option(|o| o.regex = !o.regex, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleCase, _, cx| {
                this.toggle_option(|o| o.case_sensitive = !o.case_sensitive, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleWord, _, cx| {
                this.toggle_option(|o| o.whole_word = !o.whole_word, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FindInFile, window, cx| this.open_find(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ReplaceInFile, window, cx| this.open_find(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ReplaceAll, _, cx| this.replace_everywhere(cx)))
            .on_action(cx.listener(|this, _: &FindNext, window, cx| {
                this.step_or_open_find(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindPrevious, window, cx| {
                this.step_or_open_find(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &GoToFile, window, cx| this.open_tab(Tab::Files, window, cx)),
            )
            .on_action(cx.listener(|this, _: &GoToSymbol, window, cx| {
                this.open_tab(Tab::Symbols, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindAction, window, cx| {
                this.open_tab(Tab::Actions, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RecentFiles, window, cx| {
                this.open_tab(Tab::Recent, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FindInFiles, window, cx| {
                    this.open_tab(Tab::Text, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleProjectTree, _, cx| this.toggle_tree(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleMarkdownPreview, _, cx| this.toggle_md_preview(cx)),
            )
            .on_action(cx.listener(|this, _: &NavigateBack, _, cx| this.navigate(true, cx)))
            .on_action(cx.listener(|this, _: &NavigateForward, _, cx| this.navigate(false, cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.navigate(true, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.navigate(false, cx)),
            )
            .on_action(cx.listener(|this, _: &FindUsages, window, cx| this.find_usages(window, cx)))
            .on_action(cx.listener(|this, _: &GoToDefinition, _, cx| this.go_to_definition(cx)))
            .on_action(cx.listener(|this, _: &ReformatCode, _, cx| this.reformat(cx)))
            .on_action(cx.listener(|this, _: &FocusTree, _, cx| this.focus_tree(true, cx)))
            .on_action(cx.listener(|this, _: &FocusEditor, _, cx| this.focus_tree(false, cx)))
            .on_action(
                cx.listener(|this, _: &ExtendSelection, _, cx| this.change_selection(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShrinkSelection, _, cx| this.change_selection(false, cx)),
            )
            .on_action(cx.listener(|this, _: &CollapseRegion, _, cx| this.fold('c', cx)))
            .on_action(cx.listener(|this, _: &ExpandRegion, _, cx| this.fold('o', cx)))
            .on_action(cx.listener(|this, _: &CollapseAll, _, cx| this.fold('M', cx)))
            .on_action(cx.listener(|this, _: &ExpandAll, _, cx| this.fold('R', cx)))
            .on_action(cx.listener(|this, _: &NextMethod, _, cx| this.jump_to_method(true, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousMethod, _, cx| this.jump_to_method(false, cx)),
            )
            .on_action(cx.listener(|this, _: &SwitchTab, _, cx| {
                if this.popup.is_some() {
                    this.switch_tab(cx);
                }
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let Some(popup) = &mut this.popup else {
                    this.editor_key(event, window, cx);
                    return;
                };
                match event.keystroke.key.as_str() {
                    "escape" => this.close_popup(window, cx),
                    "down" => popup.move_selection(1),
                    "up" => popup.move_selection(-1),
                    "pagedown" => popup.move_selection(10),
                    "pageup" => popup.move_selection(-10),
                    "enter" => this.open_selected(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .children(self.tree_visible.then(|| self.project_tree(cx)))
                    .child(match &self.viewer {
                        Some(viewer) => div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .text_color(rgb(theme::TEXT))
                            .children(self.find_panel(cx))
                            .child(viewer.render(
                                std::rc::Rc::new({
                                    let workspace = cx.entity();
                                    move |line, cx| {
                                        workspace.update(cx, |this, cx| {
                                            if let Some(viewer) = &mut this.viewer {
                                                viewer.toggle_fold(line);
                                                cx.notify();
                                            }
                                        });
                                    }
                                }),
                                std::rc::Rc::new({
                                    let workspace = cx.entity();
                                    move |over, cx| {
                                        workspace.update(cx, |this, cx| {
                                            if let Some(viewer) = &mut this.viewer
                                                && viewer.set_hover(over)
                                            {
                                                cx.notify();
                                            }
                                        });
                                    }
                                }),
                                std::rc::Rc::new({
                                    let workspace = cx.entity();
                                    move |row, at, cx| {
                                        workspace.update(cx, |this, cx| {
                                            if let Some(viewer) = &mut this.viewer
                                                && viewer.click(row, at)
                                            {
                                                this.tree_focused = false; // keys are the editor's
                                                cx.notify();
                                            }
                                        });
                                    }
                                }),
                            ))
                            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                                if event.modifiers.control {
                                    this.zoom(event, window, cx);
                                }
                            })),
                        None => div().flex_1().p_4().text_color(rgb(theme::DIM)).child(
                            "Press Space twice to open the popup. Alt+1 shows the project tree.",
                        ),
                    })
                    .children(preview.map(|view| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .p_4()
                            .border_l_1()
                            .border_color(rgb(theme::BORDER))
                            .child(view)
                    })),
            )
            .child(statusbar::render(
                self.viewer.as_ref().map(Viewer::status),
                self.branch.as_ref(),
            ));

        let Some(popup) = &self.popup else {
            return root;
        };
        let options = (popup.tab == Tab::Text).then(|| {
            let o = popup.options;
            div()
                .flex()
                .gap_1()
                .ml_auto()
                .child(chip(
                    "Cc",
                    "Match case (Alt+C)",
                    o.case_sensitive,
                    "case",
                    cx,
                    |o| o.case_sensitive = !o.case_sensitive,
                ))
                .child(chip(
                    "W",
                    "Whole word (Alt+W)",
                    o.whole_word,
                    "word",
                    cx,
                    |o| o.whole_word = !o.whole_word,
                ))
                .child(chip(".*", "Regex (Alt+R)", o.regex, "regex", cx, |o| {
                    o.regex = !o.regex
                }))
        });
        let filters = (popup.tab == Tab::Text).then(|| {
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(&popup.mask_input)))
                .child(div().flex_1().child(Input::new(&popup.dir_input)))
        });
        let mut frame = popup_frame(
            popup.tab,
            &popup.input,
            self.status(popup),
            options,
            filters,
        );
        let selected = popup.selected;
        let rows = match (popup.tab, &self.index) {
            (Tab::Actions, _) => Some(action_items(&popup.actions, &self.registry)),
            (Tab::Files | Tab::Recent, Some(index)) => Some(file_items(index, &popup.files)),
            (Tab::Text, Some(index)) => Some(text_items(index, &popup.text)),
            (Tab::Symbols, Some(index)) => self
                .symbols
                .as_ref()
                .map(|symbols| symbol_items(index, symbols, &popup.symbols)),
            _ => None,
        };
        if let Some(rows) = rows {
            let list = command_list(
                &popup.commands,
                rows,
                selected,
                popup.tab,
                &cx.entity(),
                window,
                cx,
            );
            frame = frame.child(div().w_full().h(px(320.)).child(list));
        }
        root = root.child(frame);
        root
    }
}

impl Workspace {
    fn find_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let panel = self.find.as_ref()?;
        let status = self.viewer.as_ref().and_then(Viewer::find_status);
        let options = panel.options;
        let pattern_empty = panel.find.read(cx).value().is_empty();
        let summary = match &status {
            Some(FindStatus { error: Some(e), .. }) => e.clone(),
            Some(_) if pattern_empty => String::new(),
            Some(FindStatus { total: 0, .. }) => "No matches".to_string(),
            Some(FindStatus {
                total,
                current: Some(i),
                ..
            }) => format!("{i}/{total}"),
            Some(FindStatus { total, .. }) => format!("{total} matches"),
            None => String::new(),
        };
        let failed = status
            .as_ref()
            .is_some_and(|s| s.error.is_some() || (s.total == 0 && !pattern_empty));
        let mut column =
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(rgb(theme::CONTROL_BORDER))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().child(Input::new(&panel.find)))
                        .child(
                            div()
                                .min_w(px(90.))
                                .text_color(rgb(if failed { theme::ERROR } else { theme::MUTED }))
                                .child(summary),
                        )
                        .child(
                            small_button("find-case", "Cc", options.case_sensitive).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_find_option(
                                        |o| o.case_sensitive = !o.case_sensitive,
                                        cx,
                                    )
                                }),
                            ),
                        )
                        .child(small_button("find-word", "W", options.whole_word).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.toggle_find_option(|o| o.whole_word = !o.whole_word, cx)
                            }),
                        ))
                        .child(small_button("find-regex", ".*", options.regex).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.toggle_find_option(|o| o.regex = !o.regex, cx)
                            }),
                        ))
                        .child(
                            small_button("find-prev", "↑", false)
                                .on_click(cx.listener(|this, _, _, cx| this.find_step(false, cx))),
                        )
                        .child(
                            small_button("find-next", "↓", false)
                                .on_click(cx.listener(|this, _, _, cx| this.find_step(true, cx))),
                        )
                        .child(small_button("find-close", "✕", false).on_click(
                            cx.listener(|this, _, window, cx| this.close_find(window, cx)),
                        )),
                );
        if panel.show_replace {
            column = column.child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(Input::new(&panel.replace)))
                    .child(
                        small_button("replace-one", "Replace", false)
                            .on_click(cx.listener(|this, _, _, cx| this.replace_one(cx))),
                    )
                    .child(
                        small_button("replace-all", "Replace all", false)
                            .on_click(cx.listener(|this, _, _, cx| this.replace_everywhere(cx))),
                    ),
            );
        }
        Some(column)
    }
}

fn small_button(id: &'static str, label: &'static str, on: bool) -> Stateful<Div> {
    let button = div()
        .id(id)
        .px_2()
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .cursor_pointer()
        .child(label);
    if on {
        button
            .bg(rgb(theme::ACTIVE_BG))
            .border_color(rgb(theme::ACTIVE_BORDER))
    } else {
        button
    }
}

/// The editor key for a key press, `None` for chords the editor does not handle.
fn vim_key(event: &KeyDownEvent) -> Option<zide_vim::Key> {
    use zide_vim::Key;
    let keystroke = &event.keystroke;
    let modifiers = keystroke.modifiers;
    if modifiers.alt || modifiers.platform {
        return None;
    }
    if modifiers.control {
        // Ctrl+R is Replace here (IntelliJ), so redo moves to Ctrl+Y and Ctrl+Shift+Z
        if keystroke.key == "y" || (keystroke.key == "z" && modifiers.shift) {
            return Some(Key::Ctrl('r'));
        }
        let mut chars = keystroke.key.chars();
        return match (chars.next(), chars.next()) {
            (Some(c), None) => Some(Key::Ctrl(c)),
            _ => None,
        };
    }
    match keystroke.key.as_str() {
        "escape" => Some(Key::Esc),
        "enter" => Some(Key::Enter),
        "backspace" => Some(Key::Backspace),
        "space" => Some(Key::Char(' ')),
        "tab" => Some(Key::Char('\t')),
        _ => keystroke
            .key_char
            .as_ref()
            .and_then(|text| text.chars().next())
            .map(Key::Char),
    }
}

/// Why a language server is not available and what to do about it.
fn setup_hint(spec: &zide_lsp::ServerSpec, error: &LspError) -> String {
    let dir = zide_lsp::servers_dir().map_or_else(String::new, |d| d.display().to_string());
    let install = spec.install.replace("{dir}", &dir);
    format!("{error}. Setup: {install} (docs/language-servers.md)")
}

fn kind_label(kind: StubKind) -> &'static str {
    match kind {
        StubKind::Function => "fn",
        StubKind::Method => "method",
        StubKind::Struct => "struct",
        StubKind::Class => "class",
        StubKind::Interface => "interface",
        StubKind::Enum => "enum",
        StubKind::Variant => "variant",
        StubKind::Field => "field",
        StubKind::Trait => "trait",
        StubKind::Impl => "impl",
        StubKind::Module => "mod",
        StubKind::TypeAlias => "alias",
        StubKind::Const => "const",
        StubKind::Macro => "macro",
        StubKind::Import => "use",
    }
}

fn ranges_in_chars(hit: &FileHit) -> Vec<Range<usize>> {
    hit.fragments.iter().map(|f| f.start..f.end).collect()
}

/// The rows of the Actions, Files, Recent and Symbols tabs, drawn by gpui-kit's command list.
/// Zide still matches and orders the hits and owns the query and the selection; the list only
/// shows them, and its hover and click go back to the workspace.
fn command_list(
    commands: &Entity<CommandState>,
    rows: Vec<CommandItem>,
    selected: usize,
    tab: Tab,
    workspace: &Entity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> Command {
    // Follows Zide's selection. Deferred so the list has taken this frame's items first,
    // and it scrolls the row into view.
    let state = commands.clone();
    window.defer(cx, move |window, cx| {
        state.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(selected)), window, cx)
        });
    });
    let (hovered, confirmed) = (workspace.clone(), workspace.clone());
    Command::new(commands)
        .items(rows)
        .searchable(false)
        .bordered(false)
        .max_h(px(320.))
        .empty(|_, _, _| div().p_3().text_color(rgb(theme::DIM)).child("No results"))
        .on_select(move |path, _, cx| {
            hovered.update(cx, |this, cx| {
                if let Some(popup) = this.popup.as_mut().filter(|p| p.tab == tab)
                    && popup.selected != path.row
                {
                    popup.selected = path.row;
                    cx.notify();
                }
            });
        })
        .on_confirm(move |path, window, cx| {
            confirmed.update(cx, |this, cx| {
                if let Some(popup) = &mut this.popup {
                    popup.selected = path.row;
                }
                this.open_selected(window, cx);
            });
        })
}

fn action_items(hits: &[ActionHit], registry: &std::rc::Rc<Vec<ActionDef>>) -> Vec<CommandItem> {
    hits.iter()
        .map(|hit| {
            let registry = registry.clone();
            let (index, fragments) = (hit.index, hit.fragments.clone());
            CommandItem::new().child(move |_, cx| {
                let def = &registry[index];
                let shortcut = def
                    .action
                    .as_ref()
                    .map(|a| shortcut_text(a.as_ref(), cx))
                    .unwrap_or_default();
                div()
                    .flex()
                    .flex_1()
                    .gap_2()
                    .child(highlighted(def.name.to_string(), fragments.clone()))
                    .child(
                        div()
                            .ml_auto()
                            .text_color(rgb(theme::MUTED))
                            .child(shortcut),
                    )
            })
        })
        .collect()
}

fn file_items(index: &FileIndex, hits: &[FileHit]) -> Vec<CommandItem> {
    hits.iter()
        .map(|hit| {
            let path = index.paths()[hit.index].clone();
            let ranges = ranges_in_chars(hit);
            CommandItem::new().child(move |_, _| highlighted(path.clone(), ranges.clone()))
        })
        .collect()
}

fn symbol_items(index: &FileIndex, symbols: &StubStore, hits: &[SymbolHit]) -> Vec<CommandItem> {
    hits.iter()
        .map(|hit| {
            let symbol = symbols.symbol(hit.entry);
            let kind = kind_label(symbol.node.kind);
            let name = symbol.node.name.as_deref().unwrap_or_default().to_string();
            let ranges: Vec<_> = hit.fragments.iter().map(|f| f.start..f.end).collect();
            let place = format!(
                "{}{}:{}",
                symbol
                    .container
                    .map_or(String::new(), |c| format!("{c} · ")),
                index.paths()[symbol.file],
                symbol.node.line
            );
            CommandItem::new().child(move |_, _| {
                div()
                    .flex()
                    .flex_1()
                    .gap_2()
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(64.))
                            .text_color(rgb(theme::MUTED))
                            .child(kind),
                    )
                    .child(highlighted(name.clone(), ranges.clone()))
                    .child(
                        div()
                            .ml_auto()
                            .text_color(rgb(theme::MUTED))
                            .child(place.clone()),
                    )
            })
        })
        .collect()
}

/// The shortcut of `action` from the real keymap, empty when it has none. Unlike
/// `Window::keystroke_text_for` it does not depend on the key context being rendered: the
/// command list builds its rows outside the workspace's, where that lookup finds nothing and
/// falls back to the action's type name.
fn shortcut_text(action: &dyn Action, cx: &App) -> String {
    cx.key_bindings()
        .borrow()
        .bindings_for_action(action)
        .next_back()
        .map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|keystroke| keystroke.display(cx.platform_info().os).to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn text_items(index: &FileIndex, hits: &[TextHit]) -> Vec<CommandItem> {
    hits.iter()
        .map(|hit| {
            // show the line without its indentation, moving the match ranges with it
            let text = hit.text.trim_start();
            let shift = hit.text.len() - text.len();
            let ranges: Vec<_> = hit
                .ranges
                .iter()
                .filter(|r| r.end > shift)
                .map(|r| r.start.saturating_sub(shift)..r.end - shift)
                .collect();
            let place = format!("{}:{}", index.paths()[hit.file], hit.line);
            let text = text.to_string();
            CommandItem::new().child(move |_, _| {
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(theme::MUTED))
                            .child(place.clone()),
                    )
                    .child(highlighted_bytes(text.clone(), ranges.clone()))
            })
        })
        .collect()
}

const HIGHLIGHT: fn() -> HighlightStyle = || HighlightStyle {
    color: Some(rgb(theme::ACTIVE_BORDER).into()),
    font_weight: Some(FontWeight::BOLD),
    ..Default::default()
};

/// `text` with the given `char` ranges in bold blue.
fn highlighted(text: String, char_ranges: Vec<Range<usize>>) -> StyledText {
    let byte_at: Vec<usize> = text
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(text.len()))
        .collect();
    let bytes = char_ranges
        .into_iter()
        .map(|r| byte_at[r.start]..byte_at[r.end])
        .collect();
    highlighted_bytes(text, bytes)
}

/// `text` with the given byte ranges in bold blue.
fn highlighted_bytes(text: String, byte_ranges: Vec<Range<usize>>) -> StyledText {
    let highlights: Vec<_> = byte_ranges.into_iter().map(|r| (r, HIGHLIGHT())).collect();
    StyledText::new(text).with_highlights(highlights)
}

/// A clickable toggle; `title` is informational only.
fn chip(
    label: &'static str,
    _title: &'static str,
    on: bool,
    id: &'static str,
    cx: &mut Context<Workspace>,
    toggle: fn(&mut TextOptions),
) -> Stateful<Div> {
    let chip = div()
        .id(id)
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .cursor_pointer()
        .child(label);
    let chip = if on {
        chip.bg(rgb(theme::ACTIVE_BG))
            .border_color(rgb(theme::ACTIVE_BORDER))
    } else {
        chip
    };
    chip.on_click(cx.listener(move |this, _, _, cx| this.toggle_option(toggle, cx)))
}

fn popup_frame(
    tab: Tab,
    input: &Entity<InputState>,
    status: String,
    options: Option<Div>,
    filters: Option<Div>,
) -> Div {
    let tab_label = |label: &'static str, this: Tab| {
        let item = div().px_2().rounded_sm().child(label);
        if tab == this {
            item.bg(rgb(theme::ACTIVE_BG)).font_weight(FontWeight::BOLD)
        } else {
            item
        }
    };
    div()
        .absolute()
        .top_16()
        .left_1_4()
        .w_1_2()
        .h(px(480.))
        .p_2()
        .gap_2()
        .flex()
        .flex_col()
        .bg(rgb(theme::POPUP))
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .rounded_md()
        .child(
            div()
                .flex()
                .gap_1()
                .children((tab == Tab::Recent).then(|| tab_label("Recent files", Tab::Recent)))
                .child(tab_label("Files", Tab::Files))
                .child(tab_label("Symbols", Tab::Symbols))
                .child(tab_label("Text", Tab::Text))
                .child(tab_label("Actions", Tab::Actions))
                .children(options),
        )
        .child(Input::new(input))
        .children(filters)
        .child(div().text_xs().text_color(rgb(theme::MUTED)).child(status))
}

/// The window's content: the welcome screen until a project is opened, then its workspace.
/// It also owns the updates and their notifications, so they work on both screens.
struct Shell {
    content: Content,
    _opened: Subscription,
    _activation: Option<Subscription>,
    _workspace_requests: Vec<Subscription>,
    /// Where notifications are shown.
    window: AnyWindowHandle,
    /// A newer release found by a check and announced, waiting to be installed.
    update: Option<zide_update::Update>,
    updating: bool,
    /// The release installed by this run: it starts after a restart, so it is not news again.
    installed: Option<zide_update::Version>,
    /// Checks at startup and then every [`UPDATE_INTERVAL`].
    _updates: Task<()>,
}

enum Content {
    Welcome(Entity<Welcome>),
    Workspace(Entity<Workspace>),
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let welcome = cx.new(|cx| Welcome::new(window, cx));
        let opened = cx.subscribe_in(
            &welcome,
            window,
            |this, _, event: &OpenProject, window, cx| {
                this.show_workspace(event.0.clone(), window, cx)
            },
        );
        let updates = cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| this.check_for_update(false, cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(UPDATE_INTERVAL).await;
            }
        });
        Self {
            content: Content::Welcome(welcome),
            _opened: opened,
            _activation: None,
            _workspace_requests: Vec::new(),
            window: window.window_handle(),
            update: None,
            updating: false,
            installed: None,
            _updates: updates,
        }
    }

    /// Tells the user something about updates: a notification, and the editor's status line
    /// (when a project is open).
    fn update_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notify(Notification::info(text.clone()), cx);
        self.status_notice(text, cx);
    }

    /// The status line and the log, without a toast.
    fn status_notice(&mut self, text: String, cx: &mut Context<Self>) {
        eprintln!("update: {text}");
        if let Content::Workspace(workspace) = &self.content {
            workspace.update(cx, |this, cx| this.set_notice(text, cx));
        }
    }

    /// The update is installed: the toast stays until the user restarts into it.
    fn announce_restart(
        &self,
        version: &zide_update::Version,
        exe: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let shell = cx.weak_entity();
        let notification = Notification::info(format!("Zide {version} is installed"))
            .title("Restart to update")
            .action(move |_, _, cx| {
                let shell = shell.clone();
                let exe = exe.clone();
                let handle = cx.entity();
                Button::new("restart")
                    .primary()
                    .label("Restart")
                    .on_click(move |_, window, cx| {
                        handle.update(cx, |n, cx| n.dismiss(window, cx));
                        shell.update(cx, |this, cx| this.restart(&exe, cx)).ok();
                    })
            });
        self.notify(notification, cx);
    }

    /// Starts the installed Zide on the open project (or on the welcome screen) and quits this
    /// one. `exe` is the path the update was installed to: after the replacement the running
    /// executable's own path reads as deleted.
    fn restart(&mut self, exe: &Path, cx: &mut Context<Self>) {
        self.autosave(cx);
        let args: Vec<std::ffi::OsString> = match &self.content {
            Content::Workspace(workspace) => vec![workspace.read(cx).root.clone().into()],
            Content::Welcome(_) => Vec::new(),
        };
        match std::process::Command::new(exe).args(args).spawn() {
            Ok(_) => cx.quit(),
            Err(e) => self.update_notice(format!("restart failed: {e}"), cx),
        }
    }

    fn notify(&self, notification: Notification, cx: &mut Context<Self>) {
        self.window
            .update(cx, |_, window, cx| {
                window.push_notification(notification.placement(Anchor::BottomRight), cx)
            })
            .ok();
    }

    /// A newer release exists and the button installs it.
    fn announce_update(&self, version: &zide_update::Version, cx: &mut Context<Self>) {
        let shell = cx.weak_entity();
        let notification = Notification::info(format!("Zide {version} is available"))
            .title("Update available")
            .action(move |_, _, cx| {
                let shell = shell.clone();
                let handle = cx.entity();
                Button::new("install-update")
                    .primary()
                    .label("Update")
                    .on_click(move |_, window, cx| {
                        shell.update(cx, |this, cx| this.update_zide(cx)).ok();
                        handle.update(cx, |n, cx| n.dismiss(window, cx));
                    })
            });
        self.notify(notification, cx);
    }

    /// Looks for a newer release in the background. On its own (`explicit == false`) it only
    /// speaks about a release it has not announced yet; asked for, it also says when there is
    /// none or it failed. Nothing is installed here.
    fn check_for_update(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if self.updating {
            return;
        }
        // debug: ZIDE_FAKE_VERSION=0.0.1 makes the running version look old, to see the update notification
        let version =
            std::env::var("ZIDE_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
        let Ok(current) = zide_update::Version::parse(&version) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(
                    async move { zide_update::check(&zide_update::Online, UPDATE_REPO, &current) },
                )
                .await;
            this.update(cx, |this, cx| match outcome {
                Ok(Some(update)) => {
                    let announced = this.update.as_ref().map(|known| &known.version);
                    if zide_update::should_announce(
                        &update.version,
                        announced,
                        this.installed.as_ref(),
                        explicit,
                    ) {
                        this.announce_update(&update.version, cx);
                        this.update = Some(update);
                    } else if explicit {
                        this.update_notice(
                            format!("Zide {} is installed: restart to use it", update.version),
                            cx,
                        );
                    }
                }
                Ok(None) if explicit => this.update_notice("Zide is up to date".into(), cx),
                Err(e) if explicit => this.update_notice(format!("update check failed: {e}"), cx),
                _ => {}
            })
            .ok();
        })
        .detach();
    }

    /// Installs the release found earlier (or looks for one first). The new version runs the
    /// next time Zide starts.
    fn update_zide(&mut self, cx: &mut Context<Self>) {
        if self.updating {
            return;
        }
        let Some(update) = self.update.clone() else {
            return self.check_for_update(true, cx);
        };
        let Ok(exe) = std::env::current_exe() else {
            return self.update_notice("update failed: unknown executable path".into(), cx);
        };
        self.updating = true;
        self.update_notice(format!("downloading Zide {}...", update.version), cx);
        let installed_at = exe.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { zide_update::apply(&zide_update::Online, &update, &exe) })
                .await;
            this.update(cx, |this, cx| {
                this.updating = false;
                match outcome {
                    Ok(version) => {
                        this.update = None;
                        this.installed = Some(version.clone());
                        this.status_notice(
                            format!("Zide {version} installed: restart to use it"),
                            cx,
                        );
                        this.announce_restart(&version, installed_at, cx);
                    }
                    Err(e) => this.update_notice(format!("update failed: {e}"), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens `root` in this window and remembers it for the welcome screen.
    fn show_workspace(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let root = root.canonicalize().unwrap_or(root);
        projects::record(&root);
        let workspace = cx.new(|cx| Workspace::new(root, window, cx));
        let saver = workspace.downgrade();
        self._activation = Some(cx.observe_window_activation(window, move |_, window, cx| {
            if !window.is_window_active() {
                saver
                    .update(cx, |this, cx| {
                        this.autosave(cx);
                        cx.notify();
                    })
                    .ok();
            }
        }));
        self._workspace_requests = vec![
            cx.subscribe(&workspace, |this, _, _: &UpdateRequested, cx| {
                this.update_zide(cx)
            }),
            cx.subscribe_in(
                &workspace,
                window,
                |this, _, _: &OpenRequested, window, cx| this.open_folder(window, cx),
            ),
        ];
        workspace.read(cx).focus.clone().focus(window, cx);
        self.content = Content::Workspace(workspace);
        cx.notify();
    }

    /// Asks for a folder with the system's picker and opens it in this window.
    fn open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = welcome::choose_folder(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Some(folder) = picked.await {
                this.update_in(cx, |this, window, cx| {
                    this.autosave(cx);
                    this.show_workspace(folder, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn autosave(&self, cx: &mut Context<Self>) {
        if let Content::Workspace(workspace) = &self.content {
            workspace.update(cx, |this, cx| this.autosave(cx));
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(match &self.content {
            Content::Welcome(welcome) => welcome.clone().into_any_element(),
            Content::Workspace(workspace) => workspace.clone().into_any_element(),
        })
    }
}

/// Zide's palette on the colors gpui-kit's components read, so the command list, the
/// notifications and the rest look like the popup they sit in instead of the library's own dark.
/// These colors are only read by the components, never by Zide's own elements.
fn apply_palette(cx: &mut App) {
    let colors = &mut gpui_kit::component::Theme::global_mut(cx).colors;
    // background and text
    colors.popover = rgb(theme::POPUP).into();
    colors.popover_foreground = rgb(theme::TEXT).into();
    // the selected row, like the active tab chip
    colors.accent = rgb(theme::ACTIVE_BG).into();
    colors.accent_foreground = rgb(theme::STRONG).into();
    // shortcuts, paths, hints
    colors.muted_foreground = rgb(theme::MUTED).into();
}

fn main() {
    let first = std::env::args_os().nth(1);
    if matches!(
        first.as_deref().and_then(|a| a.to_str()),
        Some("--version" | "-V")
    ) {
        println!("zide {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // without a folder, the welcome screen lists the projects opened before
    let root = first.map(PathBuf::from);

    let app = gpui_kit::application().with_assets(file_icon::AppAssets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        apply_palette(cx);
        // Root binds Tab to focus-next; a deeper context wins over it.
        cx.bind_keys([
            KeyBinding::new("tab", SwitchTab, Some("Workspace")),
            // IntelliJ's keymap
            KeyBinding::new("ctrl-f", FindInFile, Some("Workspace")),
            KeyBinding::new("ctrl-r", ReplaceInFile, Some("Workspace")),
            KeyBinding::new("ctrl-alt-enter", ReplaceAll, Some("Workspace")),
            KeyBinding::new("f3", FindNext, Some("Workspace")),
            KeyBinding::new("shift-f3", FindPrevious, Some("Workspace")),
            KeyBinding::new("ctrl-shift-f", FindInFiles, Some("Workspace")),
            KeyBinding::new("ctrl-e", RecentFiles, Some("Workspace")),
            KeyBinding::new("alt-1", ToggleProjectTree, Some("Workspace")),
            KeyBinding::new("ctrl-shift-m", ToggleMarkdownPreview, Some("Workspace")),
            KeyBinding::new("ctrl-alt-left", NavigateBack, Some("Workspace")),
            KeyBinding::new("ctrl-alt-right", NavigateForward, Some("Workspace")),
            KeyBinding::new("alt-f7", FindUsages, Some("Workspace")),
            KeyBinding::new("ctrl-b", GoToDefinition, Some("Workspace")),
            KeyBinding::new("ctrl-alt-l", ReformatCode, Some("Workspace")),
            KeyBinding::new("ctrl-h", FocusTree, Some("Workspace")),
            KeyBinding::new("ctrl-l", FocusEditor, Some("Workspace")),
            KeyBinding::new("ctrl-w", ExtendSelection, Some("Workspace")),
            KeyBinding::new("ctrl-shift-w", ShrinkSelection, Some("Workspace")),
            KeyBinding::new("ctrl--", CollapseRegion, Some("Workspace")),
            KeyBinding::new("ctrl-=", ExpandRegion, Some("Workspace")),
            KeyBinding::new("ctrl-shift--", CollapseAll, Some("Workspace")),
            KeyBinding::new("ctrl-shift-=", ExpandAll, Some("Workspace")),
            KeyBinding::new("alt-down", NextMethod, Some("Workspace")),
            KeyBinding::new("alt-up", PreviousMethod, Some("Workspace")),
            KeyBinding::new("ctrl-shift-n", GoToFile, Some("Workspace")),
            KeyBinding::new("ctrl-alt-shift-n", GoToSymbol, Some("Workspace")),
            KeyBinding::new("ctrl-shift-a", FindAction, Some("Workspace")),
            KeyBinding::new("ctrl-shift-a", FindAction, Some("Welcome")),
            KeyBinding::new("alt-x", ToggleRegex, Some("Workspace")),
            KeyBinding::new("alt-r", ToggleRegex, Some("Workspace")),
            KeyBinding::new("alt-c", ToggleCase, Some("Workspace")),
            KeyBinding::new("alt-w", ToggleWord, Some("Workspace")),
        ]);
        gpui_kit::open_window(
            WindowOptions {
                app_id: Some("zide".into()),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let shell = cx.new(|cx| Shell::new(window, cx));
                if let Some(root) = root {
                    shell.update(cx, |this, cx| this.show_workspace(root, window, cx));
                }
                let saver = shell.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    saver.update(cx, |this, cx| this.autosave(cx)).ok();
                    true
                });
                shell
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
