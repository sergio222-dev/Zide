//! What the git log offers on a commit: cherry-pick it, revert it, reset the branch to it and tag
//! it.

use std::path::Path;

use crate::run;

/// What `git reset` does to the index and the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetMode {
    /// Only moves the branch; what was committed after stays staged.
    Soft,
    /// Moves the branch and empties the index; the files keep their changes.
    #[default]
    Mixed,
    /// Moves the branch and makes the files match: uncommitted changes are lost.
    Hard,
    /// Like hard, but stops instead of overwriting uncommitted changes.
    Keep,
}

impl ResetMode {
    pub const ALL: [ResetMode; 4] = [Self::Soft, Self::Mixed, Self::Hard, Self::Keep];

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Mixed => "Mixed",
            Self::Hard => "Hard",
            Self::Keep => "Keep",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Soft => {
                "Files stay as they are and the changes of the dropped commits stay staged"
            }
            Self::Mixed => {
                "Files stay as they are and the changes of the dropped commits become unstaged"
            }
            Self::Hard => "Files are reset to that commit: every uncommitted change is lost",
            Self::Keep => {
                "Files are reset to that commit, keeping uncommitted changes (stops if they clash)"
            }
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
            Self::Keep => "--keep",
        }
    }
}

/// The options a merge commit needs to be applied or undone: its changes are taken against the
/// first parent.
fn mainline(root: &Path, hash: &str) -> Result<Vec<&'static str>, String> {
    let parents = run(root, &["rev-list", "--parents", "-n", "1", hash], false)?;
    // the commit itself, then its parents
    Ok(if parents.split_whitespace().count() > 2 {
        vec!["-m", "1"]
    } else {
        Vec::new()
    })
}

/// Applies the changes of `hash` on top of the checked-out branch as a new commit.
pub fn cherry_pick(root: &Path, hash: &str) -> Result<(), String> {
    let mut args = vec!["cherry-pick"];
    args.extend(mainline(root, hash)?);
    args.push(hash);
    run(root, &args, false).map(drop)
}

/// Adds a commit that undoes the changes of `hash`.
pub fn revert(root: &Path, hash: &str) -> Result<(), String> {
    let mut args = vec!["revert", "--no-edit"];
    args.extend(mainline(root, hash)?);
    args.push(hash);
    run(root, &args, false).map(drop)
}

/// Moves the checked-out branch to `hash`.
pub fn reset(root: &Path, hash: &str, mode: ResetMode) -> Result<(), String> {
    run(root, &["reset", mode.flag(), hash], false).map(drop)
}

/// Why `name` cannot be the name of a new tag: empty, taken, or not a name git accepts.
pub fn check_new_tag_name(root: &Path, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("The tag name is empty".into());
    }
    let full = format!("refs/tags/{name}");
    if run(root, &["check-ref-format", &full], false).is_err() {
        return Err(format!("'{name}' is not a valid tag name"));
    }
    if run(root, &["rev-parse", "--verify", "--quiet", &full], false).is_ok() {
        return Err(format!("A tag named '{name}' already exists"));
    }
    Ok(())
}

/// Creates the tag `name` on `hash`.
pub fn create_tag(root: &Path, name: &str, hash: &str) -> Result<(), String> {
    run(root, &["tag", name, hash], false).map(drop)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository with `a.txt`, one commit.
    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-ops-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        commit(&dir, "a.txt", "one", "first");
        dir
    }

    fn commit(dir: &Path, file: &str, text: &str, message: &str) -> String {
        fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn subjects(dir: &Path) -> Vec<String> {
        git(dir, &["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[::core::prelude::v1::test]
    fn a_cherry_pick_copies_a_commit_and_a_merge_is_taken_against_its_first_parent() {
        let dir = repo("pick");
        git(&dir, &["switch", "-q", "-c", "side"]);
        let side = commit(&dir, "b.txt", "b", "side work");
        git(&dir, &["switch", "-q", "main"]);
        commit(&dir, "c.txt", "c", "main work");
        cherry_pick(&dir, &side).unwrap();
        assert_eq!(subjects(&dir), ["side work", "main work", "first"]);
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b");

        // a merge commit: its first parent is the mainline
        git(&dir, &["switch", "-q", "-c", "target", "HEAD~2"]);
        git(
            &dir,
            &["merge", "-q", "--no-ff", "-m", "Merge side", "side"],
        );
        let merge = git(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["switch", "-q", "main"]);
        git(&dir, &["rm", "-q", "b.txt"]);
        git(&dir, &["commit", "-q", "-m", "drop b"]);
        cherry_pick(&dir, &merge).unwrap();
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_cherry_pick_that_clashes_says_why_and_leaves_the_pick_under_way() {
        let dir = repo("clash");
        git(&dir, &["switch", "-q", "-c", "side"]);
        let side = commit(&dir, "a.txt", "side", "side edit");
        git(&dir, &["switch", "-q", "main"]);
        commit(&dir, "a.txt", "main", "main edit");
        assert!(cherry_pick(&dir, &side).is_err());
        assert!(dir.join(".git/CHERRY_PICK_HEAD").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_revert_adds_a_commit_that_undoes_the_changes() {
        let dir = repo("revert");
        let second = commit(&dir, "b.txt", "b", "add b");
        revert(&dir, &second).unwrap();
        assert!(!dir.join("b.txt").exists());
        assert_eq!(subjects(&dir)[0], "Revert \"add b\"");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn each_reset_mode_leaves_the_index_and_the_files_as_it_promises() {
        let dir = repo("reset");
        let first = git(&dir, &["rev-parse", "HEAD"]);
        let state = |dir: &Path| {
            (
                git(dir, &["status", "--porcelain"]),
                fs::read_to_string(dir.join("b.txt")).ok(),
            )
        };
        for (mode, status, file) in [
            (ResetMode::Soft, "A  b.txt", Some("b")),
            (ResetMode::Mixed, "?? b.txt", Some("b")),
            (ResetMode::Hard, "", None),
            (ResetMode::Keep, "", None),
        ] {
            commit(&dir, "b.txt", "b", "add b");
            reset(&dir, &first, mode).unwrap();
            assert_eq!(
                state(&dir),
                (status.to_string(), file.map(str::to_string)),
                "{mode:?}"
            );
            assert_eq!(git(&dir, &["rev-parse", "HEAD"]), first);
            git(&dir, &["clean", "-fdq"]);
            git(&dir, &["reset", "-q", "--hard"]);
        }
        // keep refuses to overwrite what is uncommitted
        commit(&dir, "a.txt", "two", "edit a");
        fs::write(dir.join("a.txt"), "dirty").unwrap();
        assert!(reset(&dir, &first, ResetMode::Keep).is_err());
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "dirty");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_tag_name_is_checked_and_the_tag_lands_on_the_commit() {
        let dir = repo("tag");
        let first = git(&dir, &["rev-parse", "HEAD"]);
        commit(&dir, "b.txt", "b", "second");
        assert!(check_new_tag_name(&dir, "").is_err());
        assert!(check_new_tag_name(&dir, "bad name").is_err());
        assert_eq!(check_new_tag_name(&dir, "v1"), Ok(()));
        create_tag(&dir, "v1", &first).unwrap();
        assert_eq!(git(&dir, &["rev-parse", "v1^{commit}"]), first);
        assert!(
            check_new_tag_name(&dir, "v1")
                .unwrap_err()
                .contains("already exists")
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
