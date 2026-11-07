use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// The installed `claude`, or `None` when it is missing or does not answer.
pub fn installed() -> Option<Version> {
    let output = Command::new(program()).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| Version::parse(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
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

    #[test]
    fn older_releases_are_not_supported() {
        assert!(Version::parse("2.1.319").unwrap().is_supported());
        assert!(Version::parse("2.2.0").unwrap().is_supported());
        assert!(!Version::parse("2.1.318").unwrap().is_supported());
        assert!(!Version::parse("1.9.999").unwrap().is_supported());
    }
}
