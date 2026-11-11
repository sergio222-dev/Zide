//! The port the chat drives an agent process through, whatever protocol it speaks: Claude
//! Code's stream-json ([`crate::Session`]) or the Agent Client Protocol (the `zide-acp` crate).
//! Both report what happens as [`crate::Event`]s, which [`crate::Transcript`] turns into the
//! conversation.

use std::io;

use serde_json::Value;

use crate::Image;

/// The `terminal_reason` of an [`crate::Event::Result`] for a turn the user stopped, in any
/// protocol. Claude Code's own reason, `aborted_streaming`, means the same.
pub const CANCELLED: &str = "cancelled";

/// A running agent: messages go in, the answers come out as events on the channel it was
/// started with. Dropping it stops the process.
pub trait AgentSession {
    fn send_with_images(&mut self, text: &str, images: &[Image]) -> io::Result<()>;

    /// Stops the running turn; it ends with a `Result` whose reason is a cancellation.
    fn interrupt(&mut self) -> io::Result<()>;

    /// Lets the tool of permission request `request_id` run, with `input` as it was asked.
    fn allow(&mut self, request_id: &str, input: &Value) -> io::Result<()>;

    /// Answers a question request with `(question, answer)` pairs.
    fn answer(
        &mut self,
        request_id: &str,
        input: &Value,
        answers: &[(&str, &str)],
    ) -> io::Result<()>;

    fn deny(&mut self, request_id: &str, reason: &str) -> io::Result<()>;

    fn set_model(&mut self, model: &str) -> io::Result<()>;

    fn set_effort(&mut self, effort: &str) -> io::Result<()>;
}
