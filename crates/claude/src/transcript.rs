use serde_json::Value;

use crate::event::{Event, Question, Usage};

/// What the chat is doing, which is what an indicator shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The process is gone.
    Inactive,
    /// A turn is over and Claude waits for the next message.
    Idle,
    /// A turn is running.
    Working,
    /// Claude waits for the user to allow a tool or answer a question.
    NeedsUser,
}

/// How a permission request or a question ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Allowed,
    Denied,
    /// (question text, chosen option label), as `Session::answer` takes them.
    Answered(Vec<(String, String)>),
    /// The turn ended (or the process died) before the user decided.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// `None` while the tool is still running.
    pub output: Option<ToolOutput>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Permission {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub resolution: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub request_id: String,
    pub questions: Vec<Question>,
    pub resolution: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    User(String),
    /// The assistant's text, markdown.
    Text(String),
    Thinking(String),
    Tool(ToolCall),
    Permission(Permission),
    Question(Ask),
    /// The user stopped the turn.
    Interrupted,
    Error(String),
}

/// The conversation as the chat shows it, built by feeding it the session's events.
/// It holds no process and no UI, so it is plain data to test and to render.
#[derive(Debug, Clone)]
pub struct Transcript {
    blocks: Vec<Block>,
    session_id: Option<String>,
    turn_open: bool,
    alive: bool,
    usage: Option<Usage>,
    cost_usd: f64,
}

impl Default for Transcript {
    fn default() -> Self {
        Transcript {
            blocks: Vec::new(),
            session_id: None,
            turn_open: false,
            alive: true,
            usage: None,
            cost_usd: 0.0,
        }
    }
}

impl Transcript {
    pub fn new() -> Transcript {
        Transcript::default()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn usage(&self) -> Option<Usage> {
        self.usage
    }

    /// The cost the CLI reported in the latest result.
    pub fn cost_usd(&self) -> f64 {
        self.cost_usd
    }

    pub fn status(&self) -> Status {
        if !self.alive {
            Status::Inactive
        } else if self.blocks.iter().any(is_pending) {
            Status::NeedsUser
        } else if self.turn_open {
            Status::Working
        } else {
            Status::Idle
        }
    }

    /// Records what the user sent, which the stream does not echo back, and opens the turn.
    /// Sending means there is a live process, even when an earlier one had exited.
    pub fn user_message(&mut self, text: &str) {
        self.push_user(text);
        self.turn_open = true;
        self.alive = true;
    }

    /// A message from a stored conversation: shown, but it opens no turn.
    pub fn push_user(&mut self, text: &str) {
        self.blocks.push(Block::User(text.to_owned()));
    }

    /// Records the user's decision on a permission request or a question.
    pub fn resolve(&mut self, request_id: &str, resolution: Resolution) {
        for block in &mut self.blocks {
            match block {
                Block::Permission(p) if p.request_id == request_id && p.resolution.is_none() => {
                    p.resolution = Some(resolution);
                    return;
                }
                Block::Question(q) if q.request_id == request_id && q.resolution.is_none() => {
                    q.resolution = Some(resolution);
                    return;
                }
                _ => {}
            }
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Init { session_id } => self.session_id = Some(session_id),
            Event::TextDelta(chunk) => match self.blocks.last_mut() {
                Some(Block::Text(text)) => text.push_str(&chunk),
                _ => self.blocks.push(Block::Text(chunk)),
            },
            Event::ThinkingDelta(chunk) => match self.blocks.last_mut() {
                Some(Block::Thinking(text)) => text.push_str(&chunk),
                _ => self.blocks.push(Block::Thinking(chunk)),
            },
            // The question itself is the block; the tool call around it adds nothing.
            Event::ToolUse { name, .. } if name == "AskUserQuestion" => {}
            Event::ToolUse { id, name, input } => {
                if self.tool_mut(&id).is_none() {
                    self.blocks.push(Block::Tool(ToolCall {
                        id,
                        name,
                        input,
                        output: None,
                    }));
                }
            }
            Event::ToolResult {
                tool_use_id,
                output,
                is_error,
            } => {
                if let Some(call) = self.tool_mut(&tool_use_id) {
                    call.output = Some(ToolOutput {
                        text: output,
                        is_error,
                    });
                }
            }
            Event::Usage(usage) => self.usage = Some(usage),
            Event::PermissionRequest {
                request_id,
                tool_name,
                input,
            } => self.blocks.push(Block::Permission(Permission {
                request_id,
                tool_name,
                input,
                resolution: None,
            })),
            Event::Question {
                request_id,
                questions,
                ..
            } => self.blocks.push(Block::Question(Ask {
                request_id,
                questions,
                resolution: None,
            })),
            Event::Result {
                text,
                is_error,
                cost_usd,
                terminal_reason,
            } => {
                self.turn_open = false;
                self.cost_usd = cost_usd;
                self.cancel_pending();
                if terminal_reason.as_deref() == Some("aborted_streaming") {
                    self.blocks.push(Block::Interrupted);
                } else if is_error {
                    let message = if text.is_empty() {
                        "The turn failed.".to_owned()
                    } else {
                        text
                    };
                    self.blocks.push(Block::Error(message));
                }
            }
            Event::Exit => {
                self.alive = false;
                self.turn_open = false;
                self.cancel_pending();
            }
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.blocks.iter_mut().rev().find_map(|block| match block {
            Block::Tool(call) if call.id == id => Some(call),
            _ => None,
        })
    }

    fn cancel_pending(&mut self) {
        for block in &mut self.blocks {
            match block {
                Block::Permission(Permission { resolution, .. })
                | Block::Question(Ask { resolution, .. })
                    if resolution.is_none() =>
                {
                    *resolution = Some(Resolution::Cancelled);
                }
                _ => {}
            }
        }
    }
}

fn is_pending(block: &Block) -> bool {
    matches!(
        block,
        Block::Permission(Permission {
            resolution: None,
            ..
        }) | Block::Question(Ask {
            resolution: None,
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_use(id: &str, name: &str) -> Event {
        Event::ToolUse {
            id: id.into(),
            name: name.into(),
            input: json!({"k": id}),
        }
    }

    fn permission(id: &str) -> Event {
        Event::PermissionRequest {
            request_id: id.into(),
            tool_name: "Bash".into(),
            input: json!({}),
        }
    }

    fn done() -> Event {
        Event::Result {
            text: "ok".into(),
            is_error: false,
            cost_usd: 0.25,
            terminal_reason: None,
        }
    }

    #[test]
    fn a_new_transcript_is_idle() {
        assert_eq!(Transcript::new().status(), Status::Idle);
    }

    #[test]
    fn a_message_opens_the_turn_and_the_result_closes_it() {
        let mut t = Transcript::new();
        t.user_message("hi");
        assert_eq!(t.status(), Status::Working);
        t.apply(done());
        assert_eq!(t.status(), Status::Idle);
        assert_eq!(t.cost_usd(), 0.25);
    }

    #[test]
    fn text_deltas_grow_one_block_until_something_else_arrives() {
        let mut t = Transcript::new();
        t.apply(Event::TextDelta("Hel".into()));
        t.apply(Event::TextDelta("lo".into()));
        t.apply(tool_use("t1", "Read"));
        t.apply(Event::TextDelta("Bye".into()));
        let texts: Vec<_> = t
            .blocks()
            .iter()
            .map(|b| match b {
                Block::Text(s) => s.as_str(),
                Block::Tool(_) => "<tool>",
                _ => "?",
            })
            .collect();
        assert_eq!(texts, ["Hello", "<tool>", "Bye"]);
    }

    #[test]
    fn thinking_and_text_are_separate_blocks() {
        let mut t = Transcript::new();
        t.apply(Event::ThinkingDelta("hm".into()));
        t.apply(Event::ThinkingDelta("m".into()));
        t.apply(Event::TextDelta("answer".into()));
        assert_eq!(
            t.blocks(),
            [Block::Thinking("hmm".into()), Block::Text("answer".into())]
        );
    }

    #[test]
    fn a_repeated_tool_use_is_one_call() {
        let mut t = Transcript::new();
        t.apply(tool_use("t1", "Read"));
        t.apply(tool_use("t1", "Read"));
        assert_eq!(t.blocks().len(), 1);
    }

    #[test]
    fn the_result_lands_on_its_tool_call() {
        let mut t = Transcript::new();
        t.apply(tool_use("t1", "Read"));
        t.apply(tool_use("t2", "Grep"));
        t.apply(Event::ToolResult {
            tool_use_id: "t1".into(),
            output: "contents".into(),
            is_error: false,
        });
        let outputs: Vec<_> = t
            .blocks()
            .iter()
            .map(|b| match b {
                Block::Tool(c) => c.output.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(
            outputs,
            [
                Some(ToolOutput {
                    text: "contents".into(),
                    is_error: false
                }),
                None
            ]
        );
    }

    #[test]
    fn a_result_for_an_unknown_tool_is_ignored() {
        let mut t = Transcript::new();
        t.apply(Event::ToolResult {
            tool_use_id: "ghost".into(),
            output: "x".into(),
            is_error: false,
        });
        assert!(t.blocks().is_empty());
    }

    #[test]
    fn a_permission_makes_the_status_needs_user_until_resolved() {
        let mut t = Transcript::new();
        t.user_message("run ls");
        t.apply(permission("r1"));
        assert_eq!(t.status(), Status::NeedsUser);
        t.resolve("r1", Resolution::Allowed);
        assert_eq!(t.status(), Status::Working);
    }

    #[test]
    fn a_question_makes_the_status_needs_user_and_keeps_the_answers() {
        let mut t = Transcript::new();
        t.user_message("ask me");
        t.apply(Event::Question {
            request_id: "q1".into(),
            input: json!({}),
            questions: vec![],
        });
        assert_eq!(t.status(), Status::NeedsUser);
        let answers = vec![("Color?".to_owned(), "Azul".to_owned())];
        t.resolve("q1", Resolution::Answered(answers.clone()));
        assert_eq!(t.status(), Status::Working);
        let Block::Question(ask) = &t.blocks()[1] else {
            panic!("expected a question block");
        };
        assert_eq!(ask.resolution, Some(Resolution::Answered(answers)));
    }

    #[test]
    fn the_question_tool_call_is_not_shown_twice() {
        let mut t = Transcript::new();
        t.apply(tool_use("t1", "AskUserQuestion"));
        assert!(t.blocks().is_empty());
    }

    #[test]
    fn resolving_twice_keeps_the_first_decision() {
        let mut t = Transcript::new();
        t.apply(permission("r1"));
        t.resolve("r1", Resolution::Denied);
        t.resolve("r1", Resolution::Allowed);
        let Block::Permission(p) = &t.blocks()[0] else {
            panic!("expected a permission");
        };
        assert_eq!(p.resolution, Some(Resolution::Denied));
    }

    #[test]
    fn the_end_of_the_turn_cancels_what_is_still_pending() {
        let mut t = Transcript::new();
        t.user_message("go");
        t.apply(permission("r1"));
        t.apply(Event::Result {
            text: String::new(),
            is_error: false,
            cost_usd: 0.0,
            terminal_reason: Some("aborted_streaming".into()),
        });
        assert_eq!(t.status(), Status::Idle);
        let Block::Permission(p) = &t.blocks()[1] else {
            panic!("expected a permission");
        };
        assert_eq!(p.resolution, Some(Resolution::Cancelled));
        assert_eq!(t.blocks().last(), Some(&Block::Interrupted));
    }

    #[test]
    fn a_failed_turn_leaves_an_error_block() {
        let mut t = Transcript::new();
        t.user_message("go");
        t.apply(Event::Result {
            text: String::new(),
            is_error: true,
            cost_usd: 0.0,
            terminal_reason: None,
        });
        assert_eq!(
            t.blocks().last(),
            Some(&Block::Error("The turn failed.".into()))
        );
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn exit_makes_the_status_inactive_whatever_was_going_on() {
        let mut t = Transcript::new();
        t.user_message("go");
        t.apply(permission("r1"));
        t.apply(Event::Exit);
        assert_eq!(t.status(), Status::Inactive);
    }

    #[test]
    fn a_new_message_after_exit_revives_the_status() {
        let mut t = Transcript::new();
        t.apply(Event::Exit);
        t.user_message("again");
        assert_eq!(t.status(), Status::Working);
    }

    #[test]
    fn init_and_usage_are_kept() {
        let mut t = Transcript::new();
        t.apply(Event::Init {
            session_id: "s1".into(),
        });
        let usage = Usage {
            context_tokens: 10,
            output_tokens: 2,
        };
        t.apply(Event::Usage(usage));
        assert_eq!(t.session_id(), Some("s1"));
        assert_eq!(t.usage(), Some(usage));
    }
}
