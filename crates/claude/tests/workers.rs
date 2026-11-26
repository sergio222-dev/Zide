//! A real stream with two workers (captured from claude 2.1.322, sanitized): their own
//! conversations come out apart, and the chat's conversation stays what it was.

use zide_claude::{Block, Event, Transcript, parse_events};

const STREAM: &str = include_str!("fixtures/workers.jsonl");
const FIRST: &str = "toolu_012Htv5HL8KEA8kDycceTcRo";
const SECOND: &str = "toolu_018BCQvaBwTAs3cayQe1PN8U";

fn events() -> Vec<Event> {
    STREAM.lines().flat_map(parse_events).collect()
}

/// The events of worker `id`, unwrapped.
fn of_worker(id: &str) -> Vec<Event> {
    events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Worker { tool_use_id, event } if tool_use_id == id => Some(*event),
            _ => None,
        })
        .collect()
}

#[test]
fn both_workers_start_with_their_description_and_kind() {
    let started: Vec<_> = events()
        .into_iter()
        .filter_map(|event| match event {
            Event::WorkerStarted {
                tool_use_id,
                description,
                kind,
            } => Some((tool_use_id, description, kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        [
            (
                FIRST.to_owned(),
                "count files".to_owned(),
                "general-purpose".to_owned()
            ),
            (
                SECOND.to_owned(),
                "background count".to_owned(),
                "general-purpose".to_owned()
            ),
        ]
    );
}

#[test]
fn a_worker_reads_a_file_and_answers_in_its_own_events() {
    let [
        Event::ToolUse { id, name, .. },
        Event::ToolResult {
            tool_use_id,
            output,
            ..
        },
        Event::TextDelta(answer),
    ] = &of_worker(FIRST)[..]
    else {
        panic!("unexpected worker events: {:#?}", of_worker(FIRST));
    };
    assert_eq!(name, "Read");
    assert_eq!(tool_use_id, id);
    assert_eq!(output, "1\tzide-host\n2\t");
    assert_eq!(answer, "zide-host");
    assert_eq!(of_worker(SECOND).len(), 3);
}

#[test]
fn the_activity_and_the_end_of_each_worker_are_reported() {
    let events = events();
    for id in [FIRST, SECOND] {
        assert!(events.contains(&Event::WorkerActivity {
            tool_use_id: id.into(),
            activity: "Reading /etc/hostname".into(),
        }));
        assert!(events.contains(&Event::WorkerEnded {
            tool_use_id: id.into(),
            status: "completed".into(),
        }));
    }
}

#[test]
fn the_chats_own_events_are_the_agent_calls_and_the_results() {
    let main: Vec<_> = events().into_iter().filter(|e| !e.is_worker()).collect();
    let shape: Vec<String> = main
        .iter()
        .map(|event| match event {
            Event::ToolUse { id, name, .. } => format!("use {name} {id}"),
            Event::ToolResult { tool_use_id, .. } => format!("result {tool_use_id}"),
            Event::Result { text, .. } => format!("end {text}"),
            other => format!("{other:?}"),
        })
        .filter(|s| !s.starts_with("Usage") && !s.starts_with("ContextWindow"))
        .collect();
    assert_eq!(shape.len(), 7, "{shape:#?}");
    assert_eq!(shape[0], format!("use Agent {FIRST}"));
    assert_eq!(shape[1], format!("result {FIRST}"));
    assert_eq!(shape[2], format!("use Agent {SECOND}"));
    assert_eq!(shape[3], format!("result {SECOND}"));
    assert!(shape[4..].iter().all(|s| s.starts_with("end ")));
    // the lines of a worker's conversation give nothing but its wrapped events
    for line in STREAM
        .lines()
        .filter(|l| l.contains(r#""parent_tool_use_id":"toolu"#))
    {
        assert!(
            parse_events(line)
                .iter()
                .all(|e| matches!(e, Event::Worker { .. }))
        );
    }
}

#[test]
fn a_workers_text_never_enters_the_chats_transcript() {
    let mut transcript = Transcript::new();
    transcript.user_message("go");
    for event in events() {
        transcript.apply(event);
    }
    for block in transcript.blocks() {
        match block {
            // what the chat's assistant itself said about the result is its own text
            Block::Text(text) => assert!(!text.starts_with("zide-host"), "{text}"),
            Block::Thinking(text) => assert!(!text.contains("zide-host")),
            Block::Tool(call) => assert_eq!(call.name, "Agent"),
            _ => {}
        }
    }
    let tools = transcript
        .blocks()
        .iter()
        .filter(|b| matches!(b, Block::Tool(_)))
        .count();
    assert_eq!(tools, 2);
}
