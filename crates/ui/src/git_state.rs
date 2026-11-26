//! What the workspace knows of the repository: the checked-out branch and the state of every
//! changed file. Read in the background by the workspace; asked here by whoever needs it (the
//! status bar, the tree's colors, the commit panel, the git dialogs, the actions on offer).

use std::collections::HashMap;
use std::sync::Arc;

use zide_git::{BranchStatus, ChangeKind, FileChange, Operation};

#[derive(Default)]
pub struct GitState {
    /// `None` outside a repository.
    branch: Option<BranchStatus>,
    /// By path relative to the root; shared with the tree, which draws its colors from it.
    status: Arc<HashMap<String, ChangeKind>>,
}

impl GitState {
    pub fn branch(&self) -> Option<&BranchStatus> {
        self.branch.as_ref()
    }

    /// The checked-out branch (the short commit when HEAD is detached); empty outside a repository.
    pub fn branch_name(&self) -> String {
        self.branch
            .as_ref()
            .map(|branch| branch.name.clone())
            .unwrap_or_default()
    }

    pub fn is_checked_out(&self, name: &str) -> bool {
        self.branch
            .as_ref()
            .is_some_and(|branch| branch.name == name)
    }

    /// A merge is under way: committing concludes it.
    pub fn merging(&self) -> bool {
        self.branch
            .as_ref()
            .and_then(|branch| branch.progress.as_ref())
            .is_some_and(|progress| progress.operation == Operation::Merge)
    }

    /// Takes what git says of the branch now; whether it changed.
    pub fn set_branch(&mut self, branch: Option<BranchStatus>) -> bool {
        let changed = self.branch != branch;
        self.branch = branch;
        changed
    }

    pub fn status(&self) -> Arc<HashMap<String, ChangeKind>> {
        self.status.clone()
    }

    /// Some file is left with conflicts to resolve.
    pub fn has_conflicts(&self) -> bool {
        self.status
            .values()
            .any(|kind| *kind == ChangeKind::Conflicted)
    }

    /// Takes the changes git reports; whether that changed if there are conflicts.
    pub fn set_changes(&mut self, changes: &[FileChange]) -> bool {
        let before = self.has_conflicts();
        self.status = Arc::new(
            changes
                .iter()
                .map(|change| (change.path.clone(), change.kind))
                .collect(),
        );
        before != self.has_conflicts()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zide_git::Progress;

    fn branch(name: &str, operation: Option<Operation>) -> BranchStatus {
        BranchStatus {
            name: name.into(),
            tracked: false,
            ahead: 0,
            behind: 0,
            progress: operation.map(|operation| Progress {
                operation,
                theirs: "feature".into(),
            }),
        }
    }

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            from: None,
            kind,
        }
    }

    #[test]
    fn outside_a_repository_there_is_no_branch_and_nothing_under_way() {
        let git = GitState::default();
        assert!(git.branch().is_none());
        assert_eq!(git.branch_name(), "");
        assert!(!git.is_checked_out("main"));
        assert!(!git.merging());
        assert!(!git.has_conflicts());
    }

    #[test]
    fn the_branch_names_what_is_checked_out() {
        let mut git = GitState::default();
        git.set_branch(Some(branch("main", None)));
        assert_eq!(git.branch_name(), "main");
        assert!(git.is_checked_out("main"));
        assert!(!git.is_checked_out("feature"));
    }

    #[test]
    fn only_a_merge_under_way_is_merging() {
        let mut git = GitState::default();
        git.set_branch(Some(branch("main", Some(Operation::Merge))));
        assert!(git.merging());
        git.set_branch(Some(branch("main", Some(Operation::Rebase))));
        assert!(!git.merging());
        git.set_branch(Some(branch("main", None)));
        assert!(!git.merging());
    }

    #[test]
    fn setting_the_branch_says_whether_it_changed() {
        let mut git = GitState::default();
        assert!(git.set_branch(Some(branch("main", None))));
        assert!(!git.set_branch(Some(branch("main", None))));
        assert!(git.set_branch(Some(branch("feature", None))));
        assert!(git.set_branch(None));
        assert!(git.branch().is_none());
    }

    #[test]
    fn the_changes_give_each_path_its_kind() {
        let mut git = GitState::default();
        git.set_changes(&[
            change("a.rs", ChangeKind::Modified),
            change("b.rs", ChangeKind::Added),
        ]);
        let status = git.status();
        assert_eq!(status.len(), 2);
        assert_eq!(status.get("a.rs"), Some(&ChangeKind::Modified));
        assert_eq!(status.get("b.rs"), Some(&ChangeKind::Added));
        git.set_changes(&[]);
        assert!(
            git.status().is_empty(),
            "the old changes are replaced, not added to"
        );
    }

    #[test]
    fn setting_the_changes_says_whether_conflicts_came_or_went() {
        let mut git = GitState::default();
        assert!(!git.set_changes(&[change("a.rs", ChangeKind::Modified)]));
        assert!(git.set_changes(&[change("a.rs", ChangeKind::Conflicted)]));
        assert!(git.has_conflicts());
        assert!(!git.set_changes(&[
            change("a.rs", ChangeKind::Conflicted),
            change("b.rs", ChangeKind::Modified),
        ]));
        assert!(git.set_changes(&[change("b.rs", ChangeKind::Modified)]));
        assert!(!git.has_conflicts());
    }
}
