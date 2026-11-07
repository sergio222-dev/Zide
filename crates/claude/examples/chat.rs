//! Terminal chat over one live Claude Code process: type a line, see the answer
//! stream in; when Claude asks to run a tool, answer `y` or anything else to deny.
//! `cargo run -p zide-claude --example chat -- [permission-mode] [session-id-to-resume]`

use std::{
    env,
    io::{self, BufRead, Write},
};

use zide_claude::{Event, Options, Session};

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let permission_mode = args.next();
    let resume = args.next();
    let cwd = env::current_dir()?;
    let options = Options {
        resume: resume.as_deref(),
        permission_mode: permission_mode.as_deref(),
        ..Options::default()
    };
    let (mut session, events) = Session::spawn(&cwd, options)?;
    let mut lines = io::stdin().lock().lines();

    while let Some(line) = lines.next().transpose()? {
        if line.trim().is_empty() {
            continue;
        }
        session.send(&line)?;
        for event in &events {
            match event {
                Event::Init { session_id } => eprintln!("[session {session_id}]"),
                Event::TextDelta(chunk) => {
                    print!("{chunk}");
                    io::stdout().flush()?;
                }
                Event::ToolUse { name, .. } => eprintln!("\n[tool {name}]"),
                Event::PermissionRequest {
                    request_id,
                    tool_name,
                    input,
                } => {
                    eprintln!("\n[allow {tool_name} {input}? y/N]");
                    let answer = lines.next().transpose()?.unwrap_or_default();
                    if answer.trim() == "y" {
                        session.allow(&request_id, &input)?;
                    } else {
                        session.deny(&request_id, "denied by the user")?;
                    }
                }
                Event::Question {
                    request_id,
                    input,
                    questions,
                } => {
                    let mut answers = Vec::new();
                    for q in &questions {
                        eprintln!("\n[{}] {}", q.header, q.question);
                        for (i, o) in q.options.iter().enumerate() {
                            eprintln!("  {}) {} - {}", i + 1, o.label, o.description);
                        }
                        let pick = lines.next().transpose()?.unwrap_or_default();
                        let index = pick
                            .trim()
                            .parse::<usize>()
                            .unwrap_or(1)
                            .clamp(1, q.options.len());
                        answers.push((q.question.as_str(), q.options[index - 1].label.as_str()));
                    }
                    session.answer(&request_id, &input, &answers)?;
                }
                Event::Result {
                    is_error, cost_usd, ..
                } => {
                    println!();
                    eprintln!("[turn done, error={is_error}, ${cost_usd:.3}]");
                    break;
                }
                Event::Exit => return Ok(()),
                _ => {}
            }
        }
    }
    Ok(())
}
