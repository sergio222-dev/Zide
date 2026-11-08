//! The history of the repository for the log tab: the commits (merges included) with the refs
//! that point at them, filtered by branch or by a search, and the lanes of the graph.

use std::path::Path;

use crate::{Commit, parse_offset, run};

/// What kind of ref a label is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
    /// HEAD when it points at no branch.
    Head,
}

/// A branch or tag that points at a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    /// The branch HEAD is on.
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub commit: Commit,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
}

/// Which history to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every branch and tag.
    #[default]
    All,
    Head,
    /// One ref, by its full name (`refs/heads/main`, `refs/remotes/origin/main`).
    Ref(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogQuery {
    pub scope: Scope,
    /// Text to look for in the message and the author, or the start of a hash. Empty: no filter.
    pub text: String,
    /// Only the commits whose author has this name or email. Empty: any.
    pub user: String,
    /// Only the commits newer than this, as git reads a date (`7 days ago`). Empty: any.
    pub since: String,
    /// Only the commits that touch one of these paths. Empty: any.
    pub paths: Vec<String>,
}

impl LogQuery {
    /// The history is cut down by more than the branch, so it is no longer a connected graph.
    pub fn is_filtered(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.user.is_empty()
            || !self.since.is_empty()
            || !self.paths.is_empty()
    }

    /// The options that limit by user and date.
    fn limits(&self) -> Vec<String> {
        let mut limits = Vec::new();
        if !self.user.is_empty() {
            limits.push(format!("--author={}", self.user));
        }
        if !self.since.is_empty() {
            limits.push(format!("--since={}", self.since));
        }
        limits
    }
}

/// Someone who made commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
    pub commits: usize,
}

/// Everyone who made commits on any branch, the most active first.
pub fn authors(root: &Path) -> Result<Vec<Author>, String> {
    let out = run(root, &["shortlog", "-sne", "--all"], false)?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (commits, who) = line.trim().split_once('\t')?;
            let (name, email) = who.rsplit_once(" <")?;
            Some(Author {
                name: name.to_string(),
                email: email.trim_end_matches('>').to_string(),
                commits: commits.trim().parse().ok()?,
            })
        })
        .collect())
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%D%x1f%an%x1f%ae%x1f%at%x1f%ad%x1f%s%x1f%b%x1e";

/// The newest `limit` commits of `query`, newest first. Without a text they keep the order the
/// graph needs (children before parents); with one, they are the matches by date.
pub fn read(root: &Path, query: &LogQuery, limit: usize) -> Result<Vec<LogEntry>, String> {
    let text = query.text.trim();
    let scope: Vec<&str> = match &query.scope {
        Scope::All => vec!["--all"],
        Scope::Head => vec!["HEAD"],
        Scope::Ref(name) => vec![name.as_str()],
    };
    let n = format!("-n{limit}");
    let limits = query.limits();
    let paths: Vec<&str> = std::iter::once("--")
        .chain(query.paths.iter().map(String::as_str))
        .collect();
    let base = |extra: &[&str], scope: &[&str]| {
        let mut args = vec!["log", "--decorate=full", "--date=format:%z", FORMAT, &n];
        args.extend(limits.iter().map(String::as_str));
        args.extend(extra);
        args.extend(scope);
        args.extend(&paths);
        run(root, &args, false).map(|out| parse(&out))
    };
    if text.is_empty() {
        return base(&["--topo-order"], &scope);
    }
    let grep = format!("--grep={text}");
    let flags = ["--regexp-ignore-case", "--fixed-strings"];
    let mut found = base(&[flags[0], flags[1], &grep], &scope)?;
    if query.user.is_empty() {
        // a second --author would not narrow the chosen one down, it would add to it
        let author = format!("--author={text}");
        found.extend(base(&[flags[0], flags[1], &author], &scope)?);
    }
    if text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        let spec = format!("{text}^{{commit}}");
        if let Ok(full) = run(root, &["rev-parse", "--verify", "--quiet", &spec], false) {
            // that commit alone, if it passes the same limits
            found.extend(base(&["--no-walk"], &[&full])?);
        }
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|entry| seen.insert(entry.commit.hash.clone())); // a commit matched twice
    found.sort_by_key(|entry| std::cmp::Reverse(entry.commit.time));
    found.truncate(limit);
    Ok(found)
}

fn parse(output: &str) -> Vec<LogEntry> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(9, '\x1f');
            let hash = fields.next().filter(|hash| !hash.is_empty())?;
            let parents = fields.next()?;
            let refs = fields.next()?;
            let (author, email) = (fields.next()?, fields.next()?);
            let time = fields.next()?.parse().ok()?;
            let tz = fields.next()?;
            let subject = fields.next()?;
            let body = fields.next().unwrap_or("");
            Some(LogEntry {
                commit: Commit {
                    hash: hash.to_string(),
                    author: author.to_string(),
                    email: email.to_string(),
                    time,
                    tz_minutes: parse_offset(tz),
                    subject: subject.to_string(),
                    body: body.trim().to_string(),
                },
                parents: parents.split_whitespace().map(str::to_string).collect(),
                refs: parse_refs(refs),
            })
        })
        .collect()
}

/// `HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1` (what `%D` gives with
/// `--decorate=full`) as labels, the branches before the tags.
fn parse_refs(decoration: &str) -> Vec<RefLabel> {
    let mut labels: Vec<RefLabel> = Vec::new();
    for part in decoration
        .split(", ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let (part, current) = match part.strip_prefix("HEAD -> ") {
            Some(rest) => (rest, true),
            None => (part, false),
        };
        let label = if let Some(tag) = part.strip_prefix("tag: ") {
            RefLabel {
                name: tag.strip_prefix("refs/tags/").unwrap_or(tag).to_string(),
                kind: RefKind::Tag,
                current,
            }
        } else if let Some(name) = part.strip_prefix("refs/heads/") {
            RefLabel {
                name: name.to_string(),
                kind: RefKind::Local,
                current,
            }
        } else if let Some(name) = part.strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            RefLabel {
                name: name.to_string(),
                kind: RefKind::Remote,
                current,
            }
        } else if part == "HEAD" {
            RefLabel {
                name: "HEAD".into(),
                kind: RefKind::Head,
                current: true,
            }
        } else {
            continue; // stash and the like
        };
        labels.push(label);
    }
    labels.sort_by_key(|label| match label.kind {
        RefKind::Head => 0,
        RefKind::Local => 1,
        RefKind::Remote => 2,
        RefKind::Tag => 3,
    });
    labels
}

/// The branches (local and remote) that contain the commit `hash`.
pub fn branches_with(root: &Path, hash: &str) -> Result<Vec<String>, String> {
    let out = run(
        root,
        &[
            "branch",
            "-a",
            "--contains",
            hash,
            "--format=%(refname:short)",
        ],
        false,
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.ends_with("/HEAD") && !name.starts_with('('))
        .map(str::to_string)
        .collect())
}

/// How one row of the graph is drawn. Lanes are columns, numbered from 0 on the left; a link
/// is `(lane, color)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphRow {
    /// The lane of the commit's dot, and its color.
    pub node: usize,
    pub color: usize,
    /// Lanes that run past the row, top to bottom.
    pub through: Vec<(usize, usize)>,
    /// Lanes that end at the dot, from above.
    pub top: Vec<(usize, usize)>,
    /// Lanes that start at the dot and go down: one for each parent.
    pub bottom: Vec<(usize, usize)>,
}

impl GraphRow {
    /// The columns the row uses.
    pub fn width(&self) -> usize {
        self.through
            .iter()
            .chain(&self.top)
            .chain(&self.bottom)
            .map(|(lane, _)| *lane)
            .chain([self.node])
            .max()
            .unwrap_or(0)
            + 1
    }
}

struct Lane {
    /// The commit this lane runs to.
    hash: String,
    color: usize,
}

/// The lanes of the graph for `entries`, which must come children before parents (topological
/// order). A lane belongs to the commit it is waiting for; a commit takes the lanes waiting for
/// it, and gives one to each of its parents.
pub fn graph(entries: &[LogEntry]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut colors = 0;
    let mut rows = Vec::with_capacity(entries.len());
    let mut take_color = || {
        colors += 1;
        colors - 1
    };
    let free = |lanes: &mut Vec<Option<Lane>>| match lanes.iter().position(Option::is_none) {
        Some(at) => at,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    for entry in entries {
        let hash = &entry.commit.hash;
        let waiting: Vec<usize> = (0..lanes.len())
            .filter(|&i| lanes[i].as_ref().is_some_and(|lane| lane.hash == *hash))
            .collect();
        let (node, color) = match waiting.first() {
            Some(&at) => (at, lanes[at].as_ref().map_or(0, |lane| lane.color)),
            None => (free(&mut lanes), take_color()),
        };
        let through = (0..lanes.len())
            .filter(|i| !waiting.contains(i))
            .filter_map(|i| lanes[i].as_ref().map(|lane| (i, lane.color)))
            .collect();
        let top = waiting
            .iter()
            .filter_map(|&i| lanes[i].as_ref().map(|lane| (i, lane.color)))
            .collect();
        for &i in &waiting {
            lanes[i] = None;
        }
        let mut bottom = Vec::new();
        for (k, parent) in entry.parents.iter().enumerate() {
            let existing = lanes
                .iter()
                .position(|lane| lane.as_ref().is_some_and(|lane| lane.hash == *parent));
            if let Some(at) = existing {
                bottom.push((at, lanes[at].as_ref().map_or(0, |lane| lane.color)));
            } else if k == 0 {
                lanes[node] = Some(Lane {
                    hash: parent.clone(),
                    color,
                });
                bottom.push((node, color));
            } else {
                let at = free(&mut lanes);
                let color = take_color();
                lanes[at] = Some(Lane {
                    hash: parent.clone(),
                    color,
                });
                bottom.push((at, color));
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow {
            node,
            color,
            through,
            top,
            bottom,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            commit: Commit {
                hash: hash.into(),
                author: "Ada".into(),
                email: "ada@x.org".into(),
                time: 0,
                tz_minutes: 0,
                subject: hash.into(),
                body: String::new(),
            },
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: Vec::new(),
        }
    }

    #[test]
    fn a_straight_history_stays_in_one_lane() {
        let rows = graph(&[entry("c", &["b"]), entry("b", &["a"]), entry("a", &[])]);
        assert!(
            rows.iter()
                .all(|row| row.node == 0 && row.through.is_empty())
        );
        assert_eq!(rows[0].bottom, [(0, 0)]);
        assert_eq!(rows[1].top, [(0, 0)]);
        assert!(rows[2].bottom.is_empty());
    }

    #[test]
    fn a_merge_opens_a_lane_for_its_second_parent_and_the_branch_point_joins_them() {
        //   m (a, b)   merge
        //   |\
        //   a b
        //   |/
        //   r
        let rows = graph(&[
            entry("m", &["a", "b"]),
            entry("a", &["r"]),
            entry("b", &["r"]),
            entry("r", &[]),
        ]);
        assert_eq!(rows[0].bottom, [(0, 0), (1, 1)]); // two lanes leave the merge
        assert_eq!((rows[1].node, rows[1].through.clone()), (0, vec![(1, 1)]));
        assert_eq!(rows[2].node, 1);
        assert_eq!(rows[2].through, [(0, 0)]); // `a`'s lane waits for `r` beside it
        assert_eq!(rows[2].bottom, [(0, 0)]); // `b` joins that lane
        assert_eq!(rows[3].top, [(0, 0)]);
    }

    #[test]
    fn two_lanes_waiting_for_the_same_commit_end_at_it() {
        let rows = graph(&[entry("x", &["r"]), entry("y", &["r"]), entry("r", &[])]);
        // `y` finds `r` already waited for in lane 0 and joins it from lane 1
        assert_eq!(rows[1].node, 1);
        assert_eq!(rows[1].bottom, [(0, 0)]);
        assert_eq!(rows[2].top, [(0, 0)]);
    }

    #[test]
    fn decorations_become_labels_with_the_branches_first() {
        let labels = parse_refs(
            "HEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main, \
             refs/remotes/origin/HEAD, refs/stash",
        );
        let names: Vec<_> = labels
            .iter()
            .map(|l| (l.name.as_str(), l.kind, l.current))
            .collect();
        assert_eq!(
            names,
            [
                ("main", RefKind::Local, true),
                ("origin/main", RefKind::Remote, false),
                ("v1", RefKind::Tag, false),
            ]
        );
    }
}
