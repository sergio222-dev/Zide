//! Vim-style modal editing as a pure state machine over a [`Buffer`]: no UI types, so the UI
//! only translates its key events to [`Key`]. Offsets are `char` indices.

mod motion;
mod normal;
mod objects;

use std::ops::Range;

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
pub(crate) enum Operator {
    Delete,
    Change,
    Yank,
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
    /// A `g` was typed and waits for its second key.
    pub(crate) g_prefix: bool,
    /// `i` (true) or `a` (false) was typed and waits for the object key.
    pub(crate) object: Option<bool>,
    /// Where the Visual selection started.
    pub(crate) anchor: usize,
    /// Keys of the command being typed; becomes `last_change` if it edited the text.
    current: Vec<Key>,
    start_revision: Revision,
    last_change: Vec<Key>,
    pub(crate) replaying: bool,
    pub(crate) register: Register,
    /// Text yanked and not yet taken for the system clipboard.
    yanked: Option<String>,
    /// The system clipboard as Zide last saw it, to tell text copied elsewhere from its own.
    clipboard: String,
}

impl Default for Vim {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            cursor: 0,
            want_column: 0,
            count: 0,
            operator: None,
            g_prefix: false,
            object: None,
            anchor: 0,
            current: Vec::new(),
            start_revision: Revision::default(),
            last_change: Vec::new(),
            replaying: false,
            register: Register::default(),
            yanked: None,
            clipboard: String::new(),
        }
    }
}

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn cursor(&self) -> usize {
        self.cursor
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
        self.count = 0;
        self.operator = None;
        self.g_prefix = false;
        self.object = None;
        self.set_cursor(buffer, offset);
    }

    /// Moves the cursor to `line` (clamped), on the column it wants: what scrolling by pages does.
    pub fn move_to_line(&mut self, buffer: &Buffer, line: usize) {
        let line = line.min(motion::last_line(buffer));
        let last_column = buffer.line_len(line).saturating_sub(1);
        self.cursor = buffer.line_start(line) + self.want_column.min(last_column);
    }

    /// What the user has typed of an unfinished command (`2d`, `g`), for a status line.
    pub fn pending(&self) -> String {
        let mut out = String::new();
        if self.count > 0 {
            out += &self.count.to_string();
        }
        match self.operator {
            Some((Operator::Delete, _)) => out.push('d'),
            Some((Operator::Change, _)) => out.push('c'),
            Some((Operator::Yank, _)) => out.push('y'),
            None => {}
        }
        if self.g_prefix {
            out.push('g');
        }
        match self.object {
            Some(true) => out.push('i'),
            Some(false) => out.push('a'),
            None => {}
        }
        out
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

    /// Text yanked since the last call, for the system clipboard.
    pub fn take_yank(&mut self) -> Option<String> {
        let text = self.yanked.take()?;
        self.clipboard.clone_from(&text);
        Some(text)
    }

    /// Offers the system clipboard before a paste. Text copied elsewhere since Zide last saw
    /// the clipboard becomes the register, linewise when it ends with a line break; otherwise
    /// the register keeps what Zide put there, such as deleted text.
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

    /// Keeps the register's text as yanked, for [`Vim::take_yank`].
    pub(crate) fn note_yank(&mut self) {
        self.yanked = Some(self.register.text.clone());
    }

    fn idle(&self) -> bool {
        self.count == 0 && self.operator.is_none() && !self.g_prefix && self.object.is_none()
    }

    pub fn handle(&mut self, buffer: &mut Buffer, key: Key) {
        if self.mode == Mode::Normal && self.idle() {
            self.current.clear();
            self.start_revision = buffer.revision();
        }
        self.current.push(key);
        match self.mode {
            Mode::Insert => self.insert(buffer, key),
            _ => self.normal(buffer, key),
        }
        if self.mode == Mode::Normal && self.idle() && !self.replaying {
            // undo, redo and `.` itself are not changes to repeat
            let history = matches!(
                self.current.first(),
                Some(Key::Char('u' | '.') | Key::Ctrl('r'))
            );
            if buffer.revision() != self.start_revision && !history {
                self.last_change = std::mem::take(&mut self.current);
            } else {
                self.current.clear();
            }
        }
    }

    fn insert(&mut self, buffer: &mut Buffer, key: Key) {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                buffer.commit();
                let column = buffer.position_of(self.cursor).column;
                self.cursor -= usize::from(column > 0);
                self.want_column = buffer.position_of(self.cursor).column;
            }
            Key::Char(c) => self.type_char(buffer, c),
            Key::Enter => self.type_text(buffer, "\n"),
            Key::Backspace if self.cursor > 0 => {
                // between an empty pair both halves go
                let pair = buffer
                    .char_at(self.cursor - 1)
                    .zip(buffer.char_at(self.cursor))
                    .is_some_and(|(open, close)| closer_of(open) == Some(close));
                let end = self.cursor + usize::from(pair);
                buffer.delete(self.cursor - 1..end);
                self.cursor -= 1;
            }
            // pastes the register, which the UI fills from the system clipboard
            Key::Ctrl('v') => {
                let text = self.register.text.clone();
                self.type_text(buffer, &text);
            }
            Key::Backspace | Key::Ctrl(_) => {}
        }
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
