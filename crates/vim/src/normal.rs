use std::ops::Range;

use zide_text::Buffer;

use crate::{
    Action, Binding, Key, Mode, Operator, Request, Step, Vim, keymap,
    motion::{Motion, first_non_blank, last_line, line_end, motion},
    objects::text_object,
};

impl Vim {
    /// A key outside Insert mode: a digit of a count, or the next key of the mode's
    /// [`keymap`] tree. A key the tree does not bind there drops the half-typed command.
    pub(crate) fn normal(&mut self, buffer: &mut Buffer, key: Key) -> Option<Request> {
        let mut request = None;
        match key {
            Key::Esc => {
                self.reset_pending();
                if self.is_visual() {
                    self.mode = Mode::Normal;
                    self.clamp(buffer);
                }
            }
            Key::Char(c) if self.counts(c) => {
                self.count = self.count * 10 + c.to_digit(10).unwrap() as usize;
                self.typed.push(c);
            }
            _ => {
                let keymap = self.keymap.clone(); // the walk below changes `self`
                let list = match self.group_in(&keymap) {
                    Some(group) => &group.children[..],
                    None if self.path.is_empty() => keymap.root(self.mode),
                    None => &[],
                };
                match keymap::find(list, key).filter(|binding| self.allows(binding)) {
                    Some(binding) if binding.is_group() => self.enter_group(binding),
                    Some(binding) => request = self.run(buffer, binding.action),
                    None => self.reset_pending(),
                }
            }
        }
        if self.mode == Mode::Normal {
            buffer.commit();
        }
        request
    }

    /// True when `c` is a digit of a count here: before a command or after an operator, and
    /// `0` only after another digit (alone it is a motion).
    fn counts(&self, c: char) -> bool {
        let here = match self.group_in(&self.keymap) {
            None => self.path.is_empty(),
            Some(group) => matches!(group.action, Action::Operator(_)),
        };
        here && c.is_ascii_digit() && (c != '0' || self.count > 0)
    }

    /// A group was typed: it waits for one of its children.
    fn enter_group(&mut self, group: &Binding) {
        match group.action {
            Action::Operator(op) => {
                self.operator = Some((op, self.count.max(1)));
                self.count = 0;
            }
            Action::Object { inner } => self.object = Some(inner),
            _ => {}
        }
        self.path.push(group.key);
        self.typed += &keymap::label(group.key);
    }

    /// Runs the action of a leaf, with what the groups before it left pending.
    fn run(&mut self, buffer: &mut Buffer, action: Action) -> Option<Request> {
        let count = self.count.max(1);
        match action {
            Action::Motion(c) => self.run_motion(buffer, c),
            // `dd`, `cc`, `yy`: the operator typed twice works on whole lines
            Action::Line => {
                if let Some((op, before)) = self.operator {
                    let lines = before * count;
                    self.reset_pending();
                    self.line_operator(buffer, op, lines);
                }
            }
            Action::TextObject(kind) => {
                let inner = self.object.unwrap_or(true);
                self.object_command(buffer, inner, kind);
            }
            Action::Command(c) => {
                self.reset_pending();
                self.command(buffer, c, count);
            }
            Action::Redo => {
                self.reset_pending();
                if let Some(at) = buffer.redo() {
                    self.set_cursor(buffer, at);
                }
            }
            Action::AddToNumber(sign) => {
                self.reset_pending();
                self.add_to_number(buffer, count as i64 * sign);
            }
            Action::Visual(target) => {
                self.reset_pending();
                if self.mode == target {
                    self.mode = Mode::Normal;
                } else if self.is_visual() {
                    self.mode = target;
                } else {
                    self.start_visual(target);
                }
                self.want_column = buffer.position_of(self.cursor).column;
            }
            Action::SwapEnds => {
                self.reset_pending();
                std::mem::swap(&mut self.anchor, &mut self.cursor);
            }
            Action::VisualOperator(op) => self.visual_operator(buffer, op),
            Action::Editor(request) => {
                self.reset_pending();
                return Some(request);
            }
            Action::Operator(_) | Action::Object { .. } | Action::Prefix => {}
        }
        self.reset_pending();
        None
    }

    /// `Ctrl-A` / `Ctrl-X`: adds `step` to the decimal number under or after the cursor on its
    /// line (a `-` right before it makes it negative, unless it follows a word character), keeps
    /// leading zeros, and leaves the cursor on the number's last digit.
    fn add_to_number(&mut self, buffer: &mut Buffer, step: i64) {
        let line = buffer.position_of(self.cursor).line;
        let start = buffer.line_start(line);
        let chars: Vec<char> = buffer.line(line).chars().collect();
        let column = self.cursor - start;
        let digit = |i: usize| chars.get(i).is_some_and(char::is_ascii_digit);
        let Some(mut first) = (column..chars.len()).find(|&i| digit(i)) else {
            return;
        };
        if first == column {
            while first > 0 && digit(first - 1) {
                first -= 1;
            }
        }
        let mut end = first;
        while digit(end) {
            end += 1;
        }
        let negative = first > 0
            && chars[first - 1] == '-'
            && !(first > 1 && (chars[first - 2].is_alphanumeric() || chars[first - 2] == '_'));
        let from = if negative { first - 1 } else { first };
        let digits: String = chars[first..end].iter().collect();
        let Ok(magnitude) = digits.parse::<i64>() else {
            return;
        };
        let value = (if negative { -magnitude } else { magnitude }).saturating_add(step);
        let width = if digits.starts_with('0') {
            digits.len()
        } else {
            0
        };
        let text = format!(
            "{}{:0>width$}",
            if value < 0 { "-" } else { "" },
            value.unsigned_abs()
        );
        buffer.edit(start + from..start + end, &text);
        self.cursor = start + from + text.chars().count() - 1;
        self.want_column = self.cursor - start;
    }

    pub(crate) fn reset_pending(&mut self) {
        self.count = 0;
        self.operator = None;
        self.object = None;
        self.path.clear();
        self.typed.clear();
    }

    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
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
        // `cw` behaves like `ce`, `cW` like `cE`
        let key_for_motion = match key {
            'w' if operator == Some(Operator::Change) => 'e',
            'W' if operator == Some(Operator::Change) => 'E',
            _ => key,
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
        self.set_register(buffer.slice(start..end), false);
        match op {
            Operator::Yank => {
                self.cursor = start;
            }
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
        self.set_register(text, true);
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
            'o' | 'O' => self.open_line(buffer, c == 'o'),
            'x' => {
                let length = buffer.line_len(position.line);
                let end = self.cursor + count.min(length - position.column.min(length));
                self.delete_to(buffer, end);
            }
            'D' => self.delete_to(buffer, line_end(buffer, position.line)),
            'p' | 'P' => self.paste(buffer, c == 'p', count),
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
            self.set_register(buffer.slice(self.cursor..end), false);
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
        for step in self.last_change.clone() {
            match step {
                Step::Key(key) => {
                    self.handle(buffer, key);
                }
                Step::Complete(completed) => self.apply_completed(buffer, &completed),
            }
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
