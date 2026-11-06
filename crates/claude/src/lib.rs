//! Drives a live `claude` process over its stream-json protocol. The process is the
//! user's own Claude Code, so everything configured in `~/.claude` and the project's
//! `.claude` (CLAUDE.md, hooks, MCP servers, skills, output style) applies unchanged.

mod event;
mod session;

pub use event::{Event, Question, QuestionOption, parse_event};
pub use session::{Options, Session};
