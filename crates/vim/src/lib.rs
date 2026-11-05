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
            Key::Char(c) => self.type_text(buffer, c.encode_utf8(&mut [0; 4])),
            Key::Enter => self.type_text(buffer, "\n"),
            Key::Backspace if self.cursor > 0 => {
                buffer.delete(self.cursor - 1..self.cursor);
                self.cursor -= 1;
            }
            Key::Backspace | Key::Ctrl(_) => {}
        }
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

#[cfg(test)]
mod tests;
