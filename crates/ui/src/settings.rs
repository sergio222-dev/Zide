//! Zide's own settings, in `~/.config/zide/settings.json`. They say how to start the agent:
//! `agent_servers` maps a name to the command that runs Claude (a sandbox wrapper, say), and
//! `agent` names the one in use; and `confirm_stop: false` lets Esc stop Claude without asking.
//! `editor_font` and `ui_font` name the font families Zide draws code and its interface in.
//! `indent` is the indent level of files that do not show their own: `"tab"` or a number of spaces.
//! `keymap` rebinds Vim keys over Zide's defaults (see `zide_vim::Keymap::with_overrides`).
//! Keys Zide does not know are kept when the file is saved.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};
use zide_claude::Launcher;
use zide_vim::Keymap;

/// How Zide talks to an agent: Claude Code's stream-json (the default) or the Agent Client
/// Protocol (`"protocol": "acp"`), which other agents such as Copilot speak.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    Claude,
    Acp,
}

static PROTOCOL: std::sync::RwLock<Protocol> = std::sync::RwLock::new(Protocol::Claude);

/// The protocol of the agent new chats start, as the settings last applied say.
pub fn protocol() -> Protocol {
    *PROTOCOL.read().unwrap_or_else(|e| e.into_inner())
}

static CONFIRM_STOP: AtomicBool = AtomicBool::new(true);

/// Whether Esc asks before it stops the turn Claude is working on, as the settings last applied
/// (or the last toggle) say.
pub fn confirm_stop() -> bool {
    CONFIRM_STOP.load(Ordering::Relaxed)
}

/// Asking is the default: only `"confirm_stop": false` turns it off.
fn confirm_stop_in(file: &Value) -> bool {
    file.get("confirm_stop").and_then(Value::as_bool) != Some(false)
}

/// `file` with the confirmation turned on or off; the other settings stay.
pub fn with_confirm_stop(mut file: Value, on: bool) -> Value {
    if !file.is_object() {
        file = json!({});
    }
    file["confirm_stop"] = json!(on);
    file
}

/// Claude: Confirm Before Stopping. Takes effect at once and is saved; a file that cannot be
/// saved is reported, and the choice then lasts until Zide closes.
pub fn toggle_confirm_stop() -> Result<bool, String> {
    let on = !confirm_stop();
    CONFIRM_STOP.store(on, Ordering::Relaxed);
    let path = path().ok_or("No config folder to save the setting in")?;
    update(&path, |file| with_confirm_stop(file, on)).map_err(|e| e.to_string())?;
    Ok(on)
}

/// One indent level for files that do not show their own: `"indent": "tab"` or a number of
/// spaces (`"indent": 2`). Four spaces without the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Tab,
    Spaces(usize),
}

impl Indent {
    /// The text one level is typed as.
    pub fn unit(self) -> String {
        match self {
            Indent::Tab => "\t".into(),
            Indent::Spaces(n) => " ".repeat(n),
        }
    }
}

const DEFAULT_INDENT: Indent = Indent::Spaces(4);

static INDENT: std::sync::RwLock<Indent> = std::sync::RwLock::new(DEFAULT_INDENT);

/// The default indent level, as the settings last applied say.
pub fn indent() -> Indent {
    *INDENT.read().unwrap_or_else(|e| e.into_inner())
}

/// A value that is neither `"tab"` nor 1 to 8 spaces is the default.
fn indent_in(file: &Value) -> Indent {
    match file.get("indent") {
        Some(Value::String(text)) if matches!(text.trim(), "tab" | "tabs") => Indent::Tab,
        Some(value) => match value.as_u64() {
            Some(n @ 1..=8) => Indent::Spaces(n as usize),
            _ => DEFAULT_INDENT,
        },
        None => DEFAULT_INDENT,
    }
}

static KEYMAP: std::sync::RwLock<Option<Arc<Keymap>>> = std::sync::RwLock::new(None);

/// What was wrong in the `keymap` setting last applied, not yet shown to the user.
static KEYMAP_ERRORS: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// The Vim keys: Zide's defaults with the `keymap` setting merged over them.
pub fn keymap() -> Arc<Keymap> {
    KEYMAP
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(Keymap::builtin)
}

/// The problems of the `keymap` setting, once: the next call returns none until the settings
/// are applied again.
pub fn take_keymap_errors() -> Vec<String> {
    std::mem::take(&mut *KEYMAP_ERRORS.write().unwrap_or_else(|e| e.into_inner()))
}

/// The keymap `file` asks for, and what was wrong in it (its bad entries are left out).
fn keymap_in(file: &Value) -> (Arc<Keymap>, Vec<String>) {
    match file.get("keymap") {
        None | Some(Value::Null) => (Keymap::builtin(), Vec::new()),
        Some(user) => {
            let (keymap, errors) = Keymap::with_overrides(user);
            (Arc::new(keymap), errors)
        }
    }
}

/// The font families chosen in `editor_font` and `ui_font`; `None` is Zide's own pick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fonts {
    pub editor: Option<String>,
    pub ui: Option<String>,
}

static FONTS: std::sync::RwLock<Fonts> = std::sync::RwLock::new(Fonts {
    editor: None,
    ui: None,
});

/// The fonts as the settings last applied (or the Fonts dialog) say.
pub fn fonts() -> Fonts {
    FONTS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn fonts_in(file: &Value) -> Fonts {
    let family = |key: &str| {
        file.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    };
    Fonts {
        editor: family("editor_font"),
        ui: family("ui_font"),
    }
}

/// `file` with `fonts`; a font left to Zide has no key. The other settings stay.
pub fn with_fonts(mut file: Value, fonts: &Fonts) -> Value {
    if !file.is_object() {
        file = json!({});
    }
    let map = file.as_object_mut().expect("made an object above");
    for (key, family) in [("editor_font", &fonts.editor), ("ui_font", &fonts.ui)] {
        match family {
            Some(family) => map.insert(key.into(), json!(family)),
            None => map.remove(key),
        };
    }
    file
}

/// The Fonts dialog: takes effect at once and is saved; a file that cannot be saved is
/// reported, and the choice then lasts until Zide closes.
pub fn set_fonts(fonts: Fonts) -> Result<(), String> {
    *FONTS.write().unwrap_or_else(|e| e.into_inner()) = fonts.clone();
    let path = path().ok_or("No config folder to save the setting in")?;
    update(&path, |file| with_fonts(file, &fonts)).map_err(|e| e.to_string())?;
    Ok(())
}

/// One entry of `agent_servers`. `args` go before Claude's own arguments: Zide speaks Claude's
/// stream-json, so the command must end up running `claude` with what Zide appends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Where that Claude keeps its config and sessions (inside a sandbox, its own home).
    pub config_dir: Option<String>,
    pub protocol: Protocol,
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

fn entry(name: &str, entry: &Value) -> AgentServer {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
    AgentServer {
        name: name.to_string(),
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
        protocol: match text("protocol").as_deref() {
            Some("acp") => Protocol::Acp,
            _ => Protocol::Claude,
        },
    }
}

/// Every entry of `agent_servers`, in the order the file is read.
pub fn agents(file: &Value) -> Vec<AgentServer> {
    file.get("agent_servers")
        .and_then(Value::as_object)
        .map(|servers| servers.iter().map(|(name, e)| entry(name, e)).collect())
        .unwrap_or_default()
}

/// The agent in use: the one `agent` names; without that key (or naming none that exists), the
/// first entry; `"agent": null` is Claude from `PATH`, as is a file without entries.
pub fn active(file: &Value) -> Option<AgentServer> {
    let agents = agents(file);
    match file.get("agent") {
        Some(Value::Null) => None,
        Some(Value::String(name)) => agents
            .iter()
            .find(|agent| &agent.name == name)
            .or(agents.first())
            .cloned(),
        _ => agents.into_iter().next(),
    }
}

/// The `agent_servers` object of `file`, created when missing.
fn servers(file: &mut Value) -> &mut Map<String, Value> {
    if !file.is_object() {
        *file = json!({});
    }
    let servers = file
        .as_object_mut()
        .expect("an object")
        .entry("agent_servers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    servers.as_object_mut().expect("an object")
}

/// `file` with the entry `previous` replaced by `agent` (a new one without `previous`); the
/// other entries and settings stay, and a renamed active entry stays the active one.
pub fn with_agent(mut file: Value, previous: Option<&str>, agent: &AgentServer) -> Value {
    let servers = servers(&mut file);
    if let Some(previous) = previous {
        servers.remove(previous);
    }
    let mut entry = json!({ "command": agent.command, "args": agent.args });
    if let Some(dir) = &agent.config_dir {
        entry["config_dir"] = json!(dir);
    }
    if agent.protocol == Protocol::Acp {
        entry["protocol"] = json!("acp");
    }
    servers.insert(agent.name.clone(), entry);
    if previous.is_some() && file.get("agent").and_then(Value::as_str) == previous {
        file["agent"] = json!(agent.name);
    }
    file
}

/// `file` without the entry `name`; when it was the active one, the first entry left is.
pub fn without_agent(mut file: Value, name: &str) -> Value {
    servers(&mut file).remove(name);
    if file.get("agent").and_then(Value::as_str) == Some(name) {
        file.as_object_mut().expect("an object").remove("agent");
    }
    file
}

/// `file` with `name` as the agent in use; `None` is Claude from `PATH`.
pub fn with_active(mut file: Value, name: Option<&str>) -> Value {
    if !file.is_object() {
        file = json!({});
    }
    file["agent"] = name.map_or(Value::Null, |name| json!(name));
    file
}

/// The file as JSON: `{}` when it does not exist, an error when it is not JSON (which is then
/// never overwritten: it is the user's, and rewriting it would lose what they wrote).
pub fn read(path: &Path) -> io::Result<Value> {
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

pub fn write(path: &Path, file: &Value) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
    std::fs::write(path, text + "\n")
}

/// Reads the file at `path`, changes it with `change` and writes it back; a file that is not
/// JSON is left alone. Returns what was written.
pub fn update(path: &Path, change: impl FnOnce(Value) -> Value) -> io::Result<Value> {
    let file = change(read(path)?);
    write(path, &file)?;
    Ok(file)
}

/// Makes Claude start, and Esc stop it, as `file` says.
pub fn use_file(file: &Value) {
    let agent = active(file);
    zide_claude::set_launcher(
        agent
            .as_ref()
            .map(AgentServer::launcher)
            .unwrap_or_default(),
    );
    *PROTOCOL.write().unwrap_or_else(|e| e.into_inner()) =
        agent.map(|agent| agent.protocol).unwrap_or_default();
    CONFIRM_STOP.store(confirm_stop_in(file), Ordering::Relaxed);
    *FONTS.write().unwrap_or_else(|e| e.into_inner()) = fonts_in(file);
    *INDENT.write().unwrap_or_else(|e| e.into_inner()) = indent_in(file);
    let (keymap, errors) = keymap_in(file);
    for error in &errors {
        eprintln!("settings: {error}");
    }
    *KEYMAP.write().unwrap_or_else(|e| e.into_inner()) = Some(keymap);
    *KEYMAP_ERRORS.write().unwrap_or_else(|e| e.into_inner()) = errors;
}

/// Reads the settings and makes Claude start as they say; a problem is logged, and Claude then
/// starts as by default.
pub fn apply() {
    match path().map(|path| read(&path)) {
        Some(Ok(file)) => use_file(&file),
        Some(Err(e)) => eprintln!("settings: {e}"),
        None => {}
    }
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
            protocol: Protocol::Claude,
        }
    }

    fn named(name: &str) -> Option<String> {
        Some(name.to_string())
    }

    fn active_name(file: &Value) -> Option<String> {
        active(file).map(|agent| agent.name)
    }

    #[::core::prelude::v1::test]
    fn the_keymap_setting_merges_over_the_defaults_and_reports_bad_entries() {
        let (keymap, errors) = keymap_in(&json!({}));
        assert_eq!(keymap, Keymap::builtin());
        assert!(errors.is_empty());
        let (keymap, errors) = keymap_in(&json!({
            "keymap": { "normal": { "keys": { "Q": "editor.find", "x": "visual.yank" } } }
        }));
        assert_ne!(keymap, Keymap::builtin());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("normal x"), "{errors:?}");
    }

    #[::core::prelude::v1::test]
    fn the_first_agent_server_is_used_in_the_documented_shape() {
        let file = json!({
            "agent_servers": {
                "Agente aislado": {
                    "command": "/home/vos/bin/agente-sandbox.sh",
                    "args": ["mi-agente", "--acp"]
                }
            }
        });
        assert_eq!(
            active(&file),
            Some(AgentServer {
                name: "Agente aislado".into(),
                command: "/home/vos/bin/agente-sandbox.sh".into(),
                args: vec!["mi-agente".into(), "--acp".into()],
                config_dir: None,
                protocol: Protocol::Claude,
            })
        );
        assert_eq!(active(&json!({})), None);
        assert_eq!(active(&json!({"agent_servers": {}})), None);
    }

    #[::core::prelude::v1::test]
    fn agent_picks_the_one_in_use() {
        let file = json!({
            "agent_servers": { "A": { "command": "a" }, "B": { "command": "b" } },
            "agent": "B"
        });
        assert_eq!(
            agents(&file)
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(active_name(&file), named("B"));
        // naming one that is gone: the first
        assert_eq!(
            active_name(&with_active(file.clone(), Some("Gone"))),
            named("A")
        );
        // null: claude from PATH even with entries
        assert_eq!(active(&with_active(file, None)), None);
    }

    #[::core::prelude::v1::test]
    fn saving_keeps_the_other_settings_and_replaces_the_edited_entry() {
        let file = json!({
            "theme": "dark",
            "agent_servers": { "Old": { "command": "x" }, "Other": { "command": "y" } },
            "agent": "Old"
        });
        let saved = with_agent(file, Some("Old"), &sandbox());
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["agent_servers"].get("Old"), None);
        assert_eq!(saved["agent_servers"]["Other"]["command"], "y");
        assert_eq!(
            saved["agent_servers"]["Sandboxed"],
            json!({"command": "/bin/agent.sh", "args": ["claude"], "config_dir": "/sandbox/.claude"})
        );
        // the renamed entry is still the one in use
        assert_eq!(saved["agent"], "Sandboxed");
        // a new entry does not take over
        let mut other = sandbox();
        other.name = "New".into();
        assert_eq!(with_agent(saved, None, &other)["agent"], "Sandboxed");
    }

    #[::core::prelude::v1::test]
    fn deleting_an_entry_removes_it_and_hands_over_the_active_one() {
        let file = json!({
            "agent_servers": { "A": { "command": "a" }, "B": { "command": "b" } },
            "agent": "B"
        });
        let left = without_agent(file.clone(), "B");
        assert_eq!(
            left,
            json!({ "agent_servers": { "A": { "command": "a" } } })
        );
        assert_eq!(active_name(&left), named("A"));
        // deleting another one keeps the active one
        assert_eq!(active_name(&without_agent(file, "A")), named("B"));
        assert_eq!(active(&without_agent(left, "A")), None);
    }

    #[::core::prelude::v1::test]
    fn the_protocol_is_claude_unless_the_entry_says_acp() {
        let file = json!({
            "agent_servers": {
                "Copilot": { "command": "copilot", "args": ["--acp", "--stdio"], "protocol": "acp" },
                "Plain": { "command": "claude" },
                "Odd": { "command": "x", "protocol": "smoke-signals" }
            }
        });
        let protocol = |name: &str| {
            agents(&file)
                .into_iter()
                .find(|agent| agent.name == name)
                .unwrap()
                .protocol
        };
        assert_eq!(protocol("Copilot"), Protocol::Acp);
        assert_eq!(protocol("Plain"), Protocol::Claude);
        assert_eq!(protocol("Odd"), Protocol::Claude);
        // written back only when it is not the default
        let mut copilot = sandbox();
        copilot.protocol = Protocol::Acp;
        assert_eq!(
            with_agent(json!({}), None, &copilot)["agent_servers"]["Sandboxed"]["protocol"],
            "acp"
        );
        assert_eq!(
            with_agent(json!({}), None, &sandbox())["agent_servers"]["Sandboxed"].get("protocol"),
            None
        );
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
        assert!(update(&path, |file| with_agent(file, None, &sandbox())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
        // a missing file is created, and reads back
        let fresh = dir.join("sub/settings.json");
        update(&fresh, |file| with_agent(file, None, &sandbox())).unwrap();
        assert_eq!(active(&read(&fresh).unwrap()), Some(sandbox()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn stopping_claude_is_confirmed_unless_the_file_says_otherwise() {
        assert!(confirm_stop_in(&json!({})));
        assert!(confirm_stop_in(&json!({ "confirm_stop": "no" })));
        assert!(!confirm_stop_in(&json!({ "confirm_stop": false })));
    }

    #[::core::prelude::v1::test]
    fn turning_the_confirmation_off_keeps_the_other_settings() {
        let file = with_agent(json!({}), None, &sandbox());
        let off = with_confirm_stop(file, false);
        assert!(!confirm_stop_in(&off));
        assert_eq!(active(&off), Some(sandbox()));
        assert!(confirm_stop_in(&with_confirm_stop(off, true)));
    }

    #[::core::prelude::v1::test]
    fn the_indent_is_a_tab_or_some_spaces_and_four_spaces_otherwise() {
        assert_eq!(indent_in(&json!({})), Indent::Spaces(4));
        assert_eq!(indent_in(&json!({ "indent": "tab" })), Indent::Tab);
        assert_eq!(indent_in(&json!({ "indent": 2 })), Indent::Spaces(2));
        assert_eq!(indent_in(&json!({ "indent": 0 })), Indent::Spaces(4));
        assert_eq!(indent_in(&json!({ "indent": "wide" })), Indent::Spaces(4));
        assert_eq!(Indent::Spaces(2).unit(), "  ");
        assert_eq!(Indent::Tab.unit(), "\t");
    }

    #[::core::prelude::v1::test]
    fn fonts_are_saved_beside_the_other_settings_and_dropped_when_left_to_zide() {
        assert_eq!(fonts_in(&json!({})), Fonts::default());
        assert_eq!(fonts_in(&json!({ "editor_font": "  " })), Fonts::default());
        let chosen = Fonts {
            editor: named("MonoLisaCode"),
            ui: named("Inter"),
        };
        let file = with_fonts(with_confirm_stop(json!({}), false), &chosen);
        assert_eq!(fonts_in(&file), chosen);
        assert!(!confirm_stop_in(&file));
        let file = with_fonts(file, &Fonts::default());
        assert_eq!(file, json!({ "confirm_stop": false }));
    }
}
