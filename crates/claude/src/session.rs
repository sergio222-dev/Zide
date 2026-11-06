use std::{
    io::{self, BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

use serde_json::{Value, json};

use crate::event::{Event, parse_event};

#[derive(Default)]
pub struct Options<'a> {
    /// Session id from an earlier `Event::Init`, to continue that conversation.
    pub resume: Option<&'a str>,
    /// `claude --permission-mode`; `None` keeps whatever the user's settings say.
    pub permission_mode: Option<&'a str>,
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
        let mut command = Command::new("claude");
        command
            .args(["-p", "--verbose", "--include-partial-messages"])
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            // Without this, anything that would prompt is denied on the spot.
            .args(["--permission-prompt-tool", "stdio"]);
        if let Some(id) = options.resume {
            command.args(["--resume", id]);
        }
        if let Some(mode) = options.permission_mode {
            command.args(["--permission-mode", mode]);
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
                if let Some(event) = parse_event(&line)
                    && sender.send(event).is_err()
                {
                    break;
                }
            }
        });
        Ok((Session { child, stdin }, receiver))
    }

    pub fn send(&mut self, text: &str) -> io::Result<()> {
        let message = json!({"type": "user", "message": {"role": "user", "content": text}});
        self.write(&message)
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

    /// Lets the tool run. `input` is the request's input, possibly edited.
    pub fn allow(&mut self, request_id: &str, input: &Value) -> io::Result<()> {
        self.respond(request_id, json!({"behavior": "allow", "updatedInput": input}))
    }

    /// Answers an `Event::Question`: each pair is (question text, chosen option label).
    pub fn answer(
        &mut self,
        request_id: &str,
        input: &Value,
        answers: &[(&str, &str)],
    ) -> io::Result<()> {
        let mut updated = input.clone();
        updated["answers"] = answers.iter().map(|&(q, a)| (q.to_owned(), json!(a))).collect();
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
