//! Vim-style modal editing as a pure state machine over a [`Buffer`]: no UI types, so the UI
//! only translates its key events to [`Key`]. Offsets are `char` indices.

mod motion;
mod normal;

use zide_text::Buffer;

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
        out
    }

    pub fn handle(&mut self, buffer: &mut Buffer, key: Key) {
        match self.mode {
            Mode::Normal => self.normal(buffer, key),
            Mode::Insert => self.insert(buffer, key),
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
