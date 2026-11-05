//! Search Everywhere prototype: double-Shift popup with file search over a real project.
//!
//! `zide-ui [root]` indexes `root` (default: the current directory) in the background;
//! the window shows up before the index is ready (ADR-009). Timings go to stderr.

use std::{
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
use zide_search::{FileHit, FileQuery, search_files_cancellable};

const MAX_RESULTS: usize = 200;

struct Popup {
    input: Entity<InputState>,
    query: String,
    hits: Arc<Vec<FileHit>>,
    selected: usize,
    scroll: UniformListScrollHandle,
    opened_at: Option<Instant>,
    _subscription: Subscription,
}

impl Popup {
    fn set_hits(&mut self, hits: Vec<FileHit>) {
        self.hits = Arc::new(hits);
        self.select(0);
    }

    fn select(&mut self, row: usize) {
        self.selected = row;
        if !self.hits.is_empty() {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.hits.len().saturating_sub(1);
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
    /// Bumped on every search; a finished search applies only if it is still the latest.
    search_generation: u64,
    cancel_search: Option<Arc<AtomicBool>>,
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
                let query = this.popup.as_ref().map(|p| p.query.clone());
                if let Some(query) = query {
                    this.search(&query, cx);
                }
                cx.notify();
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
        }
    }

    fn open_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().to_string();
                this.search(&query, cx);
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.popup = Some(Popup {
            input,
            query: String::new(),
            hits: Arc::default(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            opened_at: Some(Instant::now()),
            _subscription: subscription,
        });
        self.search("", cx);
    }

    /// There is no editor yet: "opening" a file logs it and closes the popup.
    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.popup else {
            return;
        };
        if let (Some(hit), Some(index)) = (popup.hits.get(popup.selected), &self.index) {
            let line = FileQuery::parse(&popup.query).line;
            eprintln!("open {} line {line:?}", index.paths()[hit.index]);
            self.close_popup(window, cx);
        }
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Runs the search on a background thread. Typing again cancels the running search
    /// and drops its results, so the UI thread never waits and never shows stale hits.
    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel_search.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.search_generation += 1;
        let Some(popup) = &mut self.popup else {
            return;
        };
        popup.query = query.to_string();
        let Some(index) = self.index.clone() else {
            cx.notify();
            return;
        };

        if query.trim().is_empty() {
            // nothing typed yet: show the first files
            let hits = (0..index.len().min(MAX_RESULTS))
                .map(|index| FileHit {
                    index,
                    score: 0,
                    fragments: Vec::new(),
                })
                .collect();
            popup.set_hits(hits);
            cx.notify();
            return;
        }

        let generation = self.search_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_search = Some(cancel.clone());
        let query = query.to_string();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let label = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move {
                    search_files_cancellable(
                        &index,
                        &FileQuery::parse(&query),
                        MAX_RESULTS,
                        &cancel,
                    )
                })
                .await;
            let Some(hits) = hits else {
                return; // cancelled by a newer keystroke
            };
            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                eprintln!(
                    "search {label:?}: {} hits in {:?}",
                    hits.len(),
                    started.elapsed()
                );
                if let Some(popup) = &mut this.popup {
                    popup.set_hits(hits);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn status(&self) -> String {
        match &self.index {
            Some(index) => format!("{} files · {}", index.len(), self.root.display()),
            None => format!("Indexing {}…", self.root.display()),
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
            .track_focus(&self.focus)
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
            .child("Press Shift twice to open the popup. Esc closes it.");

        if let (Some(popup), Some(index)) = (&self.popup, &self.index) {
            let (index, hits, selected) = (index.clone(), popup.hits.clone(), popup.selected);
            root = root.child(
                popup_frame(&popup.input, self.status()).child(
                    uniform_list("results", hits.len(), move |range, _, _| {
                        range
                            .map(|row| {
                                let item = div().h(px(24.)).px_2().child(highlighted(
                                    index.paths()[hits[row].index].clone(),
                                    &hits[row],
                                ));
                                if row == selected {
                                    item.bg(rgb(0xd6e4ff))
                                } else {
                                    item
                                }
                            })
                            .collect()
                    })
                    .track_scroll(&popup.scroll)
                    .w_full()
                    .h(px(340.)),
                ),
            );
        } else if let Some(popup) = &self.popup {
            root = root.child(popup_frame(&popup.input, self.status()));
        }
        root
    }
}

/// The path with the matched fragments in bold blue.
fn highlighted(path: String, hit: &FileHit) -> StyledText {
    // fragments are `char` ranges, GPUI wants byte ranges
    let byte_at: Vec<usize> = path
        .char_indices()
        .map(|(b, _)| b)
        .chain(std::iter::once(path.len()))
        .collect();
    let style = HighlightStyle {
        color: Some(rgb(0x1a5fd0).into()),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let highlights: Vec<_> = hit
        .fragments
        .iter()
        .map(|f| (byte_at[f.start]..byte_at[f.end], style))
        .collect();
    StyledText::new(path).with_highlights(highlights)
}

fn popup_frame(input: &Entity<InputState>, status: String) -> Div {
    div()
        .absolute()
        .top_16()
        .left_1_4()
        .w_1_2()
        .h(px(420.))
        .p_2()
        .gap_2()
        .flex()
        .flex_col()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xc8c8c8))
        .rounded_md()
        .child(Input::new(input))
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
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(root, cx));
            workspace.read(cx).focus.clone().focus(window, cx);
            workspace
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
