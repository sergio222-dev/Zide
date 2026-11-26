//! What the git log offers on a commit: cherry-pick it, revert it, reset the branch to it and tag
//! it.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{ChangeKind, FileChange, run, run_raw};

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

/// The changes `hash` made to `files`, as a patch (a merge commit against its first parent).
fn patch_of(root: &Path, hash: &str, files: &[FileChange]) -> Result<String, String> {
    let mut args = vec![
        "--literal-pathspecs",
        "show",
        "--format=",
        "--no-color",
        "--no-ext-diff",
        "--binary",
        "-M",
        "--diff-merges=first-parent",
        hash,
        "--",
    ];
    for file in files {
        args.extend(file.from.iter().map(String::as_str));
        args.push(&file.path);
    }
    run_raw(root, &args)
}

/// Applies the changes `hash` made to `files` to the working tree, or with `reverse` undoes them
/// there. Nothing is staged and nothing is committed.
pub fn apply_changes(
    root: &Path,
    hash: &str,
    files: &[FileChange],
    reverse: bool,
) -> Result<(), String> {
    let patch = patch_of(root, hash, files)?;
    if patch.trim().is_empty() {
        return Err("The commit has no changes in those files".into());
    }
    let file = std::env::temp_dir().join(format!(
        "zide-patch-{}-{}.diff",
        std::process::id(),
        hash.get(..8).unwrap_or(hash)
    ));
    fs::write(&file, patch).map_err(|e| e.to_string())?;
    let path = file.to_string_lossy().into_owned();
    let mut args = vec!["apply", "--whitespace=nowarn"];
    if reverse {
        args.push("-R");
    }
    args.push(&path);
    let result = run(root, &args, false).map(drop);
    fs::remove_file(&file).ok();
    result
}

/// Puts `files` as they are in `hash` into the working tree (a file the commit deleted is
/// deleted). Nothing is staged and nothing is committed.
pub fn get_from_revision(root: &Path, hash: &str, files: &[FileChange]) -> Result<(), String> {
    // read everything first, so a failure leaves the files as they were
    let mut writes = Vec::new();
    for file in files {
        if file.kind == ChangeKind::Deleted {
            writes.push((file, None));
            continue;
        }
        let spec = format!("{hash}:{}", file.path);
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "blob", &spec])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("git could not run: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        writes.push((file, Some(out.stdout)));
    }
    for (file, content) in writes {
        let target = root.join(&file.path);
        match content {
            Some(bytes) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&target, bytes).map_err(|e| format!("{}: {e}", file.path))?;
            }
            None => match fs::remove_file(&target) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: {e}", file.path));
                }
                _ => {}
            },
        }
    }
    Ok(())
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

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            from: None,
            kind,
        }
    }

    #[::core::prelude::v1::test]
    fn the_changes_of_some_files_are_applied_or_undone_in_the_working_tree_only() {
        let dir = repo("files");
        fs::write(dir.join("b.txt"), "b").unwrap();
        fs::write(dir.join("c.txt"), "c").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "add b and c"]);
        fs::write(dir.join("a.txt"), "two").unwrap();
        fs::write(dir.join("b.txt"), "bb").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "edit a and b"]);
        let edit = git(&dir, &["rev-parse", "HEAD"]);
        let a_and_b = [
            change("a.txt", ChangeKind::Modified),
            change("b.txt", ChangeKind::Modified),
        ];

        // revert only a.txt: the files change, the index and the history do not
        apply_changes(&dir, &edit, &a_and_b[..1], true).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "bb");
        assert_eq!(git(&dir, &["diff", "--name-only"]), "a.txt"); // not staged
        assert_eq!(git(&dir, &["diff", "--cached", "--name-only"]), "");
        assert_eq!(subjects(&dir)[0], "edit a and b");
        // and back again: cherry-picking the same changes
        apply_changes(&dir, &edit, &a_and_b[..1], false).unwrap();
        assert_eq!(git(&dir, &["status", "--porcelain"]), "");
        // two files at once
        apply_changes(&dir, &edit, &a_and_b, true).unwrap();
        assert_eq!(git(&dir, &["status", "--porcelain"]).lines().count(), 2);
        // what does not apply says so and changes nothing
        assert!(apply_changes(&dir, &edit, &a_and_b, true).is_err());
        // a file the commit did not touch has nothing to apply
        assert!(
            apply_changes(&dir, &edit, &[change("c.txt", ChangeKind::Modified)], true).is_err()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn getting_files_from_a_revision_writes_them_without_staging_and_deletes_removed_ones() {
        let dir = repo("get");
        let first = git(&dir, &["rev-parse", "HEAD"]);
        fs::write(dir.join("a.txt"), "two").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/new.txt"), "new").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "second"]);
        let second = git(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["rm", "-q", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "third"]);
        let third = git(&dir, &["rev-parse", "HEAD"]);

        // the file as it was in the second commit, and a file inside a folder that is gone
        fs::remove_dir_all(dir.join("sub")).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "fourth"]);
        get_from_revision(
            &dir,
            &second,
            &[
                change("a.txt", ChangeKind::Modified),
                change("sub/new.txt", ChangeKind::Added),
            ],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "two");
        assert_eq!(fs::read_to_string(dir.join("sub/new.txt")).unwrap(), "new");
        assert_eq!(git(&dir, &["status", "--porcelain"]), "?? a.txt\n?? sub/");
        // a file the commit deleted is deleted
        get_from_revision(&dir, &third, &[change("a.txt", ChangeKind::Deleted)]).unwrap();
        assert!(!dir.join("a.txt").exists());
        // a path that is not in that revision stops before touching anything
        assert!(
            get_from_revision(&dir, &first, &[change("sub/new.txt", ChangeKind::Added)]).is_err()
        );
        assert!(dir.join("sub/new.txt").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
