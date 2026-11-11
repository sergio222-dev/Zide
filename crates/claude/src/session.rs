use std::{
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::event::{Event, parse_events};

/// A picture sent along with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`: what Claude accepts.
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The stdin line for a user message: plain text, or blocks when there are pictures.
fn user_message(text: &str, images: &[Image]) -> Value {
    if images.is_empty() {
        return json!({"type": "user", "message": {"role": "user", "content": text}});
    }
    let mut content: Vec<Value> = images
        .iter()
        .map(|image| {
            json!({"type": "image", "source": {
                "type": "base64",
                "media_type": image.media_type,
                "data": STANDARD.encode(&image.bytes),
            }})
        })
        .collect();
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    json!({"type": "user", "message": {"role": "user", "content": content}})
}

#[derive(Default)]
pub struct Options<'a> {
    /// Session id from an earlier `Event::Init`, to continue that conversation.
    pub resume: Option<&'a str>,
    /// `claude --permission-mode`; `None` keeps whatever the user's settings say.
    pub permission_mode: Option<&'a str>,
    /// `claude --model`: an alias (`opus`, `sonnet`) or a full name; `None` keeps the default.
    pub model: Option<&'a str>,
    /// `claude --effort`: `low`, `medium`, `high`, `xhigh` or `max`; `None` keeps the default.
    pub effort: Option<&'a str>,
    /// Starts `claude` bare-bones: none of the user's settings, hooks, plugins, MCP servers
    /// or skills, and only the `Read` tool. A turn then costs about 8k tokens instead of the
    /// 140k a heavy setup adds (measured), which is what tests that spend real tokens want.
    /// The login still works, unlike `claude --bare`, which only accepts an API key.
    pub minimal: bool,
}

/// One long-lived `claude` process: messages go in, events come out, and the
/// conversation keeps its context between messages.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
}

impl Session {
    /// Starts `claude` in `cwd` (its config is looked up from there).
    pub fn spawn(cwd: &Path, options: Options) -> io::Result<(Session, Receiver<Event>)> {
        let mut command = crate::cli::command();
        command
            .args(["-p", "--verbose", "--include-partial-messages"])
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            // tells when a message sent mid-turn is taken up (folded into the running turn or not)
            .arg("--replay-user-messages")
            // Without this, anything that would prompt is denied on the spot.
            .args(["--permission-prompt-tool", "stdio"]);
        if let Some(id) = options.resume {
            command.args(["--resume", id]);
        }
        if let Some(mode) = options.permission_mode {
            command.args(["--permission-mode", mode]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(effort) = options.effort {
            command.args(["--effort", effort]);
        }
        if options.minimal {
            command
                .args(["--setting-sources", ""])
                .args(["--strict-mcp-config", "--disable-slash-commands"])
                .args(["--tools", "Read,AskUserQuestion"]);
        }
        let mut child = command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                for event in parse_events(&line) {
                    if sender.send(event).is_err() {
                        return;
                    }
                }
            }
            let _ = sender.send(Event::Exit);
        });
        Ok((Session { child, stdin }, receiver))
    }

    pub fn send(&mut self, text: &str) -> io::Result<()> {
        self.send_with_images(text, &[])
    }

    /// Like `send`, with pictures the message talks about.
    pub fn send_with_images(&mut self, text: &str, images: &[Image]) -> io::Result<()> {
        self.write(&user_message(text, images))
    }

    /// Stops the turn in progress. The turn still ends with an `Event::Result` (its
    /// `terminal_reason` says it was aborted) and the session stays usable.
    pub fn interrupt(&mut self) -> io::Result<()> {
        self.write(&json!({
            "type": "control_request",
            "request_id": "interrupt",
            "request": {"subtype": "interrupt"},
        }))
    }

    /// Switches the model for the turns to come, without restarting the process.
    pub fn set_model(&mut self, model: &str) -> io::Result<()> {
        self.write(&json!({
            "type": "control_request",
            "request_id": "set-model",
            "request": {"subtype": "set_model", "model": model},
        }))
    }

    /// Changes the effort for the turns to come, without restarting the process.
    pub fn set_effort(&mut self, effort: &str) -> io::Result<()> {
        self.write(&json!({
            "type": "control_request",
            "request_id": "set-effort",
            "request": {"subtype": "apply_flag_settings", "settings": {"effortLevel": effort}},
        }))
    }

    /// Lets the tool run. `input` is the request's input, possibly edited.
    pub fn allow(&mut self, request_id: &str, input: &Value) -> io::Result<()> {
        self.respond(
            request_id,
            json!({"behavior": "allow", "updatedInput": input}),
        )
    }

    /// Answers an `Event::Question`: each pair is (question text, chosen option label).
    pub fn answer(
        &mut self,
        request_id: &str,
        input: &Value,
        answers: &[(&str, &str)],
    ) -> io::Result<()> {
        let mut updated = input.clone();
        updated["answers"] = answers
            .iter()
            .map(|&(q, a)| (q.to_owned(), json!(a)))
            .collect();
        self.allow(request_id, &updated)
    }

    /// Refuses the tool; Claude sees `reason` and carries on without it.
    pub fn deny(&mut self, request_id: &str, reason: &str) -> io::Result<()> {
        self.respond(request_id, json!({"behavior": "deny", "message": reason}))
    }

    fn respond(&mut self, request_id: &str, response: Value) -> io::Result<()> {
        self.write(&json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request_id, "response": response},
        }))
    }

    fn write(&mut self, message: &Value) -> io::Result<()> {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Image, user_message};

    #[test]
    fn text_alone_stays_a_plain_string() {
        assert_eq!(
            user_message("hi", &[]),
            json!({"type": "user", "message": {"role": "user", "content": "hi"}})
        );
    }

    #[test]
    fn pictures_come_first_as_base64_blocks_and_the_text_last() {
        let image = Image {
            media_type: "image/png",
            bytes: vec![1, 2, 3],
        };
        let message = user_message("what is it?", std::slice::from_ref(&image));
        assert_eq!(
            message["message"]["content"],
            json!([
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AQID"}},
                {"type": "text", "text": "what is it?"},
            ])
        );
        // no text: only the picture
        let bare = user_message("", &[image]);
        assert_eq!(bare["message"]["content"].as_array().unwrap().len(), 1);
    }
}
