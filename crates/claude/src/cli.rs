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

/// The installed `claude`, or `None` when it is missing or does not answer.
pub fn installed() -> Option<Version> {
    let output = Command::new("claude").arg("--version").output().ok()?;
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
