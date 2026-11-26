//! The workers of a chat: the subagents its Claude launched with the `Agent` tool. Each one
//! keeps its own conversation, apart from the chat's, so the user can watch it work; nothing
//! is ever sent to it. Plain data, so its rules are tested without a window.

use zide_claude::{Block, Event, Transcript};

pub(super) struct Worker {
    /// The chat's `Agent` call that launched the worker, which names it in the stream.
    pub tool_use_id: String,
    pub description: String,
    /// The subagent type, e.g. `general-purpose`.
    pub kind: String,
    /// What it is doing right now, as the CLI last said.
    pub activity: Option<String>,
    pub transcript: Transcript,
    /// It ended while its view was open: it stays until the user leaves the view.
    pub finished: bool,
}

/// The workers of one chat, in the order they were launched.
#[derive(Default)]
pub(super) struct Workers {
    list: Vec<Worker>,
}

impl Workers {
    pub fn list(&self) -> &[Worker] {
        &self.list
    }

    pub fn get(&self, tool_use_id: &str) -> Option<&Worker> {
        self.list.iter().find(|w| w.tool_use_id == tool_use_id)
    }

    fn get_mut(&mut self, tool_use_id: &str) -> Option<&mut Worker> {
        self.list.iter_mut().find(|w| w.tool_use_id == tool_use_id)
    }

    /// Takes in a worker event of the chat's stream; other events are none of its business.
    /// `viewed` is the worker whose view is open, if it is one of this chat's: a worker that
    /// ends while the user watches it stays, marked finished, until the view is left.
    /// Returns the worker whose conversation grew, so its Markdown can follow.
    pub fn apply(&mut self, event: Event, viewed: Option<&str>) -> Option<String> {
        match event {
            Event::WorkerStarted {
                tool_use_id,
                description,
                kind,
            } => {
                if self.get(&tool_use_id).is_none() {
                    self.list.push(Worker {
                        tool_use_id,
                        description,
                        kind,
                        activity: None,
                        transcript: Transcript::new(),
                        finished: false,
                    });
                }
                None
            }
            Event::Worker { tool_use_id, event } => {
                let worker = self.get_mut(&tool_use_id)?;
                // each text of a worker is a whole message: the next one is a new paragraph
                let event = match *event {
                    Event::TextDelta(text)
                        if matches!(worker.transcript.blocks().last(), Some(Block::Text(_))) =>
                    {
                        Event::TextDelta(format!("\n\n{text}"))
                    }
                    event => event,
                };
                worker.transcript.apply(event);
                Some(tool_use_id)
            }
            Event::WorkerActivity {
                tool_use_id,
                activity,
            } => {
                if let Some(worker) = self.get_mut(&tool_use_id) {
                    worker.activity = Some(activity);
                }
                None
            }
            Event::WorkerEnded { tool_use_id, .. } => {
                if viewed == Some(tool_use_id.as_str()) {
                    if let Some(worker) = self.get_mut(&tool_use_id) {
                        worker.finished = true;
                    }
                } else {
                    self.list.retain(|w| w.tool_use_id != tool_use_id);
                }
                None
            }
            _ => None,
        }
    }

    /// The user left the view of `tool_use_id`: a worker that finished meanwhile goes now.
    pub fn left(&mut self, tool_use_id: &str) {
        self.list
            .retain(|w| !(w.tool_use_id == tool_use_id && w.finished));
    }

    /// The chat's process is gone, and its workers with it.
    pub fn clear(&mut self) {
        self.list.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str) -> Event {
        Event::WorkerStarted {
            tool_use_id: id.into(),
            description: format!("task {id}"),
            kind: "general-purpose".into(),
        }
    }

    fn of(id: &str, event: Event) -> Event {
        Event::Worker {
            tool_use_id: id.into(),
            event: Box::new(event),
        }
    }

    fn ended(id: &str) -> Event {
        Event::WorkerEnded {
            tool_use_id: id.into(),
            status: "completed".into(),
        }
    }

    fn ids(workers: &Workers) -> Vec<&str> {
        workers
            .list()
            .iter()
            .map(|w| w.tool_use_id.as_str())
            .collect()
    }

    #[test]
    fn workers_are_listed_in_the_order_they_started() {
        let mut workers = Workers::default();
        workers.apply(started("a"), None);
        workers.apply(started("b"), None);
        workers.apply(started("a"), None);
        assert_eq!(ids(&workers), ["a", "b"]);
        let a = workers.get("a").unwrap();
        assert_eq!(a.description, "task a");
        assert_eq!(a.kind, "general-purpose");
        assert!(!a.finished);
    }

    #[test]
    fn each_workers_events_go_to_its_own_conversation() {
        let mut workers = Workers::default();
        workers.apply(started("a"), None);
        workers.apply(started("b"), None);
        let grew = workers.apply(of("b", Event::TextDelta("hi".into())), None);
        workers.apply(of("b", Event::TextDelta("there".into())), None);
        workers.apply(
            Event::WorkerActivity {
                tool_use_id: "b".into(),
                activity: "Reading x".into(),
            },
            None,
        );
        assert_eq!(grew.as_deref(), Some("b"));
        assert!(workers.get("a").unwrap().transcript.blocks().is_empty());
        let b = workers.get("b").unwrap();
        assert_eq!(b.transcript.blocks(), [Block::Text("hi\n\nthere".into())]);
        assert_eq!(b.activity.as_deref(), Some("Reading x"));
        // a worker nobody started has nowhere to go
        assert_eq!(
            workers.apply(of("z", Event::TextDelta("x".into())), None),
            None
        );
    }

    #[test]
    fn a_worker_that_ends_leaves_the_list() {
        let mut workers = Workers::default();
        workers.apply(started("a"), None);
        workers.apply(started("b"), Some("b"));
        workers.apply(ended("a"), Some("b"));
        assert_eq!(ids(&workers), ["b"]);
    }

    #[test]
    fn a_worker_being_watched_stays_finished_until_its_view_is_left() {
        let mut workers = Workers::default();
        workers.apply(started("a"), None);
        workers.apply(ended("a"), Some("a"));
        assert_eq!(ids(&workers), ["a"]);
        assert!(workers.get("a").unwrap().finished);
        workers.left("a");
        assert!(workers.list().is_empty());
    }

    #[test]
    fn leaving_the_view_of_a_running_worker_keeps_it() {
        let mut workers = Workers::default();
        workers.apply(started("a"), None);
        workers.left("a");
        assert_eq!(ids(&workers), ["a"]);
    }

    #[test]
    fn the_workers_go_with_the_process() {
        let mut workers = Workers::default();
        workers.apply(started("a"), Some("a"));
        workers.apply(started("b"), None);
        workers.clear();
        assert!(workers.list().is_empty());
    }
}
