//! GPUI spike: window, double-Shift popup, input and a virtual list over 100k names
//! filtered by the matcher. Logs timings to stderr.

use std::{sync::Arc, time::Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use zide_core::DoubleShiftDetector;
use zide_matcher::{FixingLayoutMatcher, Matcher, MatchingMode, RussianToEnglish};

const NAME_COUNT: usize = 100_000;
const MAX_VISIBLE_RESULTS: usize = 200;

struct Popup {
    input: Entity<InputState>,
    results: Arc<Vec<usize>>,
    opened_at: Option<Instant>,
    _subscription: Subscription,
}

struct Workspace {
    focus: FocusHandle,
    detector: DoubleShiftDetector,
    names: Arc<Vec<String>>,
    popup: Option<Popup>,
}

fn synthetic_names() -> Vec<String> {
    let words = [
        "Goto",
        "File",
        "Action",
        "Search",
        "Everywhere",
        "Matcher",
        "Index",
        "Project",
        "Symbol",
        "Text",
    ];
    (0..NAME_COUNT)
        .map(|i| {
            let (a, b, c) = (words[i % 10], words[(i / 10) % 10], words[(i / 100) % 10]);
            format!("src/{c}/{a}{b}Impl{i}.rs")
        })
        .collect()
}

impl Workspace {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            detector: DoubleShiftDetector::default(),
            names: Arc::new(synthetic_names()),
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
        let all = Arc::new((0..self.names.len().min(MAX_VISIBLE_RESULTS)).collect());
        self.popup = Some(Popup {
            input,
            results: all,
            opened_at: Some(Instant::now()),
            _subscription: subscription,
        });
        cx.notify();
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let started = Instant::now();
        let matcher =
            FixingLayoutMatcher::new(query, MatchingMode::IgnoreCase, "", &RussianToEnglish);
        let results: Vec<usize> = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, name)| matcher.matches(name))
            .map(|(i, _)| i)
            .take(MAX_VISIBLE_RESULTS)
            .collect();
        eprintln!(
            "search {query:?}: {} results in {:?}",
            results.len(),
            started.elapsed()
        );
        if let Some(popup) = &mut self.popup {
            popup.results = Arc::new(results);
        }
        cx.notify();
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

        if let Some(popup) = &self.popup {
            let names = self.names.clone();
            let results = popup.results.clone();
            root = root.child(
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
                    .child(Input::new(&popup.input))
                    .child(
                        uniform_list("results", results.len(), move |range, _, _| {
                            range
                                .map(|row| div().h(px(24.)).child(names[results[row]].clone()))
                                .collect()
                        })
                        .w_full()
                        .h(px(360.)),
                    ),
            );
        }
        root
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let workspace = cx.new(Workspace::new);
            workspace.read(cx).focus.clone().focus(window, cx);
            workspace
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
