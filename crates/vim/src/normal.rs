use zide_text::Buffer;

use crate::{
    Key, Mode, Operator, Register, Vim,
    motion::{Motion, first_non_blank, last_line, line_end, motion},
};

impl Vim {
    pub(crate) fn normal(&mut self, buffer: &mut Buffer, key: Key) {
        match key {
            Key::Esc => self.reset_pending(),
            Key::Ctrl('r') => {
                self.reset_pending();
                if let Some(at) = buffer.redo() {
                    self.jump(buffer, at);
                }
            }
            Key::Char(c) => self.normal_char(buffer, c),
            Key::Ctrl(_) | Key::Enter | Key::Backspace => self.reset_pending(),
        }
        if self.mode == Mode::Normal {
            buffer.commit();
        }
    }

    fn reset_pending(&mut self) {
        self.count = 0;
        self.operator = None;
        self.g_prefix = false;
    }

    fn jump(&mut self, buffer: &Buffer, offset: usize) {
        self.set_cursor(buffer, offset);
    }

    fn normal_char(&mut self, buffer: &mut Buffer, c: char) {
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
        let operator = match c {
            'd' => Some(Operator::Delete),
            'c' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            _ => None,
        };
        if let Some(operator) = operator {
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
        if c == 'g' {
            self.g_prefix = true;
            return;
        }
        if "hjklwbe0^$G".contains(c) {
            return self.run_motion(buffer, c);
        }
        if self.operator.is_some() {
            return self.reset_pending(); // an operator followed by something that is no motion
        }
        let count = self.count.max(1);
        self.reset_pending();
        self.command(buffer, c, count);
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
        if start == end {
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
        if !text.ends_with('\n') {
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
                let from = if end == buffer.len()
                    && start > 0
                    && !buffer.slice(start..end).ends_with('\n')
                {
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
                if end > self.cursor {
                    self.register = Register {
                        text: buffer.slice(self.cursor..end),
                        linewise: false,
                    };
                    buffer.delete(self.cursor..end);
                    self.clamp(buffer);
                }
            }
            'D' => {
                let end = line_end(buffer, position.line);
                if end > self.cursor {
                    self.register = Register {
                        text: buffer.slice(self.cursor..end),
                        linewise: false,
                    };
                    buffer.delete(self.cursor..end);
                    self.clamp(buffer);
                }
            }
            'p' | 'P' => self.paste(buffer, c == 'p', count),
            'u' => {
                if let Some(at) = buffer.undo() {
                    self.jump(buffer, at);
                }
            }
            _ => {}
        }
        if self.mode == Mode::Normal {
            self.want_column = buffer.position_of(self.cursor).column;
        }
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
