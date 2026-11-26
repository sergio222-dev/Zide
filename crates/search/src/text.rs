use std::{
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
use rayon::prelude::*;
use zide_index::FileIndex;

/// Files searched in parallel before their hits are delivered.
const CHUNK_SIZE: usize = 256;
/// Longer lines are cut, so one minified file can't flood the results.
const MAX_LINE_BYTES: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct TextQuery {
    pub pattern: String,
    /// Treat `pattern` as a regular expression instead of literal text.
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Comma-separated globs for file names, like `*.rs, *.toml`.
    pub file_mask: Option<String>,
    /// Only search files under this directory (relative to the project root).
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextHit {
    /// Index into the [`FileIndex`].
    pub file: usize,
    /// 1-based line number.
    pub line: u64,
    /// The line, without its terminator, cut at [`MAX_LINE_BYTES`].
    pub text: String,
    /// Byte ranges of the matches inside `text`.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSearchError {
    InvalidPattern(String),
    InvalidFileMask(String),
}

impl std::fmt::Display for TextSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPattern(e) => write!(f, "invalid pattern: {e}"),
            Self::InvalidFileMask(e) => write!(f, "invalid file mask: {e}"),
        }
    }
}

impl std::error::Error for TextSearchError {}

/// Searches the contents of the indexed files, like ripgrep, without a text index.
///
/// Files are searched in parallel, in index order, `CHUNK_SIZE` at a time. After each chunk
/// `on_hits` gets that chunk's hits (ordered by file, then line), so results stream in a stable
/// order. The search stops after `limit` hits, or as soon as `cancel` is set.
/// Returns the number of hits delivered.
pub fn search_text(
    index: &FileIndex,
    root: &Path,
    query: &TextQuery,
    limit: usize,
    cancel: &AtomicBool,
    mut on_hits: impl FnMut(Vec<TextHit>),
) -> Result<usize, TextSearchError> {
    if query.pattern.is_empty() || limit == 0 {
        return Ok(0);
    }
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(!query.regex)
        .case_insensitive(!query.case_sensitive)
        .word(query.whole_word)
        .build(&query.pattern)
        .map_err(|e| TextSearchError::InvalidPattern(e.to_string()))?;
    let mask = file_mask(query.file_mask.as_deref())?;
    let directory = query
        .directory
        .as_deref()
        .map(|d| d.trim_matches('/'))
        .filter(|d| !d.is_empty());

    let candidates: Vec<usize> = index
        .paths()
        .iter()
        .enumerate()
        .filter(|(_, path)| {
            directory.is_none_or(|d| path.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
                && mask.as_ref().is_none_or(|m| matches_mask(m, path))
        })
        .map(|(i, _)| i)
        .collect();

    let mut delivered = 0;
    for chunk in candidates.chunks(CHUNK_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let per_file: Vec<Vec<TextHit>> = chunk
            .par_iter()
            .map(|&file| search_file(&matcher, root, &index.paths()[file], file, cancel))
            .collect();
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut hits: Vec<TextHit> = per_file.into_iter().flatten().collect();
        hits.truncate(limit - delivered);
        delivered += hits.len();
        if !hits.is_empty() {
            on_hits(hits);
        }
        if delivered >= limit {
            break;
        }
    }
    Ok(delivered)
}

fn file_mask(mask: Option<&str>) -> Result<Option<GlobSet>, TextSearchError> {
    let globs: Vec<&str> = mask
        .unwrap_or_default()
        .split([',', ' '])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect();
    if globs.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(Glob::new(glob).map_err(|e| TextSearchError::InvalidFileMask(e.to_string()))?);
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| TextSearchError::InvalidFileMask(e.to_string()))
}

fn matches_mask(mask: &GlobSet, path: &str) -> bool {
    let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
    mask.is_match(name) || mask.is_match(path)
}

fn search_file(
    matcher: &RegexMatcher,
    root: &Path,
    relative: &str,
    file: usize,
    cancel: &AtomicBool,
) -> Vec<TextHit> {
    let mut hits = Vec::new();
    if cancel.load(Ordering::Relaxed) {
        return hits;
    }
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();
    let sink = UTF8(|line, text| {
        hits.push(make_hit(matcher, file, line, text));
        Ok(!cancel.load(Ordering::Relaxed))
    });
    // unreadable and non-UTF-8 files are skipped, like ripgrep does with its default settings
    let _ = searcher.search_path(matcher, root.join(relative), sink);
    hits
}

fn make_hit(matcher: &RegexMatcher, file: usize, line: u64, text: &str) -> TextHit {
    let text = text.trim_end_matches(['\n', '\r']);
    let mut cut = text.len().min(MAX_LINE_BYTES);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let text = &text[..cut];
    let mut ranges = Vec::new();
    let _ = matcher.find_iter(text.as_bytes(), |m| {
        ranges.push(m.start()..m.end());
        true
    });
    TextHit {
        file,
        line,
        text: text.to_string(),
        ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct Project {
        root: PathBuf,
        index: FileIndex,
    }

    impl Project {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let root =
                std::env::temp_dir().join(format!("zide-text-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let mut paths = Vec::new();
            for (path, content) in files {
                let full = root.join(path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(full, content).unwrap();
                paths.push(path.to_string());
            }
            Self {
                root,
                index: FileIndex::new(paths),
            }
        }

        fn search(&self, query: TextQuery) -> Vec<(String, u64, String)> {
            let mut all = Vec::new();
            search_text(
                &self.index,
                &self.root,
                &query,
                1000,
                &AtomicBool::new(false),
                |hits| all.extend(hits),
            )
            .unwrap();
            all.into_iter()
                .map(|h| (self.index.paths()[h.file].clone(), h.line, h.text))
                .collect()
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn q(pattern: &str) -> TextQuery {
        TextQuery {
            pattern: pattern.into(),
            ..Default::default()
        }
    }

    fn hit(path: &str, line: u64, text: &str) -> (String, u64, String) {
        (path.into(), line, text.into())
    }

    #[test]
    fn finds_literal_text_with_line_numbers_ignoring_case() {
        let p = Project::new(
            "literal",
            &[
                ("a.rs", "fn main() {}\nlet Foo = 1;\n"),
                ("b.rs", "foo(bar)\n"),
            ],
        );
        assert_eq!(
            p.search(q("foo")),
            [hit("a.rs", 2, "let Foo = 1;"), hit("b.rs", 1, "foo(bar)")]
        );
    }

    #[test]
    fn literal_mode_does_not_interpret_regex_syntax() {
        let p = Project::new("fixed", &[("a.txt", "a.c\nabc\n")]);
        assert_eq!(p.search(q("a.c")), [hit("a.txt", 1, "a.c")]);
        let regex = TextQuery {
            regex: true,
            ..q("a.c")
        };
        assert_eq!(p.search(regex).len(), 2);
    }

    #[test]
    fn case_sensitive_and_whole_word_options() {
        let p = Project::new("options", &[("a.txt", "Foo\nfoo\nfoobar\n")]);
        let sensitive = TextQuery {
            case_sensitive: true,
            ..q("Foo")
        };
        assert_eq!(p.search(sensitive), [hit("a.txt", 1, "Foo")]);
        let word = TextQuery {
            whole_word: true,
            ..q("foo")
        };
        assert_eq!(
            p.search(word),
            [hit("a.txt", 1, "Foo"), hit("a.txt", 2, "foo")]
        );
    }

    #[test]
    fn file_mask_and_directory_filter() {
        let p = Project::new(
            "filters",
            &[
                ("src/a.rs", "needle\n"),
                ("src/b.kt", "needle\n"),
                ("docs/c.rs", "needle\n"),
            ],
        );
        let rs = TextQuery {
            file_mask: Some("*.rs, *.toml".into()),
            ..q("needle")
        };
        assert_eq!(p.search(rs).len(), 2);
        let src_only = TextQuery {
            directory: Some("src".into()),
            ..q("needle")
        };
        assert_eq!(p.search(src_only).len(), 2);
        let both = TextQuery {
            file_mask: Some("*.rs".into()),
            directory: Some("src/".into()),
            ..q("needle")
        };
        assert_eq!(p.search(both), [hit("src/a.rs", 1, "needle")]);
    }

    #[test]
    fn reports_match_ranges_within_the_line() {
        let p = Project::new("ranges", &[("a.txt", "ab ab\n")]);
        let mut hits = Vec::new();
        search_text(
            &p.index,
            &p.root,
            &q("ab"),
            10,
            &AtomicBool::new(false),
            |h| hits.extend(h),
        )
        .unwrap();
        assert_eq!(hits[0].ranges, [0..2, 3..5]);
    }

    #[test]
    fn skips_binary_files() {
        let p = Project::new(
            "binary",
            &[("a.bin", "needle\0needle\n"), ("b.txt", "needle\n")],
        );
        assert_eq!(p.search(q("needle")), [hit("b.txt", 1, "needle")]);
    }

    #[test]
    fn stops_at_the_limit_and_when_cancelled() {
        let p = Project::new("limit", &[("a.txt", "x\nx\nx\n"), ("b.txt", "x\nx\n")]);
        let mut count = 0;
        let delivered = search_text(
            &p.index,
            &p.root,
            &q("x"),
            4,
            &AtomicBool::new(false),
            |h| count += h.len(),
        )
        .unwrap();
        assert_eq!((delivered, count), (4, 4));

        let mut count = 0;
        let delivered = search_text(&p.index, &p.root, &q("x"), 4, &AtomicBool::new(true), |h| {
            count += h.len()
        })
        .unwrap();
        assert_eq!((delivered, count), (0, 0));
    }

    #[test]
    fn reports_invalid_patterns_and_masks() {
        let p = Project::new("invalid", &[("a.txt", "x\n")]);
        let cancel = AtomicBool::new(false);
        let bad_regex = TextQuery {
            regex: true,
            ..q("(")
        };
        assert!(matches!(
            search_text(&p.index, &p.root, &bad_regex, 10, &cancel, |_| {}),
            Err(TextSearchError::InvalidPattern(_))
        ));
        let bad_mask = TextQuery {
            file_mask: Some("[".into()),
            ..q("x")
        };
        assert!(matches!(
            search_text(&p.index, &p.root, &bad_mask, 10, &cancel, |_| {}),
            Err(TextSearchError::InvalidFileMask(_))
        ));
    }

    #[test]
    fn cuts_long_lines_on_a_char_boundary() {
        let long = format!("needle {}\n", "é".repeat(500));
        let p = Project::new("long", &[("a.txt", long.as_str())]);
        let hits = p.search(q("needle"));
        assert!(hits[0].2.len() <= MAX_LINE_BYTES);
        assert!(hits[0].2.starts_with("needle é"));
    }
}
