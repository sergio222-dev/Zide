//! Drives an agent that speaks the Agent Client Protocol (ACP): JSON-RPC 2.0 over the
//! process's stdin and stdout, one message per line. It implements the chat's
//! [`zide_claude::AgentSession`] and reports everything as [`zide_claude::Event`]s, so the
//! chat shows an ACP agent with the same [`zide_claude::Transcript`] it uses for Claude.
//!
//! The client declares neither file system nor terminal capabilities: the agent reads,
//! writes and runs commands itself, and asks before it does through permission requests.

mod client;
mod protocol;

use std::{
    io::{self, BufRead, BufReader},
    path::Path,
    process::{Child, Command as Process, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use zide_claude::{AgentSession, Event, Image, Launcher};

use client::{Client, Command, Input};

/// How long `probe` waits for the agent to answer `initialize`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// A running ACP agent. Its methods only queue work for the worker thread, so they never
/// block on the process; dropping it stops the process.
pub struct AcpSession {
    child: Child,
    input: Sender<Input>,
}

impl AcpSession {
    fn command(&self, command: Command) -> io::Result<()> {
        self.input
            .send(Input::Command(command))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the agent has exited"))
    }
}

fn unsupported(what: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("an ACP agent cannot {what}"),
    ))
}

impl AgentSession for AcpSession {
    fn send_with_images(&mut self, text: &str, images: &[Image]) -> io::Result<()> {
        self.command(Command::Send(text.to_owned(), images.to_vec()))
    }

    fn interrupt(&mut self) -> io::Result<()> {
        self.command(Command::Interrupt)
    }

    /// ACP permission options carry no edited input, so `input` is not sent back.
    fn allow(&mut self, request_id: &str, _input: &Value) -> io::Result<()> {
        self.command(Command::Allow(request_id.to_owned()))
    }

    fn answer(
        &mut self,
        _request_id: &str,
        _input: &Value,
        _answers: &[(&str, &str)],
    ) -> io::Result<()> {
        unsupported("answer questions")
    }

    fn deny(&mut self, request_id: &str, _reason: &str) -> io::Result<()> {
        self.command(Command::Deny(request_id.to_owned()))
    }

    fn set_model(&mut self, _model: &str) -> io::Result<()> {
        unsupported("switch models from the chat")
    }

    fn set_effort(&mut self, _effort: &str) -> io::Result<()> {
        unsupported("change the effort")
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The agent's process with its stdio piped. An ACP agent is always a configured command;
/// there is no default one to fall back to.
fn process(launcher: &Launcher) -> io::Result<Process> {
    if launcher.command.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an ACP agent needs a command",
        ));
    }
    let mut command = zide_claude::command_for(launcher);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    Ok(command)
}

/// Forwards each JSON line the agent prints into `input`, then `Closed` when it exits.
/// Lines that are not JSON are dropped here, before they reach the protocol.
fn read_messages(stdout: impl io::Read + Send + 'static, input: Sender<Input>) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && input.send(Input::Message(message)).is_err()
            {
                return;
            }
        }
        let _ = input.send(Input::Closed);
    });
}

/// Starts the agent `launcher` names in `cwd`. Returns at once; the handshake runs in the
/// background and its outcome arrives as events.
pub fn spawn(launcher: &Launcher, cwd: &Path) -> io::Result<(AcpSession, Receiver<Event>)> {
    let mut child = process(launcher)?.current_dir(cwd).spawn()?;
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");

    let (input, inputs) = mpsc::channel();
    let (events, receiver) = mpsc::channel();
    read_messages(stdout, input.clone());
    let mut client = Client::new(stdin, events, cwd.to_owned());
    thread::spawn(move || {
        client.start();
        for next in inputs {
            if !client.handle(next) {
                break;
            }
        }
    });
    Ok((AcpSession { child, input }, receiver))
}

/// Runs the `initialize` handshake and stops the process: the agent's name and version, or
/// why it could not be talked to.
pub fn probe(launcher: &Launcher) -> Result<String, String> {
    let mut child = process(launcher)
        .and_then(|mut command| command.spawn())
        .map_err(|e| e.to_string())?;
    let answer = initialize(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    answer
}

fn initialize(child: &mut Child) -> Result<String, String> {
    use std::io::Write;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let (input, inputs) = mpsc::channel();
    read_messages(child.stdout.take().expect("stdout is piped"), input);
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": protocol::initialize_params()});
    // An agent that already exited breaks the pipe; its closed stdout says so below.
    let _ = writeln!(stdin, "{request}").and_then(|()| stdin.flush());

    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match inputs.recv_timeout(left) {
            Ok(Input::Message(message))
                if message["id"] == 1 && message.get("method").is_none() =>
            {
                return match message.get("error").filter(|e| !e.is_null()) {
                    Some(error) => Err(protocol::failure_text(error, &Value::Null)),
                    None => Ok(protocol::agent_name(&message["result"])),
                };
            }
            Ok(Input::Message(_) | Input::Command(_)) => {}
            Ok(Input::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Err("the agent exited without answering `initialize`".to_owned());
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "the agent did not answer `initialize` within {} s",
                    PROBE_TIMEOUT.as_secs()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn launcher(command: &Path, args: &[&str]) -> Launcher {
        Launcher {
            command: Some(command.to_owned()),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            config_dir: None,
        }
    }

    /// Runs a fake agent through `sh`, the way a launcher prefix runs a command. Executing
    /// the freshly written file itself races with other tests forking (`Text file busy`).
    fn fake(script: &Path) -> Launcher {
        launcher(Path::new("/bin/sh"), &[script.to_str().unwrap()])
    }

    /// A fake agent: a shell script in a fresh temp dir, which the test removes.
    #[cfg(unix)]
    fn script(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zide-acp-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn next(events: &Receiver<Event>) -> Event {
        events
            .recv_timeout(Duration::from_secs(10))
            .expect("an event")
    }

    #[test]
    fn an_acp_agent_needs_a_command() {
        let none = Launcher {
            command: None,
            args: Vec::new(),
            config_dir: None,
        };
        let error = spawn(&none, Path::new(".")).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(probe(&none).unwrap_err().contains("needs a command"));
    }

    #[cfg(unix)]
    #[test]
    fn a_conversation_with_a_fake_agent() {
        // ids are deterministic: 1 initialize, 2 session/new, 3 the first prompt
        let path = script(
            "chat",
            r#"read -r line
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"image":false}},"authMethods":[]}}'
read -r line
echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s-1","models":{"currentModelId":"model-x"}}}'
read -r line
echo 'not json'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}}}'
echo '{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"s-1","toolCall":{"toolCallId":"t1","title":"Run ls","kind":"execute","rawInput":{"command":"ls"}},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
read -r reply
id=$(echo "$reply" | sed 's/.*"id":"\([^"]*\)".*/\1/')
option=$(echo "$reply" | sed 's/.*"optionId":"\([^"]*\)".*/\1/')
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"'"$option for $id"'"}}}}'
echo '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'"#,
        );
        let cwd = path.parent().unwrap();
        let (mut session, events) = spawn(&fake(&path), cwd).unwrap();
        assert_eq!(
            next(&events),
            Event::Init {
                session_id: "s-1".into(),
                commands: Vec::new(),
                model: Some("model-x".into()),
            }
        );
        session.send_with_images("hi", &[]).unwrap();
        assert_eq!(next(&events), Event::Consumed);
        assert_eq!(next(&events), Event::TextDelta("Hello".into()));
        let Event::PermissionRequest {
            request_id,
            tool_name,
            input,
        } = next(&events)
        else {
            panic!("expected a permission request");
        };
        assert_eq!(
            (request_id.as_str(), tool_name.as_str()),
            ("perm-1", "Run ls")
        );
        session.allow(&request_id, &input).unwrap();
        assert_eq!(next(&events), Event::TextDelta("yes for perm-1".into()));
        assert!(matches!(
            next(&events),
            Event::Result { is_error: false, terminal_reason: Some(r), .. } if r == "end_turn"
        ));
        assert_eq!(next(&events), Event::Exit);
        drop(session);
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn probing_names_the_agent() {
        let path = script(
            "probe",
            r#"read -r line
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"fake","title":"Fake Agent","version":"0.1.0"}}}'
read -r line"#,
        );
        assert_eq!(probe(&fake(&path)), Ok("Fake Agent 0.1.0".into()));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_probe_says_why() {
        let path = script("probe-fail", "exit 1");
        assert!(probe(&fake(&path)).unwrap_err().contains("exited"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// Spends the user's Copilot quota: `cargo test -p zide-acp -- --ignored`.
    #[test]
    #[ignore]
    fn copilot_answers_a_prompt() {
        let copilot = launcher(Path::new("copilot"), &["--acp", "--stdio"]);
        let cwd = std::env::temp_dir().join(format!("zide-acp-live-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let (mut session, events) = spawn(&copilot, &cwd).unwrap();
        session
            .send_with_images("Reply with exactly: pong", &[])
            .unwrap();
        let mut text = String::new();
        let result = loop {
            match events.recv_timeout(Duration::from_secs(120)).unwrap() {
                Event::TextDelta(chunk) => text.push_str(&chunk),
                event @ Event::Result { .. } => break event,
                Event::Exit => panic!("copilot exited"),
                _ => {}
            }
        };
        assert!(
            matches!(
                result,
                Event::Result {
                    is_error: false,
                    ..
                }
            ),
            "{result:?}"
        );
        assert!(text.to_lowercase().contains("pong"), "{text}");
        drop(session);
        std::fs::remove_dir_all(&cwd).unwrap();
    }
}
