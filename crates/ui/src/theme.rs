//! Colors of Zide. Every view reads the theme in use through `current()` while it renders, so
//! switching themes (`set`) repaints with the new colors on the next frame.

use std::sync::RwLock;

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
