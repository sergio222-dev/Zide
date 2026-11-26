//! Talks to the real `claude` (needs it installed and logged in, and spends tokens):
//! `cargo test -p zide-claude --test live -- --ignored`

use std::{env, time::Duration};

use zide_claude::{Block, Event, Options, Session, history};

#[test]
#[ignore = "runs the real claude and spends tokens (about a cent)"]
fn interrupt_ends_the_turn_and_keeps_the_session_usable() {
    let cwd = env::current_dir().unwrap();
    let (mut session, events) = Session::spawn(
        &cwd,
        Options {
            minimal: true,
            ..Options::default()
        },
    )
    .unwrap();
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

/// Runs one turn on `session` and returns the text of its result.
fn ask(session: &mut Session, events: &std::sync::mpsc::Receiver<Event>, text: &str) -> String {
    session.send(text).unwrap();
    loop {
        if let Event::Result { text, .. } = events
            .recv_timeout(Duration::from_secs(120))
            .expect("the turn went silent")
        {
            return text;
        }
    }
}

#[test]
#[ignore = "runs the real claude and spends tokens (about a cent)"]
fn a_stored_session_is_listed_shown_and_resumed() {
    let project = env::temp_dir().join(format!("zide-live-{}", std::process::id()));
    std::fs::create_dir_all(&project).unwrap();
    let minimal = |resume| Options {
        resume,
        minimal: true,
        ..Options::default()
    };

    let (mut first, events) = Session::spawn(&project, minimal(None)).unwrap();
    ask(
        &mut first,
        &events,
        "Remember the secret word BANANA. Reply only: ok",
    );
    drop(first);

    let sessions = history::list(&project);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let id = sessions[0].id.clone();

    let shown = history::load(&project, &id).unwrap();
    assert!(
        matches!(shown.blocks().first(), Some(Block::User(t)) if t.contains("BANANA")),
        "{:#?}",
        shown.blocks()
    );

    let (mut again, events) = Session::spawn(&project, minimal(Some(&id))).unwrap();
    let answer = ask(
        &mut again,
        &events,
        "What was the secret word? Reply only the word.",
    );
    drop(again);
    let _ = std::fs::remove_dir_all(history::project_dir(&project).unwrap());
    let _ = std::fs::remove_dir_all(&project);
    assert!(answer.to_uppercase().contains("BANANA"), "got {answer:?}");
}
