//! What Zide remembers about a project between runs: the file that was open and the font size.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// One small file per project and kind of fact, named after the project's path.
fn file_for(root: &Path, kind: &str) -> Option<PathBuf> {
    let name = root.to_string_lossy().replace(['/', '\\', ':'], "_");
    Some(
        dirs::data_dir()?
            .join("zide")
            .join("sessions")
            .join(format!("{name}.{kind}")),
    )
}

/// The file (relative to `root`) that was open last, if it still exists.
pub fn last_file(root: &Path) -> Option<String> {
    let relative = fs::read_to_string(file_for(root, "last-file")?).ok()?;
    let relative = relative.trim();
    root.join(relative).is_file().then(|| relative.to_string())
}

/// Remembers `relative` as the file that was open last. Best effort: losing it only means the
/// editor starts empty next time.
pub fn remember_file(root: &Path, relative: &str) {
    write(root, "last-file", relative);
}

/// The editor font size this project was left with.
pub fn font_size(root: &Path) -> Option<f32> {
    let size: f32 = fs::read_to_string(file_for(root, "font-size")?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    size.is_finite().then_some(size)
}

/// Best effort, like `remember_file`.
pub fn remember_font_size(root: &Path, size: f32) {
    write(root, "font-size", &size.to_string());
}

fn write(root: &Path, kind: &str, content: &str) {
    let Some(file) = file_for(root, kind) else {
        return;
    };
    if let Some(folder) = file.parent() {
        let _ = fs::create_dir_all(folder);
    }
    let _ = fs::write(file, content);
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
        fs::remove_file(file_for(&root, "last-file").unwrap()).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_font_size_is_remembered_per_project() {
        let root = std::env::temp_dir().join(format!("zide-font-{}", std::process::id()));
        assert_eq!(font_size(&root), None);
        remember_font_size(&root, 21.5);
        assert_eq!(font_size(&root), Some(21.5));
        fs::write(file_for(&root, "font-size").unwrap(), "big").unwrap();
        assert_eq!(font_size(&root), None);
        fs::remove_file(file_for(&root, "font-size").unwrap()).unwrap();
    }
}
