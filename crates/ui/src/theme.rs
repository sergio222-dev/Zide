//! Colors of Zide. Every view reads the theme in use through `current()` while it renders, so
//! switching themes (`set`) repaints with the new colors on the next frame.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde_json::Value;

/// A color scheme. Colors are `0xRRGGBB`, except those documented as `0xRRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    /// Light themes switch gpui-kit's components to their light mode.
    pub light: bool,

    pub background: u32,
    pub text: u32,
    /// Line numbers, separators.
    pub dim: u32,
    /// Status line, breadcrumb path.
    pub muted: u32,
    /// The current file in the breadcrumb.
    pub strong: u32,
    pub border: u32,
    pub symbol: u32,

    pub selection: u32,
    /// Other occurrences of the word under the caret.
    pub word: u32,
    /// The bracket under the caret and its partner.
    pub bracket: u32,
    pub find_match: u32,
    pub find_current: u32,
    pub caret_bar: u32,
    /// The block caret of Vim's Normal mode (RGBA).
    pub caret_block: u32,

    pub tree_selected: u32,
    pub tree_focused: u32,

    /// The popup, the find panel and their controls.
    pub popup: u32,
    pub control_border: u32,
    pub active_bg: u32,
    pub active_border: u32,
    pub error: u32,

    /// Rainbow indent: one translucent band per indent level, cycling through these (RGBA).
    pub indent_bands: [u32; 4],
    /// The dots that mark whitespace (RGBA).
    pub whitespace: u32,

    /// Gutter bars: lines new, changed or removed since the last commit.
    pub change_added: u32,
    pub change_modified: u32,
    pub change_removed: u32,

    /// The diff of a file: lines added, removed and replaced (blue, on both sides).
    pub diff_added: u32,
    pub diff_removed: u32,
    pub diff_changed: u32,
    /// Lines both sides changed, in the merge editor.
    pub diff_conflict: u32,
    /// The lines a chat's edit added or removed, over the text (RGBA).
    pub tint_added: u32,
    pub tint_removed: u32,

    /// The minimap: the line of the cursor.
    pub minimap_cursor: u32,
    /// The minimap: the part of the file the editor shows (RGBA).
    pub minimap_view: u32,

    /// The chip with a parameter's name before an argument.
    pub hint_bg: u32,
    pub hint_text: u32,

    /// Icons of the project tree.
    pub icon_folder: u32,
    pub icon_code: u32,
    pub icon_data: u32,
    pub icon_media: u32,

    /// Success marks (a tool that finished well).
    pub ok: u32,
    /// A chat that is thinking.
    pub working: u32,
    /// A chat that waits for the user to allow something.
    pub needs_user: u32,

    /// Syntax highlighting, one color per class of token.
    pub syntax_keyword: u32,
    pub syntax_string: u32,
    pub syntax_number: u32,
    pub syntax_comment: u32,
    pub syntax_function: u32,
    pub syntax_type: u32,
    pub syntax_property: u32,
    pub syntax_variable: u32,
    pub syntax_operator: u32,
    pub syntax_emphasis: u32,
    pub syntax_link: u32,
}

/// Zide's own dark scheme, in the spirit of One Dark.
pub fn dark() -> Theme {
    Theme {
        name: "Zide Dark".into(),
        light: false,
        background: 0x1c1d20,
        text: 0xabb2bf,
        dim: 0x5c6370,
        muted: 0x9da5b4,
        strong: 0xe6e6e6,
        border: 0x2f3136,
        symbol: 0xc678dd,
        selection: 0x3e4451,
        word: 0x33384a,
        bracket: 0x3f4b6e,
        find_match: 0x5a4a22,
        find_current: 0x8a6d1d,
        caret_bar: 0x61afef,
        caret_block: 0xff7a0099,
        tree_selected: 0x2c313a,
        tree_focused: 0x2f4a73,
        popup: 0x25262b,
        control_border: 0x3a3c42,
        active_bg: 0x2f4a73,
        active_border: 0x61afef,
        error: 0xe06c75,
        indent_bands: [0xe5c07b1c, 0x98c3791c, 0x61afef1c, 0xc678dd1c],
        whitespace: 0x5c6370b0,
        change_added: 0x4f9a56,
        change_modified: 0x9a7040,
        change_removed: 0x7d848f,
        diff_added: 0x2b4a38,
        diff_removed: 0x3a3d44,
        diff_changed: 0x2a4163,
        diff_conflict: 0x4a3030,
        tint_added: 0x98c37926,
        tint_removed: 0xe06c7526,
        minimap_cursor: 0x3b82f6,
        minimap_view: 0xffffff1a,
        hint_bg: 0x2f3239,
        hint_text: 0x8c93a1,
        icon_folder: 0xe5c07b,
        icon_code: 0x61afef,
        icon_data: 0x98c379,
        icon_media: 0xc678dd,
        ok: 0x98c379,
        working: 0xe5a14e,
        needs_user: 0x61afef,
        syntax_keyword: 0xc678dd,
        syntax_string: 0x98c379,
        syntax_number: 0xd19a66,
        syntax_comment: 0x7f848e,
        syntax_function: 0x61afef,
        syntax_type: 0xe5c07b,
        syntax_property: 0xd19a66,
        syntax_variable: 0xe06c75,
        syntax_operator: 0xabb2bf,
        syntax_emphasis: 0xc678dd,
        syntax_link: 0x56b6c2,
    }
}

static CURRENT: RwLock<Option<&'static Theme>> = RwLock::new(None);

/// The theme in use: the default dark one until `set` picks another.
pub fn current() -> &'static Theme {
    let current = *CURRENT.read().unwrap_or_else(|e| e.into_inner());
    current.unwrap_or_else(|| {
        let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
        *slot.get_or_insert_with(|| Box::leak(Box::new(dark())))
    })
}

/// Makes `theme` the one `current` returns. Each theme is kept for the rest of the run (it is a
/// few hundred bytes), so views can hold the reference without a lock.
#[allow(dead_code)] // the theme selector calls it
pub fn set(theme: &'static Theme) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(theme);
}

/// Every theme to pick from: the built-in ones (the default first), then the user's from
/// `~/.config/zide/themes/*.json`, sorted by name. A user theme named like a built-in one takes its
/// place. The files are read once, on the first call.
#[allow(dead_code)] // the theme selector calls it
pub fn all() -> &'static [&'static Theme] {
    &loaded().0
}

/// The theme called `name`, if there is one.
#[allow(dead_code)] // the theme selector calls it
pub fn find(name: &str) -> Option<&'static Theme> {
    all().iter().copied().find(|theme| theme.name == name)
}

/// What was wrong in the theme files read by `all`, one line each, to report once.
#[allow(dead_code)] // the theme selector calls it
pub fn problems() -> Vec<String> {
    loaded().1.clone()
}

fn loaded() -> &'static (Vec<&'static Theme>, Vec<String>) {
    static LOADED: OnceLock<(Vec<&'static Theme>, Vec<String>)> = OnceLock::new();
    LOADED.get_or_init(|| {
        let dir = crate::settings::path().and_then(|path| Some(path.parent()?.join("themes")));
        let (themes, problems) = load(BUILT_IN, dir.as_deref());
        let themes = themes
            .into_iter()
            .map(|t| &*Box::leak(Box::new(t)))
            .collect();
        (themes, problems)
    })
}

/// The themes shipped with Zide, as (file name, text). The first is the default.
const BUILT_IN: &[(&str, &str)] = &[
    ("zide-dark.json", include_str!("../themes/zide-dark.json")),
    ("zide-light.json", include_str!("../themes/zide-light.json")),
    ("zide-ember.json", include_str!("../themes/zide-ember.json")),
    ("zide-fjord.json", include_str!("../themes/zide-fjord.json")),
    (
        "zide-high-contrast.json",
        include_str!("../themes/zide-high-contrast.json"),
    ),
];

/// The built-in themes, then the `.json` files of `user_dir` (if any) sorted by theme name; a user
/// theme with a built-in's name replaces it. Files that cannot be read or parsed are left out.
/// Every problem found comes back as a line that starts with the file's name.
fn load(built_in: &[(&str, &str)], user_dir: Option<&Path>) -> (Vec<Theme>, Vec<String>) {
    let mut problems = Vec::new();
    let read = |file: &str, text: &str, out: &mut Vec<Theme>, problems: &mut Vec<String>| {
        match parse(text) {
            Ok((theme, found)) => {
                problems.extend(found.into_iter().map(|p| format!("{file}: {p}")));
                out.push(theme);
            }
            Err(e) => problems.push(format!("{file}: {e}")),
        }
    };
    let mut themes = Vec::new();
    for (file, text) in built_in {
        read(file, text, &mut themes, &mut problems);
    }
    let mut user = Vec::new();
    let mut files: Vec<PathBuf> = user_dir
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    for path in files {
        let file = path.display().to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => read(&file, &text, &mut user, &mut problems),
            Err(e) => problems.push(format!("{file}: {e}")),
        }
    }
    user.sort_by(|a, b| a.name.cmp(&b.name));
    for theme in user {
        match themes.iter_mut().find(|t| t.name == theme.name) {
            Some(slot) => *slot = theme,
            None => themes.push(theme),
        }
    }
    (themes, problems)
}

/// Reads a theme file: `{"name": "...", "light": false, "colors": {"background": "#1c1d20", ...}}`.
/// Colors the file leaves out are the ones of `dark()`. Entries that are wrong (an unknown key, a
/// bad color) are skipped and described in the returned list; only a file that is not a JSON
/// object or has no name is an error.
fn parse(text: &str) -> Result<(Theme, Vec<String>), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let Value::Object(file) = value else {
        return Err("a theme must be a JSON object".into());
    };
    let mut theme = dark();
    let mut problems = Vec::new();
    match file.get("name").and_then(Value::as_str).map(str::trim) {
        Some(name) if !name.is_empty() => theme.name = name.to_string(),
        _ => return Err("\"name\" must be a non-empty string".into()),
    }
    match file.get("light") {
        None => theme.light = false,
        Some(Value::Bool(light)) => theme.light = *light,
        Some(_) => problems.push("\"light\" must be true or false".into()),
    }
    for key in file.keys() {
        if !matches!(key.as_str(), "name" | "light" | "colors") {
            problems.push(format!("unknown key \"{key}\""));
        }
    }
    match file.get("colors") {
        None => {}
        Some(Value::Object(colors)) => {
            for (key, value) in colors {
                if let Err(e) = set_color(&mut theme, key, value) {
                    problems.push(format!("colors.{key}: {e}"));
                }
            }
        }
        Some(_) => problems.push("\"colors\" must be an object".into()),
    }
    Ok((theme, problems))
}

/// Sets the color `key` of `theme` from a JSON value, leaving it as it was when the value is wrong.
fn set_color(theme: &mut Theme, key: &str, value: &Value) -> Result<(), String> {
    if key == "indent_bands" {
        let bands = value
            .as_array()
            .filter(|bands| bands.len() == theme.indent_bands.len())
            .ok_or("must be a list of 4 colors")?;
        let mut parsed = [0; 4];
        for (slot, band) in parsed.iter_mut().zip(bands) {
            *slot = color(band, true)?;
        }
        theme.indent_bands = parsed;
        return Ok(());
    }
    let (slot, alpha) = color_slot(theme, key).ok_or("unknown color")?;
    *slot = color(value, alpha)?;
    Ok(())
}

/// A color written as "#rrggbb", or also "#rrggbbaa" where `alpha` (an opaque "#rrggbb" then gets
/// alpha ff).
fn color(value: &Value, alpha: bool) -> Result<u32, String> {
    let text = value.as_str().ok_or("must be a string like \"#rrggbb\"")?;
    let hex = text
        .strip_prefix('#')
        .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()));
    let bad = || {
        if alpha {
            format!("\"{text}\" is not a color like \"#rrggbb\" or \"#rrggbbaa\"")
        } else {
            format!("\"{text}\" is not a color like \"#rrggbb\"")
        }
    };
    let hex = hex.ok_or_else(bad)?;
    let n = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
    match hex.len() {
        6 if alpha => Ok(n << 8 | 0xff),
        6 => Ok(n),
        8 if alpha => Ok(n),
        _ => Err(bad()),
    }
}

/// Lists every single color of `Theme` once: its key in a theme file, and whether it has alpha.
macro_rules! color_keys {
    ($($field:ident $(: $alpha:ident)?),* $(,)?) => {
        /// Every key of `colors` in a theme file.
        #[cfg(test)]
        const COLOR_KEYS: &[&str] = &["indent_bands", $(stringify!($field)),*];

        /// The color `key` of `theme`, and whether it is RGBA.
        fn color_slot<'a>(theme: &'a mut Theme, key: &str) -> Option<(&'a mut u32, bool)> {
            match key {
                $(stringify!($field) => Some((&mut theme.$field, color_keys!(@alpha $($alpha)?))),)*
                _ => None,
            }
        }
    };
    (@alpha rgba) => { true };
    (@alpha) => { false };
}

color_keys! {
    background, text, dim, muted, strong, border, symbol,
    selection, word, bracket, find_match, find_current, caret_bar, caret_block: rgba,
    tree_selected, tree_focused,
    popup, control_border, active_bg, active_border, error,
    whitespace: rgba,
    change_added, change_modified, change_removed,
    diff_added, diff_removed, diff_changed, diff_conflict, tint_added: rgba, tint_removed: rgba,
    minimap_cursor, minimap_view: rgba,
    hint_bg, hint_text,
    icon_folder, icon_code, icon_data, icon_media,
    ok, working, needs_user,
    syntax_keyword, syntax_string, syntax_number, syntax_comment, syntax_function, syntax_type,
    syntax_property, syntax_variable, syntax_operator, syntax_emphasis, syntax_link,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(themes: &[Theme]) -> Vec<&str> {
        themes.iter().map(|t| t.name.as_str()).collect()
    }

    /// A fresh empty directory under the system temp dir.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-themes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_built_in_theme_parses_without_problems_and_gives_every_color() {
        for (file, text) in BUILT_IN {
            let (_, problems) = parse(text).unwrap();
            assert!(problems.is_empty(), "{file}: {problems:?}");
            let file_keys: Value = serde_json::from_str(text).unwrap();
            let colors = file_keys["colors"].as_object().unwrap();
            for key in COLOR_KEYS {
                assert!(colors.contains_key(*key), "{file} lacks {key}");
            }
        }
    }

    #[test]
    fn zide_dark_file_is_the_rust_default() {
        let (theme, _) = parse(BUILT_IN[0].1).unwrap();
        assert_eq!(theme, dark());
    }

    #[test]
    fn built_ins_come_first_with_the_default_first() {
        let (themes, problems) = load(BUILT_IN, None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            names(&themes),
            [
                "Zide Dark",
                "Zide Light",
                "Zide Ember",
                "Zide Fjord",
                "Zide High Contrast"
            ]
        );
        assert!(themes[1].light);
    }

    #[test]
    fn a_partial_file_falls_back_to_the_default_colors() {
        let (theme, problems) =
            parse(r##"{"name": "Mine", "colors": {"background": "#102030"}}"##).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.background, 0x102030);
        assert_eq!(theme.text, dark().text);
        assert_eq!(theme.indent_bands, dark().indent_bands);
        assert!(!theme.light);
    }

    #[test]
    fn colors_take_alpha_only_where_documented() {
        let (theme, problems) = parse(
            r##"{"name": "A", "colors": {"caret_block": "#11223344", "whitespace": "#112233",
                "indent_bands": ["#01020304", "#05060708", "#090a0b0c", "#0d0e0f"]}}"##,
        )
        .unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.caret_block, 0x11223344);
        assert_eq!(theme.whitespace, 0x112233ff);
        assert_eq!(
            theme.indent_bands,
            [0x01020304, 0x05060708, 0x090a0b0c, 0x0d0e0fff]
        );
    }

    #[test]
    fn wrong_entries_are_reported_and_skipped() {
        let (theme, problems) = parse(
            r##"{"name": "Bad", "light": "yes", "extra": 1, "colors": {
                "background": "#zzzzzz", "text": "#11223344", "bogus": "#000000",
                "dim": 5, "indent_bands": ["#000000"], "muted": "#123456"}}"##,
        )
        .unwrap();
        let base = dark();
        assert_eq!(theme.background, base.background);
        assert_eq!(theme.text, base.text);
        assert_eq!(theme.dim, base.dim);
        assert_eq!(theme.indent_bands, base.indent_bands);
        assert_eq!(theme.muted, 0x123456);
        assert!(!theme.light);
        let all = problems.join("\n");
        for expected in [
            "\"light\" must be true or false",
            "unknown key \"extra\"",
            "colors.background: \"#zzzzzz\" is not a color like \"#rrggbb\"",
            "colors.text: \"#11223344\" is not a color like \"#rrggbb\"",
            "colors.bogus: unknown color",
            "colors.dim: must be a string",
            "colors.indent_bands: must be a list of 4 colors",
        ] {
            assert!(all.contains(expected), "missing {expected:?} in:\n{all}");
        }
        assert_eq!(problems.len(), 7, "{all}");
    }

    #[test]
    fn unreadable_files_are_skipped_and_reported() {
        assert!(
            parse("{not json")
                .unwrap_err()
                .starts_with("not valid JSON")
        );
        assert!(parse("[1]").is_err());
        assert!(parse(r#"{"colors": {}}"#).unwrap_err().contains("\"name\""));

        let dir = scratch("broken");
        std::fs::write(dir.join("broken.json"), "{oops").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let (themes, problems) = load(BUILT_IN, Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(themes.len(), BUILT_IN.len());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("broken.json: not valid JSON"),
            "{problems:?}"
        );
    }

    #[test]
    fn user_themes_follow_sorted_by_name_and_replace_a_built_in_with_their_name() {
        let dir = scratch("user");
        std::fs::write(dir.join("a.json"), r#"{"name": "Zeta"}"#).unwrap();
        std::fs::write(dir.join("b.json"), r#"{"name": "Alpha"}"#).unwrap();
        std::fs::write(
            dir.join("c.json"),
            r##"{"name": "Zide Light", "colors": {"background": "#ababab"}}"##,
        )
        .unwrap();
        let (themes, problems) = load(BUILT_IN, Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            names(&themes),
            [
                "Zide Dark",
                "Zide Light",
                "Zide Ember",
                "Zide Fjord",
                "Zide High Contrast",
                "Alpha",
                "Zeta"
            ]
        );
        assert_eq!(themes[1].background, 0xababab);
        assert!(!themes[1].light);
    }

    #[test]
    fn a_missing_user_folder_is_not_a_problem() {
        let (themes, problems) = load(BUILT_IN, Some(Path::new("/nonexistent/zide/themes")));
        assert_eq!(themes.len(), BUILT_IN.len());
        assert!(problems.is_empty());
    }
}
