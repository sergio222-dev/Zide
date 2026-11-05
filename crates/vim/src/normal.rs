use std::ops::Range;

use zide_text::Buffer;

use crate::{
    Key, Mode, Operator, Register, Vim,
    motion::{Motion, first_non_blank, last_line, line_end, motion},
    objects::text_object,
};

impl Vim {
    pub(crate) fn normal(&mut self, buffer: &mut Buffer, key: Key) {
        match key {
            Key::Esc => {
                self.reset_pending();
                if self.is_visual() {
                    self.mode = Mode::Normal;
                    self.clamp(buffer);
                }
            }
            Key::Ctrl('r') => {
                self.reset_pending();
                if let Some(at) = buffer.redo() {
                    self.set_cursor(buffer, at);
                }
            }
            Key::Char(c) => self.normal_char(buffer, c),
            Key::Ctrl(_) | Key::Enter | Key::Backspace => self.reset_pending(),
        }
        if self.mode == Mode::Normal {
            buffer.commit();
        }
    }

    pub(crate) fn reset_pending(&mut self) {
        self.count = 0;
        self.operator = None;
        self.g_prefix = false;
        self.object = None;
    }

    fn normal_char(&mut self, buffer: &mut Buffer, c: char) {
        if let Some(inner) = self.object.take() {
            return self.object_command(buffer, inner, c);
        }
        if self.g_prefix {
            self.g_prefix = false;
            if c == 'g' {
                return self.run_motion(buffer, 'g');
            }
            return self.reset_pending();
        }
        if c.is_ascii_digit() && (c != '0' || self.count > 0) {
            self.count = self.count * 10 + c.to_digit(10).unwrap() as usize;
            return;
        }
        if self.is_visual() && self.visual_command(buffer, c) {
            return;
        }
        let operator = match c {
            'd' => Some(Operator::Delete),
            'c' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            _ => None,
        };
        if let Some(operator) = operator.filter(|_| !self.is_visual()) {
            return match self.operator {
                None => {
                    self.operator = Some((operator, self.count.max(1)));
                    self.count = 0;
                }
                // `dd`, `cc`, `yy`: the operator typed twice works on whole lines
                Some((pending, before)) if pending == operator => {
                    let lines = before * self.count.max(1);
                    self.reset_pending();
                    self.line_operator(buffer, operator, lines);
                }
                Some(_) => self.reset_pending(),
            };
        }
        if (c == 'i' || c == 'a') && (self.operator.is_some() || self.is_visual()) {
            self.object = Some(c == 'i');
            return;
        }
        if c == 'g' {
            self.g_prefix = true;
            return;
        }
        if "hjklwbe0^$G".contains(c) {
            return self.run_motion(buffer, c);
        }
        if self.operator.is_some() || self.is_visual() {
            return self.reset_pending(); // something that is no motion
        }
        let count = self.count.max(1);
        self.reset_pending();
        self.command(buffer, c, count);
    }

    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    /// Commands that only exist in Visual mode; `false` leaves the key to the motions.
    fn visual_command(&mut self, buffer: &mut Buffer, c: char) -> bool {
        match c {
            'v' | 'V' => {
                let target = if c == 'v' {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                self.mode = if self.mode == target {
                    Mode::Normal
                } else {
                    target
                };
                self.reset_pending();
            }
            'o' => {
                std::mem::swap(&mut self.anchor, &mut self.cursor);
                self.reset_pending();
            }
            'd' | 'x' => self.visual_operator(buffer, Operator::Delete),
            'y' => self.visual_operator(buffer, Operator::Yank),
            'c' | 's' => self.visual_operator(buffer, Operator::Change),
            _ => return false,
        }
        true
    }

    fn visual_operator(&mut self, buffer: &mut Buffer, op: Operator) {
        let linewise = self.mode == Mode::VisualLine;
        let selection = self.selection_range(buffer);
        self.mode = Mode::Normal;
        self.reset_pending();
        if linewise {
            let a = buffer.position_of(selection.start).line;
            let b = buffer.position_of(selection.end.saturating_sub(1)).line;
            self.lines(buffer, op, a, b);
            if op == Operator::Yank {
                self.cursor = buffer.line_start(a);
            }
        } else {
            self.operate_chars(buffer, op, selection.start, selection.end);
        }
        self.clamp(buffer);
        self.want_column = buffer.position_of(self.cursor).column;
    }

    /// The selected text as a char range, whole lines included in line-wise Visual mode.
    pub(crate) fn selection_range(&self, buffer: &Buffer) -> Range<usize> {
        let (low, high) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        if self.mode == Mode::VisualLine {
            let a = buffer.position_of(low).line;
            let b = buffer.position_of(high).line;
            let end = if b + 1 < buffer.line_count() {
                buffer.line_start(b + 1)
            } else {
                buffer.len()
            };
            buffer.line_start(a)..end
        } else {
            low..(high + 1).min(buffer.len())
        }
    }

    fn object_command(&mut self, buffer: &mut Buffer, inner: bool, kind: char) {
        let range = text_object(buffer, self.cursor, inner, kind);
        let operator = self.operator.take().map(|(op, _)| op);
        self.reset_pending();
        let Some(range) = range else {
            return;
        };
        match operator {
            Some(op) => self.operate_chars(buffer, op, range.start, range.end),
            None => {
                // Visual mode: the object becomes the selection
                if self.mode == Mode::VisualLine {
                    self.mode = Mode::Visual;
                }
                self.anchor = range.start;
                self.cursor = range.end.saturating_sub(1).max(range.start);
            }
        }
    }

    fn run_motion(&mut self, buffer: &mut Buffer, key: char) {
        let typed = (self.count > 0).then_some(self.count);
        let pending = self.operator.take();
        let count = match (pending, typed) {
            (Some((_, before)), Some(after)) => Some(before * after),
            (Some((_, before)), None) if before > 1 => Some(before),
            _ => typed,
        };
        self.count = 0;
        let cursor = self.cursor;
        let operator = pending.map(|(op, _)| op);
        // `cw` behaves like `ce`
        let key_for_motion = if operator == Some(Operator::Change) && key == 'w' {
            'e'
        } else {
            key
        };
        let Some(found) = motion(
            buffer,
            key_for_motion,
            cursor,
            self.want_column,
            count,
            operator.is_some(),
        ) else {
            return;
        };
        match operator {
            None => {
                self.cursor = found.target;
                self.clamp(buffer);
                if !"jk".contains(key) {
                    self.want_column = buffer.position_of(self.cursor).column;
                }
            }
            Some(op) => self.apply(buffer, op, cursor, found),
        }
    }

    fn apply(&mut self, buffer: &mut Buffer, op: Operator, from: usize, found: Motion) {
        if found.linewise {
            let a = buffer.position_of(from.min(found.target)).line;
            let b = buffer.position_of(from.max(found.target)).line;
            return self.lines(buffer, op, a, b);
        }
        let (start, mut end) = (from.min(found.target), from.max(found.target));
        if found.inclusive {
            end = (end + 1).min(buffer.len());
        }
        self.operate_chars(buffer, op, start, end);
    }

    /// Applies `op` to the characters `start..end`.
    fn operate_chars(&mut self, buffer: &mut Buffer, op: Operator, start: usize, end: usize) {
        if start == end {
            // `ci"` on `""` has nothing to remove but still starts typing there
            if op == Operator::Change {
                self.cursor = start;
                self.mode = Mode::Insert;
            }
            return;
        }
        self.register = Register {
            text: buffer.slice(start..end),
            linewise: false,
        };
        match op {
            Operator::Yank => self.cursor = start,
            Operator::Delete | Operator::Change => {
                buffer.delete(start..end);
                self.cursor = start;
                if op == Operator::Change {
                    self.mode = Mode::Insert;
                } else {
                    self.clamp(buffer);
                }
            }
        }
        self.want_column = buffer.position_of(self.cursor).column;
    }

    /// `dd`, `cc`, `yy` with a count.
    fn line_operator(&mut self, buffer: &mut Buffer, op: Operator, count: usize) {
        let a = buffer.position_of(self.cursor).line;
        let b = (a + count - 1).min(last_line(buffer));
        self.lines(buffer, op, a, b);
    }

    /// Operates on lines `a..=b`.
    fn lines(&mut self, buffer: &mut Buffer, op: Operator, a: usize, b: usize) {
        let start = buffer.line_start(a);
        let end = if b + 1 < buffer.line_count() {
            buffer.line_start(b + 1)
        } else {
            buffer.len()
        };
        let mut text = buffer.slice(start..end);
        let had_break = text.ends_with('\n');
        if !had_break {
            text.push('\n');
        }
        self.register = Register {
            text,
            linewise: true,
        };
        match op {
            Operator::Yank => {}
            Operator::Change => {
                buffer.delete(start..line_end(buffer, b));
                self.cursor = start;
                self.mode = Mode::Insert;
            }
            Operator::Delete => {
                // the last line has no break after it: take the one before it instead
                let from = if !had_break && start > 0 {
                    start - 1
                } else {
                    start
                };
                buffer.delete(from..end);
                let line = a.min(last_line(buffer));
                self.cursor = first_non_blank(buffer, line);
            }
        }
        self.want_column = buffer.position_of(self.cursor).column;
    }

    fn command(&mut self, buffer: &mut Buffer, c: char, count: usize) {
        let position = buffer.position_of(self.cursor);
        match c {
            'i' => self.mode = Mode::Insert,
            'a' => {
                if buffer.line_len(position.line) > 0 {
                    self.cursor += 1;
                }
                self.mode = Mode::Insert;
            }
            'I' => {
                self.cursor = first_non_blank(buffer, position.line);
                self.mode = Mode::Insert;
            }
            'A' => {
                self.cursor = line_end(buffer, position.line);
                self.mode = Mode::Insert;
            }
            'o' | 'O' => {
                let (at, text) = if c == 'o' {
                    (line_end(buffer, position.line), "\n")
                } else {
                    (buffer.line_start(position.line), "\n")
                };
                buffer.insert(at, text);
                self.cursor = if c == 'o' { at + 1 } else { at };
                self.mode = Mode::Insert;
            }
            'x' => {
                let length = buffer.line_len(position.line);
                let end = self.cursor + count.min(length - position.column.min(length));
                self.delete_to(buffer, end);
            }
            'D' => self.delete_to(buffer, line_end(buffer, position.line)),
            'p' | 'P' => self.paste(buffer, c == 'p', count),
            'v' => self.start_visual(Mode::Visual),
            'V' => self.start_visual(Mode::VisualLine),
            '.' => self.repeat(buffer),
            'u' => {
                if let Some(at) = buffer.undo() {
                    self.set_cursor(buffer, at);
                }
            }
            _ => {}
        }
        if self.mode == Mode::Normal {
            self.want_column = buffer.position_of(self.cursor).column;
        }
    }

    fn start_visual(&mut self, mode: Mode) {
        self.mode = mode;
        self.anchor = self.cursor;
    }

    /// Deletes from the cursor to `end` (exclusive) into the register.
    fn delete_to(&mut self, buffer: &mut Buffer, end: usize) {
        if end > self.cursor {
            self.register = Register {
                text: buffer.slice(self.cursor..end),
                linewise: false,
            };
            buffer.delete(self.cursor..end);
            self.clamp(buffer);
        }
    }

    /// `.`: types the keys of the last change again.
    fn repeat(&mut self, buffer: &mut Buffer) {
        if self.replaying {
            return;
        }
        self.replaying = true;
        for key in self.last_change.clone() {
            self.handle(buffer, key);
        }
        self.replaying = false;
        // the replay rewrote the command log; `.` itself is not a change
        self.current.clear();
        self.start_revision = buffer.revision();
    }

    fn paste(&mut self, buffer: &mut Buffer, after: bool, count: usize) {
        let register = self.register.clone();
        if register.text.is_empty() {
            return;
        }
        let text = register.text.repeat(count);
        let line = buffer.position_of(self.cursor).line;
        if register.linewise {
            let last = last_line(buffer);
            let target_line = if after { line + 1 } else { line };
            if after && line == last && !buffer.text().ends_with('\n') {
                let body = text.trim_end_matches('\n');
                buffer.insert(buffer.len(), &format!("\n{body}"));
            } else {
                let at = if target_line > last {
                    buffer.len()
                } else {
                    buffer.line_start(target_line)
                };
                buffer.insert(at, &text);
            }
            self.cursor = first_non_blank(buffer, target_line);
        } else {
            let at = if after && buffer.line_len(line) > 0 {
                self.cursor + 1
            } else {
                self.cursor
            };
            buffer.insert(at, &text);
            self.cursor = at + text.chars().count() - 1;
        }
    }
}
