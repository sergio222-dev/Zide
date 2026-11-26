//! The git dialogs that float over the workspace: the branch switcher, the push dialog, the
//! pull/merge/rebase dialog and the conflict resolver. One at a time, and the merge editor only
//! over the conflicts it resolves.

use crate::branches::BranchPopup;
use crate::conflicts::ConflictsPopup;
use crate::gitop::OpDialog;
use crate::merge_editor::MergeEditor;
use crate::push::PushPopup;

#[derive(Default)]
pub enum Overlay {
    #[default]
    None,
    Branches(BranchPopup),
    /// Ctrl+Shift+K.
    Push(PushPopup),
    GitOp(OpDialog),
    /// The conflicts, and the merge editor while it is open on one of their files.
    Conflicts(ConflictsPopup, Option<MergeEditor>),
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::None)
    }

    /// The merge editor is open: no other dialog takes its place, so what was resolved in it is
    /// never dropped.
    pub fn merging(&self) -> bool {
        self.merge().is_some()
    }

    /// Closes whatever is open, unless the merge editor is (see [`Overlay::merging`]).
    pub fn clear(&mut self) {
        if !self.merging() {
            *self = Overlay::None;
        }
    }

    pub fn branches(&self) -> Option<&BranchPopup> {
        match self {
            Overlay::Branches(popup) => Some(popup),
            _ => None,
        }
    }

    pub fn branches_mut(&mut self) -> Option<&mut BranchPopup> {
        match self {
            Overlay::Branches(popup) => Some(popup),
            _ => None,
        }
    }

    pub fn push(&self) -> Option<&PushPopup> {
        match self {
            Overlay::Push(popup) => Some(popup),
            _ => None,
        }
    }

    pub fn push_mut(&mut self) -> Option<&mut PushPopup> {
        match self {
            Overlay::Push(popup) => Some(popup),
            _ => None,
        }
    }

    pub fn git_op(&self) -> Option<&OpDialog> {
        match self {
            Overlay::GitOp(dialog) => Some(dialog),
            _ => None,
        }
    }

    pub fn git_op_mut(&mut self) -> Option<&mut OpDialog> {
        match self {
            Overlay::GitOp(dialog) => Some(dialog),
            _ => None,
        }
    }

    pub fn conflicts(&self) -> Option<&ConflictsPopup> {
        match self {
            Overlay::Conflicts(popup, _) => Some(popup),
            _ => None,
        }
    }

    pub fn conflicts_mut(&mut self) -> Option<&mut ConflictsPopup> {
        match self {
            Overlay::Conflicts(popup, _) => Some(popup),
            _ => None,
        }
    }

    pub fn merge(&self) -> Option<&MergeEditor> {
        match self {
            Overlay::Conflicts(_, merge) => merge.as_ref(),
            _ => None,
        }
    }

    pub fn merge_mut(&mut self) -> Option<&mut MergeEditor> {
        match self {
            Overlay::Conflicts(_, merge) => merge.as_mut(),
            _ => None,
        }
    }

    /// Opens the merge editor over the conflicts; without them it has nothing to resolve.
    pub fn set_merge(&mut self, editor: MergeEditor) {
        if let Overlay::Conflicts(_, merge) = self {
            *merge = Some(editor);
        }
    }

    pub fn close_merge(&mut self) {
        if let Overlay::Conflicts(_, merge) = self {
            *merge = None;
        }
    }

    pub fn close_branches(&mut self) {
        if matches!(self, Overlay::Branches(_)) {
            *self = Overlay::None;
        }
    }

    pub fn close_push(&mut self) {
        if matches!(self, Overlay::Push(_)) {
            *self = Overlay::None;
        }
    }

    pub fn close_git_op(&mut self) {
        if matches!(self, Overlay::GitOp(_)) {
            *self = Overlay::None;
        }
    }

    /// Closes the conflicts, and the merge editor over them.
    pub fn close_conflicts(&mut self) {
        if matches!(self, Overlay::Conflicts(..)) {
            *self = Overlay::None;
        }
    }
}
