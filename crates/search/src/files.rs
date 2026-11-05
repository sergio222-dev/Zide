use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use zide_index::FileIndex;
use zide_matcher::{FixingLayoutMatcher, Matcher, MatchingMode, RussianToEnglish};

/// A file search query: what to match, and the line to open (`Foo.rs:42`).
#[derive(Debug, PartialEq, Eq)]
pub struct FileQuery<'a> {
    pub pattern: &'a str,
    pub line: Option<u32>,
}

impl<'a> FileQuery<'a> {
    pub fn parse(input: &'a str) -> Self {
        if let Some((pattern, line)) = input.rsplit_once(':')
            && !line.is_empty()
            && let Ok(line) = line.parse()
        {
            return Self {
                pattern,
                line: Some(line),
            };
        }
        Self {
            pattern: input,
            line: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    /// Index into the [`FileIndex`].
    pub index: usize,
    pub score: i32,
}

/// Fuzzy file search over names and paths, best matches first.
///
/// `dir/Name` matches `Name` against the file name and `dir` against the directories,
/// so `lang/Goto` finds `lang-impl/src/GotoFileAction.java`.
pub fn search_files(index: &FileIndex, query: &FileQuery<'_>, limit: usize) -> Vec<FileHit> {
    search_files_cancellable(index, query, limit, &AtomicBool::new(false)).unwrap_or_default()
}

/// Like [`search_files`], but gives up with `None` as soon as `cancel` is set.
pub fn search_files_cancellable(
    index: &FileIndex,
    query: &FileQuery<'_>,
    limit: usize,
    cancel: &AtomicBool,
) -> Option<Vec<FileHit>> {
    let pattern = query.pattern.trim();
    if pattern.is_empty() || limit == 0 {
        return Some(Vec::new());
    }
    let (dir_pattern, name_pattern) = match pattern.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, pattern),
    };
    let name_matcher = navigation_matcher(name_pattern);
    let dir_matcher = dir_pattern
        .filter(|d| !d.is_empty())
        .map(navigation_matcher);

    let mut hits: Vec<FileHit> = index
        .paths()
        .par_iter()
        .enumerate()
        .filter_map(|(i, path)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            score_path(path, &name_matcher, dir_matcher.as_ref())
                .map(|score| FileHit { index: i, score })
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let order = |a: &FileHit, b: &FileHit| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                index.paths()[a.index]
                    .len()
                    .cmp(&index.paths()[b.index].len())
            })
            .then_with(|| a.index.cmp(&b.index))
    };
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit - 1, order);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(order);
    Some(hits)
}

/// Like IntelliJ's navigation: a leading `*` lets the pattern match anywhere in the name.
fn navigation_matcher(pattern: &str) -> FixingLayoutMatcher {
    FixingLayoutMatcher::new(
        &format!("*{pattern}"),
        MatchingMode::IgnoreCase,
        "",
        &RussianToEnglish,
    )
}

fn score_path(
    path: &str,
    name_matcher: &FixingLayoutMatcher,
    dir_matcher: Option<&FixingLayoutMatcher>,
) -> Option<i32> {
    let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
    let name_chars: Vec<char> = name.chars().collect();
    let fragments = name_matcher.match_chars(&name_chars)?;
    let mut score = name_matcher.degree_for_fragments(&name_chars, false, Some(&fragments));
    if let Some(dir_matcher) = dir_matcher {
        let dir_chars: Vec<char> = dir.chars().collect();
        let dir_fragments = dir_matcher.match_chars(&dir_chars)?;
        score += dir_matcher.degree_for_fragments(&dir_chars, false, Some(&dir_fragments)) / 10;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> FileIndex {
        FileIndex::new(paths.iter().map(|p| p.to_string()).collect())
    }

    fn found(index: &FileIndex, query: &str) -> Vec<String> {
        search_files(index, &FileQuery::parse(query), 10)
            .into_iter()
            .map(|h| index.paths()[h.index].clone())
            .collect()
    }

    #[test]
    fn parses_line_suffix() {
        assert_eq!(
            FileQuery::parse("Foo.rs:42"),
            FileQuery {
                pattern: "Foo.rs",
                line: Some(42)
            }
        );
        assert_eq!(
            FileQuery::parse("Foo.rs:"),
            FileQuery {
                pattern: "Foo.rs:",
                line: None
            }
        );
        assert_eq!(
            FileQuery::parse("Foo.rs"),
            FileQuery {
                pattern: "Foo.rs",
                line: None
            }
        );
        assert_eq!(
            FileQuery::parse("a:b"),
            FileQuery {
                pattern: "a:b",
                line: None
            }
        );
    }

    #[test]
    fn directory_and_name_parts_match_separately() {
        let idx = index(&[
            "platform/lang-impl/src/ide/GotoFileAction.java",
            "platform/other/GotoFileAction.java",
            "platform/lang-impl/README.md",
        ]);
        assert_eq!(
            found(&idx, "lang/Goto"),
            ["platform/lang-impl/src/ide/GotoFileAction.java"]
        );
    }

    #[test]
    fn camel_humps_match_file_names() {
        let idx = index(&["src/MinusculeMatcher.kt", "src/Other.kt"]);
        assert_eq!(found(&idx, "MiMa"), ["src/MinusculeMatcher.kt"]);
    }

    #[test]
    fn exact_name_ranks_before_longer_matches() {
        let idx = index(&["a/FooBarBaz.rs", "b/Foo.rs", "c/FooBar.rs"]);
        assert_eq!(found(&idx, "Foo")[0], "b/Foo.rs");
    }

    #[test]
    fn a_cancelled_search_returns_nothing() {
        let idx = index(&["a/x1.rs", "a/x2.rs"]);
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            search_files_cancellable(&idx, &FileQuery::parse("x"), 10, &cancelled),
            None
        );
        let live = AtomicBool::new(false);
        assert_eq!(
            search_files_cancellable(&idx, &FileQuery::parse("x"), 10, &live)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn respects_limit_and_empty_query() {
        let idx = index(&["a/x1.rs", "a/x2.rs", "a/x3.rs"]);
        assert_eq!(search_files(&idx, &FileQuery::parse("x"), 2).len(), 2);
        assert!(found(&idx, "").is_empty());
        assert!(found(&idx, "zzz").is_empty());
    }
}
