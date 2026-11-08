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

/// How full the model's context is, as of its latest message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Everything sent to the model: fresh, cached and cache-written input.
    pub context_tokens: u64,
    pub output_tokens: u64,
}

/// How much of a usage limit is spent, and when it starts over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// From 0 to 1.
    pub utilization: f64,
    /// Seconds since the Unix epoch.
    pub resets_at: u64,
}

/// The account's usage limits, as the CLI last reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Limits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

fn parse_window(value: &Value) -> Option<Window> {
    Some(Window {
        utilization: value["utilization"].as_f64()?,
        resets_at: value["resetsAt"].as_u64().unwrap_or(0),
    })
}

/// The part of the stream a client cares about. Lines that carry nothing a chat shows
/// (hooks, status, rate limits, protocol bookkeeping) produce no event.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The session is ready. `session_id` is what `Session::spawn` takes to resume it;
    /// `commands` are the names the user can type after `/` (skills included).
    Init {
        session_id: String,
        commands: Vec<String>,
        /// The model the session runs, as its full name.
        model: Option<String>,
    },
    /// A chunk of the assistant's text as it is produced.
    TextDelta(String),
    /// A chunk of the assistant's reasoning as it is produced.
    ThinkingDelta(String),
    /// The assistant asked to run a tool. The stream may repeat the same `id`.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// What a tool printed, for the `ToolUse` with the same id.
    ToolResult {
        tool_use_id: String,
        output: String,
        is_error: bool,
    },
    Usage(Usage),
    /// The account's usage limits.
    Limits(Limits),
    /// How many tokens the model's context holds, reported when a turn ends.
    ContextWindow(u64),
    /// The turn is over.
    Result {
        text: String,
        is_error: bool,
        /// The cost the CLI reports in this result.
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
    /// The process is gone; nothing more will arrive.
    Exit,
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

/// A tool result's content is a string, or a list of blocks whose text is what counts.
fn tool_output(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The usage of a message the model wrote. Messages the CLI makes up itself (the answer to a
/// local command like `/context`) report all zeros: that is no measure of the context.
fn parse_usage(usage: &Value) -> Option<Usage> {
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let usage = Usage {
        context_tokens: count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens"),
        output_tokens: count("output_tokens"),
    };
    (usage.context_tokens > 0 || usage.output_tokens > 0).then_some(usage)
}

/// The events one line of the stream stands for: none for noise or text that is not
/// JSON, several when one message holds several blocks.
pub fn parse_events(line: &str) -> Vec<Event> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    // A subagent's own messages are not part of the conversation the user is having.
    if value["parent_tool_use_id"].is_string() {
        return Vec::new();
    }
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    match value["type"].as_str().unwrap_or_default() {
        "system" if value["subtype"] == "init" => value["session_id"]
            .as_str()
            .map(|id| Event::Init {
                session_id: id.to_owned(),
                commands: value["slash_commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect(),
                model: value["model"].as_str().map(str::to_owned),
            })
            .into_iter()
            .collect(),
        "stream_event" => {
            let delta = &value["event"]["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => vec![Event::TextDelta(text(&delta["text"]))],
                Some("thinking_delta") => vec![Event::ThinkingDelta(text(&delta["thinking"]))],
                _ => Vec::new(),
            }
        }
        "assistant" => {
            let message = &value["message"];
            let tools = message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "tool_use")
                .filter_map(|block| {
                    Some(Event::ToolUse {
                        id: block["id"].as_str()?.to_owned(),
                        name: block["name"].as_str()?.to_owned(),
                        input: block["input"].clone(),
                    })
                });
            tools
                .chain(parse_usage(&message["usage"]).map(Event::Usage))
                .collect()
        }
        "user" => value["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_result")
            .filter_map(|block| {
                Some(Event::ToolResult {
                    tool_use_id: block["tool_use_id"].as_str()?.to_owned(),
                    output: tool_output(&block["content"]),
                    is_error: block["is_error"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
        "control_request" if value["request"]["subtype"] == "can_use_tool" => {
            let Some(request_id) = value["request_id"].as_str() else {
                return Vec::new();
            };
            let request = &value["request"];
            let input = request["input"].clone();
            vec![if request["tool_name"] == "AskUserQuestion" {
                Event::Question {
                    request_id: request_id.to_owned(),
                    questions: parse_questions(&input),
                    input,
                }
            } else {
                Event::PermissionRequest {
                    request_id: request_id.to_owned(),
                    tool_name: text(&request["tool_name"]),
                    input,
                }
            }]
        }
        "rate_limit_event" => {
            let windows = &value["rate_limit_info"]["unifiedWindows"];
            let limits = Limits {
                five_hour: parse_window(&windows["five_hour"]),
                seven_day: parse_window(&windows["seven_day"]),
            };
            if limits == Limits::default() {
                Vec::new()
            } else {
                vec![Event::Limits(limits)]
            }
        }
        "result" => {
            let window = value["modelUsage"].as_object().and_then(|models| {
                models
                    .values()
                    .filter_map(|m| m["contextWindow"].as_u64())
                    .max()
            });
            window
                .map(Event::ContextWindow)
                .into_iter()
                .chain([Event::Result {
                    text: text(&value["result"]),
                    is_error: value["is_error"].as_bool().unwrap_or(false),
                    cost_usd: value["total_cost_usd"].as_f64().unwrap_or(0.0),
                    terminal_reason: value["terminal_reason"].as_str().map(str::to_owned),
                }])
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn one(line: &str) -> Event {
        let mut events = parse_events(line);
        assert_eq!(events.len(), 1, "{events:?}");
        events.remove(0)
    }

    #[test]
    fn init_carries_the_session_id_and_the_commands() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc","cwd":"/x","slash_commands":["compact","review"],"model":"claude-x"}"#;
        assert_eq!(
            one(line),
            Event::Init {
                session_id: "abc".into(),
                commands: vec!["compact".into(), "review".into()],
                model: Some("claude-x".into()),
            }
        );
        let bare = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
        assert_eq!(
            one(bare),
            Event::Init {
                session_id: "abc".into(),
                commands: Vec::new(),
                model: None,
            }
        );
    }

    #[test]
    fn other_system_events_are_ignored() {
        let line = r#"{"type":"system","subtype":"hook_started"}"#;
        assert_eq!(parse_events(line), vec![]);
    }

    #[test]
    fn text_delta_carries_the_chunk() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hola"}}}"#;
        assert_eq!(one(line), Event::TextDelta("Hola".into()));
    }

    #[test]
    fn thinking_delta_carries_the_chunk() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#;
        assert_eq!(one(line), Event::ThinkingDelta("hmm".into()));
    }

    #[test]
    fn other_deltas_are_ignored() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#;
        assert_eq!(parse_events(line), vec![]);
    }

    #[test]
    fn assistant_tool_use_carries_id_name_and_input() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert_eq!(
            one(line),
            Event::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "ls"}),
            }
        );
    }

    #[test]
    fn one_message_with_two_tool_uses_gives_two_events() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Read","input":{}},{"type":"tool_use","id":"b","name":"Grep","input":{}}]}}"#;
        let ids: Vec<_> = parse_events(line)
            .into_iter()
            .filter_map(|e| match e {
                Event::ToolUse { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn assistant_text_only_is_not_an_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_events(line), vec![]);
    }

    #[test]
    fn a_rate_limit_event_carries_both_windows() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.77,"resetsAt":100,"status":"allowed"},"seven_day":{"utilization":0.5,"resetsAt":200}}}}"#;
        assert_eq!(
            one(line),
            Event::Limits(Limits {
                five_hour: Some(Window {
                    utilization: 0.77,
                    resets_at: 100
                }),
                seven_day: Some(Window {
                    utilization: 0.5,
                    resets_at: 200
                }),
            })
        );
        assert!(parse_events(r#"{"type":"rate_limit_event"}"#).is_empty());
    }

    #[test]
    fn a_result_reports_the_context_window() {
        let line = r#"{"type":"result","result":"x","modelUsage":{"m":{"contextWindow":1000000}}}"#;
        let events = parse_events(line);
        assert_eq!(events[0], Event::ContextWindow(1_000_000));
        assert!(matches!(events[1], Event::Result { .. }));
    }

    #[test]
    fn assistant_usage_adds_up_the_context() {
        let line = r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":7}}}"#;
        assert_eq!(
            one(line),
            Event::Usage(Usage {
                context_tokens: 112,
                output_tokens: 7
            })
        );
    }

    #[test]
    fn a_subagents_messages_are_left_out() {
        let line = r#"{"type":"assistant","parent_tool_use_id":"t9","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#;
        assert_eq!(parse_events(line), vec![]);
        let delta = r#"{"type":"stream_event","parent_tool_use_id":"t9","event":{"delta":{"type":"text_delta","text":"x"}}}"#;
        assert_eq!(parse_events(delta), vec![]);
    }

    #[test]
    fn tool_result_with_a_string_content() {
        let line = r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"hello"}]}}"#;
        assert_eq!(
            one(line),
            Event::ToolResult {
                tool_use_id: "t1".into(),
                output: "hello".into(),
                is_error: false,
            }
        );
    }

    #[test]
    fn tool_result_with_block_content_and_an_error() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","is_error":true,"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}]}}"#;
        assert_eq!(
            one(line),
            Event::ToolResult {
                tool_use_id: "t1".into(),
                output: "a\nb".into(),
                is_error: true,
            }
        );
    }

    #[test]
    fn a_plain_user_message_is_not_an_event() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        assert_eq!(parse_events(line), vec![]);
    }

    #[test]
    fn result_ends_the_turn() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.5}"#;
        assert_eq!(
            one(line),
            Event::Result {
                text: "done".into(),
                is_error: false,
                cost_usd: 0.5,
                terminal_reason: None,
            }
        );
    }

    #[test]
    fn interrupted_result_says_why() {
        let line = r#"{"type":"result","is_error":false,"terminal_reason":"aborted_streaming"}"#;
        assert!(matches!(
            one(line),
            Event::Result { terminal_reason: Some(r), .. } if r == "aborted_streaming"
        ));
    }

    #[test]
    fn a_made_up_message_has_no_usage() {
        let line = r#"{"type":"assistant","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"Context Usage"}],"usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        assert!(parse_events(line).is_empty());
    }

    #[test]
    fn error_result_is_flagged() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;
        assert!(matches!(one(line), Event::Result { is_error: true, .. }));
    }

    #[test]
    fn can_use_tool_is_a_permission_request() {
        let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#;
        assert_eq!(
            one(line),
            Event::PermissionRequest {
                request_id: "r1".into(),
                tool_name: "Bash".into(),
                input: json!({"command": "ls"}),
            }
        );
    }

    #[test]
    fn ask_user_question_is_a_question_not_a_permission() {
        let line = r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Color?","header":"Color","options":[{"label":"Rojo","description":"warm"},{"label":"Azul","description":"cold"}],"multiSelect":false}]}}}"#;
        let Event::Question {
            request_id,
            questions,
            ..
        } = one(line)
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
        let line =
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback"}}"#;
        assert_eq!(parse_events(line), vec![]);
    }

    #[test]
    fn garbage_is_not_an_event() {
        assert_eq!(parse_events("not json"), vec![]);
        assert_eq!(parse_events(r#"{"no":"type"}"#), vec![]);
        assert_eq!(parse_events(r#"{"type":"something_new"}"#), vec![]);
    }
}
