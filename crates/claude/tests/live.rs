//! Talks to the real `claude` (needs it installed and logged in, and spends tokens):
//! `cargo test -p zide-claude --test live -- --ignored`

use std::{env, time::Duration};

use zide_claude::{Event, Options, Session};

#[test]
#[ignore = "runs the real claude and costs money"]
fn interrupt_ends_the_turn_and_keeps_the_session_usable() {
    let cwd = env::current_dir().unwrap();
    let (mut session, events) = Session::spawn(&cwd, Options::default()).unwrap();
    let wait = Duration::from_secs(120);

    session
        .send("Count from 1 to 300, one number per line, no comments.")
        .unwrap();
    let mut chunks = 0;
    let first = loop {
        match events.recv_timeout(wait).expect("the turn went silent") {
            Event::TextDelta(_) => {
                chunks += 1;
                if chunks == 5 {
                    session.interrupt().unwrap();
                }
            }
            Event::Result {
                terminal_reason, ..
            } => break terminal_reason,
            _ => {}
        }
    };
    assert_eq!(first.as_deref(), Some("aborted_streaming"));

    session.send("Say only: alive").unwrap();
    let answer = loop {
        if let Event::Result { text, .. } = events.recv_timeout(wait).expect("no second turn") {
            break text;
        }
    };
    assert!(answer.to_lowercase().contains("alive"), "got {answer:?}");
}
