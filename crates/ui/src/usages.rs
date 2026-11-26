//! Turns the locations a language server reports into rows of the Text list.

use std::{collections::HashMap, fs, path::Path};

use zide_index::FileIndex;
use zide_lsp::{Location, char_column, uri_to_path};
use zide_search::TextHit;

/// Longest line shown: minified files have lines of megabytes.
const MAX_LINE_CHARS: usize = 300;

/// One row per location inside the project, ordered by file and line. Locations elsewhere
/// (the standard library, `node_modules`) or in files the index does not list are left out.
pub fn hits(root: &Path, index: &FileIndex, locations: &[Location]) -> Vec<TextHit> {
    let mut files: HashMap<String, Option<(usize, Vec<String>)>> = HashMap::new();
    let mut hits = Vec::new();
    for location in locations {
        let Some(path) = uri_to_path(location.uri.as_str()) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let known = files.entry(relative.clone()).or_insert_with(|| {
            let file = index.paths().iter().position(|p| *p == relative)?;
            let text = fs::read_to_string(&path).ok()?;
            Some((file, text.lines().map(str::to_string).collect()))
        });
        let Some((file, lines)) = known else {
            continue;
        };
        let Some(line) = lines.get(location.range.start.line as usize) else {
            continue;
        };
        let start = char_column(line, location.range.start.character);
        let end = if location.range.end.line == location.range.start.line {
            char_column(line, location.range.end.character)
        } else {
            line.chars().count() // a range over several lines: mark to the end of the first
        };
        let shown: String = line.chars().take(MAX_LINE_CHARS).collect();
        let byte_of = |char_index: usize| {
            shown
                .char_indices()
                .nth(char_index)
                .map_or(shown.len(), |(byte, _)| byte)
        };
        let range = byte_of(start)..byte_of(end);
        hits.push(TextHit {
            file: *file,
            line: u64::from(location.range.start.line) + 1,
            text: shown.clone(),
            ranges: if range.is_empty() {
                Vec::new()
            } else {
                vec![range]
            },
        });
    }
    hits.sort_by(|a, b| {
        index.paths()[a.file]
            .cmp(&index.paths()[b.file])
            .then(a.line.cmp(&b.line))
            .then(
                a.ranges
                    .first()
                    .map(|r| r.start)
                    .cmp(&b.ranges.first().map(|r| r.start)),
            )
    });
    hits.dedup();
    hits
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use zide_lsp::{Position, Range, path_to_uri};

    use super::*;

    fn location(path: &Path, line: u32, from: u32, to: u32) -> Location {
        Location {
            uri: FromStr::from_str(&path_to_uri(path)).unwrap(),
            range: Range {
                start: Position {
                    line,
                    character: from,
                },
                end: Position {
                    line,
                    character: to,
                },
            },
        }
    }

    fn project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-usages-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn locations_become_rows_with_the_match_marked() {
        let root = project("rows");
        fs::write(root.join("src/b.rs"), "fn go() {}\nlet 😀 = go();\n").unwrap();
        fs::write(root.join("src/a.rs"), "go();\n").unwrap();
        let index = FileIndex::new(vec!["src/b.rs".into(), "src/a.rs".into()]);
        let found = hits(
            &root,
            &index,
            &[
                location(&root.join("src/b.rs"), 1, 9, 11), // after an emoji of two UTF-16 units
                location(&root.join("src/b.rs"), 0, 3, 5),
                location(&root.join("src/a.rs"), 0, 0, 2),
            ],
        );
        let shown: Vec<_> = found
            .iter()
            .map(|h| {
                (
                    index.paths()[h.file].as_str(),
                    h.line,
                    &h.text[h.ranges[0].clone()],
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("src/a.rs", 1, "go"),
                ("src/b.rs", 1, "go"),
                ("src/b.rs", 2, "go")
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locations_outside_the_project_or_the_index_are_left_out() {
        let root = project("outside");
        fs::write(root.join("src/a.rs"), "x\n").unwrap();
        fs::write(root.join("src/ignored.rs"), "x\n").unwrap();
        let index = FileIndex::new(vec!["src/a.rs".into()]);
        let found = hits(
            &root,
            &index,
            &[
                location(&root.join("src/ignored.rs"), 0, 0, 1),
                location(Path::new("/usr/lib/rustlib/src/core.rs"), 0, 0, 1),
                location(&root.join("src/a.rs"), 99, 0, 1), // a line that is gone
            ],
        );
        assert!(found.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_locations_show_once() {
        let root = project("dup");
        fs::write(root.join("src/a.rs"), "go go\n").unwrap();
        let index = FileIndex::new(vec!["src/a.rs".into()]);
        let same = location(&root.join("src/a.rs"), 0, 0, 2);
        assert_eq!(hits(&root, &index, &[same.clone(), same]).len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
