//! Suggestions from a local model (Qwen2.5-coder, served by Ollama) shown as ghost text at the
//! caret. Off at launch; the status bar indicator turns it on and off. Language servers keep
//! the completion popup, which goes first while open: with the model off or failing, editing is
//! as without it.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use gpui_kit::*;
use zide_ai::{Error, MODEL, Processor, Server};

use crate::{Workspace, cancel, theme};

/// How long typing has to pause before the model is asked.
const PAUSE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Off,
    /// Reaching or starting the Ollama server.
    Starting,
    /// Downloading the model, the first time.
    Pulling,
    Ready,
    /// A suggestion is being written.
    Working,
    Failed(String),
}

impl State {
    /// The dot of the indicator: gray when off, orange while busy, green when ready, red when
    /// it failed.
    pub fn color(&self) -> u32 {
        match self {
            State::Off => theme::current().dim,
            State::Starting | State::Pulling | State::Working => theme::current().working,
            State::Ready => theme::current().ok,
            State::Failed(_) => theme::current().error,
        }
    }

    pub fn label(&self) -> String {
        match self {
            State::Off => "Qwen off".into(),
            State::Starting => "Qwen starting…".into(),
            State::Pulling => "Qwen downloading…".into(),
            State::Ready | State::Working => "Qwen".into(),
            State::Failed(why) => format!("Qwen: {why}"),
        }
    }

    fn on(&self) -> bool {
        !matches!(self, State::Off | State::Failed(_))
    }
}

impl Ai {
    pub fn color(&self) -> u32 {
        self.state.color()
    }

    /// The state, and once a suggestion came, where the model runs: `Qwen · GPU`, `Qwen · CPU`
    /// (slow: no usable GPU) or `Qwen · 52% GPU` (it did not fit).
    pub fn label(&self) -> String {
        let label = self.state.label();
        match (&self.state, self.processor) {
            (State::Ready | State::Working, Some(processor)) => {
                format!("{label} · {}", processor_label(processor))
            }
            _ => label,
        }
    }
}

fn processor_label(processor: Processor) -> String {
    match processor {
        Processor::Gpu => "GPU".into(),
        Processor::Cpu => "CPU".into(),
        Processor::Split(gpu) => format!("{gpu}% GPU"),
    }
}

#[derive(Default)]
pub struct Ai {
    pub state: State,
    /// The server, once it answers and has the model.
    server: Option<Arc<Server>>,
    /// Bumped at each start and stop: a start that ends after a newer one is dropped.
    epoch: u64,
    /// Set to give up on the suggestion asked last.
    asked: Option<Arc<AtomicBool>>,
    /// Where the server ran the model at the last suggestion.
    processor: Option<Processor>,
    /// Stops the server this instance started when the app quits.
    _quit: Option<Subscription>,
}

impl Workspace {
    /// The indicator was clicked: turns the model on, or off.
    pub(crate) fn toggle_ai(&mut self, cx: &mut Context<Self>) {
        if self.ai.state.on() {
            self.stop_ai();
        } else {
            self.start_ai(cx);
        }
        cx.notify();
    }

    fn stop_ai(&mut self) {
        self.ai.epoch += 1;
        cancel(&mut self.ai.asked);
        self.ai.server = None; // stops the server if this instance started it
        self.ai.processor = None;
        self.ai.state = State::Off;
        if let Some(viewer) = &mut self.viewer {
            viewer.clear_ghost();
        }
    }

    /// Reuses or starts the Ollama server and downloads the model when it is missing, in the
    /// background.
    fn start_ai(&mut self, cx: &mut Context<Self>) {
        self.ai.epoch += 1;
        let epoch = self.ai.epoch;
        self.ai.server = None;
        self.ai.processor = None;
        self.ai.state = State::Starting;
        if self.ai._quit.is_none() {
            self.ai._quit = Some(cx.on_app_quit(|this, _| {
                this.ai.server = None;
                async {}
            }));
        }
        cx.spawn(async move |this, cx| {
            let set = |state: State, server: Option<Arc<Server>>, cx: &mut AsyncApp| {
                this.update(cx, |this, cx| {
                    if this.ai.epoch == epoch {
                        this.ai.state = state;
                        this.ai.server = server;
                        cx.notify();
                    }
                })
                .ok();
            };
            let started = cx
                .background_executor()
                .spawn(async { Server::start() })
                .await;
            let server = match started {
                Ok(server) => Arc::new(server),
                Err(error) => return set(State::Failed(error.to_string()), None, cx),
            };
            let found = cx
                .background_executor()
                .spawn({
                    let server = server.clone();
                    async move { server.has_model(MODEL) }
                })
                .await;
            match found {
                Ok(true) => {}
                Ok(false) => {
                    set(State::Pulling, None, cx);
                    let pulled = cx
                        .background_executor()
                        .spawn({
                            let server = server.clone();
                            async move { server.pull(MODEL) }
                        })
                        .await;
                    if let Err(error) = pulled {
                        return set(State::Failed(error.to_string()), None, cx);
                    }
                }
                Err(error) => return set(State::Failed(error.to_string()), None, cx),
            }
            set(State::Ready, Some(server), cx);
        })
        .detach();
    }

    /// After a key typed in the editor: asks the model what follows the caret once typing
    /// pauses, unless it is off, a popup is open, a suggestion is still being typed, or the
    /// file is not code. A newer key gives up on the question in flight.
    pub(crate) fn ghost_as_typed(&mut self, cx: &mut Context<Self>) {
        cancel(&mut self.ai.asked);
        let Some(server) = self.ai.server.clone() else {
            return;
        };
        let Some(viewer) = &self.viewer else {
            return;
        };
        if viewer.ghost_text().is_some() {
            return; // what is typed of it narrows it down
        }
        let Some((at, revision)) = viewer.ghost_point() else {
            return;
        };
        let file = viewer.path().to_string();
        let text = viewer.text();
        // prose would get suggestions at every pause
        if self.open_file_server().is_none() && zide_stubs::language_for_path(&file).is_none() {
            return;
        }
        let asked = Arc::new(AtomicBool::new(false));
        self.ai.asked = Some(asked.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            if asked.load(Ordering::Relaxed) {
                return;
            }
            this.update(cx, |this, cx| {
                if this.ai.state == State::Ready {
                    this.ai.state = State::Working;
                    cx.notify();
                }
            })
            .ok();
            let (outcome, processor) = cx
                .background_executor()
                .spawn({
                    let (asked, server) = (asked.clone(), server.clone());
                    async move {
                        let outcome = server.complete(MODEL, &text, at, &asked);
                        // loaded by now: where it went (a quick local question)
                        let processor = match outcome {
                            Ok(_) => server.processor(MODEL).ok().flatten(),
                            Err(_) => None,
                        };
                        (outcome, processor)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                // turned off, or started again, meanwhile
                if !this
                    .ai
                    .server
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &server))
                {
                    return;
                }
                if this.ai.state == State::Working {
                    this.ai.state = State::Ready;
                }
                if processor.is_some() {
                    this.ai.processor = processor;
                }
                match outcome {
                    Ok(Some(text)) if !asked.load(Ordering::Relaxed) => {
                        if let Some(viewer) = this.viewer.as_mut().filter(|v| v.path() == file) {
                            viewer.show_ghost(&text, at, revision);
                        }
                    }
                    Ok(_) => {}
                    // the instance that started the server quit: this one starts it again
                    Err(Error::Unreachable) => this.start_ai(cx),
                    Err(error) => {
                        this.ai.server = None;
                        this.ai.state = State::Failed(error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Tab with a suggestion shown and no popup open: puts it in. True when it did.
    pub(crate) fn accept_ghost(&mut self, cx: &mut Context<Self>) -> bool {
        let accepted = self
            .viewer
            .as_mut()
            .is_some_and(|viewer| viewer.accept_ghost());
        if accepted {
            cx.notify();
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn the_indicator_tells_off_busy_ready_and_failed_apart() {
        assert_eq!(State::default(), State::Off);
        assert_eq!(State::Off.color(), theme::current().dim);
        for busy in [State::Starting, State::Pulling, State::Working] {
            assert_eq!(busy.color(), theme::current().working);
            assert!(busy.on());
        }
        assert_eq!(State::Ready.color(), theme::current().ok);
        let failed = State::Failed("ollama is not installed".into());
        assert_eq!(failed.color(), theme::current().error);
        assert_eq!(failed.label(), "Qwen: ollama is not installed");
        let ai = |state, processor| Ai {
            state,
            processor,
            ..Ai::default()
        };
        assert_eq!(ai(State::Ready, None).label(), "Qwen");
        assert_eq!(ai(State::Ready, Some(Processor::Gpu)).label(), "Qwen · GPU");
        assert_eq!(
            ai(State::Working, Some(Processor::Cpu)).label(),
            "Qwen · CPU"
        );
        assert_eq!(
            ai(State::Ready, Some(Processor::Split(52))).label(),
            "Qwen · 52% GPU"
        );
        assert_eq!(ai(State::Off, Some(Processor::Gpu)).label(), "Qwen off");
        // clicking a failed indicator tries again
        assert!(!failed.on());
        assert!(!State::Off.on());
    }
}
