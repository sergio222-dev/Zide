//! The stash: what is set aside, and bringing it back.

use std::path::Path;

use crate::{run, run_raw};

/// One entry of `git stash list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// Its place in the list, `stash@{index}`; 0 is the latest.
    pub index: usize,
    /// The commit that holds it, to read its files and patches.
    pub hash: String,
    /// The branch it was made on.
    pub branch: String,
    /// What the user called it; for an unnamed stash, git's `<hash> <subject>` of where it was made.
    pub message: String,
}

/// The stashes of the repository at `root`, the latest first.
pub fn stashes(root: &Path) -> Result<Vec<Stash>, String> {
    let list = run_raw(root, &["stash", "list", "--format=%H%x1f%gs"])?;
    Ok(list
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let (hash, subject) = line.split_once('\x1f')?;
            let (branch, message) = parse_subject(subject);
            Some(Stash {
                index,
                hash: hash.to_string(),
                branch,
                message,
            })
        })
        .collect())
}

/// `On main: message` (a named stash) and `WIP on main: abc1234 subject` give the branch and the
/// message; anything else is all message.
fn parse_subject(subject: &str) -> (String, String) {
    let rest = subject
        .strip_prefix("WIP on ")
        .or_else(|| subject.strip_prefix("On "));
    match rest.and_then(|rest| rest.split_once(": ")) {
        Some((branch, message)) => (branch.to_string(), message.to_string()),
        None => (String::new(), subject.to_string()),
    }
}

fn name(index: usize) -> String {
    format!("stash@{{{index}}}")
}

/// Sets the changes aside under `message` (git names it after the commit when empty).
/// `keep_index` leaves what is staged in place; `untracked` takes the new files too.
pub fn push(root: &Path, message: &str, keep_index: bool, untracked: bool) -> Result<(), String> {
    let mut args = vec!["stash", "push"];
    if keep_index {
        args.push("--keep-index");
    }
    if untracked {
        args.push("--include-untracked");
    }
    if !message.is_empty() {
        args.extend(["-m", message]);
    }
    // with nothing to save git says so and still succeeds
    match run(root, &args, false)?.starts_with("No local changes") {
        true => Err("There are no local changes to stash".into()),
        false => Ok(()),
    }
}

/// Applies the stash and keeps it in the list.
pub fn apply(root: &Path, index: usize) -> Result<(), String> {
    run(root, &["stash", "apply", &name(index)], false).map(drop)
}

/// Applies the stash and removes it from the list (git keeps it when applying clashes).
pub fn pop(root: &Path, index: usize) -> Result<(), String> {
    run(root, &["stash", "pop", &name(index)], false).map(drop)
}

/// Removes the stash from the list.
pub fn drop_stash(root: &Path, index: usize) -> Result<(), String> {
    run(root, &["stash", "drop", &name(index)], false).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn subjects_give_the_branch_and_the_message() {
        let named = parse_subject("On feat/x: asd?");
        assert_eq!(named, ("feat/x".into(), "asd?".into()));
        let unnamed = parse_subject("WIP on main: abc1234 first");
        assert_eq!(unnamed, ("main".into(), "abc1234 first".into()));
        assert_eq!(parse_subject("odd"), (String::new(), "odd".into()));
    }

    #[test]
    fn stashes_are_listed_applied_popped_and_dropped() {
        let dir = std::env::temp_dir().join(format!("zide-stash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-q", "-m", "first"]);
        assert_eq!(stashes(&dir), Ok(vec![]));
        assert!(push(&dir, "", false, false).is_err()); // nothing changed yet

        fs::write(dir.join("a.txt"), "two\n").unwrap();
        git_in(&dir, &["stash", "push", "-q", "-m", "older"]);
        fs::write(dir.join("a.txt"), "three\n").unwrap();
        git_in(&dir, &["stash", "push", "-q", "-m", "newer"]);
        let found = stashes(&dir).unwrap();
        let shown: Vec<_> = found
            .iter()
            .map(|s| (s.index, s.branch.as_str(), s.message.as_str()))
            .collect();
        assert_eq!(shown, [(0, "main", "newer"), (1, "main", "older")]);
        assert_eq!(
            crate::commit_files(&dir, &found[0].hash).unwrap()[0].path,
            "a.txt"
        );

        fs::write(dir.join("b.txt"), "new\n").unwrap();
        fs::write(dir.join("a.txt"), "four\n").unwrap();
        assert_eq!(push(&dir, "mine", false, true), Ok(()));
        assert!(!dir.join("b.txt").exists()); // the untracked file went too
        assert_eq!(stashes(&dir).unwrap()[0].message, "mine");
        assert_eq!(drop_stash(&dir, 0), Ok(()));

        assert_eq!(apply(&dir, 1), Ok(()));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "two\n");
        assert_eq!(stashes(&dir).unwrap().len(), 2); // apply keeps it
        git_in(&dir, &["checkout", "-q", "--", "."]);
        assert_eq!(pop(&dir, 1), Ok(()));
        assert_eq!(stashes(&dir).unwrap().len(), 1);
        assert_eq!(drop_stash(&dir, 0), Ok(()));
        assert!(stashes(&dir).unwrap().is_empty());
        assert!(drop_stash(&dir, 0).is_err()); // nothing left to drop
        fs::remove_dir_all(dir).unwrap();
    }
}
