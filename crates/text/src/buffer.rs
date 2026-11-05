use std::ops::Range;

use ropey::Rope;

use crate::history::History;

/// Changes on every mutation of a [`Buffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(pub u64);

/// Replace `range` (char offsets) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// Zero-based line and column, the column counted in `char`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// A place in the text that follows edits, tied to the revision it was made at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub offset: usize,
    pub revision: Revision,
    /// Where it goes when text is inserted exactly at it.
    pub bias: Bias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    Left,
    Right,
}

#[derive(Debug, Default)]
pub struct Buffer {
    rope: Rope,
    revision: Revision,
    history: History,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            ..Self::default()
        }
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Length in `char`s.
    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of lines; an empty buffer and one ending in `\n` still have a last (empty) line.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char_at(&self, offset: usize) -> Option<char> {
        self.rope.get_char(offset)
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    /// The line without its line break.
    pub fn line(&self, line: usize) -> String {
        let text = self.rope.line(line).to_string();
        text.trim_end_matches(['\n', '\r']).to_string()
    }

    /// Length of `line` in `char`s, without its line break.
    pub fn line_len(&self, line: usize) -> usize {
        self.line(line).chars().count()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    pub fn offset_of(&self, position: Position) -> usize {
        let line = position.line.min(self.line_count() - 1);
        self.line_start(line) + position.column.min(self.line_len(line))
    }

    pub fn position_of(&self, offset: usize) -> Position {
        let offset = offset.min(self.len());
        let line = self.rope.char_to_line(offset);
        Position {
            line,
            column: offset - self.line_start(line),
        }
    }

    pub fn byte_of(&self, offset: usize) -> usize {
        self.rope.char_to_byte(offset.min(self.len()))
    }

    pub fn offset_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.rope.len_bytes()))
    }

    pub fn anchor(&self, offset: usize, bias: Bias) -> Anchor {
        Anchor {
            offset: offset.min(self.len()),
            revision: self.revision,
            bias,
        }
    }

    /// The offset of `anchor`, or `None` when it belongs to another revision (stale: drop it,
    /// or move it through the edits with [`Buffer::rebase`]).
    pub fn resolve(&self, anchor: Anchor) -> Option<usize> {
        (anchor.revision == self.revision).then_some(anchor.offset)
    }

    /// Replace `range` with `text`, recording it for undo.
    pub fn edit(&mut self, range: Range<usize>, text: &str) {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let removed = self.slice(range.clone());
        let edit = Edit {
            range: range.clone(),
            text: text.to_string(),
        };
        self.apply(&edit);
        let inverse = Edit {
            range: range.start..range.start + text.chars().count(),
            text: removed,
        };
        self.history.open.undo.insert(0, inverse);
        self.history.open.redo.push(edit);
        self.history.redo.clear();
    }

    pub fn insert(&mut self, offset: usize, text: &str) {
        self.edit(offset..offset, text);
    }

    pub fn delete(&mut self, range: Range<usize>) {
        self.edit(range, "");
    }

    /// Closes the current group: one `undo` reverts everything since the last commit.
    pub fn commit(&mut self) {
        let group = std::mem::take(&mut self.history.open);
        if !group.redo.is_empty() {
            self.history.undo.push(group);
        }
    }

    /// Reverts the last group; returns the offset of the first change it made.
    pub fn undo(&mut self) -> Option<usize> {
        self.commit();
        let group = self.history.undo.pop()?;
        let at = group.undo.last().map(|e| e.range.start);
        for edit in &group.undo {
            self.apply(edit);
        }
        self.history.redo.push(group);
        at
    }

    pub fn redo(&mut self) -> Option<usize> {
        self.commit();
        let group = self.history.redo.pop()?;
        let at = group.redo.first().map(|e| e.range.start);
        for edit in &group.redo {
            self.apply(edit);
        }
        self.history.undo.push(group);
        at
    }

    /// Moves `anchor` through `edit`, which must be the edit that produced the current revision.
    pub fn rebase(&self, anchor: Anchor, edit: &Edit) -> Anchor {
        let inserted = edit.text.chars().count();
        let offset = if anchor.offset < edit.range.start
            || (anchor.offset == edit.range.start && anchor.bias == Bias::Left && inserted > 0)
        {
            anchor.offset
        } else if anchor.offset >= edit.range.end {
            anchor.offset - edit.range.len() + inserted
        } else {
            edit.range.start
        };
        Anchor {
            offset,
            revision: self.revision,
            bias: anchor.bias,
        }
    }

    fn apply(&mut self, edit: &Edit) {
        self.rope.remove(edit.range.clone());
        self.rope.insert(edit.range.start, &edit.text);
        self.revision.0 += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_and_clamp() {
        let b = Buffer::new("héllo\nwörld\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(0), "héllo");
        assert_eq!(b.offset_of(Position { line: 1, column: 2 }), 8);
        assert_eq!(b.position_of(8), Position { line: 1, column: 2 });
        // column past the end of the line stops before the line break
        assert_eq!(
            b.offset_of(Position {
                line: 0,
                column: 99
            }),
            5
        );
        assert_eq!(b.byte_of(2), 3); // 'é' is two bytes
        assert_eq!(b.offset_of_byte(3), 2);
    }

    #[test]
    fn every_edit_bumps_the_revision() {
        let mut b = Buffer::new("");
        let r0 = b.revision();
        b.insert(0, "a");
        assert!(b.revision() > r0);
    }

    #[test]
    fn undo_and_redo_revert_whole_groups() {
        let mut b = Buffer::new("abc");
        b.insert(3, "d");
        b.insert(4, "e");
        b.commit();
        b.delete(0..1);
        b.commit();
        assert_eq!(b.text(), "bcde");
        assert_eq!(b.undo(), Some(0));
        assert_eq!(b.text(), "abcde");
        b.undo();
        assert_eq!(b.text(), "abc");
        assert_eq!(b.undo(), None);
        b.redo();
        assert_eq!(b.text(), "abcde");
        b.redo();
        assert_eq!(b.text(), "bcde");
    }

    #[test]
    fn a_new_edit_discards_the_redo_stack() {
        let mut b = Buffer::new("a");
        b.insert(1, "b");
        b.undo();
        b.insert(1, "c");
        b.commit();
        assert_eq!(b.redo(), None);
        assert_eq!(b.text(), "ac");
    }

    #[test]
    fn undo_restores_replaced_text() {
        let mut b = Buffer::new("hello world");
        b.edit(0..5, "bye");
        b.edit(4..9, "x");
        b.undo();
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn anchors_follow_edits_and_go_stale() {
        let mut b = Buffer::new("hello world");
        let left = b.anchor(5, Bias::Left);
        let right = b.anchor(5, Bias::Right);
        let edit = Edit {
            range: 5..5,
            text: ",".into(),
        };
        b.insert(5, ",");
        assert_eq!(b.resolve(left), None);
        assert_eq!(b.rebase(left, &edit).offset, 5);
        let right = b.rebase(right, &edit);
        assert_eq!(right.offset, 6);
        assert_eq!(b.resolve(right), Some(6));
    }
}
