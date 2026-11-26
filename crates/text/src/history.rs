use crate::buffer::Edit;

/// A group of edits undone and redone together (one typing run, one `dd`, ...).
#[derive(Debug, Default, Clone)]
pub(crate) struct Group {
    /// Inverse edits, in the order they must be applied to undo.
    pub undo: Vec<Edit>,
    /// Edits, in the order they must be applied to redo.
    pub redo: Vec<Edit>,
}

#[derive(Debug, Default)]
pub(crate) struct History {
    pub undo: Vec<Group>,
    pub redo: Vec<Group>,
    /// Group being built; committed by `Buffer::commit`.
    pub open: Group,
}
