//! What git knows of a file: who last changed each line (blame) and what changed since the last
//! commit.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use similar::{Algorithm, DiffOp, TextDiff};

/// How a line of the file differs from the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange {
    /// A line that is not in the commit.
    Added,
    /// A line that replaces other lines of the commit.
    Modified,
    /// Lines of the commit were deleted just above this line.
    Removed,
    /// Lines of the commit were deleted after the last line of the file.
    RemovedBelow,
}

/// The file as of the last commit. `None` when git is missing, `root` is not a repository or the
/// file is not in the commit (untracked): there is nothing to compare with.
pub fn head_content(root: &Path, relative: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(format!("HEAD:./{relative}"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The lines of `new` that differ from `old`, by 0-based line. A diff that takes too long to
/// compute reports nothing rather than stalling the editor.
pub fn line_changes(old: &str, new: &str) -> HashMap<usize, LineChange> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(Duration::from_millis(100))
        .diff_lines(old, new);
    let total = new.lines().count();
    let mut changes = HashMap::new();
    let removed = |changes: &mut HashMap<usize, LineChange>, at: usize| {
        if at < total {
            changes.entry(at).or_insert(LineChange::Removed);
        } else if total == 0 {
            changes.entry(0).or_insert(LineChange::Removed);
        } else {
            changes.entry(total - 1).or_insert(LineChange::RemovedBelow);
        }
    };
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for line in new_index..new_index + new_len {
                    changes.insert(line, LineChange::Added);
                }
            }
            DiffOp::Delete { new_index, .. } => removed(&mut changes, new_index),
            DiffOp::Replace {
                old_len,
                new_index,
                new_len,
                ..
            } => {
                // as many lines changed as there were; the rest are new
                for line in new_index..new_index + new_len {
                    let kind = if line - new_index < old_len {
                        LineChange::Modified
                    } else {
                        LineChange::Added
                    };
                    changes.insert(line, kind);
                }
            }
        }
    }
    changes
}

/// For each line of `new`, the line of `old` it is (`None` for a line that is not in `old`).
/// A diff that takes too long to compute maps every line to `None`.
pub fn line_map(old: &str, new: &str) -> Vec<Option<usize>> {
    let mut map = vec![None; new.lines().count()];
    if old == new {
        map.iter_mut().enumerate().for_each(|(i, to)| *to = Some(i));
        return map;
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(Duration::from_millis(100))
        .diff_lines(old, new);
    for op in diff.ops() {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = *op
        {
            for k in 0..len {
                map[new_index + k] = Some(old_index + k);
            }
        }
    }
    map
}

/// Who last changed one line, as `git blame` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub author: String,
    /// Unix time of the change, and the author's UTC offset in minutes.
    pub time: i64,
    pub tz_minutes: i32,
    pub summary: String,
    /// False for a line that is not in any commit yet.
    pub committed: bool,
}

/// What the gutter says of a line that is not in any commit.
pub const UNCOMMITTED_LABEL: &str = "You, Today \u{2022} Uncommitted changes";

impl BlameLine {
    /// `Molina, 8/21/26, 10:12 PM • added list and ready card creator`: the author's last name
    /// (or the whole name when it is one word), the date and the time in the author's own
    /// time zone, then the commit's summary.
    pub fn label(&self) -> String {
        if !self.committed {
            return UNCOMMITTED_LABEL.into();
        }
        let name = self.author.split_whitespace().next_back().unwrap_or("");
        let (date, clock) = date_and_time(self.time, self.tz_minutes);
        format!("{name}, {date}, {clock} • {}", self.summary)
    }
}

/// `8/21/26` and `10:12 PM` for a Unix time seen from a UTC offset in minutes.
pub fn date_and_time(time: i64, tz_minutes: i32) -> (String, String) {
    let local = time + i64::from(tz_minutes) * 60;
    let (year, month, day) = civil_from_days(local.div_euclid(86_400));
    let minutes = local.rem_euclid(86_400) / 60;
    let (hour, minute) = (minutes / 60, minutes % 60);
    let suffix = if hour < 12 { "AM" } else { "PM" };
    let hour = match hour % 12 {
        0 => 12,
        h => h,
    };
    (
        format!("{month}/{day}/{:02}", year.rem_euclid(100)),
        format!("{hour}:{minute:02} {suffix}"),
    )
}

/// Blame of `relative` (a path under `root`), one entry per line of the file on disk. `None`
/// when git is missing, `root` is not a repository or the file is not tracked.
pub fn blame(root: &Path, relative: &str) -> Option<Vec<BlameLine>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["blame", "--line-porcelain", "--", relative])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `git blame --line-porcelain`: per line a header (`<sha> <orig> <final> [count]`), the
/// commit's fields, and the line's text after a tab.
fn parse(porcelain: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut current = BlameLine {
        author: String::new(),
        time: 0,
        tz_minutes: 0,
        summary: String::new(),
        committed: true,
    };
    for line in porcelain.lines() {
        if line.starts_with('\t') {
            lines.push(current.clone());
        } else if let Some(author) = line.strip_prefix("author ") {
            current.author = author.to_string();
        } else if let Some(time) = line.strip_prefix("author-time ") {
            current.time = time.parse().unwrap_or(0);
        } else if let Some(tz) = line.strip_prefix("author-tz ") {
            current.tz_minutes = parse_tz(tz);
        } else if let Some(summary) = line.strip_prefix("summary ") {
            current.summary = summary.to_string();
        } else if let Some(sha) = line.split(' ').next().filter(|s| s.len() == 40) {
            current.committed = sha.bytes().any(|b| b != b'0');
        }
    }
    lines
}

/// `+0200` -> 120, `-0330` -> -210.
fn parse_tz(tz: &str) -> i32 {
    let sign = if tz.starts_with('-') { -1 } else { 1 };
    let digits = tz.trim_start_matches(['+', '-']);
    let (Some(hours), Some(minutes)) = (digits.get(..2), digits.get(2..4)) else {
        return 0;
    };
    sign * (hours.parse().unwrap_or(0) * 60 + minutes.parse().unwrap_or(0))
}

/// Year, month and day of a day count since 1970-01-01 (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Where the checked-out branch stands against the one it tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchStatus {
    /// The branch name, or the short commit when HEAD is detached.
    pub name: String,
    /// Whether it has an upstream to compare with; without one `ahead` and `behind` are 0.
    pub tracked: bool,
    /// Commits to push.
    pub ahead: u32,
    /// Commits to pull.
    pub behind: u32,
}

/// The branch of the repository at `root` and how far it is from its upstream, as of the last
/// fetch. `None` when git is missing or `root` is not a repository.
pub fn branch_status(root: &Path) -> Option<BranchStatus> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_branch_status(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

/// Reads the `# branch.*` headers of `git status --porcelain=v2 --branch`.
fn parse_branch_status(output: &str) -> Option<BranchStatus> {
    let header = |key: &str| {
        output.lines().find_map(|line| {
            line.strip_prefix("# ")?
                .strip_prefix(key)?
                .strip_prefix(' ')
        })
    };
    let name = match header("branch.head")? {
        "(detached)" => header("branch.oid")?.chars().take(7).collect(),
        head => head.to_string(),
    };
    let counts = header("branch.ab").and_then(|ab| {
        let (ahead, behind) = ab.split_once(' ')?;
        Some((
            ahead.strip_prefix('+')?.parse().ok()?,
            behind.strip_prefix('-')?.parse().ok()?,
        ))
    });
    Some(BranchStatus {
        name,
        tracked: counts.is_some(),
        ahead: counts.map_or(0, |(ahead, _)| ahead),
        behind: counts.map_or(0, |(_, behind)| behind),
    })
}

/// A local branch and the remote branch it tracks (`origin/main`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBranch {
    pub name: String,
    pub upstream: Option<String>,
}

/// The branches of a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branches {
    /// The checked-out branch; `None` when HEAD is detached.
    pub current: Option<String>,
    pub local: Vec<LocalBranch>,
    /// Remote-tracking branches with their remote (`origin/feat/x`), `HEAD` aliases left out.
    pub remote: Vec<String>,
    /// Local branches checked out lately, the latest first.
    pub recent: Vec<String>,
}

impl Branches {
    /// One line per fact, `kind<TAB>name[<TAB>upstream]`, to keep between runs.
    pub fn to_cache(&self) -> String {
        let mut out = format!("current\t{}\n", self.current.as_deref().unwrap_or(""));
        for branch in &self.local {
            let upstream = branch.upstream.as_deref().unwrap_or("");
            out += &format!("local\t{}\t{upstream}\n", branch.name);
        }
        for remote in &self.remote {
            out += &format!("remote\t{remote}\n");
        }
        for recent in &self.recent {
            out += &format!("recent\t{recent}\n");
        }
        out
    }

    /// What [`Branches::to_cache`] wrote; lines it does not understand are skipped.
    pub fn from_cache(text: &str) -> Self {
        let mut branches = Self::default();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(kind), Some(name)) = (fields.next(), fields.next()) else {
                continue;
            };
            match kind {
                "current" if !name.is_empty() => branches.current = Some(name.to_string()),
                "local" if !name.is_empty() => branches.local.push(LocalBranch {
                    name: name.to_string(),
                    upstream: fields.next().filter(|u| !u.is_empty()).map(str::to_string),
                }),
                "remote" if !name.is_empty() => branches.remote.push(name.to_string()),
                "recent" if !name.is_empty() => branches.recent.push(name.to_string()),
                _ => {}
            }
        }
        branches
    }
}

/// How many recently checked-out branches are kept.
const RECENT_BRANCHES: usize = 5;

/// The branches of the repository at `root`; `None` when git is missing or `root` is none.
pub fn branches(root: &Path) -> Option<Branches> {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let refs = git(&[
        "for-each-ref",
        "--format=%(refname)\t%(upstream:short)\t%(HEAD)",
        "refs/heads",
        "refs/remotes",
    ])?;
    let reflog = git(&["reflog", "show", "--format=%gs", "-n", "300"]).unwrap_or_default();
    Some(parse_branches(&refs, &reflog))
}

/// Reads `for-each-ref` lines (`refname<TAB>upstream<TAB>HEAD marker`) and the subjects of the
/// reflog of HEAD, whose checkouts say which branches were visited.
fn parse_branches(refs: &str, reflog: &str) -> Branches {
    let mut branches = Branches::default();
    for line in refs.lines() {
        let mut fields = line.split('\t');
        let (Some(name), upstream, marker) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if let Some(local) = name.strip_prefix("refs/heads/") {
            if marker == Some("*") {
                branches.current = Some(local.to_string());
            }
            branches.local.push(LocalBranch {
                name: local.to_string(),
                upstream: upstream.filter(|u| !u.is_empty()).map(str::to_string),
            });
        } else if let Some(remote) = name.strip_prefix("refs/remotes/")
            && remote.contains('/')
            && !remote.ends_with("/HEAD")
        {
            branches.remote.push(remote.to_string());
        }
    }
    for subject in reflog.lines() {
        let Some(moved) = subject.strip_prefix("checkout: moving from ") else {
            continue;
        };
        let Some((_, to)) = moved.split_once(" to ") else {
            continue;
        };
        if branches.local.iter().any(|b| b.name == to)
            && !branches.recent.iter().any(|r| r == to)
            && branches.recent.len() < RECENT_BRANCHES
        {
            branches.recent.push(to.to_string());
        }
    }
    branches
}

/// Runs git in `root`; the error is what git said. `network` commands never ask for a password
/// on a terminal that is not there: they fail instead of hanging.
fn run(root: &Path, args: &[&str], network: bool) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(std::process::Stdio::null());
    if network {
        command.env("GIT_TERMINAL_PROMPT", "0");
    }
    let output = command
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if said.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or(""))
        } else {
            said
        })
    }
}

/// Like [`run`] but the output is returned as git wrote it: a patch loses its last line, and a
/// `-z` list its separators, when trimmed.
fn run_raw(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Why `name` cannot be the name of a new local branch: empty, taken by one of `existing`, or
/// not a name git accepts (spaces, `..`, a trailing `/`...).
pub fn check_new_branch_name(root: &Path, name: &str, existing: &[String]) -> Result<(), String> {
    if name.is_empty() {
        return Err("The branch name is empty".into());
    }
    if existing.iter().any(|e| e == name) {
        return Err(format!("A branch named '{name}' already exists"));
    }
    // `--branch` also expands things like `@{-1}`, which are no names
    if name.starts_with('@') && name != "@"
        || run(root, &["check-ref-format", "--branch", name], false).is_err()
    {
        return Err(format!("'{name}' is not a valid branch name"));
    }
    Ok(())
}

/// Creates the branch `name` at `start` (a branch, local or remote) and checks it out; from a
/// remote branch it tracks it.
pub fn create_branch(root: &Path, name: &str, start: &str) -> Result<(), String> {
    run(root, &["switch", "-c", name, start], false).map(drop)
}

/// Renames the local branch `old` to `new`. A tracked remote branch keeps its name.
pub fn rename_branch(root: &Path, old: &str, new: &str) -> Result<(), String> {
    run(root, &["branch", "-m", old, new], false).map(drop)
}

/// Brings the local branch `name` up to date with its upstream, only when that moves it forward
/// (no merge, no rebase, nothing lost). The checked-out branch is pulled; another one is fetched
/// into place.
pub fn update_branch(root: &Path, name: &str, checked_out: bool) -> Result<(), String> {
    let config = |key: &str| {
        run(
            root,
            &["config", "--get", &format!("branch.{name}.{key}")],
            false,
        )
    };
    let (Ok(remote), Ok(merge)) = (config("remote"), config("merge")) else {
        return Err(format!("'{name}' has no upstream branch to update from"));
    };
    if checked_out {
        run(root, &["pull", "--ff-only"], true).map(drop)
    } else {
        let source = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
        run(root, &["fetch", &remote, &format!("{source}:{name}")], true).map(drop)
    }
}

/// The remote `name` is pushed to: the one configured for it, the one it tracks, `origin`, or
/// the only remote there is.
pub fn push_remote(root: &Path, name: &str) -> Result<String, String> {
    let config = |key: &str| run(root, &["config", "--get", key], false).ok();
    if let Some(remote) = config(&format!("branch.{name}.pushRemote"))
        .or_else(|| config("remote.pushDefault"))
        .or_else(|| config(&format!("branch.{name}.remote")))
        .filter(|remote| remote != ".")
    {
        return Ok(remote);
    }
    let remotes = run(root, &["remote"], false)?;
    let remotes: Vec<&str> = remotes.lines().collect();
    match remotes.as_slice() {
        [] => Err("This repository has no remote to push to".into()),
        [only] => Ok(only.to_string()),
        _ if remotes.contains(&"origin") => Ok("origin".into()),
        _ => Err("There is more than one remote and none is set for this branch".into()),
    }
}

/// Pushes the local branch `name` to `remote`, which then tracks it. Never forced.
pub fn push_branch(root: &Path, name: &str, remote: &str) -> Result<(), String> {
    run(root, &["push", "--set-upstream", remote, name], true).map(drop)
}

/// Pushes every tag to `remote`.
pub fn push_tags(root: &Path, remote: &str) -> Result<(), String> {
    run(root, &["push", "--tags", remote], true).map(drop)
}

/// What to check out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// A local branch.
    Local(String),
    /// A remote-tracking branch (`origin/feat/x`): its local branch, created to track it when
    /// there is none yet.
    Remote(String),
}

/// Checks out `target` in the repository at `root`. The error is what git said (for instance
/// that local changes would be overwritten), for the user to read.
pub fn switch(root: &Path, target: &Switch) -> Result<(), String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| format!("git could not run: {e}"))
    };
    let output = match target {
        Switch::Local(name) => git(&["switch", name])?,
        Switch::Remote(full) => {
            let short = full
                .split_once('/')
                .map_or(full.as_str(), |(_, short)| short);
            let exists = git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{short}"),
            ])?
            .status
            .success();
            if exists {
                git(&["switch", short])?
            } else {
                git(&["switch", "--track", full])?
            }
        }
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    #[test]
    fn the_cache_text_gives_back_the_same_branches() {
        let branches = Branches {
            current: Some("feat/x".into()),
            local: vec![
                LocalBranch {
                    name: "main".into(),
                    upstream: Some("origin/main".into()),
                },
                LocalBranch {
                    name: "feat/x".into(),
                    upstream: None,
                },
            ],
            remote: vec!["origin/main".into()],
            recent: vec!["feat/x".into(), "main".into()],
        };
        assert_eq!(Branches::from_cache(&branches.to_cache()), branches);
        let detached = Branches::default();
        assert_eq!(Branches::from_cache(&detached.to_cache()), detached);
        // damaged lines are skipped, not fatal
        assert_eq!(
            Branches::from_cache("junk\nlocal\n\tx\n"),
            Branches::default()
        );
    }

    #[test]
    fn branches_are_read_from_refs_and_the_reflog() {
        let refs = "refs/heads/main\torigin/main\t*\nrefs/heads/feat/x\t\t \n\
                    refs/remotes/origin/HEAD\t\t \nrefs/remotes/origin/main\t\t \n\
                    refs/remotes/origin/feat/y\t\t \n";
        let reflog = "checkout: moving from feat/x to main\ncommit: work\n\
                      checkout: moving from main to feat/x\ncheckout: moving from a1b2c3 to main\n\
                      checkout: moving from main to gone\n";
        let found = parse_branches(refs, reflog);
        assert_eq!(found.current.as_deref(), Some("main"));
        assert_eq!(
            found.local,
            [
                LocalBranch {
                    name: "main".into(),
                    upstream: Some("origin/main".into())
                },
                LocalBranch {
                    name: "feat/x".into(),
                    upstream: None
                },
            ]
        );
        assert_eq!(found.remote, ["origin/main", "origin/feat/y"]);
        // latest first, each once, and only branches that still exist
        assert_eq!(found.recent, ["main", "feat/x"]);
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    /// An origin with `main`, and a clone of it.
    fn origin_and_clone(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("zide-{tag}-{}", std::process::id()));
        let (origin, clone) = (base.join("origin"), base.join("clone"));
        fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git_in(&origin, &["config", "receive.denyCurrentBranch", "ignore"]);
        git_in(
            &base,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        (base, clone)
    }

    #[test]
    fn a_new_branch_name_must_be_free_and_valid() {
        let (base, clone) = origin_and_clone("names");
        let existing = vec!["main".to_string()];
        let check = |name: &str| check_new_branch_name(&clone, name, &existing);
        assert_eq!(check("feat/x"), Ok(()));
        assert!(check("").unwrap_err().contains("empty"));
        assert!(check("main").unwrap_err().contains("already exists"));
        for bad in ["a b", "x..y", "/x", "x/", "x.lock", "@{-1}", "-x"] {
            assert!(check(bad).unwrap_err().contains("not a valid"), "{bad}");
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn branches_are_created_renamed_updated_and_pushed() {
        let (base, clone) = origin_and_clone("ops");
        let name = |dir: &Path| branch_status(dir).unwrap().name;

        // new branch from another one, checked out
        assert_eq!(create_branch(&clone, "feat/a", "main"), Ok(()));
        assert_eq!(name(&clone), "feat/a");
        assert!(create_branch(&clone, "feat/a", "main").is_err());

        // rename
        assert_eq!(rename_branch(&clone, "feat/a", "feat/b"), Ok(()));
        assert_eq!(name(&clone), "feat/b");
        assert!(rename_branch(&clone, "feat/b", "main").is_err());

        // push: the remote is found, the branch then tracks it
        assert_eq!(push_remote(&clone, "feat/b"), Ok("origin".to_string()));
        assert_eq!(push_branch(&clone, "feat/b", "origin"), Ok(()));
        assert!(branch_status(&clone).unwrap().tracked);

        // update a branch that is not checked out (`feat/b` is), moving it forward only
        let origin = base.join("origin");
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "second"]);
        assert_eq!(update_branch(&clone, "main", false), Ok(()));
        let head = |dir: &Path, rev: &str| run(dir, &["rev-parse", rev], false).unwrap();
        assert_eq!(head(&clone, "main"), head(&origin, "main"));
        // and the checked-out one
        git_in(&clone, &["switch", "-q", "main"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "third"]);
        assert_eq!(update_branch(&clone, "main", true), Ok(()));
        assert_eq!(head(&clone, "main"), head(&origin, "main"));
        // a branch without upstream has nothing to update from
        git_in(&clone, &["branch", "lonely"]);
        assert!(
            update_branch(&clone, "lonely", false)
                .unwrap_err()
                .contains("no upstream")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn commit_records_are_read_with_their_body_and_zone() {
        let output = "aaaaaaaaaaaa\x1fAda Lovelace\x1fada@x.org\x1f1700000000\x1f-0330\x1fsubject\x1fbody line\nsecond\n\x1e\nbbbbbbbbbbbb\x1fB\x1fb@x.org\x1f1\x1f+0200\x1fonly subject\x1f\x1e";
        let commits = parse_commits(output);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_hash(), "aaaaaaaa");
        assert_eq!(commits[0].author, "Ada Lovelace");
        assert_eq!(commits[0].tz_minutes, -210);
        assert_eq!(commits[0].body, "body line\nsecond");
        assert_eq!(commits[1].subject, "only subject");
        assert_eq!(commits[1].body, "");
        assert_eq!(commits[1].tz_minutes, 120);
        assert!(parse_commits("").is_empty());
    }

    #[test]
    fn dates_read_like_the_push_dialog_shows_them() {
        // 07:53 UTC is 04:53 in a zone 3 hours behind
        assert_eq!(
            date_and_time(1_794_037_980, -180),
            ("11/7/26".to_string(), "4:53 AM".to_string())
        );
        assert_eq!(
            date_and_time(0, 0),
            ("1/1/70".to_string(), "12:00 AM".to_string())
        );
    }

    #[test]
    fn outgoing_lists_what_the_remote_lacks_and_their_files() {
        let (base, clone) = origin_and_clone("outgoing");
        let nothing = outgoing(&clone).unwrap();
        assert_eq!((nothing.branch.as_str(), nothing.on_remote), ("main", true));
        assert!(nothing.commits.is_empty());

        fs::write(clone.join("a.txt"), "one\n").unwrap();
        fs::create_dir_all(clone.join("src")).unwrap();
        fs::write(clone.join("src/b.rs"), "fn b() {}\n").unwrap();
        git_in(&clone, &["add", "."]);
        git_in(&clone, &["commit", "-q", "-m", "first\n\nlonger text"]);
        fs::write(clone.join("a.txt"), "one\ntwo\n").unwrap();
        git_in(&clone, &["commit", "-q", "-am", "second"]);

        let found = outgoing(&clone).unwrap();
        let subjects: Vec<_> = found.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["second", "first"]);
        assert_eq!(found.commits[1].body, "longer text");

        let files = commit_files(&clone, &found.commits[1].hash).unwrap();
        let listed: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.kind)).collect();
        assert_eq!(
            listed,
            [
                ("a.txt", ChangeKind::Added),
                ("src/b.rs", ChangeKind::Added)
            ]
        );
        let modified = commit_files(&clone, &found.commits[0].hash).unwrap();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].kind, ChangeKind::Modified);
        let patch = commit_diff(&clone, &found.commits[0].hash, &modified[0]).unwrap();
        assert!(patch.contains("+two"), "{patch}");

        // a branch the remote does not have yet: every commit it holds over the remote's
        git_in(&clone, &["switch", "-q", "-c", "feat/new"]);
        let fresh = outgoing(&clone).unwrap();
        assert!(!fresh.on_remote);
        assert_eq!(fresh.commits.len(), 2);

        // pushing leaves nothing outgoing
        assert_eq!(push_branch(&clone, "feat/new", "origin"), Ok(()));
        git_in(&clone, &["tag", "v1"]);
        assert_eq!(push_tags(&clone, "origin"), Ok(()));
        assert!(outgoing(&clone).unwrap().commits.is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_final_newline_that_appears_is_not_a_changed_line() {
        let (base, clone) = origin_and_clone("eol");
        fs::write(clone.join("t.js"), "prueba").unwrap();
        git_in(&clone, &["add", "."]);
        git_in(&clone, &["commit", "-q", "-m", "a"]);
        fs::write(clone.join("t.js"), "prueba\n\n").unwrap();
        git_in(&clone, &["commit", "-q", "-am", "b"]);
        let head = run(&clone, &["rev-parse", "HEAD"], false).unwrap();
        let files = commit_files(&clone, &head).unwrap();
        let patch = commit_diff(&clone, &head, &files[0]).unwrap();
        assert!(patch.contains("\n prueba\n+\n"), "{patch}");
        assert!(!patch.contains("-prueba"), "{patch}");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn outgoing_refuses_a_detached_head() {
        let (base, clone) = origin_and_clone("detached");
        git_in(&clone, &["switch", "-q", "--detach"]);
        assert!(outgoing(&clone).unwrap_err().contains("detached"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_push_remote_is_none_without_remotes() {
        let dir = std::env::temp_dir().join(format!("zide-noremote-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        assert!(push_remote(&dir, "main").unwrap_err().contains("no remote"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn switching_checks_out_local_branches_and_tracks_remote_ones() {
        let base = std::env::temp_dir().join(format!("zide-switch-{}", std::process::id()));
        let (origin, clone) = (base.join("origin"), base.join("clone"));
        fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git_in(&origin, &["branch", "feat/y"]);
        git_in(
            &base,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        git_in(&clone, &["branch", "local-only"]);

        assert_eq!(switch(&clone, &Switch::Local("local-only".into())), Ok(()));
        assert_eq!(branch_status(&clone).unwrap().name, "local-only");
        // no local `feat/y` yet: it is created and tracks the remote one
        assert_eq!(
            switch(&clone, &Switch::Remote("origin/feat/y".into())),
            Ok(())
        );
        let status = branch_status(&clone).unwrap();
        assert_eq!((status.name.as_str(), status.tracked), ("feat/y", true));
        // now it exists: the remote entry just goes to it
        git_in(&clone, &["switch", "-q", "main"]);
        assert_eq!(
            switch(&clone, &Switch::Remote("origin/feat/y".into())),
            Ok(())
        );
        assert_eq!(branch_status(&clone).unwrap().name, "feat/y");

        let listed = branches(&clone).unwrap();
        assert_eq!(listed.current.as_deref(), Some("feat/y"));
        assert!(listed.remote.contains(&"origin/main".to_string()));
        assert!(!listed.remote.iter().any(|r| r.ends_with("HEAD")));
        assert_eq!(listed.recent.first().map(String::as_str), Some("feat/y"));

        assert!(switch(&clone, &Switch::Local("missing".into())).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_branch_status_reads_the_headers_of_git_status() {
        let status = |text| parse_branch_status(text);
        let diverged = "# branch.oid abc\n# branch.head feat/x\n# branch.upstream origin/feat/x\n# branch.ab +4 -2\n";
        assert_eq!(
            status(diverged),
            Some(BranchStatus {
                name: "feat/x".into(),
                tracked: true,
                ahead: 4,
                behind: 2
            })
        );
        let local = "# branch.oid abc\n# branch.head wip\n";
        assert_eq!(
            status(local),
            Some(BranchStatus {
                name: "wip".into(),
                tracked: false,
                ahead: 0,
                behind: 0
            })
        );
        let detached = "# branch.oid 0123456789abcdef\n# branch.head (detached)\n";
        assert_eq!(status(detached).map(|b| b.name), Some("0123456".into()));
        assert_eq!(status(""), None);
    }

    #[test]
    fn branch_status_of_a_folder_that_is_no_repository_is_none() {
        let dir = std::env::temp_dir().join(format!("zide-nogit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(branch_status(&dir), None);
        fs::remove_dir_all(dir).unwrap();
    }

    fn line(author: &str, time: i64, tz_minutes: i32) -> BlameLine {
        BlameLine {
            author: author.into(),
            time,
            tz_minutes,
            summary: "added list".into(),
            committed: true,
        }
    }

    #[test]
    fn label_has_last_name_date_pm_time_and_summary() {
        // 2026-08-21 22:12 UTC-3 = 2026-08-22 01:12 UTC
        let at = 1_787_361_120;
        assert_eq!(
            line("Sergio Molina", at, -180).label(),
            "Molina, 8/21/26, 10:12 PM • added list"
        );
    }

    #[test]
    fn label_uses_am_and_twelve_for_midnight_and_noon() {
        let midnight = 1_787_356_800; // 2026-08-22 00:00 UTC
        assert_eq!(
            line("Ana", midnight, 0).label(),
            "Ana, 8/22/26, 12:00 AM • added list"
        );
        assert_eq!(
            line("Ana", midnight + 12 * 3600, 0).label(),
            "Ana, 8/22/26, 12:00 PM • added list"
        );
    }

    #[test]
    fn a_line_map_follows_lines_through_insertions_and_edits() {
        let map = line_map("a\nb\nc\n", "a\nnew\nb\nC\n");
        assert_eq!(map, [Some(0), None, Some(1), None]);
        assert_eq!(line_map("a\nb\n", "a\nb\n"), [Some(0), Some(1)]);
    }

    #[test]
    fn an_uncommitted_line_says_so() {
        let mut line = line("Not Committed Yet", 0, 0);
        line.committed = false;
        assert_eq!(line.label(), UNCOMMITTED_LABEL);
    }

    #[test]
    fn parses_every_line_with_its_own_commit() {
        let sha = "a".repeat(40);
        let zero = "0".repeat(40);
        let text = format!(
            "{sha} 1 1 1\nauthor Sergio Molina\nauthor-time 1787361120\nauthor-tz -0300\n\
             summary first\nfilename f\n\tone\n\
             {zero} 2 2 1\nauthor Not Committed Yet\nauthor-time 5\nauthor-tz +0000\n\
             summary Version of f\nfilename f\n\ttwo\n"
        );
        let lines = parse(&text);
        assert_eq!(lines.len(), 2);
        let mut first = line("Sergio Molina", 1_787_361_120, -180);
        first.summary = "first".into();
        assert_eq!(lines[0], first);
        assert!(!lines[1].committed);
    }

    #[test]
    fn blames_a_real_repository() {
        let dir = std::env::temp_dir().join(format!("zide-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args([
                    "-c",
                    "user.name=Sergio Molina",
                    "-c",
                    "user.email=s@example.com",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "first commit"]);
        fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();

        let lines = blame(&dir, "f.txt").unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].label().starts_with("Molina, "));
        assert!(lines[0].label().ends_with("• first commit"));
        assert_eq!(lines[2].label(), UNCOMMITTED_LABEL);
        assert!(blame(&dir, "missing.txt").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod change_tests {
    use super::*;

    fn changes(old: &str, new: &str) -> Vec<(usize, LineChange)> {
        let mut all: Vec<_> = line_changes(old, new).into_iter().collect();
        all.sort_by_key(|(line, _)| *line);
        all
    }

    #[test]
    fn identical_text_has_no_changes() {
        assert!(changes("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn new_lines_are_added() {
        assert_eq!(
            changes("a\nd\n", "a\nb\nc\nd\n"),
            [(1, LineChange::Added), (2, LineChange::Added)]
        );
    }

    #[test]
    fn rewritten_lines_are_modified_and_the_surplus_is_added() {
        assert_eq!(
            changes("a\nb\nc\n", "a\nB\nc\n"),
            [(1, LineChange::Modified)]
        );
        assert_eq!(
            changes("a\nb\n", "a\nB\nC\n"),
            [(1, LineChange::Modified), (2, LineChange::Added)]
        );
    }

    #[test]
    fn deleted_lines_mark_the_line_below_them() {
        assert_eq!(changes("a\nb\nc\n", "a\nc\n"), [(1, LineChange::Removed)]);
        assert_eq!(changes("a\nb\nc\n", "a\n"), [(0, LineChange::RemovedBelow)]);
        assert_eq!(changes("a\n", ""), [(0, LineChange::Removed)]);
    }
}

/// How a file differs from the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Not tracked by git yet.
    Untracked,
    /// Both sides changed it and the merge is not resolved.
    Conflicted,
}

/// A file with changes to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository root, with `/` separators.
    pub path: String,
    /// Where a renamed file came from: committing the rename needs both paths.
    pub from: Option<String>,
    pub kind: ChangeKind,
}

/// The files that differ from the last commit, staged or not, untracked ones included, sorted by
/// path. Empty when git is missing or `root` is not a repository.
pub fn changes(root: &Path) -> Vec<FileChange> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut changes = parse_changes(&String::from_utf8_lossy(&output.stdout));
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// Reads `git status --porcelain=v1 -z`: `XY path\0`, and for renames and copies `\0from` after.
fn parse_changes(output: &str) -> Vec<FileChange> {
    let mut entries = output.split('\0');
    let mut changes = Vec::new();
    while let Some(entry) = entries.next() {
        let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        let mut chars = code.chars();
        let (x, y) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));
        let renamed = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
        let from = renamed
            .then(|| entries.next().map(str::to_string))
            .flatten();
        let kind = match (x, y) {
            ('?', '?') => ChangeKind::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => ChangeKind::Conflicted,
            _ if renamed => ChangeKind::Renamed,
            ('A', _) => ChangeKind::Added,
            ('D', _) | (_, 'D') => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };
        changes.push(FileChange {
            path: path.to_string(),
            from,
            kind,
        });
    }
    changes
}

/// The patch of `change` against the last commit, whole file included (not only the lines around
/// the changes) so the viewer can fold what did not change. An untracked file is all additions.
pub fn diff(root: &Path, change: &FileChange) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--unified=1000000",
        "--ignore-space-at-eol",
    ]);
    if change.kind == ChangeKind::Untracked {
        command.args(["--no-index", "--", "/dev/null", &change.path]);
    } else {
        command.args(["HEAD", "-M", "--"]);
        command.args(change.from.iter().chain([&change.path]));
    }
    let output = command.output().map_err(|why| why.to_string())?;
    // `--no-index` exits with 1 when the files differ
    match output.status.code() {
        Some(0 | 1) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// A commit, as the push dialog lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// Unix time of the commit, and the author's UTC offset in minutes.
    pub time: i64,
    pub tz_minutes: i32,
    /// The first line of the message.
    pub subject: String,
    /// The rest of the message.
    pub body: String,
}

impl Commit {
    pub fn short_hash(&self) -> &str {
        self.hash.get(..8).unwrap_or(&self.hash)
    }
}

/// What pushing the checked-out branch would publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub branch: String,
    pub remote: String,
    /// Whether the remote has the branch already; if not, the push creates it.
    pub on_remote: bool,
    /// The commits the remote lacks, newest first.
    pub commits: Vec<Commit>,
}

/// The commits that a push of the checked-out branch would send.
pub fn outgoing(root: &Path) -> Result<Outgoing, String> {
    let branch = run(root, &["symbolic-ref", "--short", "-q", "HEAD"], false)
        .map_err(|_| "HEAD is detached: there is no branch to push".to_string())?;
    let remote = push_remote(root, &branch)?;
    let tracking = format!("refs/remotes/{remote}/{branch}");
    let on_remote = run(root, &["rev-parse", "--verify", "-q", &tracking], false).is_ok();
    let format = "--format=%H%x1f%an%x1f%ae%x1f%at%x1f%ad%x1f%s%x1f%b%x1e";
    let mut args = vec!["log", "--date=format:%z", format];
    let range;
    let remotes;
    if on_remote {
        range = format!("{remote}/{branch}..HEAD");
        args.push(&range);
    } else {
        // a new branch: whatever no branch of the remote has yet
        remotes = format!("--remotes={remote}");
        args.extend(["HEAD", "--not", &remotes]);
    }
    let commits = parse_commits(&run(root, &args, false)?);
    Ok(Outgoing {
        branch,
        remote,
        on_remote,
        commits,
    })
}

/// Reads the `%H|%an|%ae|%at|%ad(%z)|%s|%b` records of [`outgoing`], separated by 0x1f and 0x1e.
fn parse_commits(output: &str) -> Vec<Commit> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(7, '\x1f');
            let hash = fields.next().filter(|hash| !hash.is_empty())?;
            let (author, email) = (fields.next()?, fields.next()?);
            let time = fields.next()?.parse().ok()?;
            let tz = fields.next()?;
            let subject = fields.next()?;
            let body = fields.next().unwrap_or("");
            Some(Commit {
                hash: hash.to_string(),
                author: author.to_string(),
                email: email.to_string(),
                time,
                tz_minutes: parse_offset(tz),
                subject: subject.to_string(),
                body: body.trim().to_string(),
            })
        })
        .collect()
}

/// `+0200` is 120 minutes, `-0330` is -210.
fn parse_offset(zone: &str) -> i32 {
    let digits = zone.get(1..).and_then(|d| d.parse::<i32>().ok());
    let Some(digits) = digits else {
        return 0;
    };
    let minutes = digits / 100 * 60 + digits % 100;
    if zone.starts_with('-') {
        -minutes
    } else {
        minutes
    }
}

/// The files `hash` changed, against its first parent (the whole commit when it has none), sorted
/// by path.
pub fn commit_files(root: &Path, hash: &str) -> Result<Vec<FileChange>, String> {
    let output = run_raw(
        root,
        &[
            "show",
            "--format=",
            "--name-status",
            "-M",
            "-z",
            "--diff-merges=first-parent",
            hash,
        ],
    )?;
    let mut fields = output.split('\0');
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let Some(letter) = status.chars().next() else {
            continue;
        };
        let renamed = matches!(letter, 'R' | 'C');
        let (from, path) = if renamed {
            (fields.next(), fields.next())
        } else {
            (None, fields.next())
        };
        let Some(path) = path else {
            break;
        };
        let kind = match letter {
            'A' => ChangeKind::Added,
            'D' => ChangeKind::Deleted,
            'R' | 'C' => ChangeKind::Renamed,
            'U' => ChangeKind::Conflicted,
            _ => ChangeKind::Modified,
        };
        changes.push(FileChange {
            path: path.to_string(),
            from: from.map(str::to_string),
            kind,
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// The patch of `change` in the commit `hash`, whole file included, like [`diff`].
pub fn commit_diff(root: &Path, hash: &str, change: &FileChange) -> Result<String, String> {
    let mut args = vec![
        "show",
        "--format=",
        "--no-color",
        "--no-ext-diff",
        "--unified=1000000",
        "--ignore-space-at-eol",
        "-M",
        "--diff-merges=first-parent",
        hash,
        "--",
    ];
    args.extend(change.from.iter().map(String::as_str));
    args.push(&change.path);
    run_raw(root, &args)
}

/// Starts tracking `path` (a file, or every file under a folder) without committing it.
pub fn add(root: &Path, path: &str) -> Result<(), String> {
    run(root, &["--literal-pathspecs", "add", "--", path], false).map(drop)
}

/// Commits `paths` (and nothing else, whatever else is staged) with `message`. `amend` folds them
/// into the last commit; an empty message then keeps that commit's.
pub fn commit(root: &Path, paths: &[String], message: &str, amend: bool) -> Result<(), String> {
    let message = message.trim();
    if paths.is_empty() && !amend {
        return Err("There is nothing selected to commit".into());
    }
    if message.is_empty() && !amend {
        return Err("The commit message is empty".into());
    }
    if !paths.is_empty() {
        let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
        add.extend(paths.iter().map(String::as_str));
        run(root, &add, false)?;
    }
    let mut commit = vec!["--literal-pathspecs", "commit", "-q"];
    if amend {
        commit.push("--amend");
    }
    if message.is_empty() {
        commit.push("--no-edit");
    } else {
        commit.extend(["-m", message]);
    }
    if !paths.is_empty() {
        commit.push("--");
        commit.extend(paths.iter().map(String::as_str));
    }
    run(root, &commit, false).map(drop)
}

#[cfg(test)]
mod commit_tests {
    use super::*;
    use std::fs;

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Sergio Molina"],
            vec!["config", "user.email", "s@example.com"],
        ] {
            run(&dir, &args, false).unwrap();
        }
        dir
    }

    #[test]
    fn add_tracks_a_new_file_or_the_files_of_a_folder() {
        let dir = repo("add");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("one.txt"), "1").unwrap();
        fs::write(dir.join("src/a.rs"), "a").unwrap();
        fs::write(dir.join("src/b.rs"), "b").unwrap();
        let kinds = |dir: &Path| {
            changes(dir)
                .into_iter()
                .map(|c| (c.path, c.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(add(&dir, "one.txt"), Ok(()));
        assert_eq!(add(&dir, "src"), Ok(()));
        assert!(
            kinds(&dir)
                .iter()
                .all(|(_, kind)| *kind == ChangeKind::Added)
        );
        assert!(add(&dir, "missing.txt").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_lists_every_kind_of_change_and_the_rename_origin() {
        let output = " M a.rs\0A  b.rs\0 D c.rs\0R  new.rs\0old.rs\0?? dir/u.rs\0UU m.rs\0";
        let changes = parse_changes(output);
        let kinds: Vec<_> = changes.iter().map(|c| (c.path.as_str(), c.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("a.rs", ChangeKind::Modified),
                ("b.rs", ChangeKind::Added),
                ("c.rs", ChangeKind::Deleted),
                ("new.rs", ChangeKind::Renamed),
                ("dir/u.rs", ChangeKind::Untracked),
                ("m.rs", ChangeKind::Conflicted),
            ]
        );
        assert_eq!(changes[3].from.as_deref(), Some("old.rs"));
        assert!(parse_changes("").is_empty());
    }

    #[test]
    fn diff_has_the_whole_file_for_tracked_and_untracked_changes() {
        let dir = repo("diff");
        let lines: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        fs::write(dir.join("a.txt"), &lines).unwrap();
        run(&dir, &["add", "."], false).unwrap();
        run(&dir, &["commit", "-qm", "first"], false).unwrap();
        fs::write(dir.join("a.txt"), lines.replace("line 10\n", "ten\n")).unwrap();
        fs::write(dir.join("new.txt"), "hello\n").unwrap();

        let change = |path: &str, kind| FileChange {
            path: path.into(),
            from: None,
            kind,
        };
        let tracked = diff(&dir, &change("a.txt", ChangeKind::Modified)).unwrap();
        assert!(tracked.contains("-line 10") && tracked.contains("+ten"));
        assert!(tracked.contains(" line 1\n") && tracked.contains(" line 20\n"));
        let untracked = diff(&dir, &change("new.txt", ChangeKind::Untracked)).unwrap();
        assert!(untracked.contains("+hello"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn commit_takes_only_the_selected_files_and_refuses_empty_input() {
        let dir = repo("commit");
        fs::write(dir.join("keep.txt"), "1").unwrap();
        run(&dir, &["add", "keep.txt"], false).unwrap();
        run(&dir, &["commit", "-q", "-m", "base"], false).unwrap();
        fs::write(dir.join("keep.txt"), "2").unwrap(); // modified, not selected
        run(&dir, &["add", "keep.txt"], false).unwrap(); // even staged
        fs::write(dir.join("new.txt"), "n").unwrap(); // untracked, selected

        assert!(commit(&dir, &[], "msg", false).is_err());
        assert!(commit(&dir, &["new.txt".into()], "  ", false).is_err());
        commit(&dir, &["new.txt".into()], "add new", false).unwrap();

        let left = changes(&dir);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, "keep.txt");
        assert_eq!(
            run(&dir, &["log", "-1", "--format=%s"], false).unwrap(),
            "add new"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn amend_folds_files_in_and_an_empty_message_keeps_the_old_one() {
        let dir = repo("amend");
        fs::write(dir.join("a.txt"), "1").unwrap();
        commit(&dir, &["a.txt".into()], "first", false).unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        commit(&dir, &["b.txt".into()], "", true).unwrap();
        assert_eq!(run(&dir, &["log", "--format=%s"], false).unwrap(), "first");
        assert_eq!(
            run(&dir, &["show", "--name-only", "--format="], false).unwrap(),
            "a.txt\nb.txt"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
