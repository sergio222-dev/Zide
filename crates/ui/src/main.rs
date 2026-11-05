//! Search Everywhere prototype: double-Shift popup with file and text search over a real project.
//!
//! `zide-ui [root]` indexes `root` (default: the current directory) in the background;
//! the window shows up before the index is ready (ADR-009). Tab switches between the Files and
//! Text tabs. Timings go to stderr.

use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use zide_core::{DoubleShiftDetector, scan_files};
use zide_index::FileIndex;
use zide_search::{
    FileHit, FileQuery, TextHit, TextQuery, TextSearchError, search_files_with_recent, search_text,
};

actions!(zide, [SwitchTab, ToggleRegex, ToggleCase, ToggleWord]);

/// Text search toggles, like IntelliJ's Regex / Match Case / Words buttons.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct TextOptions {
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
}

const MAX_FILE_RESULTS: usize = 200;
const MAX_RECENT: usize = 50;
const MAX_TEXT_RESULTS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Files,
    Text,
}

impl Tab {
    fn toggled(self) -> Self {
        match self {
            Tab::Files => Tab::Text,
            Tab::Text => Tab::Files,
        }
    }
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
    options: TextOptions,
    selected: usize,
    scroll: UniformListScrollHandle,
    opened_at: Option<Instant>,
    /// Text tab filters: file mask (`*.rs`) and directory (`crates/ui`).
    mask_input: Entity<InputState>,
    dir_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Popup {
    fn len(&self) -> usize {
        match self.tab {
            Tab::Files => self.files.len(),
            Tab::Text => self.text.len(),
        }
    }

    fn select(&mut self, row: usize) {
        self.selected = row;
        if self.len() > 0 {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.len().saturating_sub(1);
        self.select(self.selected.saturating_add_signed(delta).min(last));
    }
}

struct Workspace {
    focus: FocusHandle,
    detector: DoubleShiftDetector,
    root: PathBuf,
    /// `None` while the project is still being scanned.
    index: Option<Arc<FileIndex>>,
    popup: Option<Popup>,
    /// Bumped on every search; results of an older search are dropped.
    search_generation: u64,
    cancel_search: Option<Arc<AtomicBool>>,
    /// Files opened from the popup (indices into `index`), most recent first.
    recent: Vec<usize>,
}

impl Workspace {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let scan_root = root.clone();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let paths = cx
                .background_executor()
                .spawn(async move { scan_files(&scan_root) })
                .await;
            let index = FileIndex::new(paths.unwrap_or_else(|e| {
                eprintln!("scan failed: {e}");
                Vec::new()
            }));
            eprintln!("indexed {} files in {:?}", index.len(), started.elapsed());
            this.update(cx, |this, cx| {
                this.index = Some(Arc::new(index));
                this.search_again(cx);
            })
            .ok();
        })
        .detach();

        Self {
            focus: cx.focus_handle(),
            detector: DoubleShiftDetector::default(),
            root,
            index: None,
            popup: None,
            search_generation: 0,
            cancel_search: None,
            recent: Vec::new(),
        }
    }

    fn open_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().to_string();
                if let Some(popup) = &mut this.popup {
                    popup.query = query;
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
        self.popup = Some(Popup {
            input,
            query: String::new(),
            tab: Tab::Files,
            files: Arc::default(),
            text: Arc::default(),
            text_status: String::new(),
            options: TextOptions::default(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            opened_at: Some(Instant::now()),
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
            return;
        };
        if popup.tab != Tab::Text {
            return;
        }
        toggle(&mut popup.options);
        self.search_again(cx);
    }

    fn switch_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = &mut self.popup {
            popup.tab = popup.tab.toggled();
            popup.selected = 0;
        }
        self.search_again(cx);
    }

    /// There is no editor yet: "opening" a result logs it and closes the popup.
    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(popup), Some(index)) = (&self.popup, &self.index) else {
            return;
        };
        let target = match popup.tab {
            Tab::Files => popup
                .files
                .get(popup.selected)
                .map(|hit| (hit.index, FileQuery::parse(&popup.query).line)),
            Tab::Text => popup
                .text
                .get(popup.selected)
                .map(|hit| (hit.file, Some(hit.line as u32))),
        };
        if let Some((file, line)) = target {
            eprintln!("open {} line {line:?}", index.paths()[file]);
            self.recent.retain(|&i| i != file);
            self.recent.insert(0, file);
            self.recent.truncate(MAX_RECENT);
            self.close_popup(window, cx);
        }
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
            Tab::Files => {
                let recent = self.recent.clone();
                self.search_files(query, recent, index, generation, cancel, cx)
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
            (None, _) => format!("Indexing {}…", self.root.display()),
            (Some(index), Tab::Files) => format!("{} files · {}", index.len(), self.root.display()),
            (Some(_), Tab::Text) if popup.query.is_empty() => "Type text to search in files".into(),
            (Some(_), Tab::Text) => popup.text_status.clone(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(opened_at) = self.popup.as_mut().and_then(|p| p.opened_at.take()) {
            eprintln!(
                "popup first render {:?} after double-Shift",
                opened_at.elapsed()
            );
        }

        let mut root = div()
            .id("workspace")
            .size_full()
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
            .on_action(cx.listener(|this, _: &SwitchTab, _, cx| {
                if this.popup.is_some() {
                    this.switch_tab(cx);
                }
            }))
            .on_modifiers_changed(
                cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                    let m = event.modifiers;
                    let others = m.control || m.alt || m.platform;
                    if this
                        .detector
                        .modifiers_changed(m.shift, others, Instant::now())
                    {
                        this.open_popup(window, cx);
                    }
                }),
            )
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.detector.key_pressed();
                let Some(popup) = &mut this.popup else {
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
            .p_4()
            .child("Press Shift twice to open the popup. Tab switches tabs, Esc closes it.");

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
        if let Some(index) = &self.index {
            let (index, selected) = (index.clone(), popup.selected);
            let list = match popup.tab {
                Tab::Files => {
                    let hits = popup.files.clone();
                    uniform_list("files", hits.len(), move |range, _, _| {
                        range
                            .map(|row| {
                                let path = index.paths()[hits[row].index].clone();
                                row_frame(row == selected)
                                    .child(highlighted(path, ranges_in_chars(&hits[row])))
                            })
                            .collect()
                    })
                }
                Tab::Text => {
                    let hits = popup.text.clone();
                    uniform_list("text", hits.len(), move |range, _, _| {
                        range
                            .map(|row| text_row(&index, &hits[row], row == selected))
                            .collect()
                    })
                }
            };
            frame = frame.child(list.track_scroll(&popup.scroll).w_full().h(px(320.)));
        }
        root = root.child(frame);
        root
    }
}

fn ranges_in_chars(hit: &FileHit) -> Vec<Range<usize>> {
    hit.fragments.iter().map(|f| f.start..f.end).collect()
}

fn row_frame(selected: bool) -> Div {
    let row = div().h(px(24.)).px_2().flex().gap_2().overflow_hidden();
    if selected { row.bg(rgb(0xd6e4ff)) } else { row }
}

fn text_row(index: &FileIndex, hit: &TextHit, selected: bool) -> Div {
    // show the line without its indentation, moving the match ranges with it
    let text = hit.text.trim_start();
    let shift = hit.text.len() - text.len();
    let ranges = hit
        .ranges
        .iter()
        .filter(|r| r.end > shift)
        .map(|r| r.start.saturating_sub(shift)..r.end - shift)
        .collect();
    row_frame(selected)
        .child(
            div()
                .flex_shrink_0()
                .text_color(rgb(0x777777))
                .child(format!("{}:{}", index.paths()[hit.file], hit.line)),
        )
        .child(highlighted_bytes(text.to_string(), ranges))
}

const HIGHLIGHT: fn() -> HighlightStyle = || HighlightStyle {
    color: Some(rgb(0x1a5fd0).into()),
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
        .border_color(rgb(0xc8c8c8))
        .cursor_pointer()
        .child(label);
    let chip = if on {
        chip.bg(rgb(0xd6e4ff)).border_color(rgb(0x1a5fd0))
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
            item.bg(rgb(0xd6e4ff)).font_weight(FontWeight::BOLD)
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
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xc8c8c8))
        .rounded_md()
        .child(
            div()
                .flex()
                .gap_1()
                .child(tab_label("Files", Tab::Files))
                .child(tab_label("Text", Tab::Text))
                .children(options),
        )
        .child(Input::new(input))
        .children(filters)
        .child(div().text_xs().text_color(rgb(0x777777)).child(status))
}

fn main() {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("no current directory"));
    let root = root.canonicalize().unwrap_or(root);

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        // Root binds Tab to focus-next; a deeper context wins over it.
        cx.bind_keys([
            KeyBinding::new("tab", SwitchTab, Some("Workspace")),
            KeyBinding::new("alt-r", ToggleRegex, Some("Workspace")),
            KeyBinding::new("alt-c", ToggleCase, Some("Workspace")),
            KeyBinding::new("alt-w", ToggleWord, Some("Workspace")),
        ]);
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(root, cx));
            workspace.read(cx).focus.clone().focus(window, cx);
            workspace
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
