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
    /// The size of the context at which the CLI compacts it by itself: the `autoCompactWindow` a
    /// model's settings name (`modelSettings.<model>`), else the one at the top.
    pub compact: Option<u64>,
    pub compact_by_model: std::collections::HashMap<String, u64>,
}

impl Defaults {
    /// Where the conversation of `model` is compacted by itself, when the settings say.
    pub fn compact_at(&self, model: Option<&str>) -> Option<u64> {
        model
            .and_then(|model| self.compact_by_model.get(model).copied())
            .or(self.compact)
    }
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
        if let Some(window) = settings["autoCompactWindow"].as_u64() {
            found.compact = Some(window);
        }
        for (model, own) in settings["modelSettings"].as_object().into_iter().flatten() {
            if let Some(window) = own["autoCompactWindow"].as_u64() {
                found.compact_by_model.insert(model.clone(), window);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Defaults, defaults_from};

    #[test]
    fn the_auto_compact_window_is_per_model_with_a_general_fallback() {
        let dir = std::env::temp_dir().join(format!("zide-compact-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.json");
        fs::write(
            &file,
            r#"{"autoCompactWindow":300000,"modelSettings":{"m1":{"autoCompactWindow":400000},"m2":{}}}"#,
        )
        .unwrap();
        let found = defaults_from([file]);
        assert_eq!(found.compact_at(Some("m1")), Some(400_000));
        assert_eq!(found.compact_at(Some("m2")), Some(300_000));
        assert_eq!(found.compact_at(None), Some(300_000));
        assert_eq!(Defaults::default().compact_at(Some("m1")), None);
        fs::remove_dir_all(dir).unwrap();
    }

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
                effort: Some("medium".into()),
                ..Defaults::default()
            }
        );
        assert_eq!(
            defaults_from([user, broken, project, missing]),
            Defaults {
                model: Some("opus".into()),
                effort: Some("medium".into()),
                ..Defaults::default()
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
