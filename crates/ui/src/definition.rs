//! Where "go to definition" lands: the locations a language server reports, or, when none can
//! answer, the declarations of the symbol index that carry the same name.

use std::{fs, path::Path};

use zide_index::FileIndex;
use zide_lsp::{Location, char_column, uri_to_path};
use zide_stubs::StubStore;

/// A spot of a project file, 0-based, `path` relative to the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

/// The locations inside the project; those elsewhere (the standard library, dependencies) are
/// left out, and so are those of files the index does not list.
pub fn in_project(root: &Path, index: &FileIndex, locations: &[Location]) -> Vec<Target> {
    locations
        .iter()
        .filter_map(|location| {
            let absolute = uri_to_path(location.uri.as_str())?;
            let path = absolute
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            index.paths().contains(&path).then_some(())?;
            let line = location.range.start.line as usize;
            let text = fs::read_to_string(&absolute).ok()?;
            let column = text
                .lines()
                .nth(line)
                .map_or(0, |l| char_column(l, location.range.start.character));
            Some(Target { path, line, column })
        })
        .collect()
}

/// The declarations named exactly `word`, those of the file `current` first.
pub fn by_name(
    root: &Path,
    index: &FileIndex,
    store: &StubStore,
    word: &str,
    current: &str,
) -> Vec<Target> {
    let mut found: Vec<Target> = (0..store.len())
        .filter_map(|entry| {
            let symbol = store.symbol(entry);
            if symbol.node.name.as_deref() != Some(word) {
                return None;
            }
            let path = index.paths()[symbol.file].clone();
            let line = (symbol.node.line as usize).saturating_sub(1);
            let column = fs::read_to_string(root.join(&path))
                .ok()
                .and_then(|text| {
                    let l = text.lines().nth(line)?;
                    Some(l[..l.find(word)?].chars().count())
                })
                .unwrap_or(0);
            Some(Target { path, line, column })
        })
        .collect();
    found.sort_by_key(|target| target.path != current); // stable: file order is kept
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::atomic::AtomicBool};

    use zide_lsp::{Position, Range, path_to_uri};

    use std::sync::Arc;

    use super::*;

    fn location(path: &Path, line: u32, character: u32) -> Location {
        Location {
            uri: path_to_uri(path).parse().unwrap(),
            range: Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            ),
        }
    }

    #[test]
    fn only_locations_of_indexed_project_files_are_targets() {
        let root = std::env::temp_dir().join(format!("zide-def-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "x\n  é fn go() {}\n").unwrap();
        let index = FileIndex::new(vec!["src/a.rs".to_string()]);
        let outside = Path::new("/rustlib/src/lib.rs");

        let targets = in_project(
            &root,
            &index,
            &[
                location(outside, 0, 0),
                location(&root.join("src/unindexed.rs"), 0, 0),
                // `é` is one UTF-16 unit: the server's column 6 is the char column 6
                location(&root.join("src/a.rs"), 1, 6),
            ],
        );

        assert_eq!(
            targets,
            [Target {
                path: "src/a.rs".into(),
                line: 1,
                column: 6
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_fallback_finds_declarations_by_exact_name_current_file_first() {
        let root = std::env::temp_dir().join(format!("zide-def-name-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.rs"), "fn go() {}\n").unwrap();
        fs::write(root.join("b.rs"), "\n    fn go() {}\nfn gone() {}\n").unwrap();
        let index = Arc::new(FileIndex::new(vec!["a.rs".into(), "b.rs".into()]));
        let store = StubStore::build(index.clone(), &root, None, &AtomicBool::new(false)).unwrap();

        let at = |path: &str, line, column| Target {
            path: path.into(),
            line,
            column,
        };
        assert_eq!(
            by_name(&root, &index, &store, "go", "b.rs"),
            [at("b.rs", 1, 7), at("a.rs", 0, 3)]
        );
        assert_eq!(
            by_name(&root, &index, &store, "go", "a.rs")[0],
            at("a.rs", 0, 3)
        );
        assert!(by_name(&root, &index, &store, "missing", "a.rs").is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
