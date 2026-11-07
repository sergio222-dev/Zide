//! What `claude` uses when nothing is chosen: the `model` and `effortLevel` of the settings
//! files, the project's over the user's.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// The model alias and effort level the settings name, `None` for what they leave unsaid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The defaults for the project at `root`.
pub fn defaults(root: &Path) -> Defaults {
    let config = match env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => Some(PathBuf::from(dir)),
        None => env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")),
    };
    let files = [
        config.map(|dir| dir.join("settings.json")),
        Some(root.join(".claude/settings.json")),
        Some(root.join(".claude/settings.local.json")),
    ];
    defaults_from(files.into_iter().flatten())
}

/// The defaults the settings `files` give, a later file overriding an earlier one.
pub fn defaults_from(files: impl IntoIterator<Item = PathBuf>) -> Defaults {
    let mut found = Defaults::default();
    for file in files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(model) = settings["model"].as_str() {
            found.model = Some(model.to_owned());
        }
        if let Some(effort) = settings["effortLevel"].as_str() {
            found.effort = Some(effort.to_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Defaults, defaults_from};

    #[test]
    fn a_later_settings_file_overrides_an_earlier_one() {
        let dir = std::env::temp_dir().join(format!("zide-defaults-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (user, project, broken) = (
            dir.join("user.json"),
            dir.join("project.json"),
            dir.join("broken.json"),
        );
        fs::write(&user, r#"{"model":"sonnet","effortLevel":"medium"}"#).unwrap();
        fs::write(&project, r#"{"model":"opus"}"#).unwrap();
        fs::write(&broken, "{not json").unwrap();
        let missing = dir.join("missing.json");
        assert_eq!(
            defaults_from([user.clone(), missing.clone()]),
            Defaults {
                model: Some("sonnet".into()),
                effort: Some("medium".into())
            }
        );
        assert_eq!(
            defaults_from([user, broken, project, missing]),
            Defaults {
                model: Some("opus".into()),
                effort: Some("medium".into())
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
