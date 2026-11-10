//! What Zide remembers about a project between runs: the file that was open, where each file was
//! scrolled to and left its caret, and the font size.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Folder inside the project where Zide keeps what it remembers about it, so what a project
/// costs in space is visible there.
const FOLDER: &str = zide_core::STATE_FOLDER;

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

/// Whether the minimap is shown in this project; `None` when it was never decided.
pub fn minimap(root: &Path) -> Option<bool> {
    match fs::read_to_string(file_for(root, "minimap")).ok()?.trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// Best effort, like `remember_file`.
pub fn remember_minimap(root: &Path, shown: bool) {
    write(root, "minimap", if shown { "on" } else { "off" });
}

/// Whether this project lists the files git ignores; `None` when it was never decided.
pub fn show_ignored(root: &Path) -> Option<bool> {
    match fs::read_to_string(file_for(root, "show-ignored"))
        .ok()?
        .trim()
    {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// Best effort, like `remember_file`.
pub fn remember_show_ignored(root: &Path, shown: bool) {
    write(root, "show-ignored", if shown { "on" } else { "off" });
}

/// The slash commands Claude reported the last time it ran in this project, one per line.
pub fn claude_commands(root: &Path) -> Vec<String> {
    fs::read_to_string(file_for(root, "claude-commands"))
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Best effort, like `remember_file`.
pub fn remember_claude_commands(root: &Path, commands: &[String]) {
    write(root, "claude-commands", &commands.join("\n"));
}

/// How the project was left: the panels that were showing and, when the Claude chat was, the
/// conversation it was on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub tree: bool,
    pub commit: bool,
    pub preview: bool,
    pub claude: bool,
    /// The conversation the chat was on: its session id and its title.
    pub chat: Option<(String, String)>,
}

/// How the project was left, `None` when it never was.
pub fn layout(root: &Path) -> Option<Layout> {
    let text = fs::read_to_string(file_for(root, "layout")).ok()?;
    let mut layout = Layout::default();
    for line in text.lines() {
        match line.split_once('=') {
            Some(("tree", on)) => layout.tree = on == "1",
            Some(("commit", on)) => layout.commit = on == "1",
            Some(("preview", on)) => layout.preview = on == "1",
            Some(("claude", on)) => layout.claude = on == "1",
            Some(("chat", chat)) => {
                let (id, title) = chat.split_once('\t')?;
                layout.chat = Some((id.to_owned(), title.to_owned()));
            }
            _ => {}
        }
    }
    Some(layout)
}

/// Best effort, like `remember_file`.
pub fn remember_layout(root: &Path, layout: &Layout) {
    let flag = |on: bool| if on { "1" } else { "0" };
    let mut text = format!(
        "tree={}\ncommit={}\npreview={}\nclaude={}\n",
        flag(layout.tree),
        flag(layout.commit),
        flag(layout.preview),
        flag(layout.claude)
    );
    if let Some((id, title)) = &layout.chat {
        text.push_str(&format!("chat={id}\t{}\n", one_line(title)));
    }
    write(root, "layout", &text);
}

/// The titles of the conversations of the project's live agents, by session id: what the CLI
/// calls each one (or the first thing said in it), kept so they need not be read again.
pub fn claude_titles(root: &Path) -> HashMap<String, String> {
    fs::read_to_string(file_for(root, "claude-titles"))
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(id, title)| (id.to_owned(), title.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Best effort, like `remember_file`.
pub fn remember_claude_titles(root: &Path, titles: &HashMap<String, String>) {
    let mut lines: Vec<_> = titles
        .iter()
        .map(|(id, title)| format!("{id}\t{}", one_line(title)))
        .collect();
    lines.sort();
    write(root, "claude-titles", &lines.join("\n"));
}

/// A title as one line without tabs, which the files separate their fields with.
fn one_line(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
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

/// The branches as of the last time the switcher read them, to show at once while it reads them
/// again. `None` when there is no cache yet.
pub fn branches(root: &Path) -> Option<zide_git::Branches> {
    let text = fs::read_to_string(file_for(root, "branches")).ok()?;
    Some(zide_git::Branches::from_cache(&text))
}

/// Keeps `branches` for the next run. Best effort, like `remember_file`.
pub fn remember_branches(root: &Path, branches: &zide_git::Branches) {
    write(root, "branches", &branches.to_cache());
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
        assert_eq!(
            fs::read_to_string(root.join(".zide/.gitignore")).unwrap(),
            "*\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_commands_come_back_in_order() {
        let root = std::env::temp_dir().join(format!("zide-commands-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(claude_commands(&root).is_empty());
        let commands = vec!["compact".to_owned(), "review".to_owned()];
        remember_claude_commands(&root, &commands);
        assert_eq!(claude_commands(&root), commands);
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
    fn the_show_ignored_choice_is_remembered_per_project() {
        let root = std::env::temp_dir().join(format!("zide-ignored-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(show_ignored(&root), None);
        remember_show_ignored(&root, true);
        assert_eq!(show_ignored(&root), Some(true));
        remember_show_ignored(&root, false);
        assert_eq!(show_ignored(&root), Some(false));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_minimap_choice_is_remembered_per_project() {
        let root = std::env::temp_dir().join(format!("zide-minimap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(minimap(&root), None);
        remember_minimap(&root, false);
        assert_eq!(minimap(&root), Some(false));
        remember_minimap(&root, true);
        assert_eq!(minimap(&root), Some(true));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_layout_and_the_titles_come_back() {
        let root = std::env::temp_dir().join(format!("zide-layout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(layout(&root), None);
        let left = Layout {
            tree: true,
            commit: false,
            preview: true,
            claude: true,
            chat: Some(("abc".to_owned(), "fix\tthe\nbug".to_owned())),
        };
        remember_layout(&root, &left);
        let back = layout(&root).unwrap();
        assert_eq!(
            back.chat,
            Some(("abc".to_owned(), "fix the bug".to_owned()))
        );
        assert_eq!(
            (back.tree, back.commit, back.preview, back.claude),
            (true, false, true, true)
        );
        let titles = HashMap::from([("s1".to_owned(), "First one".to_owned())]);
        remember_claude_titles(&root, &titles);
        assert_eq!(claude_titles(&root), titles);
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
