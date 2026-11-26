//! The projects Zide opened before, most recent first: what the welcome screen lists.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// More than this and the list stops being a shortcut.
const MAX_PROJECTS: usize = 50;

fn list_file() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("zide").join("projects"))
}

/// The projects opened before that still exist, most recent first.
pub fn recent() -> Vec<PathBuf> {
    list_file().map(|file| load(&file)).unwrap_or_default()
}

/// Puts `project` at the top of the list. Best effort: losing it only costs a shortcut.
pub fn record(project: &Path) {
    if let Some(file) = list_file() {
        remember(&file, project);
    }
}

fn load(file: &Path) -> Vec<PathBuf> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect()
}

fn remember(file: &Path, project: &Path) {
    let mut projects = load(file);
    projects.retain(|p| p != project);
    projects.insert(0, project.to_path_buf());
    projects.truncate(MAX_PROJECTS);
    let text: Vec<_> = projects.iter().map(|p| p.to_string_lossy()).collect();
    if let Some(folder) = file.parent() {
        let _ = fs::create_dir_all(folder);
    }
    let _ = fs::write(file, text.join("\n"));
}

/// The folder's name, which titles the project.
pub fn title(project: &Path) -> String {
    project
        .file_name()
        .unwrap_or(project.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Where the project lives, with the home folder written as `~`.
pub fn location(project: &Path) -> String {
    match dirs::home_dir().and_then(|home| project.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => project.display().to_string(),
    }
}

/// Up to two letters that stand for the title: the first letters of its first two words.
pub fn initials(title: &str) -> String {
    let words = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty());
    words
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-projects-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_latest_project_goes_first_and_is_listed_once() {
        let dir = scratch("order");
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let file = dir.join("projects");
        remember(&file, &a);
        remember(&file, &b);
        remember(&file, &a);
        assert_eq!(load(&file), vec![a, b]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_project_that_is_gone_drops_out() {
        let dir = scratch("gone");
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let file = dir.join("projects");
        remember(&file, &a);
        remember(&file, &b);
        fs::remove_dir(&a).unwrap();
        assert_eq!(load(&file), vec![b]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn titles_locations_and_initials() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(title(&home.join("code/tri_ecosystem")), "tri_ecosystem");
        assert_eq!(location(&home.join("code/zide")), "~/code/zide");
        assert_eq!(location(Path::new("/opt/x")), "/opt/x");
        assert_eq!(initials("tri_ecosystem"), "TE");
        assert_eq!(initials("foundation"), "F");
        assert_eq!(initials("z-card-maker"), "ZC");
    }
}
