//! What Zide remembers about a project between runs: the file that was open, where each file was
//! scrolled to and left its caret, and the font size.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Folder inside the project where Zide keeps what it remembers about it, so what a project
/// costs in space is visible there.
const FOLDER: &str = ".zide";

/// One small file per kind of fact, in the project's `.zide` folder.
fn file_for(root: &Path, kind: &str) -> PathBuf {
    root.join(FOLDER).join(kind)
}

/// The file (relative to `root`) that was open last, if it still exists.
pub fn last_file(root: &Path) -> Option<String> {
    let relative = fs::read_to_string(file_for(root, "last-file")).ok()?;
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
    let size: f32 = fs::read_to_string(file_for(root, "font-size"))
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

/// Where a file was left: the first line on screen and the caret (0-based lines and chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub top: usize,
    pub line: usize,
    pub column: usize,
}

/// How many files keep their place per project; the least recently left are forgotten.
const MAX_VIEWS: usize = 100;

/// Where `relative` was left, if it was.
pub fn view(root: &Path, relative: &str) -> Option<View> {
    let text = fs::read_to_string(file_for(root, "views")).ok()?;
    text.lines().find_map(|entry| parse_view(entry, relative))
}

/// Remembers where `relative` was left. Best effort, like `remember_file`.
pub fn remember_view(root: &Path, relative: &str, view: View) {
    if relative.contains(['\t', '\n']) {
        return;
    }
    let file = file_for(root, "views");
    let old = fs::read_to_string(&file).unwrap_or_default();
    let mut entries = vec![format!(
        "{}\t{}\t{}\t{relative}",
        view.top, view.line, view.column
    )];
    entries.extend(
        old.lines()
            .filter(|entry| parse_view(entry, relative).is_none())
            .take(MAX_VIEWS - 1)
            .map(str::to_string),
    );
    write(root, "views", &entries.join("\n"));
}

/// An entry is `top<TAB>line<TAB>column<TAB>path`.
fn parse_view(entry: &str, relative: &str) -> Option<View> {
    let mut parts = entry.splitn(4, '\t');
    let (top, line, column) = (parts.next()?, parts.next()?, parts.next()?);
    (parts.next()? == relative).then_some(())?;
    Some(View {
        top: top.parse().ok()?,
        line: line.parse().ok()?,
        column: column.parse().ok()?,
    })
}

/// Creates the folder with a `.gitignore` that ignores it whole, so the project's own
/// `.gitignore` stays untouched.
fn write(root: &Path, kind: &str, content: &str) {
    let file = file_for(root, kind);
    if let Some(folder) = file.parent() {
        if fs::create_dir_all(folder).is_err() {
            return;
        }
        let ignore = folder.join(".gitignore");
        if !ignore.exists() {
            let _ = fs::write(ignore, "*\n");
        }
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
        assert_eq!(fs::read_to_string(root.join(".zide/.gitignore")).unwrap(), "*\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn each_file_keeps_where_it_was_left_and_the_oldest_are_forgotten() {
        let root = std::env::temp_dir().join(format!("zide-views-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let place = |top| View {
            top,
            line: top + 2,
            column: 3,
        };
        assert_eq!(view(&root, "a.rs"), None);
        remember_view(&root, "a.rs", place(10));
        remember_view(&root, "b.rs", place(20));
        remember_view(&root, "a.rs", place(30)); // leaving it again replaces the old place
        assert_eq!(view(&root, "a.rs"), Some(place(30)));
        assert_eq!(view(&root, "b.rs"), Some(place(20)));
        assert_eq!(view(&root, "c.rs"), None);
        for i in 0..MAX_VIEWS {
            remember_view(&root, &format!("f{i}.rs"), place(i));
        }
        assert_eq!(view(&root, "a.rs"), None); // pushed out
        assert_eq!(view(&root, "f99.rs"), Some(place(99)));
        remember_view(&root, "tab\there", place(1)); // not storable: ignored
        assert_eq!(view(&root, "tab\there"), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_font_size_is_remembered_per_project() {
        let root = std::env::temp_dir().join(format!("zide-font-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(font_size(&root), None);
        remember_font_size(&root, 21.5);
        assert_eq!(font_size(&root), Some(21.5));
        fs::write(file_for(&root, "font-size"), "big").unwrap();
        assert_eq!(font_size(&root), None);
        fs::remove_dir_all(root).unwrap();
    }
}
