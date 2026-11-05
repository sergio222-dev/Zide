//! Search Everywhere prototype: double-Shift popup with file search over a real project.
//!
//! `zide-ui [root]` indexes `root` (default: the current directory) in the background;
//! the window shows up before the index is ready (ADR-009). Timings go to stderr.

use std::{path::PathBuf, sync::Arc, time::Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use zide_core::{DoubleShiftDetector, scan_files};
use zide_index::FileIndex;
use zide_search::{FileHit, FileQuery, search_files};

const MAX_RESULTS: usize = 200;

struct Popup {
    input: Entity<InputState>,
    query: String,
    hits: Arc<Vec<FileHit>>,
    opened_at: Option<Instant>,
    _subscription: Subscription,
}

struct Workspace {
    focus: FocusHandle,
    detector: DoubleShiftDetector,
    root: PathBuf,
    /// `None` while the project is still being scanned.
    index: Option<Arc<FileIndex>>,
    popup: Option<Popup>,
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
            opened_at: Some(Instant::now()),
            _subscription: subscription,
        });
        self.search("", cx);
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        popup.query = query.to_string();
        if let Some(index) = &self.index {
            let started = Instant::now();
            let hits = if query.trim().is_empty() {
                // nothing typed yet: show the first files
                (0..index.len().min(MAX_RESULTS))
                    .map(|index| FileHit { index, score: 0 })
                    .collect()
            } else {
                search_files(index, &FileQuery::parse(query), MAX_RESULTS)
            };
            eprintln!(
                "search {query:?}: {} hits in {:?}",
                hits.len(),
                started.elapsed()
            );
            popup.hits = Arc::new(hits);
        }
        cx.notify();
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.detector.key_pressed();
                if event.keystroke.key == "escape" {
                    this.close_popup(window, cx);
                }
            }))
            .p_4()
            .child("Press Shift twice to open the popup. Esc closes it.");

        if let (Some(popup), Some(index)) = (&self.popup, &self.index) {
            let (index, hits) = (index.clone(), popup.hits.clone());
            root = root.child(
                popup_frame(&popup.input, self.status()).child(
                    uniform_list("results", hits.len(), move |range, _, _| {
                        range
                            .map(|row| {
                                div()
                                    .h(px(24.))
                                    .child(index.paths()[hits[row].index].clone())
                            })
                            .collect()
                    })
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
