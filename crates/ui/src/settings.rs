//! Zide's own settings, in `~/.config/zide/settings.json`. For now they only say how to start
//! the agent: `agent_servers` maps a name to the command that runs Claude (a sandbox wrapper,
//! say). Keys Zide does not know are kept when the file is saved.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use zide_claude::Launcher;

/// One entry of `agent_servers`. `args` go before Claude's own arguments: Zide speaks Claude's
/// stream-json, so the command must end up running `claude` with what Zide appends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Where that Claude keeps its config and sessions (inside a sandbox, its own home).
    pub config_dir: Option<String>,
}

impl AgentServer {
    /// How to start Claude for this entry; without a command, Claude from `PATH`.
    pub fn launcher(&self) -> Launcher {
        let command = self.command.trim();
        if command.is_empty() {
            return Launcher::default();
        }
        Launcher {
            command: Some(expand_home(command)),
            args: self.args.clone(),
            config_dir: self
                .config_dir
                .as_deref()
                .map(str::trim)
                .filter(|dir| !dir.is_empty())
                .map(expand_home),
        }
    }
}

/// `~/x` is `x` in the home folder; anything else is taken as written.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

pub fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("zide").join("settings.json"))
}

/// The first entry of `agent_servers` (in the order the file is read), if there is one.
pub fn agent_from(file: &Value) -> Option<AgentServer> {
    let (name, entry) = file.get("agent_servers")?.as_object()?.iter().next()?;
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(AgentServer {
        name: name.clone(),
        command: text("command").unwrap_or_default(),
        args: entry
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        config_dir: text("config_dir"),
    })
}

/// `file` with the entry `previous` replaced by `agent` (or just removed, without one); the
/// other entries and settings stay.
pub fn with_agent(mut file: Value, previous: Option<&str>, agent: Option<&AgentServer>) -> Value {
    if !file.is_object() {
        file = json!({});
    }
    let root = file.as_object_mut().expect("an object");
    let servers = root
        .entry("agent_servers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    let servers = servers.as_object_mut().expect("an object");
    if let Some(previous) = previous {
        servers.remove(previous);
    }
    if let Some(agent) = agent {
        let mut entry = json!({ "command": agent.command, "args": agent.args });
        if let Some(dir) = &agent.config_dir {
            entry["config_dir"] = json!(dir);
        }
        servers.insert(agent.name.clone(), entry);
    }
    file
}

/// The file as JSON: `{}` when it does not exist, an error when it is not JSON.
fn read(path: &Path) -> io::Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e),
    }
}

pub fn load_agent(path: &Path) -> io::Result<Option<AgentServer>> {
    Ok(agent_from(&read(path)?))
}

/// Writes `agent` in place of `previous`. A file that is not JSON is left alone: it is the
/// user's, and rewriting it would lose what they wrote.
pub fn save_agent(
    path: &Path,
    previous: Option<&str>,
    agent: Option<&AgentServer>,
) -> io::Result<()> {
    let file = with_agent(read(path)?, previous, agent);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    std::fs::write(path, text + "\n")
}

/// Reads the settings and makes Claude start as they say; problems are logged, and Claude then
/// starts as by default.
pub fn apply() -> Option<AgentServer> {
    let agent = match path().map(|path| load_agent(&path)) {
        Some(Ok(agent)) => agent,
        Some(Err(e)) => {
            eprintln!("settings: {e}");
            None
        }
        None => None,
    };
    zide_claude::set_launcher(
        agent
            .as_ref()
            .map(AgentServer::launcher)
            .unwrap_or_default(),
    );
    agent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> AgentServer {
        AgentServer {
            name: "Sandboxed".into(),
            command: "/bin/agent.sh".into(),
            args: vec!["claude".into()],
            config_dir: Some("/sandbox/.claude".into()),
        }
    }

    #[::core::prelude::v1::test]
    fn the_first_agent_server_is_read_in_the_documented_shape() {
        let file = json!({
            "agent_servers": {
                "Agente aislado": {
                    "command": "/home/vos/bin/agente-sandbox.sh",
                    "args": ["mi-agente", "--acp"]
                }
            }
        });
        assert_eq!(
            agent_from(&file),
            Some(AgentServer {
                name: "Agente aislado".into(),
                command: "/home/vos/bin/agente-sandbox.sh".into(),
                args: vec!["mi-agente".into(), "--acp".into()],
                config_dir: None,
            })
        );
        assert_eq!(agent_from(&json!({})), None);
        assert_eq!(agent_from(&json!({"agent_servers": {}})), None);
    }

    #[::core::prelude::v1::test]
    fn saving_keeps_the_other_settings_and_replaces_the_edited_entry() {
        let file = json!({
            "theme": "dark",
            "agent_servers": { "Old": { "command": "x" }, "Other": { "command": "y" } }
        });
        let saved = with_agent(file, Some("Old"), Some(&sandbox()));
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["agent_servers"].get("Old"), None);
        assert_eq!(saved["agent_servers"]["Other"]["command"], "y");
        assert_eq!(
            saved["agent_servers"]["Sandboxed"],
            json!({"command": "/bin/agent.sh", "args": ["claude"], "config_dir": "/sandbox/.claude"})
        );
    }

    #[::core::prelude::v1::test]
    fn clearing_the_agent_removes_its_entry() {
        let file = json!({ "agent_servers": { "Old": { "command": "x" } } });
        let saved = with_agent(file, Some("Old"), None);
        assert_eq!(saved, json!({ "agent_servers": {} }));
        assert_eq!(agent_from(&saved), None);
    }

    #[::core::prelude::v1::test]
    fn the_launcher_runs_the_command_with_home_expanded() {
        let home = dirs::home_dir().unwrap();
        let mut agent = sandbox();
        agent.command = "~/bin/agent.sh".into();
        agent.config_dir = Some("~/.sandbox/.claude".into());
        let launcher = agent.launcher();
        assert_eq!(launcher.command, Some(home.join("bin/agent.sh")));
        assert_eq!(launcher.args, ["claude"]);
        assert_eq!(launcher.config_dir, Some(home.join(".sandbox/.claude")));
        // no command: Claude from PATH, as before
        agent.command = "  ".into();
        assert_eq!(agent.launcher().command, None);
    }

    #[::core::prelude::v1::test]
    fn a_file_that_is_not_json_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("zide-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ broken").unwrap();
        assert!(save_agent(&path, None, Some(&sandbox())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
        // a missing file is created, and reads back
        let fresh = dir.join("sub/settings.json");
        save_agent(&fresh, None, Some(&sandbox())).unwrap();
        assert_eq!(load_agent(&fresh).unwrap(), Some(sandbox()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
