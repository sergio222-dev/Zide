//! The worker that owns an ACP connection: it writes JSON-RPC to the agent's stdin, reads
//! what the reader thread forwards from its stdout, and keeps every bit of protocol state
//! (request ids, the session, queued messages, open permission requests) on one thread,
//! so none of it needs a lock.

use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    path::PathBuf,
    sync::mpsc::Sender,
};

use serde_json::{Value, json};
use zide_claude::{Event, Image};

use crate::protocol::{
    Updates, cancelled_outcome, failure_text, initialize_params, new_session_params,
    permission_outcome, prompt_content, turn_end,
};

/// Everything the worker reacts to, in the order it happened.
pub(crate) enum Input {
    /// A JSON message the agent printed.
    Message(Value),
    /// What the chat asked for through [`crate::AcpSession`].
    Command(Command),
    /// The agent's stdout closed: the process is gone.
    Closed,
}

pub(crate) enum Command {
    Send(String, Vec<Image>),
    Interrupt,
    Allow(String),
    Deny(String),
}

/// What a response to one of our requests is for.
enum Pending {
    Initialize,
    NewSession,
    Prompt,
}

enum Session {
    /// `initialize` or `session/new` has not answered yet.
    Starting,
    Ready(String),
    /// The handshake failed for this reason; messages get it as their answer.
    Failed(String),
}

/// A permission request the agent waits on.
struct Permission {
    /// The JSON-RPC id exactly as the agent sent it (number or string).
    id: Value,
    options: Value,
}

pub(crate) struct Client<W> {
    out: W,
    events: Sender<Event>,
    cwd: PathBuf,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session: Session,
    takes_images: bool,
    auth_methods: Value,
    /// Messages the user sent that wait for the session or for the running prompt: ACP takes
    /// one prompt at a time.
    queue: VecDeque<(String, Vec<Image>)>,
    prompting: bool,
    permissions: HashMap<String, Permission>,
    updates: Updates,
}

impl<W: Write> Client<W> {
    pub(crate) fn new(out: W, events: Sender<Event>, cwd: PathBuf) -> Self {
        Client {
            out,
            events,
            cwd,
            next_id: 1,
            pending: HashMap::new(),
            session: Session::Starting,
            takes_images: false,
            auth_methods: Value::Null,
            queue: VecDeque::new(),
            prompting: false,
            permissions: HashMap::new(),
            updates: Updates::default(),
        }
    }

    /// Opens the handshake; its outcome comes back as messages.
    pub(crate) fn start(&mut self) {
        self.request("initialize", initialize_params(), Pending::Initialize);
    }

    /// Reacts to one input; `false` once the agent is gone and the worker should stop.
    pub(crate) fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Message(message) => self.message(message),
            Input::Command(command) => self.command(command),
            Input::Closed => {
                if matches!(self.session, Session::Starting) {
                    self.fail("the agent exited before its session started".to_owned());
                }
                self.emit(Event::Exit);
                return false;
            }
        }
        true
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Send(text, images) => {
                self.queue.push_back((text, images));
                self.pump();
            }
            Command::Interrupt => {
                self.queue.clear();
                if let Session::Ready(id) = &self.session
                    && self.prompting
                {
                    let params = json!({"sessionId": id});
                    self.write(
                        &json!({"jsonrpc": "2.0", "method": "session/cancel", "params": params}),
                    );
                }
                // the spec has the client answer every open permission request with `cancelled`
                let open: Vec<Permission> = self.permissions.drain().map(|(_, p)| p).collect();
                for permission in open {
                    self.reply(permission.id, cancelled_outcome());
                }
            }
            Command::Allow(request_id) => self.decide(&request_id, true),
            Command::Deny(request_id) => self.decide(&request_id, false),
        }
    }

    fn decide(&mut self, request_id: &str, allow: bool) {
        if let Some(permission) = self.permissions.remove(request_id) {
            let outcome = permission_outcome(&permission.options, allow);
            self.reply(permission.id, outcome);
        }
    }

    fn message(&mut self, message: Value) {
        let method = message["method"].as_str().map(str::to_owned);
        match (method, message.get("id")) {
            (Some(method), Some(id)) => self.agent_request(&method, id.clone(), &message["params"]),
            (Some(method), None) => {
                if method == "session/update" {
                    for event in self.updates.events(&message["params"]["update"]) {
                        self.emit(event);
                    }
                }
            }
            (None, Some(id)) => {
                if let Some(pending) = id.as_u64().and_then(|id| self.pending.remove(&id)) {
                    self.response(pending, &message);
                }
            }
            (None, None) => {}
        }
    }

    fn agent_request(&mut self, method: &str, id: Value, params: &Value) {
        if method != "session/request_permission" {
            let error = json!({"code": -32601, "message": "Method not found"});
            self.write(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
            return;
        }
        let request_id = match &id {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        };
        let tool = &params["toolCall"];
        let tool_name = tool["title"]
            .as_str()
            .or(tool["kind"].as_str())
            .unwrap_or("tool");
        let input = match &tool["rawInput"] {
            Value::Null => json!({}),
            input => input.clone(),
        };
        let options = params["options"].clone();
        self.permissions
            .insert(request_id.clone(), Permission { id, options });
        self.emit(Event::PermissionRequest {
            request_id,
            tool_name: tool_name.to_owned(),
            input,
        });
    }

    fn response(&mut self, pending: Pending, message: &Value) {
        let result = &message["result"];
        let error = message.get("error").filter(|error| !error.is_null());
        match pending {
            Pending::Initialize => {
                if let Some(error) = error {
                    return self.fail(failure_text(error, &Value::Null));
                }
                self.takes_images = result["agentCapabilities"]["promptCapabilities"]["image"]
                    .as_bool()
                    .unwrap_or(false);
                self.auth_methods = result["authMethods"].clone();
                let params = new_session_params(&self.cwd);
                self.request("session/new", params, Pending::NewSession);
            }
            Pending::NewSession => {
                if let Some(error) = error {
                    return self.fail(failure_text(error, &self.auth_methods));
                }
                let Some(id) = result["sessionId"].as_str() else {
                    return self.fail("the agent started a session without an id".to_owned());
                };
                self.session = Session::Ready(id.to_owned());
                self.emit(Event::Init {
                    session_id: id.to_owned(),
                    commands: Vec::new(),
                    model: result["models"]["currentModelId"]
                        .as_str()
                        .map(str::to_owned),
                });
                self.pump();
            }
            Pending::Prompt => {
                self.prompting = false;
                self.emit(match error {
                    Some(error) => Event::Result {
                        text: error["message"]
                            .as_str()
                            .unwrap_or("the prompt failed")
                            .to_owned(),
                        is_error: true,
                        cost_usd: 0.0,
                        terminal_reason: None,
                    },
                    None => turn_end(result["stopReason"].as_str().unwrap_or("end_turn")),
                });
                self.pump();
            }
        }
    }

    /// The handshake failed: the user sees why, and every message waiting for the session
    /// is answered with the same reason so the chat does not wait forever.
    fn fail(&mut self, reason: String) {
        self.emit(failure(&reason));
        self.session = Session::Failed(reason);
        self.pump();
    }

    /// Sends the next queued message when the session can take it.
    fn pump(&mut self) {
        match &self.session {
            Session::Starting => {}
            Session::Failed(reason) => {
                let reason = reason.clone();
                while self.queue.pop_front().is_some() {
                    self.emit(Event::Consumed);
                    self.emit(failure(&reason));
                }
            }
            Session::Ready(id) => {
                if self.prompting {
                    return;
                }
                let Some((text, images)) = self.queue.pop_front() else {
                    return;
                };
                let params = json!({
                    "sessionId": id,
                    "prompt": prompt_content(&text, &images, self.takes_images),
                });
                self.prompting = true;
                self.emit(Event::Consumed);
                self.request("session/prompt", params, Pending::Prompt);
            }
        }
    }

    fn request(&mut self, method: &str, params: Value, pending: Pending) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn reply(&mut self, id: Value, result: Value) {
        self.write(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    /// A failed write means the process is gone; its stdout closing reports that.
    fn write(&mut self, message: &Value) {
        let _ = writeln!(self.out, "{message}").and_then(|()| self.out.flush());
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}

fn failure(reason: &str) -> Event {
    Event::Result {
        text: reason.to_owned(),
        is_error: true,
        cost_usd: 0.0,
        terminal_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use zide_claude::CANCELLED;

    use super::*;

    fn client() -> (Client<Vec<u8>>, Receiver<Event>) {
        let (sender, receiver) = mpsc::channel();
        let mut client = Client::new(Vec::new(), sender, PathBuf::from("/project"));
        client.start();
        (client, receiver)
    }

    /// The messages the client wrote since the last call.
    fn written(client: &mut Client<Vec<u8>>) -> Vec<Value> {
        let out = std::mem::take(&mut client.out);
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn events(receiver: &Receiver<Event>) -> Vec<Event> {
        receiver.try_iter().collect()
    }

    fn respond(client: &mut Client<Vec<u8>>, id: u64, result: Value) {
        client.handle(Input::Message(
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        ));
    }

    /// A client whose session `s-1` is ready, with what the handshake wrote and emitted
    /// already taken.
    fn ready(takes_images: bool) -> (Client<Vec<u8>>, Receiver<Event>) {
        let (mut client, receiver) = client();
        let capabilities = json!({"promptCapabilities": {"image": takes_images}});
        respond(
            &mut client,
            1,
            json!({"protocolVersion": 1, "agentCapabilities": capabilities}),
        );
        respond(&mut client, 2, json!({"sessionId": "s-1"}));
        written(&mut client);
        events(&receiver);
        (client, receiver)
    }

    #[test]
    fn the_handshake_initializes_then_opens_a_session_in_the_project() {
        let (mut client, receiver) = client();
        let sent = written(&mut client);
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["id"], 1);
        respond(&mut client, 1, json!({"protocolVersion": 1}));
        let sent = written(&mut client);
        assert_eq!(sent[0]["method"], "session/new");
        assert_eq!(sent[0]["id"], 2);
        assert_eq!(
            sent[0]["params"],
            json!({"cwd": "/project", "mcpServers": []})
        );
        respond(
            &mut client,
            2,
            json!({"sessionId": "s-1", "models": {"currentModelId": "m"}}),
        );
        assert_eq!(
            events(&receiver),
            [Event::Init {
                session_id: "s-1".into(),
                commands: Vec::new(),
                model: Some("m".into()),
            }]
        );
    }

    #[test]
    fn a_message_sent_before_the_session_waits_for_it() {
        let (mut client, receiver) = client();
        client.handle(Input::Command(Command::Send("hi".into(), Vec::new())));
        respond(&mut client, 1, json!({}));
        assert!(events(&receiver).is_empty());
        respond(&mut client, 2, json!({"sessionId": "s-1"}));
        assert!(matches!(
            &events(&receiver)[..],
            [Event::Init { .. }, Event::Consumed]
        ));
        let prompt = written(&mut client).pop().unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["id"], 3);
        assert_eq!(
            prompt["params"],
            json!({"sessionId": "s-1", "prompt": [{"type": "text", "text": "hi"}]})
        );
    }

    #[test]
    fn one_prompt_runs_at_a_time() {
        let (mut client, receiver) = ready(false);
        client.handle(Input::Command(Command::Send("one".into(), Vec::new())));
        client.handle(Input::Command(Command::Send("two".into(), Vec::new())));
        assert_eq!(written(&mut client).len(), 1);
        assert_eq!(events(&receiver), [Event::Consumed]);
        respond(&mut client, 3, json!({"stopReason": "end_turn"}));
        assert!(matches!(
            &events(&receiver)[..],
            [Event::Result { .. }, Event::Consumed]
        ));
        let next = written(&mut client);
        assert_eq!(next[0]["params"]["prompt"][0]["text"], "two");
    }

    #[test]
    fn pictures_go_only_to_an_agent_that_takes_them() {
        let image = Image {
            media_type: "image/png",
            bytes: vec![1],
        };
        for takes_images in [true, false] {
            let (mut client, _receiver) = ready(takes_images);
            client.handle(Input::Command(Command::Send(
                "x".into(),
                vec![image.clone()],
            )));
            let prompt = &written(&mut client)[0]["params"]["prompt"];
            assert_eq!(prompt.as_array().unwrap().len(), 1 + takes_images as usize);
        }
    }

    #[test]
    fn a_failed_prompt_ends_the_turn_with_its_message() {
        let (mut client, receiver) = ready(false);
        client.handle(Input::Command(Command::Send("x".into(), Vec::new())));
        events(&receiver);
        let error = json!({"code": -32603, "message": "boom"});
        client.handle(Input::Message(
            json!({"jsonrpc": "2.0", "id": 3, "error": error}),
        ));
        assert_eq!(
            events(&receiver),
            [Event::Result {
                text: "boom".into(),
                is_error: true,
                cost_usd: 0.0,
                terminal_reason: None,
            }]
        );
    }

    #[test]
    fn a_failed_handshake_answers_waiting_messages_with_the_reason() {
        let (mut client, receiver) = client();
        respond(
            &mut client,
            1,
            json!({"authMethods": [{"id": "l", "description": "Log in first"}]}),
        );
        client.handle(Input::Command(Command::Send("x".into(), Vec::new())));
        let error = json!({"code": -32000, "message": "Authentication required"});
        client.handle(Input::Message(
            json!({"jsonrpc": "2.0", "id": 2, "error": error}),
        ));
        let reason = "Authentication required\n\nLog in first";
        assert_eq!(
            events(&receiver),
            [failure(reason), Event::Consumed, failure(reason)]
        );
        client.handle(Input::Command(Command::Send("y".into(), Vec::new())));
        assert_eq!(events(&receiver), [Event::Consumed, failure(reason)]);
    }

    #[test]
    fn an_agent_that_exits_during_the_handshake_says_so_and_exits() {
        let (mut client, receiver) = client();
        assert!(!client.handle(Input::Closed));
        assert!(matches!(
            &events(&receiver)[..],
            [Event::Result { is_error: true, .. }, Event::Exit]
        ));
    }

    fn ask_permission(client: &mut Client<Vec<u8>>, id: Value) {
        client.handle(Input::Message(json!({
            "jsonrpc": "2.0", "id": id, "method": "session/request_permission",
            "params": {"sessionId": "s-1",
                "toolCall": {"toolCallId": "t1", "title": "Run ls", "kind": "execute", "rawInput": {"command": "ls"}},
                "options": [
                    {"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                    {"optionId": "no", "name": "No", "kind": "reject_once"},
                ]},
        })));
    }

    #[test]
    fn a_permission_request_is_answered_with_the_chosen_option_and_its_own_id() {
        let (mut client, receiver) = ready(false);
        ask_permission(&mut client, json!(7));
        assert_eq!(
            events(&receiver),
            [Event::PermissionRequest {
                request_id: "7".into(),
                tool_name: "Run ls".into(),
                input: json!({"command": "ls"}),
            }]
        );
        client.handle(Input::Command(Command::Allow("7".into())));
        let reply = &written(&mut client)[0];
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["outcome"]["optionId"], "yes");

        ask_permission(&mut client, json!("p-2"));
        client.handle(Input::Command(Command::Deny("p-2".into())));
        let reply = &written(&mut client)[0];
        assert_eq!(reply["id"], "p-2");
        assert_eq!(reply["result"]["outcome"]["optionId"], "no");
    }

    #[test]
    fn interrupting_cancels_the_prompt_and_every_open_permission() {
        let (mut client, receiver) = ready(false);
        client.handle(Input::Command(Command::Send("x".into(), Vec::new())));
        client.handle(Input::Command(Command::Send("queued".into(), Vec::new())));
        ask_permission(&mut client, json!(9));
        written(&mut client);
        events(&receiver);
        client.handle(Input::Command(Command::Interrupt));
        let sent = written(&mut client);
        assert_eq!(sent[0]["method"], "session/cancel");
        assert_eq!(sent[0]["params"], json!({"sessionId": "s-1"}));
        assert!(sent[0].get("id").is_none(), "a notification");
        assert_eq!(sent[1]["id"], 9);
        assert_eq!(sent[1]["result"], cancelled_outcome());
        respond(&mut client, 3, json!({"stopReason": "cancelled"}));
        assert!(matches!(
            &events(&receiver)[..],
            [Event::Result { terminal_reason: Some(r), .. }] if r == CANCELLED
        ));
        assert!(
            written(&mut client).is_empty(),
            "the queued message was dropped"
        );
    }

    #[test]
    fn requests_the_client_does_not_serve_are_refused() {
        let (mut client, receiver) = ready(false);
        for method in ["fs/read_text_file", "terminal/create", "something/new"] {
            client.handle(Input::Message(
                json!({"jsonrpc": "2.0", "id": "r", "method": method, "params": {}}),
            ));
            let reply = &written(&mut client)[0];
            assert_eq!(reply["id"], "r");
            assert_eq!(reply["error"]["code"], -32601);
        }
        assert!(events(&receiver).is_empty());
    }

    #[test]
    fn session_updates_become_events_and_noise_is_ignored() {
        let (mut client, receiver) = ready(false);
        let update = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}});
        client.handle(Input::Message(
            json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s-1", "update": update}}),
        ));
        respond(&mut client, 99, json!({}));
        client.handle(Input::Message(json!({"jsonrpc": "2.0"})));
        assert_eq!(events(&receiver), [Event::TextDelta("hi".into())]);
        assert!(written(&mut client).is_empty());
    }
}
