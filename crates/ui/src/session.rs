//! What Zide remembers about a project between runs: the file that was open.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// One small file per project, named after the project's path.
fn file_for(root: &Path) -> Option<PathBuf> {
    let name = root.to_string_lossy().replace(['/', '\\', ':'], "_");
    Some(
        dirs::data_dir()?
            .join("zide")
            .join("sessions")
            .join(format!("{name}.last-file")),
    )
}

/// The file (relative to `root`) that was open last, if it still exists.
pub fn last_file(root: &Path) -> Option<String> {
    let relative = fs::read_to_string(file_for(root)?).ok()?;
    let relative = relative.trim();
    root.join(relative).is_file().then(|| relative.to_string())
}

/// Remembers `relative` as the file that was open last. Best effort: losing it only means the
/// editor starts empty next time.
pub fn remember_file(root: &Path, relative: &str) {
    let Some(file) = file_for(root) else {
        return;
    };
    if let Some(folder) = file.parent() {
        let _ = fs::create_dir_all(folder);
    }
    let _ = fs::write(file, relative);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_file_comes_back_only_while_it_exists() {
        let root = std::env::temp_dir().join(format!("zide-session-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(last_file(&root), None);
        fs::write(root.join("src/a.rs"), "").unwrap();
        remember_file(&root, "src/a.rs");
        assert_eq!(last_file(&root).as_deref(), Some("src/a.rs"));
        fs::remove_file(root.join("src/a.rs")).unwrap();
        assert_eq!(last_file(&root), None);
        fs::remove_file(file_for(&root).unwrap()).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
