//! Colors of the editor and the project tree (a dark scheme in the spirit of One Dark).

pub const BACKGROUND: u32 = 0x1c1d20;
pub const TEXT: u32 = 0xabb2bf;
/// Line numbers, separators.
pub const DIM: u32 = 0x5c6370;
/// Status line, breadcrumb path.
pub const MUTED: u32 = 0x9da5b4;
/// The current file in the breadcrumb.
pub const STRONG: u32 = 0xe6e6e6;
pub const BORDER: u32 = 0x2f3136;
pub const SYMBOL: u32 = 0xc678dd;

pub const SELECTION: u32 = 0x3e4451;
/// Other occurrences of the word under the caret.
pub const WORD: u32 = 0x33384a;
/// The bracket under the caret and its partner.
pub const BRACKET: u32 = 0x3f4b6e;
pub const FIND_MATCH: u32 = 0x5a4a22;
pub const FIND_CURRENT: u32 = 0x8a6d1d;
pub const CARET_BAR: u32 = 0x61afef;

pub const TREE_SELECTED: u32 = 0x2c313a;
pub const TREE_FOCUSED: u32 = 0x2f4a73;

/// The popup, the find panel and their controls.
pub const POPUP: u32 = 0x25262b;
pub const CONTROL_BORDER: u32 = 0x3a3c42;
pub const ACTIVE_BG: u32 = 0x2f4a73;
pub const ACTIVE_BORDER: u32 = 0x61afef;
pub const ERROR: u32 = 0xe06c75;

/// Rainbow indent: one translucent band per indent level, cycling through these (RGBA).
pub const INDENT_BANDS: [u32; 4] = [0xe5c07b1c, 0x98c3791c, 0x61afef1c, 0xc678dd1c];
/// The dots that mark whitespace (RGBA).
pub const WHITESPACE: u32 = 0x5c6370b0;

/// Gutter bars: lines new, changed or removed since the last commit.
pub const CHANGE_ADDED: u32 = 0x4f9a56;
pub const CHANGE_MODIFIED: u32 = 0x9a7040;
pub const CHANGE_REMOVED: u32 = 0x7d848f;

/// The diff of a file: lines added, removed and replaced (blue, on both sides).
pub const DIFF_ADDED: u32 = 0x2b4a38;
pub const DIFF_REMOVED: u32 = 0x3a3d44;
pub const DIFF_CHANGED: u32 = 0x2a4163;

/// The minimap: the line of the cursor.
pub const MINIMAP_CURSOR: u32 = 0x3b82f6;

/// The chip with a parameter's name before an argument.
pub const HINT_BG: u32 = 0x2f3239;
pub const HINT_TEXT: u32 = 0x8c93a1;

/// Icons of the project tree.
pub const ICON_FOLDER: u32 = 0xe5c07b;
pub const ICON_CODE: u32 = 0x61afef;
pub const ICON_DATA: u32 = 0x98c379;
pub const ICON_MEDIA: u32 = 0xc678dd;

/// Success marks (a tool that finished well).
pub const OK: u32 = 0x98c379;
/// A chat that is thinking.
pub const WORKING: u32 = 0xe5a14e;
/// A chat that waits for the user to allow something.
pub const NEEDS_USER: u32 = 0x61afef;
