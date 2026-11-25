//! Every key combination of Normal and Visual mode. What a key can run is a fixed set of
//! opcodes (`motion.word`, `operator.delete`, `editor.go_to_definition`...), implemented in
//! Rust; which key runs which opcode, and how keys group (`d` then a motion, `g` then `d`), is
//! data: [`default_keymap.json`](../default_keymap.json), with the user's `keymap` setting
//! merged over it key by key.
//!
//! The keys are trees: a group waits for one of its children, a leaf runs its [`Action`].
//! [`Vim`](crate::Vim) dispatches by walking the tree of its mode, and the key hints panel
//! lists the children of the group being typed, so a command cannot exist without its
//! description.
//!
//! The file's shape: `lists` holds key lists several groups share (the motions, the text
//! objects); `normal` and `visual` are the root groups. A group is
//! `{ "code": .., "name": .., "include": [list names], "keys": { key: node } }` (a group with
//! no code is a plain prefix such as `g`), a leaf is an opcode or `{ "code": .., "name": .. }`.
//! A group's own keys win over the lists it includes, and later lists over earlier ones.
//!
//! Counts are not in the trees: digits are read before a command and after an operator.
//! Insert mode types text and is not a set of combinations, so it is not here either.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use serde_json::{Map, Value};

use crate::{Key, Mode, Operator};

/// What a key does: the meaning of a group that waits for more keys, or what a leaf runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A group: the operator waits for a motion or a text object to act on.
    Operator(Operator),
    /// A group: `i` (inner) or `a` (around) waits for the kind of text object.
    Object {
        inner: bool,
    },
    /// A group that only gathers more keys (`g`, `z`).
    Prefix,
    /// Moves the cursor, or says what a pending operator acts on. The char names the motion
    /// as Vim's own key for it, whatever key the keymap binds it to.
    Motion(char),
    /// The operator typed twice (`dd`, `cc`, `yy`): whole lines.
    Line,
    /// A text object of this kind, named by Vim's key for it (`w`, `"`, `(`...).
    TextObject(char),
    /// A Normal-mode command that edits or enters a mode, named by Vim's key for it.
    Command(char),
    Redo,
    /// `Ctrl-A` (+1) / `Ctrl-X` (-1), times the count.
    AddToNumber(i64),
    /// Enters this Visual mode, switches to it, or leaves it when already there.
    Visual(Mode),
    /// Visual `o`: the cursor goes to the other end of the selection.
    SwapEnds,
    /// Visual `d`, `c`, `y`: acts on the selection at once.
    VisualOperator(Operator),
    /// Something only the editor around the text can do: handed back by `Vim::handle`.
    Editor(Request),
}

impl Action {
    /// True when the action changes the text or the mode; a read-only buffer refuses them.
    pub fn edits(self) -> bool {
        !matches!(self, Action::Motion(_) | Action::Prefix | Action::Editor(_))
    }

    /// True for the actions that wait for more keys.
    fn is_group(self) -> bool {
        matches!(
            self,
            Action::Operator(_) | Action::Object { .. } | Action::Prefix
        )
    }
}

/// Where a scroll command puts the cursor's line on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Top,
    Center,
    Bottom,
}

/// A command the editor runs, not the text's state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    GoToDefinition,
    /// `za`, `zo`, `zc`, `zM`, `zR`: Vim's key after `z`.
    Fold(char),
    /// `zt`, `zz`, `zb`; `first_non_blank` for `z⏎`, `z.`, `z-`, which also move the cursor
    /// to the line's first non-blank character.
    Scroll {
        place: Place,
        first_non_blank: bool,
    },
    /// `Ctrl-D` (down) / `Ctrl-U`.
    HalfPage {
        down: bool,
    },
    /// `/`.
    OpenFind,
    /// `n` (forward) / `N`.
    FindNext {
        forward: bool,
    },
}

/// An opcode: its name in the keymap, what it runs and how the hints panel describes it
/// unless the keymap names it.
struct Opcode {
    code: &'static str,
    action: Action,
    description: &'static str,
}

const fn op(code: &'static str, action: Action, description: &'static str) -> Opcode {
    Opcode {
        code,
        action,
        description,
    }
}

const fn scroll(
    code: &'static str,
    place: Place,
    first: bool,
    description: &'static str,
) -> Opcode {
    op(
        code,
        Action::Editor(Request::Scroll {
            place,
            first_non_blank: first,
        }),
        description,
    )
}

/// Every opcode there is.
const OPCODES: &[Opcode] = &[
    op("prefix", Action::Prefix, ""),
    op(
        "operator.change",
        Action::Operator(Operator::Change),
        "Change",
    ),
    op(
        "operator.delete",
        Action::Operator(Operator::Delete),
        "Delete",
    ),
    op("operator.yank", Action::Operator(Operator::Yank), "Yank"),
    op("line", Action::Line, "Whole line"),
    op("object.inside", Action::Object { inner: true }, "Inside"),
    op("object.around", Action::Object { inner: false }, "Around"),
    op("motion.line_start", Action::Motion('0'), "Start of line"),
    op(
        "motion.first_non_blank",
        Action::Motion('^'),
        "Start of line (non ws)",
    ),
    op("motion.line_end", Action::Motion('$'), "End of line"),
    op("motion.left", Action::Motion('h'), "Left"),
    op("motion.down", Action::Motion('j'), "Down"),
    op("motion.up", Action::Motion('k'), "Up"),
    op("motion.right", Action::Motion('l'), "Right"),
    op("motion.word", Action::Motion('w'), "Next word"),
    op("motion.big_word", Action::Motion('W'), "Next WORD"),
    op("motion.word_back", Action::Motion('b'), "Prev word"),
    op("motion.word_end", Action::Motion('e'), "Next end of word"),
    op(
        "motion.big_word_end",
        Action::Motion('E'),
        "Next end of WORD",
    ),
    op("motion.first_line", Action::Motion('g'), "First line"),
    op("motion.last_line", Action::Motion('G'), "Last line"),
    op("object.word", Action::TextObject('w'), "word"),
    op("object.paren", Action::TextObject('('), "() block"),
    op("object.brace", Action::TextObject('{'), "{} block"),
    op("object.bracket", Action::TextObject('['), "[] block"),
    op("object.angle", Action::TextObject('<'), "<> block"),
    op(
        "object.double_quote",
        Action::TextObject('"'),
        "\"\" string",
    ),
    op("object.single_quote", Action::TextObject('\''), "'' string"),
    op("object.backtick", Action::TextObject('`'), "`` string"),
    op("command.insert", Action::Command('i'), "Insert"),
    op(
        "command.insert_line_start",
        Action::Command('I'),
        "Insert at line start",
    ),
    op("command.append", Action::Command('a'), "Append"),
    op(
        "command.append_line_end",
        Action::Command('A'),
        "Append at end of line",
    ),
    op(
        "command.open_below",
        Action::Command('o'),
        "Open line below",
    ),
    op(
        "command.open_above",
        Action::Command('O'),
        "Open line above",
    ),
    op("command.delete_char", Action::Command('x'), "Delete char"),
    op(
        "command.delete_line_end",
        Action::Command('D'),
        "Delete to end of line",
    ),
    op("command.paste_after", Action::Command('p'), "Paste after"),
    op("command.paste_before", Action::Command('P'), "Paste before"),
    op("command.undo", Action::Command('u'), "Undo"),
    op("command.repeat", Action::Command('.'), "Repeat last change"),
    op("redo", Action::Redo, "Redo"),
    op(
        "number.increment",
        Action::AddToNumber(1),
        "Increment number",
    ),
    op(
        "number.decrement",
        Action::AddToNumber(-1),
        "Decrement number",
    ),
    op("visual.char", Action::Visual(Mode::Visual), "Visual"),
    op(
        "visual.line",
        Action::Visual(Mode::VisualLine),
        "Visual line",
    ),
    op("visual.other_end", Action::SwapEnds, "Other end"),
    op(
        "visual.change",
        Action::VisualOperator(Operator::Change),
        "Change selection",
    ),
    op(
        "visual.delete",
        Action::VisualOperator(Operator::Delete),
        "Delete selection",
    ),
    op(
        "visual.yank",
        Action::VisualOperator(Operator::Yank),
        "Yank selection",
    ),
    op(
        "editor.go_to_definition",
        Action::Editor(Request::GoToDefinition),
        "Go to definition",
    ),
    op(
        "editor.find",
        Action::Editor(Request::OpenFind),
        "Search forward",
    ),
    op(
        "editor.find_next",
        Action::Editor(Request::FindNext { forward: true }),
        "Next search result",
    ),
    op(
        "editor.find_previous",
        Action::Editor(Request::FindNext { forward: false }),
        "Prev search result",
    ),
    op(
        "editor.half_page_down",
        Action::Editor(Request::HalfPage { down: true }),
        "Half page down",
    ),
    op(
        "editor.half_page_up",
        Action::Editor(Request::HalfPage { down: false }),
        "Half page up",
    ),
    op(
        "editor.fold_toggle",
        Action::Editor(Request::Fold('a')),
        "Toggle fold",
    ),
    op(
        "editor.fold_open",
        Action::Editor(Request::Fold('o')),
        "Open fold",
    ),
    op(
        "editor.fold_close",
        Action::Editor(Request::Fold('c')),
        "Close fold",
    ),
    op(
        "editor.fold_open_all",
        Action::Editor(Request::Fold('R')),
        "Open all folds",
    ),
    op(
        "editor.fold_close_all",
        Action::Editor(Request::Fold('M')),
        "Close all folds",
    ),
    scroll("editor.scroll_top", Place::Top, false, "Line at top"),
    scroll(
        "editor.scroll_center",
        Place::Center,
        false,
        "Line at center",
    ),
    scroll(
        "editor.scroll_bottom",
        Place::Bottom,
        false,
        "Line at bottom",
    ),
    scroll(
        "editor.scroll_top_first_non_blank",
        Place::Top,
        true,
        "Line at top, first non-blank",
    ),
    scroll(
        "editor.scroll_center_first_non_blank",
        Place::Center,
        true,
        "Line at center, first non-blank",
    ),
    scroll(
        "editor.scroll_bottom_first_non_blank",
        Place::Bottom,
        true,
        "Line at bottom, first non-blank",
    ),
];

fn opcode(code: &str) -> Option<&'static Opcode> {
    OPCODES.iter().find(|op| op.code == code)
}

/// The names of every opcode, for documentation and error messages.
pub fn opcodes() -> impl Iterator<Item = &'static str> {
    OPCODES.iter().map(|op| op.code)
}

/// One key of a tree: what it does and how the hints panel names it. A binding with children
/// is a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub key: Key,
    pub description: String,
    pub action: Action,
    pub children: Vec<Binding>,
}

impl Binding {
    pub fn is_group(&self) -> bool {
        !self.children.is_empty()
    }

    /// The child bound to `key`, if any.
    pub fn child(&self, key: Key) -> Option<&Binding> {
        find(&self.children, key)
    }
}

/// The binding of `key` in `list`.
pub(crate) fn find(list: &[Binding], key: Key) -> Option<&Binding> {
    list.iter().find(|b| b.key == key)
}

/// The key trees of Normal and Visual mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    normal: Vec<Binding>,
    visual: Vec<Binding>,
}

const DEFAULT: &str = include_str!("default_keymap.json");

static BUILTIN: LazyLock<Arc<Keymap>> = LazyLock::new(|| {
    let (keymap, errors) = Keymap::with_overrides(&Value::Null);
    debug_assert!(errors.is_empty(), "{errors:?}");
    Arc::new(keymap)
});

impl Keymap {
    /// Zide's own keys, as `default_keymap.json` has them.
    pub fn builtin() -> Arc<Keymap> {
        BUILTIN.clone()
    }

    /// The default keys with `user` (the `keymap` setting) merged over them. An entry that
    /// makes no sense (an unknown opcode, an opcode where it cannot work, a malformed key) is
    /// left out, the default kept in its place, and explained in the returned messages.
    /// `null` unbinds a key.
    pub fn with_overrides(user: &Value) -> (Keymap, Vec<String>) {
        let defaults: Value = serde_json::from_str(DEFAULT).expect("default_keymap.json is JSON");
        let mut errors = Vec::new();
        let user = match user {
            Value::Null => None,
            Value::Object(map) => {
                for name in map.keys() {
                    if !matches!(name.as_str(), "lists" | "normal" | "visual") {
                        errors.push(format!(
                            "keymap: unknown `{name}` (expected lists, normal or visual)"
                        ));
                    }
                }
                Some(map)
            }
            _ => {
                errors.push("keymap: expected an object".into());
                None
            }
        };
        let mut builder = Builder {
            user_lists: user.and_then(|u| u.get("lists")).and_then(Value::as_object),
            default_lists: defaults["lists"].as_object().expect("default lists"),
            errors,
        };
        let mut root = |mode: Mode, name: &str| {
            let ctx = Context {
                mode,
                parent: Parent::Root,
                in_operator: false,
                depth: 0,
            };
            let user = user.and_then(|u| u.get(name));
            if user.is_some_and(|u| !u.is_object()) {
                builder.errors.push(format!(
                    "keymap {name}: expected an object with `include` and `keys`"
                ));
            }
            builder.entries(ctx, name, user, Some(&defaults[name]))
        };
        let normal = root(Mode::Normal, "normal");
        let visual = root(Mode::Visual, "visual");
        let mut errors = builder.errors;
        let mut seen = std::collections::HashSet::new();
        errors.retain(|e| seen.insert(e.clone())); // a shared list repeats its problems
        (Keymap { normal, visual }, errors)
    }

    /// The tree of a mode; Insert mode has none.
    pub fn root(&self, mode: Mode) -> &[Binding] {
        match mode {
            Mode::Normal => &self.normal,
            Mode::Visual | Mode::VisualLine => &self.visual,
            Mode::Insert => &[],
        }
    }
}

/// How the hints panel and the keymap write a key.
pub fn label(key: Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Ctrl(c) => format!("<C-{c}>"),
        Key::Esc => "<Esc>".into(),
        Key::Enter => "<CR>".into(),
        Key::Backspace => "<BS>".into(),
    }
}

/// A key as the keymap writes it: one character, or `<C-x>`, `<CR>`, `<BS>`, `<Space>`.
fn parse_key(text: &str) -> Result<Key, String> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(Key::Char(c));
    }
    let named = text
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .ok_or_else(|| format!("`{text}` is not one key; put the next key in `keys`"))?;
    if let Some(c) = named
        .strip_prefix("C-")
        .or_else(|| named.strip_prefix("c-"))
    {
        let mut chars = c.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Ok(Key::Ctrl(c.to_ascii_lowercase()));
        }
    }
    match named.to_ascii_lowercase().as_str() {
        "cr" | "enter" => Ok(Key::Enter),
        "bs" | "backspace" => Ok(Key::Backspace),
        "space" => Ok(Key::Char(' ')),
        "esc" => Err("<Esc> always cancels the command and cannot be bound".into()),
        _ => Err(format!("unknown key `{text}`")),
    }
}

/// What a key's group is, which decides the opcodes that make sense under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent {
    Root,
    Operator,
    Object,
    Prefix,
}

#[derive(Debug, Clone, Copy)]
struct Context {
    mode: Mode,
    parent: Parent,
    /// An operator waits somewhere above.
    in_operator: bool,
    depth: usize,
}

impl Context {
    fn under(self, action: Action) -> Context {
        Context {
            mode: self.mode,
            parent: match action {
                Action::Operator(_) => Parent::Operator,
                Action::Object { .. } => Parent::Object,
                _ => Parent::Prefix,
            },
            in_operator: self.in_operator || matches!(action, Action::Operator(_)),
            depth: self.depth + 1,
        }
    }

    /// Digits are a count here, not keys.
    fn counts(self) -> bool {
        matches!(self.parent, Parent::Root | Parent::Operator)
    }
}

/// Why `action` cannot go where `ctx` says, if it cannot.
fn misplaced(action: Action, ctx: Context) -> Option<&'static str> {
    let normal = ctx.mode == Mode::Normal;
    match action {
        Action::TextObject(_) if ctx.parent != Parent::Object => {
            Some("a text object goes only after object.inside / object.around")
        }
        Action::TextObject(_) => None,
        _ if ctx.parent == Parent::Object => {
            Some("only text objects go after object.inside / object.around")
        }
        Action::Line if ctx.parent != Parent::Operator => {
            Some("`line` goes only right after an operator")
        }
        Action::Line | Action::Motion(_) | Action::Prefix => None,
        Action::Object { .. } if !ctx.in_operator && normal => {
            Some("text objects need an operator before them, or Visual mode")
        }
        Action::Object { .. } => None,
        _ if ctx.in_operator => Some("only motions and text objects go after an operator"),
        Action::Operator(_) | Action::Command(_) | Action::AddToNumber(_) if !normal => {
            Some("it works only in normal mode")
        }
        Action::SwapEnds | Action::VisualOperator(_) if normal => {
            Some("it works only in visual mode")
        }
        _ => None,
    }
}

/// A node as written: an opcode, or an object with its fields.
#[derive(Default, Clone, Copy)]
struct Spec<'a> {
    code: Option<&'a str>,
    name: Option<&'a str>,
    include: Option<&'a Vec<Value>>,
    keys: Option<&'a Map<String, Value>>,
}

impl<'a> Spec<'a> {
    fn of(value: &'a Value) -> Result<Spec<'a>, String> {
        match value {
            Value::String(code) => Ok(Spec {
                code: Some(code),
                ..Spec::default()
            }),
            Value::Object(map) => {
                for field in map.keys() {
                    if !matches!(field.as_str(), "code" | "name" | "include" | "keys") {
                        return Err(format!(
                            "unknown field `{field}` (expected code, name, include or keys)"
                        ));
                    }
                }
                let text = |field: &str| match map.get(field) {
                    None => Ok(None),
                    Some(Value::String(s)) => Ok(Some(s.as_str())),
                    Some(_) => Err(format!("`{field}` must be a string")),
                };
                Ok(Spec {
                    code: text("code")?,
                    name: text("name")?,
                    include: match map.get("include") {
                        None => None,
                        Some(Value::Array(lists)) => Some(lists),
                        Some(_) => return Err("`include` must be a list of list names".into()),
                    },
                    keys: match map.get("keys") {
                        None => None,
                        Some(Value::Object(keys)) => Some(keys),
                        Some(_) => return Err("`keys` must be an object".into()),
                    },
                })
            }
            _ => Err("expected an opcode or an object".into()),
        }
    }

    fn has_keys(&self) -> bool {
        self.include.is_some_and(|l| !l.is_empty()) || self.keys.is_some_and(|k| !k.is_empty())
    }
}

/// Builds the trees from the defaults and the user's keymap side by side, so a user entry that
/// fails falls back to the default one.
struct Builder<'a> {
    user_lists: Option<&'a Map<String, Value>>,
    default_lists: &'a Map<String, Value>,
    errors: Vec<String>,
}

/// The user's and the default value of one key in a group.
#[derive(Default, Clone, Copy)]
struct Pair<'a> {
    user: Option<&'a Value>,
    default: Option<&'a Value>,
}

impl<'a> Builder<'a> {
    /// The children of a group, from the lists it includes and its own keys.
    fn entries(
        &mut self,
        ctx: Context,
        path: &str,
        user: Option<&'a Value>,
        default: Option<&'a Value>,
    ) -> Vec<Binding> {
        if ctx.depth > 8 {
            self.errors.push(format!(
                "keymap {path}: too deep; does a list include itself?"
            ));
            return Vec::new();
        }
        let user = user.and_then(|v| Spec::of(v).ok()).unwrap_or_default();
        let default = default.and_then(|v| Spec::of(v).ok()).unwrap_or_default();
        let mut order: Vec<&'a str> = Vec::new();
        let mut pairs: HashMap<&'a str, Pair<'a>> = HashMap::new();
        let mut add = |key: &'a str, value: &'a Value, from_user: bool| {
            let pair = pairs.entry(key).or_insert_with(|| {
                order.push(key);
                Pair::default()
            });
            if from_user {
                pair.user = Some(value);
            } else {
                pair.default = Some(value);
            }
        };
        let includes = user.include.or(default.include).into_iter().flatten();
        for name in includes {
            let Some(name) = name.as_str() else {
                self.errors
                    .push(format!("keymap {path}: `include` holds list names"));
                continue;
            };
            let from_user = self.user_lists.and_then(|l| l.get(name));
            let from_default = self.default_lists.get(name);
            if from_user.is_none() && from_default.is_none() {
                self.errors
                    .push(format!("keymap {path}: no list named `{name}`"));
                continue;
            }
            for (key, value) in from_default
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                add(key, value, false);
            }
            for (key, value) in from_user.and_then(Value::as_object).into_iter().flatten() {
                add(key, value, true);
            }
        }
        for (key, value) in default.keys.into_iter().flatten() {
            add(key, value, false);
        }
        for (key, value) in user.keys.into_iter().flatten() {
            add(key, value, true);
        }

        let mut children = Vec::new();
        for text in order {
            let pair = pairs[text];
            if pair.user == Some(&Value::Null) {
                continue; // unbound by the user
            }
            let here = format!("{path} {text}");
            let key = match parse_key(text) {
                Ok(Key::Char(c @ '1'..='9')) if ctx.counts() => {
                    Err(format!("`{c}` is part of a count here"))
                }
                other => other,
            };
            let key = match key {
                Ok(key) => key,
                Err(e) => {
                    self.errors.push(format!("keymap {here}: {e}"));
                    continue;
                }
            };
            let built = match pair.user {
                Some(user) => match self.node(ctx, &here, key, user, pair.default) {
                    Ok(binding) => Some(binding),
                    Err(e) => {
                        self.errors.push(format!("keymap {here}: {e}"));
                        // the user's entry failed: the default stays
                        pair.default
                            .and_then(|d| self.node(ctx, &here, key, d, None).ok())
                    }
                },
                None => match pair.default.map(|d| self.node(ctx, &here, key, d, None)) {
                    Some(Ok(binding)) => Some(binding),
                    Some(Err(e)) => {
                        self.errors.push(format!("keymap {here}: {e}"));
                        None
                    }
                    None => None,
                },
            };
            children.extend(built);
        }
        children
    }

    /// One key: `value`, its fields falling back to `default`'s when it is an object over a
    /// default object with the same opcode.
    fn node(
        &mut self,
        ctx: Context,
        path: &str,
        key: Key,
        value: &'a Value,
        default: Option<&'a Value>,
    ) -> Result<Binding, String> {
        let spec = Spec::of(value)?;
        // a plain opcode replaces the default whole; an object refines it
        let base = match (value, default) {
            (Value::Object(_), Some(default)) => Spec::of(default).unwrap_or_default(),
            _ => Spec::default(),
        };
        let code = spec
            .code
            .or(base.code)
            .or((spec.has_keys() || base.has_keys()).then_some("prefix"))
            .ok_or("no `code`: say which opcode the key runs")?;
        let op = opcode(code).ok_or_else(|| {
            format!("unknown opcode `{code}` (see the opcode list in the README)")
        })?;
        if let Some(why) = misplaced(op.action, ctx) {
            return Err(format!("`{code}` cannot go here: {why}"));
        }
        let same_code = base.code.unwrap_or("prefix") == code;
        let description = spec
            .name
            .or(base.name.filter(|_| same_code))
            .map(str::to_string)
            .or_else(|| (!op.description.is_empty()).then(|| op.description.to_string()))
            .unwrap_or_else(|| label(key));
        let children = if op.action.is_group() {
            let inherited = if same_code {
                default.filter(|_| value.is_object())
            } else {
                None
            };
            let children = self.entries(ctx.under(op.action), path, Some(value), inherited);
            if children.is_empty() {
                return Err(format!(
                    "`{code}` needs keys after it (`keys` or `include`)"
                ));
            }
            children
        } else {
            if spec.has_keys() {
                return Err(format!("`{code}` runs at once and takes no keys after it"));
            }
            Vec::new()
        };
        Ok(Binding {
            key,
            description,
            action: op.action,
            children,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn walk<'k>(list: &'k [Binding], path: String, out: &mut Vec<(String, &'k [Binding])>) {
        out.push((path.clone(), list));
        for b in list.iter().filter(|b| b.is_group()) {
            walk(&b.children, format!("{path}{}", label(b.key)), out);
        }
    }

    fn at<'k>(keymap: &'k Keymap, mode: Mode, keys: &str) -> Option<&'k Binding> {
        let mut chars = keys.chars();
        let mut binding = find(keymap.root(mode), Key::Char(chars.next()?))?;
        for c in chars {
            binding = binding.child(Key::Char(c))?;
        }
        Some(binding)
    }

    #[test]
    fn the_default_keymap_has_no_errors_and_the_expected_shape() {
        let (keymap, errors) = Keymap::with_overrides(&Value::Null);
        assert_eq!(errors, Vec::<String>::new());
        let delete = at(&keymap, Mode::Normal, "d").unwrap();
        assert_eq!(delete.description, "Delete");
        assert_eq!(
            delete.child(Key::Char('w')).unwrap().action,
            Action::Motion('w')
        );
        assert_eq!(delete.child(Key::Char('d')).unwrap().action, Action::Line);
        let paren = at(&keymap, Mode::Normal, "ci(").unwrap();
        assert_eq!(paren.action, Action::TextObject('('));
        assert_eq!(
            at(&keymap, Mode::Normal, "gd").unwrap().action,
            Action::Editor(Request::GoToDefinition)
        );
        assert!(at(&keymap, Mode::Normal, "dgd").is_none());
        assert_eq!(
            at(&keymap, Mode::Visual, "x").unwrap().action,
            Action::VisualOperator(Operator::Delete)
        );
        assert_eq!(
            find(keymap.root(Mode::Normal), Key::Ctrl('d'))
                .unwrap()
                .action,
            Action::Editor(Request::HalfPage { down: true })
        );
    }

    #[test]
    fn groups_and_only_groups_wait_for_more_keys() {
        let keymap = Keymap::builtin();
        let mut groups = Vec::new();
        walk(keymap.root(Mode::Normal), String::new(), &mut groups);
        walk(keymap.root(Mode::Visual), String::new(), &mut groups);
        for (path, list) in groups {
            for b in list {
                assert_eq!(b.action.is_group(), b.is_group(), "{path}{}", label(b.key));
            }
        }
    }

    #[test]
    fn user_keys_merge_over_the_defaults() {
        let (keymap, errors) = Keymap::with_overrides(&json!({
            "lists": { "motions": { "L": "motion.line_end" } },
            "normal": { "keys": {
                "g": { "keys": { "h": { "code": "editor.go_to_definition", "name": "Hop" } } },
                "x": null,
                "Q": { "name": "Mine", "keys": { "q": "editor.find" } }
            } }
        }));
        assert_eq!(errors, Vec::<String>::new());
        // a list entry reaches every group that includes it
        assert_eq!(
            at(&keymap, Mode::Normal, "L").unwrap().action,
            Action::Motion('$')
        );
        assert_eq!(
            at(&keymap, Mode::Normal, "dL").unwrap().action,
            Action::Motion('$')
        );
        assert_eq!(
            at(&keymap, Mode::Visual, "L").unwrap().action,
            Action::Motion('$')
        );
        // `g` keeps its keys and name and gains one
        let goto = at(&keymap, Mode::Normal, "g").unwrap();
        assert_eq!(goto.description, "Goto");
        assert!(goto.child(Key::Char('d')).is_some());
        assert_eq!(goto.child(Key::Char('h')).unwrap().description, "Hop");
        assert!(at(&keymap, Mode::Normal, "x").is_none(), "null unbinds");
        assert_eq!(at(&keymap, Mode::Normal, "Q").unwrap().description, "Mine");
        assert!(at(&keymap, Mode::Normal, "Qq").is_some());
    }

    #[test]
    fn opcodes_in_places_they_make_no_sense_are_refused_and_the_default_kept() {
        let (keymap, errors) = Keymap::with_overrides(&json!({
            "normal": { "keys": {
                "x": "visual.delete",
                "J": "object.paren",
                "K": "line",
                "d": { "keys": { "p": "command.paste_after", "1": "motion.word" } },
                "U": "no.such.code",
                "gx": "motion.word",
                "<Esc>": "redo",
                "Z": { "keys": {} }
            } },
            "visual": { "keys": { "a": "command.insert" } }
        }));
        let has = |text: &str| errors.iter().any(|e| e.contains(text));
        assert!(
            has("normal x: `visual.delete` cannot go here: it works only in visual mode"),
            "{errors:#?}"
        );
        assert!(
            has("normal J: `object.paren` cannot go here"),
            "{errors:#?}"
        );
        assert!(has("normal K: `line` cannot go here"), "{errors:#?}");
        assert!(
            has("normal d p: `command.paste_after` cannot go here: only motions"),
            "{errors:#?}"
        );
        assert!(
            has("normal d 1: `1` is part of a count here"),
            "{errors:#?}"
        );
        assert!(has("unknown opcode `no.such.code`"), "{errors:#?}");
        assert!(has("`gx` is not one key"), "{errors:#?}");
        assert!(has("<Esc> always cancels"), "{errors:#?}");
        assert!(has("normal Z: no `code`"), "{errors:#?}");
        assert!(
            has("visual a: `command.insert` cannot go here"),
            "{errors:#?}"
        );
        // the defaults of the refused keys stay
        assert_eq!(
            at(&keymap, Mode::Normal, "x").unwrap().action,
            Action::Command('x')
        );
        assert_eq!(
            at(&keymap, Mode::Visual, "a").unwrap().description,
            "Around"
        );
        assert!(at(&keymap, Mode::Normal, "dp").is_none());
    }

    #[test]
    fn the_readme_lists_every_opcode() {
        let readme = include_str!("../../../README.md");
        let list = readme
            .split_once("Opcodes: ")
            .expect("README has the opcode list")
            .1
            .split_once(".\n")
            .unwrap()
            .0;
        let listed: Vec<&str> = list
            .split_whitespace()
            .map(|code| code.trim_matches('`'))
            .collect();
        let known: Vec<&str> = opcodes().filter(|code| *code != "prefix").collect();
        assert_eq!(listed, known);
    }

    #[test]
    fn keys_are_written_as_in_vim() {
        assert_eq!(parse_key("w"), Ok(Key::Char('w')));
        assert_eq!(parse_key("<C-D>"), Ok(Key::Ctrl('d')));
        assert_eq!(parse_key("<CR>"), Ok(Key::Enter));
        assert_eq!(parse_key("<Space>"), Ok(Key::Char(' ')));
        assert!(parse_key("<Nope>").is_err());
        for key in [Key::Char('w'), Key::Ctrl('d'), Key::Enter, Key::Backspace] {
            assert_eq!(parse_key(&label(key)), Ok(key));
        }
    }
}
