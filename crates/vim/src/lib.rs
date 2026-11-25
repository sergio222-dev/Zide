//! Vim-style modal editing as a pure state machine over a [`Buffer`]: no UI types, so the UI
//! only translates its key events to [`Key`]. Offsets are `char` indices.

mod keymap;
mod motion;
mod normal;
mod objects;

pub use keymap::{Action, Binding, Keymap, Place, Request, label, opcodes};

use std::{ops::Range, sync::Arc};

use zide_text::{Buffer, Revision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

/// One step of a recorded command, which `.` plays again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Key(Key),
    /// A completion accepted while inserting. Kept as the edit, not as keys: typed again, its
    /// brackets and line breaks would bring auto-pairs and auto-indent along.
    Complete(Completed),
}

/// What accepting a completion did around the cursor, so it can be done again elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Completed {
    /// Chars replaced before the cursor (what was typed of the word) and after it.
    before: usize,
    after: usize,
    text: String,
    /// Where the cursor went, in chars into `text`.
    caret: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Debug)]
pub struct Vim {
    pub(crate) mode: Mode,
    pub(crate) cursor: usize,
    /// Column `j`/`k` try to keep.
    pub(crate) want_column: usize,
    /// Typed count; 0 when none.
    pub(crate) count: usize,
    pub(crate) operator: Option<(Operator, usize)>,
    /// `i` (true) or `a` (false) was typed and waits for the object key.
    pub(crate) object: Option<bool>,
    /// The keys of the [`keymap`] groups typed so far: where the next key is looked up.
    pub(crate) path: Vec<Key>,
    /// Everything typed of the unfinished command, counts included, for [`Vim::pending`].
    pub(crate) typed: String,
    /// The text cannot change: commands that edit or enter a mode are refused.
    read_only: bool,
    /// Which keys run what in Normal and Visual mode.
    keymap: Arc<Keymap>,
    /// Where the Visual selection started.
    pub(crate) anchor: usize,
    /// Steps of the command being typed; becomes `last_change` if it edited the text.
    current: Vec<Step>,
    start_revision: Revision,
    last_change: Vec<Step>,
    pub(crate) replaying: bool,
    pub(crate) register: Register,
    /// Text yanked or deleted and not yet taken for the system clipboard.
    yanked: Option<String>,
    /// The system clipboard as Zide last saw it, to tell text copied elsewhere from its own.
    clipboard: String,
    /// One indent level: a tab or some spaces. What Tab types and a new block adds.
    indent: String,
    /// Where the cursor was left after indenting a new line by itself: leaving Insert mode
    /// there, nothing typed, takes the indentation back so no blank line keeps whitespace.
    auto_indented: Option<usize>,
}

impl Default for Vim {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            cursor: 0,
            want_column: 0,
            count: 0,
            operator: None,
            object: None,
            path: Vec::new(),
            typed: String::new(),
            read_only: false,
            keymap: Keymap::builtin(),
            anchor: 0,
            current: Vec::new(),
            start_revision: Revision::default(),
            last_change: Vec::new(),
            replaying: false,
            register: Register::default(),
            yanked: None,
            clipboard: String::new(),
            indent: DEFAULT_INDENT.to_string(),
            auto_indented: None,
        }
    }
}

const DEFAULT_INDENT: &str = "    ";

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Sets one indent level: `"\t"` or some spaces. Anything else (an empty string, say) is
    /// ignored, since a level must be whitespace to be taken back by Backspace or `}`.
    pub fn set_indent(&mut self, unit: &str) {
        if unit == "\t" || (!unit.is_empty() && unit.chars().all(|c| c == ' ')) {
            self.indent = unit.to_string();
        }
    }

    pub fn indent(&self) -> &str {
        &self.indent
    }

    /// Marks the text as one to read, not edit (an error message shown in place of a file):
    /// only motions and the editor's own commands (scrolling, folds, search) work.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Uses `keymap` from the next key on; a command half typed is dropped, since its keys may
    /// mean nothing in the new one.
    pub fn set_keymap(&mut self, keymap: Arc<Keymap>) {
        self.keymap = keymap;
        self.reset_pending();
    }

    /// Places the cursor (clamped to the text), e.g. after the file is opened.
    pub fn set_cursor(&mut self, buffer: &Buffer, offset: usize) {
        self.cursor = offset;
        self.clamp(buffer);
        self.want_column = buffer.position_of(self.cursor).column;
    }

    /// Puts the cursor where the mouse clicked, `offset` being a char offset on a line (its end
    /// included). A command in progress and Visual mode end; Insert mode stays and its cursor
    /// may sit at the end of the line, as when typing.
    pub fn click(&mut self, buffer: &mut Buffer, offset: usize) {
        if self.mode == Mode::Insert {
            buffer.commit(); // the typing before the click is one change, what follows another
            self.cursor = offset.min(buffer.len());
            self.want_column = buffer.position_of(self.cursor).column;
            return;
        }
        self.mode = Mode::Normal;
        self.reset_pending();
        self.set_cursor(buffer, offset);
    }

    /// Moves the cursor to `line` (clamped), on the column it wants: what scrolling by pages does.
    pub fn move_to_line(&mut self, buffer: &Buffer, line: usize) {
        let line = line.min(motion::last_line(buffer));
        let last_column = buffer.line_len(line).saturating_sub(1);
        self.cursor = buffer.line_start(line) + self.want_column.min(last_column);
    }

    /// What the user has typed of an unfinished command (`2d`, `g`, `ci`), for a status line.
    pub fn pending(&self) -> String {
        self.typed.clone()
    }

    /// The group being typed (`d`, `ci`, `z`) and the keys that can come next, for the key
    /// hints panel; `None` when no group waits (nothing typed, or only a count). Keys a
    /// read-only text refuses are left out.
    pub fn hints(&self) -> Option<(&Binding, Vec<&Binding>)> {
        let group = self.group_in(&self.keymap)?;
        let next = group
            .children
            .iter()
            .filter(|binding| self.allows(binding))
            .collect();
        Some((group, next))
    }

    /// The keys that lead to the group being typed, `None` when no group waits: what tells
    /// one wait for hints from another.
    pub fn waiting_group(&self) -> Option<&[Key]> {
        self.group_in(&self.keymap).map(|_| self.path.as_slice())
    }

    /// The group of `keymap` the typed keys lead to; `None` at the root of the mode's tree.
    pub(crate) fn group_in<'k>(&self, keymap: &'k Keymap) -> Option<&'k Binding> {
        let (first, rest) = self.path.split_first()?;
        let mut group = keymap::find(keymap.root(self.mode), *first)?;
        for key in rest {
            group = group.child(*key)?;
        }
        Some(group)
    }

    /// True when `binding` may run here: a read-only text refuses what would edit it.
    pub(crate) fn allows(&self, binding: &Binding) -> bool {
        !self.read_only || !binding.action.edits()
    }

    /// The selected range while in Visual mode (whole lines in line-wise Visual).
    pub fn selection(&self, buffer: &Buffer) -> Option<(Range<usize>, bool)> {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
            .then(|| (self.selection_range(buffer), self.mode == Mode::VisualLine))
    }

    /// Selects `range` in Visual mode, the cursor on its last character: what Extend Selection
    /// does. An empty range selects nothing.
    pub fn select(&mut self, buffer: &Buffer, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        self.mode = Mode::Visual;
        self.anchor = range.start;
        self.set_cursor(buffer, range.end - 1);
    }

    /// Text yanked or deleted since the last call, for the system clipboard.
    pub fn take_yank(&mut self) -> Option<String> {
        let text = self.yanked.take()?;
        self.clipboard.clone_from(&text);
        Some(text)
    }

    /// Offers the system clipboard before a paste. Text copied elsewhere since Zide last saw
    /// the clipboard becomes the register, linewise when it ends with a line break; otherwise
    /// the register keeps what Zide put there, with its own linewise flag.
    pub fn offer_clipboard(&mut self, text: &str) {
        if text.is_empty() || text == self.clipboard {
            return;
        }
        self.clipboard = text.to_string();
        let text = text.replace("\r\n", "\n");
        self.register = Register {
            linewise: text.ends_with('\n'),
            text,
        };
    }

    /// Puts yanked or deleted text in the register and keeps it for [`Vim::take_yank`]. Deletes
    /// reach the system clipboard too: were they kept from it, copying elsewhere the same text
    /// Zide last saw there would not change it, and a paste would bring back the delete
    /// instead of what was just copied (#163).
    pub(crate) fn set_register(&mut self, text: String, linewise: bool) {
        self.yanked = Some(text.clone());
        self.register = Register { text, linewise };
    }

    fn idle(&self) -> bool {
        self.count == 0 && self.path.is_empty()
    }

    /// Runs `key`. Commands of the editor around the text (`gd`, `zz`, `/`...) are handed back
    /// for it to run.
    pub fn handle(&mut self, buffer: &mut Buffer, key: Key) -> Option<Request> {
        if self.mode == Mode::Normal && self.idle() {
            self.current.clear();
            self.start_revision = buffer.revision();
        }
        self.current.push(Step::Key(key));
        let request = match self.mode {
            Mode::Insert => {
                self.insert(buffer, key);
                None
            }
            _ => self.normal(buffer, key),
        };
        if self.mode == Mode::Normal && self.idle() && !self.replaying {
            // undo, redo and `.` itself are not changes to repeat
            let history = matches!(
                self.current.first(),
                Some(Step::Key(Key::Char('u' | '.') | Key::Ctrl('r')))
            );
            if buffer.revision() != self.start_revision && !history {
                self.last_change = std::mem::take(&mut self.current);
            } else {
                self.current.clear();
            }
        }
        request
    }

    fn insert(&mut self, buffer: &mut Buffer, key: Key) {
        let auto_indented = self.auto_indented.take();
        match key {
            Key::Esc => {
                if auto_indented == Some(self.cursor) {
                    self.drop_blank_indent(buffer);
                }
                self.mode = Mode::Normal;
                buffer.commit();
                let column = buffer.position_of(self.cursor).column;
                self.cursor -= usize::from(column > 0);
                self.want_column = buffer.position_of(self.cursor).column;
            }
            Key::Char('\t') => self.type_indent(buffer),
            Key::Char(c) => self.type_char(buffer, c),
            Key::Enter => self.new_line(buffer),
            Key::Backspace if self.cursor > 0 && self.on_blank_line(buffer) => {
                self.delete_blank_line(buffer);
            }
            Key::Backspace if self.cursor > 0 => {
                // between an empty pair both halves go
                let pair = buffer
                    .char_at(self.cursor - 1)
                    .zip(buffer.char_at(self.cursor))
                    .is_some_and(|(open, close)| closer_of(open) == Some(close));
                // in the indentation of spaces, a whole level goes
                let before = self.before_cursor(buffer);
                let level = if self.indent != "\t" && is_blank(&before) && !before.contains('\t') {
                    dedent_len(&before, &self.indent)
                } else {
                    1
                };
                let end = self.cursor + usize::from(pair);
                buffer.delete(self.cursor - level..end);
                self.cursor -= level;
            }
            // pastes the register, which the UI fills from the system clipboard
            Key::Ctrl('v') => {
                let text = self.register.text.clone();
                self.type_text(buffer, &text);
            }
            Key::Backspace | Key::Ctrl(_) => {}
        }
    }

    /// Accepts a completion while inserting: `replace` (char offsets; it is stretched to hold
    /// the cursor) becomes `text`, and the cursor goes `caret` chars into it. `elsewhere` are
    /// edits away from it, such as the import the item needs, as char ranges of the text before
    /// any edit; one overlapping `replace` is left out. `.` repeats the replacement, not the
    /// edits elsewhere. Outside Insert mode nothing happens.
    pub fn complete(
        &mut self,
        buffer: &mut Buffer,
        replace: Range<usize>,
        text: &str,
        caret: usize,
        elsewhere: &[(Range<usize>, String)],
    ) {
        if self.mode != Mode::Insert {
            return;
        }
        self.auto_indented = None;
        let end = replace.end.max(self.cursor).min(buffer.len());
        let replace = replace.start.min(self.cursor)..end;
        let step = Completed {
            before: self.cursor - replace.start,
            after: replace.end - self.cursor,
            text: text.to_string(),
            caret: caret.min(text.chars().count()),
        };
        let mut apart: Vec<&(Range<usize>, String)> = elsewhere
            .iter()
            .filter(|(range, _)| range.end <= replace.start || range.start >= replace.end)
            .collect();
        // from the end, so the offsets of those still to come hold
        apart.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        for (range, new) in apart {
            let end = range.end.min(buffer.len());
            let start = range.start.min(end);
            buffer.edit(start..end, new);
            if end <= replace.start {
                // an edit above (an import) moves the cursor along with the text
                self.cursor = self.cursor + new.chars().count() - (end - start);
            }
        }
        self.apply_completed(buffer, &step);
        self.current.push(Step::Complete(step));
    }

    /// Replaces the chars around the cursor that `step` says with its text.
    pub(crate) fn apply_completed(&mut self, buffer: &mut Buffer, step: &Completed) {
        let start = self.cursor.saturating_sub(step.before);
        let end = (self.cursor + step.after).min(buffer.len());
        buffer.edit(start..end, &step.text);
        self.cursor = start + step.caret;
        self.want_column = buffer.position_of(self.cursor).column;
    }

    /// Typing a character in Insert: a closer steps over the one already there, an opener
    /// brings its closer along when what follows is not part of a word.
    fn type_char(&mut self, buffer: &mut Buffer, c: char) {
        let next = buffer.char_at(self.cursor);
        let previous = self.cursor.checked_sub(1).and_then(|at| buffer.char_at(at));
        if previous == Some('\\') && c == '"' {
            return self.type_text(buffer, "\""); // an escaped quote is just text
        }
        if matches!(c, ')' | ']' | '}' | '"') && next == Some(c) {
            self.cursor += 1; // the closer that was brought along
            return;
        }
        if matches!(c, ')' | ']' | '}') {
            // a closer first on its line ends the block: back to the opener's level
            let before = self.before_cursor(buffer);
            if is_blank(&before) {
                let level = dedent_len(&before, &self.indent);
                buffer.delete(self.cursor - level..self.cursor);
                self.cursor -= level;
            }
        }
        let Some(closer) = closer_of(c) else {
            return self.type_text(buffer, c.encode_utf8(&mut [0; 4]));
        };
        let open_ended = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}'));
        let pairs = if c == '"' {
            // `"` closes a string when inside one, and is part of a word in `don"t`
            open_ended && !previous.is_some_and(char::is_alphanumeric) && !self.in_string(buffer)
        } else {
            open_ended && !self.in_string(buffer)
        };
        if pairs {
            self.type_text(buffer, &format!("{c}{closer}"));
            self.cursor -= 1;
        } else {
            self.type_text(buffer, c.encode_utf8(&mut [0; 4]));
        }
    }

    /// True when the cursor sits inside a `"` string of its line: an odd number of
    /// unescaped quotes before it.
    fn in_string(&self, buffer: &Buffer) -> bool {
        let line = buffer.position_of(self.cursor).line;
        let before = buffer.slice(buffer.line_start(line)..self.cursor);
        let mut inside = false;
        let mut escaped = false;
        for c in before.chars() {
            match c {
                '\\' if !escaped => {
                    escaped = true;
                    continue;
                }
                '"' if !escaped => inside = !inside,
                _ => {}
            }
            escaped = false;
        }
        inside
    }

    fn type_text(&mut self, buffer: &mut Buffer, text: &str) {
        buffer.insert(self.cursor, text);
        self.cursor += text.chars().count();
    }

    /// The text of the cursor's line before the cursor.
    fn before_cursor(&self, buffer: &Buffer) -> String {
        let line = buffer.position_of(self.cursor).line;
        buffer.slice(buffer.line_start(line)..self.cursor)
    }

    /// Tab types one indent level; with spaces, up to the next multiple of the level so a
    /// column typed off the grid lines up again.
    fn type_indent(&mut self, buffer: &mut Buffer) {
        if self.indent == "\t" {
            return self.type_text(buffer, "\t");
        }
        let width = self.indent.len();
        let column = self.before_cursor(buffer).chars().count();
        self.type_text(buffer, &" ".repeat(width - column % width));
    }

    /// Enter keeps the line's indentation, one level deeper after a line that opens a block.
    /// Between a bracket and its closer, the closer goes to a line of its own at the opener's
    /// level, the cursor on the indented line between them.
    fn new_line(&mut self, buffer: &mut Buffer) {
        let before = self.before_cursor(buffer);
        let indent = leading_indent(&before);
        if is_blank(&before) {
            // the line left behind holds only indentation: it is left empty instead
            let start = self.cursor - before.chars().count();
            buffer.delete(start..self.cursor);
            self.cursor = start;
        }
        // the spaces the cursor was on would only push the moved text off the indentation
        let line_end = motion::line_end(buffer, buffer.position_of(self.cursor).line);
        let spaces = buffer
            .slice(self.cursor..line_end)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        buffer.delete(self.cursor..self.cursor + spaces);

        let opener = before.trim_end().chars().last().filter(|&c| opens_block(c));
        let inner = match opener {
            Some(_) => format!("{indent}{}", self.indent),
            None => indent.clone(),
        };
        let closed = opener
            .and_then(closer_of)
            .is_some_and(|closer| buffer.char_at(self.cursor) == Some(closer));
        if closed {
            self.type_text(buffer, &format!("\n{inner}\n{indent}"));
            self.cursor -= 1 + indent.chars().count();
        } else {
            self.type_text(buffer, &format!("\n{inner}"));
        }
        self.auto_indented = Some(self.cursor);
    }

    /// `o` and `O`: a new line below or above the cursor's, indented as Enter would.
    pub(crate) fn open_line(&mut self, buffer: &mut Buffer, below: bool) {
        let line = buffer.position_of(self.cursor).line;
        let text = buffer.line(line);
        let indent = leading_indent(&text);
        if below {
            let opens = text.trim_end().chars().last().is_some_and(opens_block);
            let inner = if opens {
                format!("{indent}{}", self.indent)
            } else {
                indent
            };
            let at = motion::line_end(buffer, line);
            buffer.insert(at, &format!("\n{inner}"));
            self.cursor = at + 1 + inner.chars().count();
        } else {
            let at = buffer.line_start(line);
            buffer.insert(at, &format!("{indent}\n"));
            self.cursor = at + indent.chars().count();
        }
        self.mode = Mode::Insert;
        self.auto_indented = Some(self.cursor);
    }

    /// True when the cursor's line holds only indentation and the cursor is past some of it:
    /// the line Enter or `o` just opened, which Backspace then removes in one go.
    fn on_blank_line(&self, buffer: &Buffer) -> bool {
        let line = buffer.position_of(self.cursor).line;
        is_blank(&self.before_cursor(buffer)) && is_blank(&buffer.line(line))
    }

    /// Removes the cursor's blank line whole, leaving the cursor at the end of the line
    /// above, so an indented empty line does not take one Backspace per level to undo.
    /// On the first line there is no line above: only the indentation goes.
    fn delete_blank_line(&mut self, buffer: &mut Buffer) {
        let line = buffer.position_of(self.cursor).line;
        let start = buffer.line_start(line);
        let end = motion::line_end(buffer, line);
        let from = start.saturating_sub(usize::from(line > 0));
        buffer.delete(from..end);
        self.cursor = from;
    }

    /// Takes back the indentation of the cursor's line when it holds nothing else.
    fn drop_blank_indent(&mut self, buffer: &mut Buffer) {
        let line = buffer.position_of(self.cursor).line;
        let start = buffer.line_start(line);
        if self.cursor == motion::line_end(buffer, line) && is_blank(&buffer.line(line)) {
            buffer.delete(start..self.cursor);
            self.cursor = start;
        }
    }

    /// Normal mode keeps the cursor on a character, never past the end of the line.
    pub(crate) fn clamp(&mut self, buffer: &Buffer) {
        self.cursor = self.cursor.min(buffer.len());
        let line = buffer.position_of(self.cursor).line;
        let length = buffer.line_len(line);
        let start = buffer.line_start(line);
        if self.cursor >= start + length {
            self.cursor = start + length.saturating_sub(1);
        }
    }
}

/// A line ending in one of these opens a block: the next line goes one level deeper (`:` for
/// Python and YAML).
fn opens_block(c: char) -> bool {
    matches!(c, '{' | '[' | '(' | ':')
}

/// The tabs and spaces `line` starts with.
fn leading_indent(line: &str) -> String {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// True when `text` has something and it is all indentation.
fn is_blank(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == ' ' || c == '\t')
}

/// How many chars at the end of the indentation `before` make one level less: a tab, or the
/// spaces back to the previous multiple of the level (all of them when fewer).
fn dedent_len(before: &str, unit: &str) -> usize {
    if before.ends_with('\t') {
        return 1;
    }
    let spaces = before.chars().rev().take_while(|c| *c == ' ').count();
    let width = if unit == "\t" { 4 } else { unit.len() };
    match spaces % width {
        0 => width.min(spaces),
        off => off,
    }
}

/// What closes `open` when it is typed: the pairs that close themselves.
fn closer_of(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
