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

impl BlameLine {
    /// `Molina, 8/21/26, 10:12 PM • added list and ready card creator`: the author's last name
    /// (or the whole name when it is one word), the date and the time in the author's own
    /// time zone, then the commit's summary.
    pub fn label(&self) -> String {
        if !self.committed {
            return "Not committed yet".into();
        }
        let name = self.author.split_whitespace().next_back().unwrap_or("");
        let local = self.time + i64::from(self.tz_minutes) * 60;
        let (year, month, day) = civil_from_days(local.div_euclid(86_400));
        let minutes = local.rem_euclid(86_400) / 60;
        let (hour, minute) = (minutes / 60, minutes % 60);
        let suffix = if hour < 12 { "AM" } else { "PM" };
        let hour = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!(
            "{name}, {month}/{day}/{:02}, {hour}:{minute:02} {suffix} • {}",
            year.rem_euclid(100),
            self.summary
        )
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
    fn an_uncommitted_line_says_so() {
        let mut line = line("Not Committed Yet", 0, 0);
        line.committed = false;
        assert_eq!(line.label(), "Not committed yet");
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
        assert_eq!(lines[2].label(), "Not committed yet");
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
