use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

use rayon::prelude::*;
use xxhash_rust::xxh3::xxh3_64;
use zide_index::FileIndex;
use zide_matcher::{FixingLayoutMatcher, MatchedFragment, Matcher, MatchingMode, RussianToEnglish};

use crate::{FileStubs, RULES_VERSION, Stamp, StubNode, extract_nodes, language_for_path};

/// Larger files (generated code, bundles) are not worth parsing.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A searchable stub: the `node`-th stub of the `file`-th file of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolRef {
    pub file: u32,
    pub node: u32,
}

/// A stub with the context needed to show it.
#[derive(Debug, Clone, Copy)]
pub struct Symbol<'a> {
    pub node: &'a StubNode,
    /// Index into the [`FileIndex`] the store was built from.
    pub file: usize,
    /// Name of the enclosing declaration (`Point` for `Point::new`).
    pub container: Option<&'a str>,
}

/// The stubs of every supported file of a project.
#[derive(Debug)]
pub struct StubStore {
    files: Arc<FileIndex>,
    /// Aligned with `files`; `None` for unsupported or unreadable files.
    stubs: Vec<Option<Arc<FileStubs>>>,
    /// Every stub worth finding by name, in file order.
    entries: Vec<SymbolRef>,
    reused: usize,
}

impl StubStore {
    /// Builds the stubs of `files`, reusing those of `previous` whose file did not change.
    /// `None` if `cancel` was set meanwhile.
    pub fn build(
        files: Arc<FileIndex>,
        root: &Path,
        previous: Option<&StubStore>,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        let known: HashMap<&str, &Arc<FileStubs>> = previous
            .map(|p| {
                p.files
                    .paths()
                    .iter()
                    .zip(&p.stubs)
                    .filter_map(|(path, stubs)| Some((path.as_str(), stubs.as_ref()?)))
                    .collect()
            })
            .unwrap_or_default();

        let built: Vec<(Option<Arc<FileStubs>>, bool)> = files
            .paths()
            .par_iter()
            .map(|path| {
                if cancel.load(Ordering::Relaxed) {
                    return (None, false);
                }
                stubs_of_file(root, path, known.get(path.as_str()).copied())
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return None;
        }

        let reused = built.iter().filter(|(_, reused)| *reused).count();
        let stubs: Vec<_> = built.into_iter().map(|(stubs, _)| stubs).collect();
        let entries = stubs
            .iter()
            .enumerate()
            .flat_map(|(file, stubs)| {
                let nodes = stubs.as_ref().map_or(&[][..], |s| &s.nodes[..]);
                nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.kind.is_searchable() && n.name.is_some())
                    .map(move |(node, _)| SymbolRef {
                        file: file as u32,
                        node: node as u32,
                    })
            })
            .collect();
        Some(Self {
            files,
            stubs,
            entries,
            reused,
        })
    }

    /// How many files kept their stubs from the previous store.
    pub fn reused(&self) -> usize {
        self.reused
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn file_stubs(&self, file: usize) -> Option<&FileStubs> {
        self.stubs.get(file)?.as_deref()
    }

    /// The `i`-th searchable symbol.
    pub fn symbol(&self, i: usize) -> Symbol<'_> {
        let SymbolRef { file, node } = self.entries[i];
        let stubs = self.stubs[file as usize]
            .as_ref()
            .expect("entries point at built files");
        let node = &stubs.nodes[node as usize];
        Symbol {
            node,
            file: file as usize,
            container: stubs.container_name(node),
        }
    }
}

/// Stubs of one file, and whether they were reused from `previous`.
fn stubs_of_file(
    root: &Path,
    path: &str,
    previous: Option<&Arc<FileStubs>>,
) -> (Option<Arc<FileStubs>>, bool) {
    let Some(language) = language_for_path(path) else {
        return (None, false);
    };
    let full_path = root.join(path);
    let Ok(metadata) = std::fs::metadata(&full_path) else {
        return (None, false);
    };
    if metadata.len() > MAX_FILE_BYTES {
        return (None, false);
    }
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    let previous = previous.filter(|p| p.rules_version == RULES_VERSION);

    // same time and size: the content did not change
    if let Some(previous) = previous
        && previous.stamp.mtime_ns == mtime_ns
        && previous.stamp.len == metadata.len()
    {
        return (Some(previous.clone()), true);
    }
    let Ok(bytes) = std::fs::read(&full_path) else {
        return (None, false);
    };
    let stamp = Stamp {
        mtime_ns,
        len: bytes.len() as u64,
        hash: xxh3_64(&bytes),
    };
    // touched, but not changed
    if let Some(previous) = previous
        && previous.stamp.hash == stamp.hash
    {
        let nodes = previous.nodes.clone();
        let stubs = FileStubs {
            stamp,
            rules_version: RULES_VERSION,
            nodes,
        };
        return (Some(Arc::new(stubs)), true);
    }
    let Ok(source) = String::from_utf8(bytes) else {
        return (None, false);
    };
    let nodes = extract_nodes(language, &source);
    (
        Some(Arc::new(FileStubs {
            stamp,
            rules_version: RULES_VERSION,
            nodes,
        })),
        false,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    /// Index of the symbol: pass it to [`StubStore::symbol`].
    pub entry: usize,
    pub score: i32,
    /// Matched parts of the name, as `char` ranges.
    pub fragments: Vec<MatchedFragment>,
}

/// Fuzzy search over symbol names, best first; `None` as soon as `cancel` is set.
pub fn search_symbols(
    store: &StubStore,
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

    let mut hits: Vec<SymbolHit> = (0..store.len())
        .into_par_iter()
        .filter_map(|entry| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let name = store.symbol(entry).node.name.as_deref()?;
            let chars: Vec<char> = name.chars().collect();
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

    let name_len = |hit: &SymbolHit| {
        store
            .symbol(hit.entry)
            .node
            .name
            .as_deref()
            .map_or(0, str::len)
    };
    let order = |a: &SymbolHit, b: &SymbolHit| {
        b.score
            .cmp(&a.score)
            .then_with(|| name_len(a).cmp(&name_len(b)))
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
    use std::{fs, path::PathBuf, time::Duration};

    fn project(name: &str, files: &[(&str, &str)]) -> (PathBuf, Arc<FileIndex>) {
        let root = std::env::temp_dir().join(format!("zide-stubs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, content) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        let paths = files.iter().map(|(p, _)| p.to_string()).collect();
        (root, Arc::new(FileIndex::new(paths)))
    }

    fn build(files: &Arc<FileIndex>, root: &Path, previous: Option<&StubStore>) -> StubStore {
        StubStore::build(files.clone(), root, previous, &AtomicBool::new(false)).unwrap()
    }

    fn names(store: &StubStore, pattern: &str) -> Vec<String> {
        search_symbols(store, pattern, 10, &AtomicBool::new(false))
            .unwrap()
            .iter()
            .map(|h| {
                store
                    .symbol(h.entry)
                    .node
                    .name
                    .as_deref()
                    .unwrap()
                    .to_string()
            })
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
        let store = build(&files, &root, None);
        let found: Vec<_> = (0..store.len())
            .map(|i| store.symbol(i))
            .map(|s| (s.node.name.as_deref().unwrap(), s.file, s.node.line))
            .collect();
        assert_eq!(found, [("alpha", 0, 1), ("Beta", 0, 2)]);
        assert!(store.file_stubs(1).is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn imports_and_impls_are_stored_but_not_searchable() {
        let (root, files) = project(
            "kinds",
            &[("a.rs", "use std::io::Read;\nstruct A;\nimpl A {}\n")],
        );
        let store = build(&files, &root, None);
        assert_eq!(store.file_stubs(0).unwrap().nodes.len(), 3);
        assert_eq!(names(&store, "A"), ["A"]);
        assert!(names(&store, "Read").is_empty());
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
        let store = build(&files, &root, None);
        assert_eq!(names(&store, "Index")[0], "Index");
        assert!(names(&store, "FI").contains(&"FileIndex".to_string()));
        assert!(names(&store, "zzz").is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symbols_know_their_container() {
        let (root, files) = project(
            "container",
            &[("a.rs", "struct P;\nimpl P {\n    fn new() {}\n}\n")],
        );
        let store = build(&files, &root, None);
        let hit = &search_symbols(&store, "new", 10, &AtomicBool::new(false)).unwrap()[0];
        assert_eq!(store.symbol(hit.entry).container, Some("P"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unchanged_files_keep_their_stubs_and_changed_ones_are_rebuilt() {
        let (root, files) = project(
            "reuse",
            &[("a.rs", "fn one() {}\n"), ("b.rs", "fn two() {}\n")],
        );
        let first = build(&files, &root, None);
        assert_eq!(first.reused(), 0);

        let second = build(&files, &root, Some(&first));
        assert_eq!(second.reused(), 2);

        // different content and size: only that file is parsed again
        std::thread::sleep(Duration::from_millis(20));
        fs::write(root.join("a.rs"), "fn one() {}\nfn three() {}\n").unwrap();
        let third = build(&files, &root, Some(&second));
        assert_eq!(third.reused(), 1);
        assert_eq!(names(&third, "three"), ["three"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn touching_a_file_without_changing_it_keeps_its_stubs() {
        let (root, files) = project("touch", &[("a.rs", "fn one() {}\n")]);
        let first = build(&files, &root, None);
        std::thread::sleep(Duration::from_millis(20));
        fs::write(root.join("a.rs"), "fn one() {}\n").unwrap(); // same bytes, new mtime
        let second = build(&files, &root, Some(&first));
        assert_eq!(second.reused(), 1);
        assert_ne!(
            first.file_stubs(0).unwrap().stamp.mtime_ns,
            second.file_stubs(0).unwrap().stamp.mtime_ns
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_cancelled_search_or_build_returns_nothing() {
        let (root, files) = project("cancel", &[("a.rs", "fn a() {}\n")]);
        let cancelled = AtomicBool::new(true);
        assert!(StubStore::build(files.clone(), &root, None, &cancelled).is_none());
        let store = build(&files, &root, None);
        assert!(search_symbols(&store, "a", 10, &cancelled).is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
