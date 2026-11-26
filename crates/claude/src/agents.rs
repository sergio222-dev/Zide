//! The background agents `claude agents --json` knows about: sessions that run on their own,
//! apart from the chats Zide drives.

use std::path::Path;

use serde_json::Value;

/// Who runs the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Claude Code the user types into: a terminal, another Zide.
    Interactive,
    /// A session that runs on its own, apart from any window.
    Background,
}

/// A live session started under the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub kind: Kind,
    /// What `claude logs` and `claude rm` take: the short id `claude agents` lists.
    pub id: String,
    /// The conversation: what `Options::resume` takes.
    pub session_id: String,
    pub name: String,
    /// `busy` or `idle`, as `claude` reports it.
    pub status: String,
    /// What it waits for or how it ended (`blocked`, ...), when `claude` says.
    pub state: Option<String>,
    /// Milliseconds since the epoch.
    pub started_at: u64,
}

/// The sessions (background, finished ones included, and interactive) started under `root`,
/// newest first. Empty when
/// `claude` is missing, too old for `agents`, or answers something unexpected.
pub fn list(root: &Path) -> Vec<Agent> {
    let output = crate::cli::command()
        .args(["agents", "--all", "--json", "--cwd"])
        .arg(root)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse(&String::from_utf8_lossy(&output.stdout), root)
        }
        _ => Vec::new(),
    }
}

/// Deletes a background agent (and its worktree when that is safe). The error is what `claude`
/// said, e.g. that the worktree has unpushed commits.
pub fn remove(id: &str) -> Result<String, String> {
    if id.starts_with('-') {
        return Err("not an agent id".into()); // an id never is an option
    }
    let output = crate::cli::command()
        .args(["rm", id])
        .output()
        .map_err(|error| error.to_string())?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    if output.status.success() {
        Ok(text(&output.stdout))
    } else {
        let (err, out) = (text(&output.stderr), text(&output.stdout));
        Err(if err.is_empty() { out } else { err })
    }
}

/// The background and interactive entries of `json` whose folder is `root` or inside it.
pub fn parse(json: &str, root: &Path) -> Vec<Agent> {
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let text = |entry: &Value, key: &str| entry[key].as_str().map(str::to_owned);
    let mut agents: Vec<Agent> = entries
        .iter()
        .filter_map(|entry| {
            let kind = match entry["kind"].as_str()? {
                "background" => Kind::Background,
                "interactive" => Kind::Interactive,
                _ => return None,
            };
            Some((kind, entry))
        })
        .filter(|(_, entry)| {
            entry["cwd"]
                .as_str()
                .is_some_and(|cwd| Path::new(cwd).starts_with(root))
        })
        .filter_map(|(kind, entry)| {
            let session_id = text(entry, "sessionId")?;
            Some(Agent {
                kind,
                id: text(entry, "id").unwrap_or_else(|| session_id.clone()),
                name: text(entry, "name").unwrap_or_else(|| session_id.clone()),
                session_id,
                status: text(entry, "status").unwrap_or_default(),
                state: text(entry, "state"),
                started_at: entry["startedAt"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    agents.sort_by_key(|agent| std::cmp::Reverse(agent.started_at));
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"pid":1,"cwd":"/p","kind":"interactive","startedAt":1,"sessionId":"i","name":"me","status":"busy"},
      {"pid":2,"id":"a","cwd":"/p","kind":"background","startedAt":5,"sessionId":"sa","name":"first","status":"idle","state":"blocked"},
      {"pid":3,"id":"b","cwd":"/p/sub","kind":"background","startedAt":9,"sessionId":"sb","status":"busy"},
      {"pid":4,"id":"c","cwd":"/other","kind":"background","startedAt":7,"sessionId":"sc","name":"elsewhere","status":"idle"}
    ]"#;

    #[test]
    fn live_agents_under_the_project_newest_first() {
        let agents = parse(LIST, Path::new("/p"));
        let names: Vec<_> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["sb", "first", "me"]); // unnamed ones go by their session
        let kinds: Vec<_> = agents.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            [Kind::Background, Kind::Background, Kind::Interactive]
        );
        assert_eq!(agents[1].state.as_deref(), Some("blocked"));
        assert_eq!(agents[0].state, None);
    }

    #[test]
    fn a_folder_that_only_shares_a_prefix_is_not_inside() {
        let json = r#"[{"cwd":"/pp","kind":"background","sessionId":"x","status":"idle"}]"#;
        assert!(parse(json, Path::new("/p")).is_empty());
    }

    #[test]
    fn an_option_is_never_taken_for_an_id() {
        assert!(remove("-f").is_err());
    }

    #[test]
    fn anything_unexpected_is_no_agents() {
        assert!(parse("not json", Path::new("/p")).is_empty());
        assert!(parse("{}", Path::new("/p")).is_empty());
    }
}
