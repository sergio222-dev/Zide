//! Drives a live `claude` process over its stream-json protocol. The process is the
//! user's own Claude Code, so everything configured in `~/.claude` and the project's
//! `.claude` (CLAUDE.md, hooks, MCP servers, skills, output style) applies unchanged.

mod cli;
mod defaults;
mod event;
pub mod history;
mod session;
mod transcript;

pub use cli::{MIN_VERSION, Version, installed};
pub use defaults::{Defaults, defaults};
pub use event::{Event, Limits, Question, QuestionOption, Usage, Window, parse_events};
pub use serde_json::Value;
pub use session::{Image, Options, Session};
pub use transcript::{
    Ask, Block, Overall, Permission, Resolution, Status, ToolCall, ToolOutput, Transcript,
};
