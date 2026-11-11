use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;

/// A `claude` release, as printed by `claude --version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// The release the stream-json handling and its fixtures were captured from. The
/// protocol is not documented as stable, so older releases are not assumed to work.
pub const MIN_VERSION: Version = Version {
    major: 2,
    minor: 1,
    patch: 319,
};

impl Version {
    /// Reads the output of `claude --version`, e.g. `2.1.319 (Claude Code)`.
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split_whitespace().next()?.split('.');
        let mut number = || parts.next()?.parse().ok();
        Some(Version {
            major: number()?,
            minor: number()?,
            patch: number()?,
        })
    }

    pub fn is_supported(self) -> bool {
        self >= MIN_VERSION
    }
}

/// Where the installer puts `claude` besides the places `PATH` names, which a launcher that is
/// not a shell (a desktop entry, a dock icon) often leaves out of it.
fn usual_places() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut places: Vec<PathBuf> = home
        .iter()
        .flat_map(|home| [home.join(".local/bin"), home.join(".claude/local")])
        .collect();
    places.extend(["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"].map(PathBuf::from));
    places
}

/// The first of `dirs` that holds a file called `name`.
fn find_in(dirs: impl IntoIterator<Item = PathBuf>, name: &str) -> Option<PathBuf> {
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|path| Path::is_file(path))
}

/// What to run for `claude`: the one `PATH` finds, else one in the usual places, else the bare
/// name, so that the error says what is missing.
pub fn program() -> PathBuf {
    let on_path = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    find_in(on_path.into_iter().chain(usual_places()), "claude")
        .unwrap_or_else(|| PathBuf::from("claude"))
}

/// How `claude` is started. The default runs `claude` itself; a configured `command` (a
/// sandbox wrapper, say) is run instead, with `args` placed before Claude's own arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launcher {
    pub command: Option<PathBuf>,
    pub args: Vec<String>,
    /// Where that `claude` keeps its config and sessions, when it is not the usual place
    /// (a sandbox with its own home).
    pub config_dir: Option<PathBuf>,
}

static LAUNCHER: RwLock<Launcher> = RwLock::new(Launcher {
    command: None,
    args: Vec::new(),
    config_dir: None,
});

/// Makes every later start of `claude` go through `launcher`; running processes keep theirs.
pub fn set_launcher(launcher: Launcher) {
    *LAUNCHER.write().unwrap_or_else(|e| e.into_inner()) = launcher;
}

pub fn launcher() -> Launcher {
    LAUNCHER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The command that starts `claude` as `launcher` says; the caller adds Claude's arguments.
pub(crate) fn command_for(launcher: &Launcher) -> Command {
    match &launcher.command {
        Some(program) => {
            let mut command = Command::new(program);
            command.args(&launcher.args);
            command
        }
        None => Command::new(program()),
    }
}

/// The command that starts `claude` as configured now.
pub(crate) fn command() -> Command {
    command_for(&launcher())
}

/// Claude's config folder: the configured one, else `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn config_dir_for(
    launcher: &Launcher,
    env: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    launcher
        .config_dir
        .clone()
        .or(env)
        .or_else(|| Some(home?.join(".claude")))
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    config_dir_for(
        &launcher(),
        env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

/// Asks `launcher`'s `claude` for its version; the error is why it could not tell (what the
/// command printed on failure, or why it did not start).
pub fn probe(launcher: &Launcher) -> Result<Version, String> {
    let output = command_for(launcher)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    if !output.status.success() {
        let (err, out) = (text(&output.stderr), text(&output.stdout));
        let why = if err.is_empty() { out } else { err };
        return Err(if why.is_empty() {
            format!("exited with {}", output.status)
        } else {
            why
        });
    }
    let out = text(&output.stdout);
    Version::parse(&out).ok_or_else(|| format!("not a Claude version: {out}"))
}

/// The installed `claude`, or `None` when it is missing or does not answer.
pub fn installed() -> Option<Version> {
    probe(&launcher()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_directory_that_has_the_file_wins() {
        let root = std::env::temp_dir().join(format!("zide-claude-find-{}", std::process::id()));
        let (empty, first, second) = (root.join("a"), root.join("b"), root.join("c"));
        for dir in [&empty, &first, &second] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(first.join("claude"), "").unwrap();
        std::fs::write(second.join("claude"), "").unwrap();
        let found = find_in([empty.clone(), first.clone(), second], "claude");
        assert_eq!(found, Some(first.join("claude")));
        assert_eq!(find_in([empty], "claude"), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_the_version_line() {
        assert_eq!(
            Version::parse("2.1.319 (Claude Code)\n"),
            Some(Version {
                major: 2,
                minor: 1,
                patch: 319
            })
        );
    }

    #[test]
    fn rejects_text_that_is_not_a_version() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("claude: command not found"), None);
        assert_eq!(Version::parse("2.1"), None);
    }

    fn configured(command: &str, args: &[&str]) -> Launcher {
        Launcher {
            command: Some(PathBuf::from(command)),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            config_dir: None,
        }
    }

    fn program_and_args(command: &Command) -> (String, Vec<String>) {
        let text = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
        (
            text(command.get_program()),
            command.get_args().map(text).collect(),
        )
    }

    #[test]
    fn without_a_configured_command_it_runs_claude() {
        let mut command = command_for(&Launcher::default());
        command.arg("--version");
        let (program, args) = program_and_args(&command);
        assert!(program.ends_with("claude"), "{program}");
        assert_eq!(args, ["--version"]);
    }

    #[test]
    fn a_configured_command_runs_its_args_before_claudes() {
        let mut command = command_for(&configured("/bin/sandbox.sh", &["claude", "--x"]));
        command.args(["-p", "--verbose"]);
        assert_eq!(
            program_and_args(&command),
            (
                "/bin/sandbox.sh".to_string(),
                vec![
                    "claude".into(),
                    "--x".into(),
                    "-p".into(),
                    "--verbose".into()
                ]
            )
        );
    }

    #[test]
    fn the_configured_config_dir_wins_over_the_environment() {
        let mut launcher = Launcher::default();
        let env = Some(PathBuf::from("/env/claude"));
        let home = Some(PathBuf::from("/home/me"));
        assert_eq!(
            config_dir_for(&launcher, env.clone(), home.clone()),
            env.clone()
        );
        assert_eq!(
            config_dir_for(&launcher, None, home.clone()),
            Some(PathBuf::from("/home/me/.claude"))
        );
        launcher.config_dir = Some(PathBuf::from("/sandbox/.claude"));
        assert_eq!(
            config_dir_for(&launcher, env, home),
            Some(PathBuf::from("/sandbox/.claude"))
        );
    }

    #[cfg(unix)]
    fn script(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zide-launcher-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn probing_runs_version_through_the_configured_command() {
        let path = script(
            "ok",
            r#"[ "$1" = claude ] && [ "$2" = --version ] && echo "2.1.400 (Claude Code)""#,
        );
        let launcher = configured(path.to_str().unwrap(), &["claude"]);
        assert_eq!(probe(&launcher), Ok(Version::parse("2.1.400").unwrap()));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_probe_says_why() {
        let path = script("fail", "echo 'bwrap: No such file' >&2; exit 1");
        let launcher = configured(path.to_str().unwrap(), &["claude"]);
        assert_eq!(probe(&launcher), Err("bwrap: No such file".to_string()));
        let missing = configured("/nonexistent/agent.sh", &[]);
        assert!(probe(&missing).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn older_releases_are_not_supported() {
        assert!(Version::parse("2.1.319").unwrap().is_supported());
        assert!(Version::parse("2.2.0").unwrap().is_supported());
        assert!(!Version::parse("2.1.318").unwrap().is_supported());
        assert!(!Version::parse("1.9.999").unwrap().is_supported());
    }
}
