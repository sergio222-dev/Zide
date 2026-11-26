//! A real turn (captured from claude 2.1.319, trimmed) through the parser and the transcript.

use zide_claude::{Block, Status, ToolOutput, Transcript, Usage, parse_events};

const TURN: &str = include_str!("fixtures/read-turn.jsonl");

fn replay(lines: impl Iterator<Item = &'static str>) -> Transcript {
    let mut transcript = Transcript::new();
    for line in lines {
        for event in parse_events(line) {
            transcript.apply(event);
        }
    }
    transcript
}

#[test]
fn a_tool_turn_becomes_a_tool_call_and_the_answer() {
    let mut transcript = Transcript::new();
    transcript.user_message("Read Cargo.toml and answer with just the version number.");
    for line in TURN.lines() {
        for event in parse_events(line) {
            transcript.apply(event);
        }
    }

    let [Block::User(_), Block::Tool(call), Block::Text(answer)] = transcript.blocks() else {
        panic!("unexpected blocks: {:#?}", transcript.blocks());
    };
    assert_eq!(call.name, "Read");
    assert_eq!(call.input["file_path"], "/work/demo/Cargo.toml");
    assert_eq!(
        call.output,
        Some(ToolOutput {
            text: "1\t[package]\n2\tname=\"demo\"\n3\tversion=\"1.2.3\"\n4\t".into(),
            is_error: false,
        })
    );
    assert_eq!(answer, "1.2.3");
    assert_eq!(transcript.session_id(), Some("sess-1"));
    assert_eq!(transcript.status(), Status::Idle);
    assert_eq!(transcript.cost_usd(), 0.5900866);
    assert_eq!(
        transcript.usage(),
        Some(Usage {
            context_tokens: 143_640,
            output_tokens: 7
        })
    );
}

#[test]
fn the_turn_is_working_until_its_result_arrives() {
    let before_result = TURN.lines().take(TURN.lines().count() - 1);
    let mut transcript = Transcript::new();
    transcript.user_message("go");
    for line in before_result {
        for event in parse_events(line) {
            transcript.apply(event);
        }
    }
    assert_eq!(transcript.status(), Status::Working);
}

#[test]
fn replaying_without_a_user_message_leaves_no_user_block() {
    let transcript = replay(TURN.lines());
    assert!(
        transcript
            .blocks()
            .iter()
            .all(|b| !matches!(b, Block::User(_)))
    );
}
