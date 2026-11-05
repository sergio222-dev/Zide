use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use rayon::prelude::*;
use zide_index::FileIndex;
use zide_matcher::{FixingLayoutMatcher, MatchedFragment, Matcher, MatchingMode, RussianToEnglish};

use crate::{SymbolKind, extract_symbols, language_for_path};

/// Larger files (generated code, bundles) are not worth parsing.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: SymbolKind,
    /// Index into the [`FileIndex`] the symbols were built from.
    pub file: usize,
    /// 1-based.
    pub line: u32,
}

/// All symbols of the supported-language files of a project, in file order.
#[derive(Debug, Default)]
pub struct SymbolIndex {
    entries: Vec<SymbolEntry>,
}

impl SymbolIndex {
    /// Parses every supported file of `files` in parallel. `None` if `cancel` was set meanwhile.
    pub fn build(files: &FileIndex, root: &Path, cancel: &AtomicBool) -> Option<Self> {
        let per_file: Vec<Vec<SymbolEntry>> = files
            .paths()
            .par_iter()
            .enumerate()
            .map(|(file, path)| {
                if cancel.load(Ordering::Relaxed) {
                    return Vec::new();
                }
                symbols_of_file(file, path, root)
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        Some(Self {
            entries: per_file.into_iter().flatten().collect(),
        })
    }

    pub fn entries(&self) -> &[SymbolEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn symbols_of_file(file: usize, path: &str, root: &Path) -> Vec<SymbolEntry> {
    let Some(language) = language_for_path(path) else {
        return Vec::new();
    };
    let full_path = root.join(path);
    if std::fs::metadata(&full_path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        return Vec::new();
    }
    let Ok(source) = std::fs::read_to_string(&full_path) else {
        return Vec::new();
    };
    extract_symbols(language, &source)
        .into_iter()
        .map(|s| SymbolEntry {
            name: s.name,
            kind: s.kind,
            file,
            line: s.line,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    /// Index into [`SymbolIndex::entries`].
    pub entry: usize,
    pub score: i32,
    /// Matched parts of the name, as `char` ranges.
    pub fragments: Vec<MatchedFragment>,
}

/// Fuzzy search over symbol names, best first; `None` as soon as `cancel` is set.
pub fn search_symbols(
    index: &SymbolIndex,
    pattern: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Option<Vec<SymbolHit>> {
    let pattern = pattern.trim();
    if pattern.is_empty() || limit == 0 {
        return Some(Vec::new());
    }
    // like IntelliJ's navigation: a leading `*` lets the pattern match anywhere in the name
    let matcher = FixingLayoutMatcher::new(
        &format!("*{pattern}"),
        MatchingMode::IgnoreCase,
        "",
        &RussianToEnglish,
    );

    let mut hits: Vec<SymbolHit> = index
        .entries
        .par_iter()
        .enumerate()
        .filter_map(|(entry, symbol)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let chars: Vec<char> = symbol.name.chars().collect();
            let fragments = matcher.match_chars(&chars)?;
            let score = matcher.degree_for_fragments(&chars, false, Some(&fragments));
            Some(SymbolHit {
                entry,
                score,
                fragments,
            })
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let order = |a: &SymbolHit, b: &SymbolHit| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                index.entries[a.entry]
                    .name
                    .len()
                    .cmp(&index.entries[b.entry].name.len())
            })
            .then_with(|| a.entry.cmp(&b.entry))
    };
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit - 1, order);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(order);
    Some(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(name: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, FileIndex) {
        let root = std::env::temp_dir().join(format!("zide-symbols-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, content) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        let paths = files.iter().map(|(p, _)| p.to_string()).collect();
        (root, FileIndex::new(paths))
    }

    fn names(index: &SymbolIndex, pattern: &str) -> Vec<String> {
        search_symbols(index, pattern, 10, &AtomicBool::new(false))
            .unwrap()
            .iter()
            .map(|h| index.entries()[h.entry].name.clone())
            .collect()
    }

    #[test]
    fn indexes_supported_files_and_skips_the_rest() {
        let (root, files) = project(
            "build",
            &[
                ("src/a.rs", "fn alpha() {}\nstruct Beta;\n"),
                ("README.md", "fn not_code() {}"),
            ],
        );
        let index = SymbolIndex::build(&files, &root, &AtomicBool::new(false)).unwrap();
        let found: Vec<_> = index
            .entries()
            .iter()
            .map(|e| (e.name.as_str(), e.file, e.line))
            .collect();
        assert_eq!(found, [("alpha", 0, 1), ("Beta", 0, 2)]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn search_matches_camel_humps_and_prefers_exact_names() {
        let (root, files) = project(
            "search",
            &[(
                "a.rs",
                "struct FileIndex;\nstruct Index;\nfn find_index() {}\n",
            )],
        );
        let index = SymbolIndex::build(&files, &root, &AtomicBool::new(false)).unwrap();
        assert_eq!(names(&index, "Index")[0], "Index");
        assert!(names(&index, "FI").contains(&"FileIndex".to_string()));
        assert!(names(&index, "zzz").is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_cancelled_search_or_build_returns_nothing() {
        let (root, files) = project("cancel", &[("a.rs", "fn a() {}\n")]);
        let cancelled = AtomicBool::new(true);
        assert!(SymbolIndex::build(&files, &root, &cancelled).is_none());
        let index = SymbolIndex::build(&files, &root, &AtomicBool::new(false)).unwrap();
        assert!(search_symbols(&index, "a", 10, &cancelled).is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
