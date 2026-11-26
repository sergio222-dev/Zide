//! The pure half of the client: the messages it sends and what the agent's messages mean as
//! chat [`Event`]s. Nothing here touches a process, so every mapping is tested on its own.

use std::{collections::HashSet, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use zide_claude::{CANCELLED, Event, Image};

/// The ACP version this client speaks.
pub(crate) const PROTOCOL_VERSION: u64 = 1;

/// What `initialize` says about the client: no file system and no terminal, so the agent
/// does that work itself instead of asking the editor to.
pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        },
        "clientInfo": {"name": "zide", "version": env!("CARGO_PKG_VERSION")},
    })
}

pub(crate) fn new_session_params(cwd: &Path) -> Value {
    json!({"cwd": cwd, "mcpServers": []})
}

/// The `prompt` of a `session/prompt`: the text, then the pictures when the agent said it
/// takes them. An agent that did not is not sent any, since it would reject the whole prompt.
pub(crate) fn prompt_content(text: &str, images: &[Image], takes_images: bool) -> Value {
    let images: Vec<Value> = images
        .iter()
        .filter(|_| takes_images)
        .map(|image| {
            json!({
                "type": "image",
                "mimeType": image.media_type,
                "data": STANDARD.encode(&image.bytes),
            })
        })
        .collect();
    // a prompt is never empty: an empty text stands in when nothing else goes
    let text =
        (!text.is_empty() || images.is_empty()).then(|| json!({"type": "text", "text": text}));
    Value::Array(text.into_iter().chain(images).collect())
}

/// The `Result` that ends a turn for a `session/prompt` that stopped for `stop_reason`.
pub(crate) fn turn_end(stop_reason: &str) -> Event {
    Event::Result {
        text: String::new(),
        is_error: stop_reason == "refusal",
        cost_usd: 0.0,
        terminal_reason: Some(if stop_reason == "cancelled" {
            CANCELLED.to_owned()
        } else {
            stop_reason.to_owned()
        }),
    }
}

/// The answer to a `session/request_permission`: the first option of the kind the user chose
/// (once before always), or a cancellation when the agent offered none of that kind.
pub(crate) fn permission_outcome(options: &Value, allow: bool) -> Value {
    let kinds = if allow {
        ["allow_once", "allow_always"]
    } else {
        ["reject_once", "reject_always"]
    };
    let options = options.as_array().map(Vec::as_slice).unwrap_or_default();
    kinds
        .iter()
        .find_map(|kind| options.iter().find(|option| option["kind"] == *kind))
        .and_then(|option| option["optionId"].as_str())
        .map_or_else(
            cancelled_outcome,
            |id| json!({"outcome": {"outcome": "selected", "optionId": id}}),
        )
}

/// The answer to a permission request the turn no longer waits for.
pub(crate) fn cancelled_outcome() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

/// Why the handshake failed, as the user reads it: the agent's message, plus how to log in
/// when the agent says that is what is missing.
pub(crate) fn failure_text(error: &Value, auth_methods: &Value) -> String {
    let message = error["message"]
        .as_str()
        .unwrap_or("the agent refused to start a session")
        .to_owned();
    let lower = message.to_lowercase();
    let about_auth = error["code"] == -32000
        || ["auth", "login", "log in", "logged in"]
            .iter()
            .any(|word| lower.contains(word));
    let hint = auth_methods
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|method| method["description"].as_str())
        .filter(|_| about_auth);
    match hint {
        Some(hint) => format!("{message}\n\n{hint}"),
        None => message,
    }
}

/// What `initialize` says the agent is: `"<title or name> <version>"`.
pub(crate) fn agent_name(result: &Value) -> String {
    let info = &result["agentInfo"];
    let Some(name) = info["title"].as_str().or(info["name"].as_str()) else {
        return format!(
            "ACP agent (protocol {})",
            result["protocolVersion"]
                .as_u64()
                .unwrap_or(PROTOCOL_VERSION)
        );
    };
    match info["version"].as_str() {
        Some(version) => format!("{name} {version}"),
        None => name.to_owned(),
    }
}

/// Turns `session/update` notifications into events. It keeps which tool calls already
/// reported their result, since an agent may repeat the final status.
#[derive(Default)]
pub(crate) struct Updates {
    finished: HashSet<String>,
}

impl Updates {
    /// The events one `session/update`'s `update` stands for.
    pub(crate) fn events(&mut self, update: &Value) -> Vec<Event> {
        let text = || update["content"]["text"].as_str().map(str::to_owned);
        let is_text = update["content"]["type"] == "text";
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" if is_text => text().map(Event::TextDelta).into_iter().collect(),
            "agent_thought_chunk" if is_text => {
                text().map(Event::ThinkingDelta).into_iter().collect()
            }
            "tool_call" => {
                let Some(id) = update["toolCallId"].as_str() else {
                    return Vec::new();
                };
                let name = update["title"]
                    .as_str()
                    .or(update["kind"].as_str())
                    .unwrap_or("tool");
                let input = match &update["rawInput"] {
                    Value::Null => json!({}),
                    input => input.clone(),
                };
                let tool_use = Event::ToolUse {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    input,
                };
                [tool_use].into_iter().chain(self.result(update)).collect()
            }
            "tool_call_update" => self.result(update).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// The `ToolResult` of a call whose status became final, the first time it does.
    fn result(&mut self, update: &Value) -> Option<Event> {
        let id = update["toolCallId"].as_str()?;
        let status = update["status"].as_str()?;
        if !matches!(status, "completed" | "failed") || !self.finished.insert(id.to_owned()) {
            return None;
        }
        Some(Event::ToolResult {
            tool_use_id: id.to_owned(),
            output: tool_output(update),
            is_error: status == "failed",
        })
    }
}

/// What a finished tool call shows: the text it produced and the files it edited, else its
/// raw output.
fn tool_output(update: &Value) -> String {
    let parts: Vec<String> = update["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| match entry["type"].as_str()? {
            "content" if entry["content"]["type"] == "text" => {
                entry["content"]["text"].as_str().map(str::to_owned)
            }
            "diff" => entry["path"].as_str().map(|path| format!("Edited {path}")),
            _ => None,
        })
        .collect();
    if !parts.is_empty() {
        return parts.join("\n");
    }
    match &update["rawOutput"] {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        raw => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_chunk(kind: &str, text: &str) -> Value {
        json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}})
    }

    #[test]
    fn agent_text_and_thoughts_stream_as_deltas() {
        let mut updates = Updates::default();
        assert_eq!(
            updates.events(&text_chunk("agent_message_chunk", "Hola")),
            [Event::TextDelta("Hola".into())]
        );
        assert_eq!(
            updates.events(&text_chunk("agent_thought_chunk", "hmm")),
            [Event::ThinkingDelta("hmm".into())]
        );
    }

    #[test]
    fn a_chunk_that_is_not_text_is_ignored() {
        let image = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "image", "data": "x"}});
        assert_eq!(Updates::default().events(&image), []);
    }

    #[test]
    fn updates_the_chat_does_not_show_are_ignored() {
        let mut updates = Updates::default();
        for kind in [
            "user_message_chunk",
            "plan",
            "available_commands_update",
            "current_mode_update",
            "something_new",
        ] {
            let update = json!({"sessionUpdate": kind, "content": {"type": "text", "text": "x"}});
            assert_eq!(updates.events(&update), [], "{kind}");
        }
    }

    #[test]
    fn a_tool_call_is_a_tool_use_named_by_its_title() {
        let update = json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read file",
            "kind": "read", "status": "pending", "rawInput": {"path": "a.txt"}});
        assert_eq!(
            Updates::default().events(&update),
            [Event::ToolUse {
                id: "t1".into(),
                name: "Read file".into(),
                input: json!({"path": "a.txt"}),
            }]
        );
    }

    #[test]
    fn a_tool_call_without_title_or_input_falls_back() {
        let kind_only = json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "edit"});
        assert_eq!(
            Updates::default().events(&kind_only),
            [Event::ToolUse {
                id: "t1".into(),
                name: "edit".into(),
                input: json!({}),
            }]
        );
        let bare = json!({"sessionUpdate": "tool_call", "toolCallId": "t2"});
        assert!(matches!(
            &Updates::default().events(&bare)[..],
            [Event::ToolUse { name, .. }] if name == "tool"
        ));
    }

    #[test]
    fn a_tool_call_that_arrives_finished_also_gives_its_result() {
        let update = json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "a.txt"}}]});
        let events = Updates::default().events(&update);
        assert!(matches!(events[0], Event::ToolUse { .. }));
        assert_eq!(
            events[1],
            Event::ToolResult {
                tool_use_id: "t1".into(),
                output: "a.txt".into(),
                is_error: false,
            }
        );
    }

    #[test]
    fn a_tool_result_comes_once_when_the_call_finishes() {
        let mut updates = Updates::default();
        let running = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"});
        assert_eq!(updates.events(&running), []);
        let done = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [
            {"type": "content", "content": {"type": "text", "text": "one"}},
            {"type": "content", "content": {"type": "text", "text": "two"}},
        ]});
        assert_eq!(
            updates.events(&done),
            [Event::ToolResult {
                tool_use_id: "t1".into(),
                output: "one\ntwo".into(),
                is_error: false,
            }]
        );
        assert_eq!(updates.events(&done), [], "the final status repeated");
    }

    #[test]
    fn a_failed_tool_call_is_an_error_result() {
        let update = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "failed",
            "content": [{"type": "content", "content": {"type": "text", "text": "denied"}}]});
        assert!(matches!(
            &Updates::default().events(&update)[..],
            [Event::ToolResult { is_error: true, output, .. }] if output == "denied"
        ));
    }

    #[test]
    fn a_diff_reads_as_the_file_it_edited() {
        let update = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
            "content": [{"type": "diff", "path": "/x/a.rs", "oldText": "a", "newText": "b"}]});
        assert!(matches!(
            &Updates::default().events(&update)[..],
            [Event::ToolResult { output, .. }] if output == "Edited /x/a.rs"
        ));
    }

    #[test]
    fn without_content_the_raw_output_is_the_result() {
        let mut updates = Updates::default();
        let raw = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
            "rawOutput": {"exitCode": 0}});
        assert!(matches!(
            &updates.events(&raw)[..],
            [Event::ToolResult { output, .. }] if output == r#"{"exitCode":0}"#
        ));
        let nothing =
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "completed"});
        assert!(matches!(
            &updates.events(&nothing)[..],
            [Event::ToolResult { output, .. }] if output.is_empty()
        ));
    }

    #[test]
    fn each_stop_reason_ends_the_turn() {
        let end = |reason: &str| turn_end(reason);
        assert_eq!(
            end("end_turn"),
            Event::Result {
                text: String::new(),
                is_error: false,
                cost_usd: 0.0,
                terminal_reason: Some("end_turn".into()),
            }
        );
        assert!(
            matches!(end("max_tokens"), Event::Result { is_error: false, terminal_reason: Some(r), .. } if r == "max_tokens")
        );
        assert!(matches!(
            end("refusal"),
            Event::Result { is_error: true, .. }
        ));
        assert!(matches!(
            end("cancelled"),
            Event::Result { is_error: false, terminal_reason: Some(r), .. } if r == CANCELLED
        ));
    }

    fn options() -> Value {
        json!([
            {"optionId": "always", "name": "Always", "kind": "allow_always"},
            {"optionId": "once", "name": "Once", "kind": "allow_once"},
            {"optionId": "no", "name": "No", "kind": "reject_once"},
            {"optionId": "never", "name": "Never", "kind": "reject_always"},
        ])
    }

    fn selected(option: &str) -> Value {
        json!({"outcome": {"outcome": "selected", "optionId": option}})
    }

    #[test]
    fn allowing_picks_once_before_always() {
        assert_eq!(permission_outcome(&options(), true), selected("once"));
        let always_only = json!([{"optionId": "a", "kind": "allow_always"}]);
        assert_eq!(permission_outcome(&always_only, true), selected("a"));
    }

    #[test]
    fn denying_picks_reject_once_before_always() {
        assert_eq!(permission_outcome(&options(), false), selected("no"));
        let never_only = json!([{"optionId": "n", "kind": "reject_always"}]);
        assert_eq!(permission_outcome(&never_only, false), selected("n"));
    }

    #[test]
    fn with_no_option_of_that_kind_the_request_is_cancelled() {
        let allow_only = json!([{"optionId": "y", "kind": "allow_once"}]);
        assert_eq!(permission_outcome(&allow_only, false), cancelled_outcome());
        assert_eq!(permission_outcome(&json!(null), true), cancelled_outcome());
    }

    fn png() -> Image {
        Image {
            media_type: "image/png",
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn a_prompt_sends_its_text_and_the_pictures_the_agent_takes() {
        assert_eq!(
            prompt_content("look", &[png()], true),
            json!([
                {"type": "text", "text": "look"},
                {"type": "image", "mimeType": "image/png", "data": "AQID"},
            ])
        );
    }

    #[test]
    fn pictures_are_dropped_for_an_agent_that_takes_none() {
        assert_eq!(
            prompt_content("look", &[png()], false),
            json!([{"type": "text", "text": "look"}])
        );
    }

    #[test]
    fn a_picture_alone_needs_no_empty_text() {
        assert_eq!(
            prompt_content("", &[png()], true),
            json!([{"type": "image", "mimeType": "image/png", "data": "AQID"}])
        );
        assert_eq!(
            prompt_content("", &[], true),
            json!([{"type": "text", "text": ""}])
        );
    }

    #[test]
    fn an_auth_failure_says_how_to_log_in() {
        let methods = json!([{"id": "login", "name": "Log in", "description": "Run `agent login` in the terminal"}]);
        let by_code = json!({"code": -32000, "message": "Authentication required"});
        assert_eq!(
            failure_text(&by_code, &methods),
            "Authentication required\n\nRun `agent login` in the terminal"
        );
        let by_text = json!({"code": -32603, "message": "Not logged in"});
        assert!(failure_text(&by_text, &methods).ends_with("in the terminal"));
    }

    #[test]
    fn other_failures_keep_the_agents_message() {
        let methods = json!([{"id": "login", "description": "Run `agent login`"}]);
        let error = json!({"code": -32603, "message": "Internal error"});
        assert_eq!(failure_text(&error, &methods), "Internal error");
        let auth = json!({"code": -32000, "message": "Authentication required"});
        assert_eq!(failure_text(&auth, &json!([])), "Authentication required");
    }

    #[test]
    fn the_agent_is_named_by_its_title_and_version() {
        let titled = json!({"agentInfo": {"name": "agent", "title": "Agent", "version": "1.2.3"}});
        assert_eq!(agent_name(&titled), "Agent 1.2.3");
        let named = json!({"agentInfo": {"name": "agent", "version": "1.2.3"}});
        assert_eq!(agent_name(&named), "agent 1.2.3");
        let anonymous = json!({"protocolVersion": 1});
        assert_eq!(agent_name(&anonymous), "ACP agent (protocol 1)");
    }

    #[test]
    fn the_client_declares_neither_file_system_nor_terminal() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], 1);
        assert_eq!(params["clientCapabilities"]["terminal"], false);
        assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], false);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
        assert_eq!(params["clientInfo"]["name"], "zide");
    }
}
