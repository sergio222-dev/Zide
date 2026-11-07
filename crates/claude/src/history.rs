//! The conversations Claude Code keeps on disk, so a chat can list them and show one
//! again before continuing it with `--resume`. The files are the CLI's own and their
//! format is not documented: everything here reads defensively and skips what it does
//! not understand.

use std::{
    env, fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::Value;

use crate::{event::Event, transcript::Transcript};

/// A stored conversation, as the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// What `Options::resume` takes.
    pub id: String,
    pub title: String,
    pub modified: SystemTime,
}

/// Where the CLI keeps the sessions started in `root`: its config folder, then
/// `projects/`, then the path with every character that is not a letter or digit
/// turned into `-`.
pub fn project_dir(root: &Path) -> Option<PathBuf> {
    let config = match env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("HOME")?).join(".claude"),
    };
    let name: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(config.join("projects").join(name))
}

/// The sessions started in `root`, the most recently used first. Empty when there are
/// none or the folder cannot be read; sessions without a single message are left out.
pub fn list(root: &Path) -> Vec<SessionInfo> {
    project_dir(root)
        .map(|dir| list_in(&dir))
        .unwrap_or_default()
}

pub fn list_in(dir: &Path) -> Vec<SessionInfo> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            let id = path.file_stem()?.to_str()?.to_owned();
            let title = title_of(&path)?;
            Some(SessionInfo {
                id,
                title,
                modified,
            })
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
    sessions
}

/// The title the CLI generated for the session, else the first thing the user said.
/// `None` for a session where nobody said anything.
fn title_of(path: &Path) -> Option<String> {
    let mut generated = None;
    let mut first_prompt = None;
    for line in BufReader::new(fs::File::open(path).ok()?)
        .lines()
        .map_while(Result::ok)
    {
        // Parsing every line of a multi-megabyte file is wasteful: look at the two kinds
        // that matter, by their marker, before parsing.
        let is_title = line.contains(r#""type":"ai-title""#);
        let is_user = first_prompt.is_none() && line.contains(r#""type":"user""#);
        if !is_title && !is_user {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if is_title && let Some(title) = value["aiTitle"].as_str() {
            generated = Some(title.to_owned());
        } else if is_user && let Some(text) = user_text(&value) {
            first_prompt = Some(text.lines().next().unwrap_or_default().trim().to_owned());
        }
    }
    generated
        .filter(|title| !title.is_empty())
        .or(first_prompt.filter(|prompt| !prompt.is_empty()))
}

/// What the user typed in a `user` record, `None` for tool results, the CLI's own
/// bookkeeping messages and the commands' output it records in the same file.
fn user_text(record: &Value) -> Option<String> {
    if ["isMeta", "isSidechain", "isCompactSummary"]
        .iter()
        .any(|flag| record[*flag].as_bool() == Some(true))
    {
        return None;
    }
    let content = &record["message"]["content"];
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    const NOISE: [&str; 5] = [
        "<local-command-",
        "<command-name>",
        "<system-reminder>",
        "<task-notification",
        "<agent-message",
    ];
    (!text.is_empty() && !NOISE.iter().any(|tag| text.starts_with(tag))).then(|| text.to_owned())
}

/// The conversation `id` of `root`, rebuilt from its file. Ready to resume: a message
/// sent next starts `claude --resume` on it.
pub fn load(root: &Path, id: &str) -> io::Result<Transcript> {
    let dir = project_dir(root).ok_or_else(|| io::Error::other("no home folder"))?;
    load_from(&dir.join(format!("{id}.jsonl")), id)
}

pub fn load_from(path: &Path, id: &str) -> io::Result<Transcript> {
    let mut transcript = Transcript::new();
    transcript.apply(Event::Init {
        session_id: id.to_owned(),
        commands: Vec::new(),
    });
    for line in BufReader::new(fs::File::open(path)?)
        .lines()
        .map_while(Result::ok)
    {
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match record["type"].as_str() {
            Some("user") => replay_user(&record, &mut transcript),
            Some("assistant") => replay_assistant(&record, &mut transcript),
            _ => {}
        }
    }
    Ok(transcript)
}

fn replay_user(record: &Value, transcript: &mut Transcript) {
    if let Some(text) = user_text(record) {
        transcript.push_user(&text);
    }
    for block in record["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if block["type"] != "tool_result" {
            continue;
        }
        let Some(tool_use_id) = block["tool_use_id"].as_str() else {
            continue;
        };
        let output = match &block["content"] {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        transcript.apply(Event::ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            output,
            is_error: block["is_error"].as_bool().unwrap_or(false),
        });
    }
}

fn replay_assistant(record: &Value, transcript: &mut Transcript) {
    for block in record["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let event = match block["type"].as_str() {
            Some("text") => block["text"]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(|t| Event::TextDelta(t.to_owned())),
            Some("thinking") => block["thinking"]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(|t| Event::ThinkingDelta(t.to_owned())),
            Some("tool_use") => {
                block["id"]
                    .as_str()
                    .zip(block["name"].as_str())
                    .map(|(id, name)| Event::ToolUse {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        input: block["input"].clone(),
                    })
            }
            _ => None,
        };
        if let Some(event) = event {
            transcript.apply(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::Duration,
    };

    use super::*;
    use crate::transcript::{Block, Status};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A fresh folder under the system's temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let dir = env::temp_dir().join(format!(
                "zide-claude-history-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn write(&self, name: &str, lines: &[&str]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, lines.join("\n")).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const USER: &str =
        r#"{"type":"user","message":{"role":"user","content":"hola, listá los archivos"}}"#;
    const THINKING: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"pienso"}]}}"#;
    const TOOL: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
    const RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a.rs"}]}}"#;
    const ANSWER: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hay a.rs"}]}}"#;

    #[test]
    fn the_folder_name_replaces_everything_but_letters_and_digits() {
        let dir = project_dir(Path::new("/home/a.b/c_d")).unwrap();
        assert_eq!(dir.file_name().unwrap(), "-home-a-b-c-d");
        assert_eq!(dir.parent().unwrap().file_name().unwrap(), "projects");
    }

    #[test]
    fn a_session_is_listed_by_the_title_the_cli_generated() {
        let scratch = Scratch::new();
        scratch.write(
            "s1.jsonl",
            &[
                USER,
                r#"{"type":"ai-title","aiTitle":"Listar archivos","sessionId":"s1"}"#,
                r#"{"type":"ai-title","aiTitle":"Listar el repo","sessionId":"s1"}"#,
            ],
        );
        let sessions = list_in(&scratch.0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "s1");
        assert_eq!(sessions[0].title, "Listar el repo");
    }

    #[test]
    fn without_a_generated_title_the_first_prompt_names_the_session() {
        let scratch = Scratch::new();
        scratch.write(
            "s1.jsonl",
            &[
                r#"{"type":"user","isMeta":true,"message":{"content":"<system-reminder>x"}}"#,
                r#"{"type":"user","message":{"content":"<local-command-stdout></local-command-stdout>"}}"#,
                USER,
                ANSWER,
            ],
        );
        assert_eq!(list_in(&scratch.0)[0].title, "hola, listá los archivos");
    }

    #[test]
    fn a_session_nobody_spoke_in_is_left_out() {
        let scratch = Scratch::new();
        scratch.write("empty.jsonl", &[r#"{"type":"mode","mode":"normal"}"#]);
        scratch.write("notes.txt", &[USER]);
        assert_eq!(list_in(&scratch.0), vec![]);
    }

    #[test]
    fn the_most_recent_session_comes_first() {
        let scratch = Scratch::new();
        let old = scratch.write("old.jsonl", &[USER]);
        scratch.write("new.jsonl", &[USER]);
        let file = fs::File::options().write(true).open(&old).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        let ids: Vec<_> = list_in(&scratch.0).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["new", "old"]);
    }

    #[test]
    fn a_missing_folder_has_no_sessions() {
        assert_eq!(list_in(Path::new("/nonexistent/zide")), vec![]);
    }

    #[test]
    fn loading_rebuilds_the_conversation_and_is_ready_to_resume() {
        let scratch = Scratch::new();
        let path = scratch.write(
            "s1.jsonl",
            &[
                r#"{"type":"mode","mode":"normal"}"#,
                USER,
                THINKING,
                TOOL,
                RESULT,
                r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"de un subagente"}]}}"#,
                ANSWER,
                "not json",
            ],
        );
        let transcript = load_from(&path, "s1").unwrap();
        let blocks = transcript.blocks();
        assert!(matches!(&blocks[0], Block::User(t) if t == "hola, listá los archivos"));
        assert!(matches!(&blocks[1], Block::Thinking(t) if t == "pienso"));
        let Block::Tool(call) = &blocks[2] else {
            panic!("expected a tool call: {blocks:#?}");
        };
        assert_eq!(call.name, "Bash");
        assert_eq!(call.output.as_ref().unwrap().text, "a.rs");
        assert!(matches!(&blocks[3], Block::Text(t) if t == "Hay a.rs"));
        assert_eq!(blocks.len(), 4);
        assert_eq!(transcript.session_id(), Some("s1"));
        assert_eq!(transcript.status(), Status::Idle);
    }

    #[test]
    fn loading_a_missing_session_is_an_error() {
        assert!(load_from(Path::new("/nonexistent/zide/x.jsonl"), "x").is_err());
    }
}
