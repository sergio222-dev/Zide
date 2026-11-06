use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// The part of the stream a client cares about; everything else is `Other`.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The session is ready. `session_id` is what `Session::spawn` takes to resume it.
    Init { session_id: String },
    /// A chunk of the assistant's text as it is produced.
    TextDelta(String),
    /// The assistant asked to run a tool.
    ToolUse { name: String },
    /// The turn is over.
    Result {
        text: String,
        is_error: bool,
        cost_usd: f64,
        /// Why the turn ended when it did not simply finish, e.g. `aborted_streaming`
        /// after `Session::interrupt`.
        terminal_reason: Option<String>,
    },
    /// Claude wants to run a tool and waits for `Session::allow` or `Session::deny`.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        /// The tool's arguments, exactly what `Session::allow` must echo back.
        input: Value,
    },
    /// Claude asks the user to choose and waits for `Session::answer`.
    Question {
        request_id: String,
        /// The request's input, which `Session::answer` echoes back with the answers.
        input: Value,
        questions: Vec<Question>,
    },
    Other,
}

fn parse_questions(input: &Value) -> Vec<Question> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| Question {
            question: text(&q["question"]),
            header: text(&q["header"]),
            multi_select: q["multiSelect"].as_bool().unwrap_or(false),
            options: q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| QuestionOption {
                    label: text(&o["label"]),
                    description: text(&o["description"]),
                })
                .collect(),
        })
        .collect()
}

/// Returns `None` for a line that is not JSON.
pub fn parse_event(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line).ok()?;
    Some(match value["type"].as_str()? {
        "system" if value["subtype"] == "init" => Event::Init {
            session_id: value["session_id"].as_str()?.to_owned(),
        },
        "stream_event" if value["event"]["delta"]["type"] == "text_delta" => {
            Event::TextDelta(value["event"]["delta"]["text"].as_str()?.to_owned())
        }
        "assistant" => value["message"]["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "tool_use"))
            .and_then(|block| block["name"].as_str())
            .map_or(Event::Other, |name| Event::ToolUse {
                name: name.to_owned(),
            }),
        "control_request"
            if value["request"]["subtype"] == "can_use_tool"
                && value["request"]["tool_name"] == "AskUserQuestion" =>
        {
            let input = value["request"]["input"].clone();
            Event::Question {
                request_id: value["request_id"].as_str()?.to_owned(),
                questions: parse_questions(&input),
                input,
            }
        }
        "control_request" if value["request"]["subtype"] == "can_use_tool" => {
            Event::PermissionRequest {
                request_id: value["request_id"].as_str()?.to_owned(),
                tool_name: value["request"]["tool_name"].as_str()?.to_owned(),
                input: value["request"]["input"].clone(),
            }
        }
        "result" => Event::Result {
            text: value["result"].as_str().unwrap_or_default().to_owned(),
            is_error: value["is_error"].as_bool().unwrap_or(false),
            cost_usd: value["total_cost_usd"].as_f64().unwrap_or(0.0),
            terminal_reason: value["terminal_reason"].as_str().map(str::to_owned),
        },
        _ => Event::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_carries_the_session_id() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc","cwd":"/x"}"#;
        assert_eq!(
            parse_event(line),
            Some(Event::Init {
                session_id: "abc".into()
            })
        );
    }

    #[test]
    fn other_system_events_are_ignored() {
        let line = r#"{"type":"system","subtype":"hook_started"}"#;
        assert_eq!(parse_event(line), Some(Event::Other));
    }

    #[test]
    fn text_delta_carries_the_chunk() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hola"}}}"#;
        assert_eq!(parse_event(line), Some(Event::TextDelta("Hola".into())));
    }

    #[test]
    fn other_deltas_are_ignored() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#;
        assert_eq!(parse_event(line), Some(Event::Other));
    }

    #[test]
    fn assistant_tool_use_is_reported_by_name() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","name":"Bash","input":{}}]}}"#;
        assert_eq!(
            parse_event(line),
            Some(Event::ToolUse {
                name: "Bash".into()
            })
        );
    }

    #[test]
    fn assistant_text_only_is_other() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_event(line), Some(Event::Other));
    }

    #[test]
    fn result_ends_the_turn() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.5}"#;
        assert_eq!(
            parse_event(line),
            Some(Event::Result {
                text: "done".into(),
                is_error: false,
                cost_usd: 0.5,
                terminal_reason: None,
            })
        );
    }

    #[test]
    fn interrupted_result_says_why() {
        let line = r#"{"type":"result","is_error":false,"terminal_reason":"aborted_streaming"}"#;
        assert!(matches!(
            parse_event(line),
            Some(Event::Result { terminal_reason: Some(r), .. }) if r == "aborted_streaming"
        ));
    }

    #[test]
    fn error_result_is_flagged() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;
        assert!(matches!(
            parse_event(line),
            Some(Event::Result { is_error: true, .. })
        ));
    }

    #[test]
    fn can_use_tool_is_a_permission_request() {
        let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#;
        assert_eq!(
            parse_event(line),
            Some(Event::PermissionRequest {
                request_id: "r1".into(),
                tool_name: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
            })
        );
    }

    #[test]
    fn ask_user_question_is_a_question_not_a_permission() {
        let line = r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Color?","header":"Color","options":[{"label":"Rojo","description":"warm"},{"label":"Azul","description":"cold"}],"multiSelect":false}]}}}"#;
        let Some(Event::Question {
            request_id,
            questions,
            ..
        }) = parse_event(line)
        else {
            panic!("expected a question");
        };
        assert_eq!(request_id, "r2");
        assert_eq!(
            questions,
            vec![Question {
                question: "Color?".into(),
                header: "Color".into(),
                multi_select: false,
                options: vec![
                    QuestionOption {
                        label: "Rojo".into(),
                        description: "warm".into()
                    },
                    QuestionOption {
                        label: "Azul".into(),
                        description: "cold".into()
                    },
                ],
            }]
        );
    }

    #[test]
    fn other_control_requests_are_ignored() {
        let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback"}}"#;
        assert_eq!(parse_event(line), Some(Event::Other));
    }

    #[test]
    fn garbage_is_not_an_event() {
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"no":"type"}"#), None);
    }
}
