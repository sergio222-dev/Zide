//! Layer 1: local syntactic analysis for the editor (highlighting for now). Tree-sitter stays
//! behind this crate's API; it never resolves types or references.

mod fold;
mod pair;
mod select;

use std::{ops::Range, path::Path, sync::OnceLock};

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, Tree};
use zide_text::ByteEdit;

/// What a piece of text is, for coloring. The UI maps classes to colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
    Constant,
    Property,
    Variable,
    Operator,
    Punctuation,
    Tag,
    Attribute,
}

/// A run of bytes of one class. Runs are sorted and never overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub class: Class,
}

struct Grammar {
    extensions: &'static [&'static str],
    language: Language,
    query: String,
    /// `query` compiled, the first time it is needed (compiling takes milliseconds).
    compiled: OnceLock<Option<Query>>,
}

impl Grammar {
    fn compiled_query(&self) -> Option<&Query> {
        self.compiled
            .get_or_init(|| Query::new(&self.language, &self.query).ok())
            .as_ref()
    }
}

fn grammars() -> &'static [Grammar] {
    static GRAMMARS: OnceLock<Vec<Grammar>> = OnceLock::new();
    GRAMMARS.get_or_init(|| {
        // the TypeScript query only adds to the JavaScript one; later patterns win
        let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
        let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
        let ts = tree_sitter_typescript::HIGHLIGHTS_QUERY;
        vec![
            Grammar {
                extensions: &["rs"],
                language: tree_sitter_rust::LANGUAGE.into(),
                query: tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
                compiled: OnceLock::new(),
            },
            Grammar {
                extensions: &["js", "mjs", "cjs", "jsx"],
                language: tree_sitter_javascript::LANGUAGE.into(),
                query: format!("{js}\n{jsx}"),
                compiled: OnceLock::new(),
            },
            Grammar {
                extensions: &["ts", "mts", "cts"],
                language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                query: format!("{js}\n{ts}"),
                compiled: OnceLock::new(),
            },
            Grammar {
                extensions: &["go"],
                language: tree_sitter_go::LANGUAGE.into(),
                query: generic_first(tree_sitter_go::HIGHLIGHTS_QUERY, "(identifier) @variable"),
                compiled: OnceLock::new(),
            },
            Grammar {
                extensions: &["py", "pyi"],
                language: tree_sitter_python::LANGUAGE.into(),
                query: tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
                compiled: OnceLock::new(),
            },
            Grammar {
                extensions: &["tsx"],
                language: tree_sitter_typescript::LANGUAGE_TSX.into(),
                query: format!("{js}\n{ts}\n{jsx}"),
                compiled: OnceLock::new(),
            },
        ]
    })
}

pub use fold::{FoldKind, FoldRegion, fold_regions};
pub use pair::{is_pair_candidate, matching_pair};
pub use select::extend_selection;

/// Moves the catch-all pattern `generic` to the front. Here the last pattern wins, as in
/// Neovim; some grammars order theirs the other way round (first wins) and would paint every
/// identifier as a plain variable.
fn generic_first(query: &str, generic: &str) -> String {
    let rest: Vec<&str> = query.lines().filter(|l| l.trim() != generic).collect();
    format!("{generic}\n{}", rest.join("\n"))
}

/// A language with highlighting, by file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageId(usize);

pub fn language_for_path(path: &str) -> Option<LanguageId> {
    let extension = Path::new(path).extension()?.to_str()?;
    grammars()
        .iter()
        .position(|g| g.extensions.contains(&extension))
        .map(LanguageId)
}

/// A file's syntax tree, kept between edits: [`Syntax::update`] reparses only what changed
/// (milliseconds in a huge file, where parsing it again takes hundreds), and everything the
/// editor derives from the syntax reads this one tree.
pub struct Syntax {
    language: LanguageId,
    parser: Parser,
    tree: Tree,
}

impl Syntax {
    /// Parses `source`. `None` when the grammar does not load.
    pub fn new(language: LanguageId, source: &str) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&grammars()[language.0].language).ok()?;
        let tree = parser.parse(source, None)?;
        Some(Self {
            language,
            parser,
            tree,
        })
    }

    /// Brings the tree to `source`, which is the text it was built from with `edits` applied,
    /// in order.
    pub fn update(&mut self, source: &str, edits: &[ByteEdit]) {
        if edits.is_empty() {
            return;
        }
        for edit in edits {
            let point = |(row, column)| Point::new(row, column);
            self.tree.edit(&InputEdit {
                start_byte: edit.start_byte,
                old_end_byte: edit.old_end_byte,
                new_end_byte: edit.new_end_byte,
                start_position: point(edit.start),
                old_end_position: point(edit.old_end),
                new_end_position: point(edit.new_end),
            });
        }
        if let Some(tree) = self.parser.parse(source, Some(&self.tree)) {
            self.tree = tree;
        }
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Highlight spans for `source`, the text of the tree. Empty when the query does not load.
    pub fn highlight(&self, source: &str) -> Vec<Span> {
        highlight_tree(&grammars()[self.language.0], &self.tree, source)
    }

    /// The foldable regions of `source`, the text of the tree.
    pub fn fold_regions(&self, source: &str) -> Vec<FoldRegion> {
        fold::regions(&grammars()[self.language.0], &self.tree, source)
    }
}

/// Highlight spans for `source`. Empty when the grammar or its query does not load.
pub fn highlight(language: LanguageId, source: &str) -> Vec<Span> {
    Syntax::new(language, source).map_or_else(Vec::new, |syntax| syntax.highlight(source))
}

fn highlight_tree(grammar: &Grammar, tree: &Tree, source: &str) -> Vec<Span> {
    let Some(query) = grammar.compiled_query() else {
        return Vec::new();
    };

    let classes: Vec<Option<Class>> = query
        .capture_names()
        .iter()
        .map(|name| class_of(name))
        .collect();

    // paint byte by byte: the pattern that comes last in the query wins, like in Neovim
    let mut painted: Vec<(usize, u8)> = vec![(0, 0); source.len()];
    let mut ids: Vec<Class> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let Some(class) = classes[capture.index as usize] else {
                continue;
            };
            let id = match ids.iter().position(|&c| c == class) {
                Some(i) => i,
                None => {
                    ids.push(class);
                    ids.len() - 1
                }
            } as u8
                + 1;
            let range = capture.node.start_byte()..capture.node.end_byte();
            for cell in &mut painted[range] {
                if m.pattern_index >= cell.0 {
                    *cell = (m.pattern_index, id);
                }
            }
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    for (byte, &(_, id)) in painted.iter().enumerate() {
        if id == 0 {
            continue;
        }
        let class = ids[id as usize - 1];
        match spans.last_mut() {
            Some(last) if last.range.end == byte && last.class == class => last.range.end += 1,
            _ => spans.push(Span {
                range: byte..byte + 1,
                class,
            }),
        }
    }
    spans
}

fn class_of(capture: &str) -> Option<Class> {
    let head = capture.split('.').next()?;
    Some(match head {
        "keyword" | "conditional" | "repeat" | "include" | "storage" => Class::Keyword,
        "string" | "escape" | "character" => Class::String,
        "number" | "float" | "boolean" => Class::Number,
        "comment" => Class::Comment,
        "function" | "method" | "constructor" => Class::Function,
        "type" | "namespace" | "module" => Class::Type,
        "constant" => Class::Constant,
        "property" | "field" => Class::Property,
        "variable" | "parameter" => Class::Variable,
        "operator" => Class::Operator,
        "punctuation" | "delimiter" => Class::Punctuation,
        "tag" => Class::Tag,
        "attribute" | "label" => Class::Attribute,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(path: &str, source: &str) -> Vec<(String, Class)> {
        let language = language_for_path(path).expect("language");
        highlight(language, source)
            .into_iter()
            .map(|s| (source[s.range].to_string(), s.class))
            .collect()
    }

    #[test]
    fn go_keywords_strings_and_functions() {
        let found = classes(
            "a.go",
            "package main\n\nfunc add(a int) int { return a } // done\nvar s = \"hi\"\n",
        );
        assert!(
            found.contains(&("func".into(), Class::Keyword)),
            "{found:?}"
        );
        assert!(
            found.contains(&("add".into(), Class::Function)),
            "{found:?}"
        );
        assert!(found.contains(&("\"hi\"".into(), Class::String)));
        assert!(found.contains(&("// done".into(), Class::Comment)));
    }

    #[test]
    fn python_keywords_strings_and_functions() {
        let found = classes("a.py", "def add(a):\n    return a  # done\n\ns = \"hi\"\n");
        assert!(found.contains(&("def".into(), Class::Keyword)));
        assert!(found.contains(&("add".into(), Class::Function)));
        assert!(found.contains(&("\"hi\"".into(), Class::String)));
        assert!(found.contains(&("# done".into(), Class::Comment)));
        assert!(language_for_path("stubs.pyi").is_some());
    }

    #[test]
    fn rust_keywords_strings_and_comments() {
        let found = classes("a.rs", "fn main() { let s = \"hi\"; } // done");
        assert!(found.contains(&("fn".into(), Class::Keyword)));
        assert!(found.contains(&("\"hi\"".into(), Class::String)));
        assert!(found.contains(&("// done".into(), Class::Comment)));
        assert!(found.contains(&("main".into(), Class::Function)));
    }

    #[test]
    fn typescript_adds_its_own_keywords() {
        let found = classes("a.ts", "interface A { x: number }\nconst n = 1;");
        assert!(found.contains(&("interface".into(), Class::Keyword)));
        assert!(found.contains(&("const".into(), Class::Keyword)));
        assert!(found.contains(&("1".into(), Class::Number)));
    }

    #[test]
    fn tsx_highlights_html_tags_and_components() {
        let found = classes("a.tsx", "const a = <div><Button label=\"x\" /></div>;");
        assert!(found.contains(&("div".into(), Class::Tag)), "{found:?}");
        assert!(found.contains(&("Button".into(), Class::Type)), "{found:?}");
        assert!(
            found.contains(&("label".into(), Class::Attribute)),
            "{found:?}"
        );
    }

    #[test]
    fn spans_are_sorted_and_disjoint() {
        let source = "fn a() { let x = 1 + 2; } // c\nstruct S;";
        let spans = highlight(language_for_path("a.rs").unwrap(), source);
        assert!(spans.windows(2).all(|w| w[0].range.end <= w[1].range.start));
    }

    #[test]
    fn unknown_extensions_have_no_language() {
        assert!(language_for_path("notes.txt").is_none());
    }

    /// Edits `source` through a buffer, updating the tree after each one, and checks that the
    /// tree always gives what parsing the text from scratch gives.
    fn assert_incremental_matches_full(path: &str, source: &str, edits: &[(usize, usize, &str)]) {
        let language = language_for_path(path).unwrap();
        let mut buffer = zide_text::Buffer::new(source);
        let mut syntax = Syntax::new(language, source).unwrap();
        let check = |buffer: &mut zide_text::Buffer, syntax: &mut Syntax, step: &str| {
            let text = buffer.text();
            syntax.update(&text, &buffer.take_edits());
            assert_eq!(
                syntax.highlight(&text),
                highlight(language, &text),
                "{step}"
            );
            assert_eq!(
                syntax.fold_regions(&text),
                fold_regions(language, &text),
                "{step}"
            );
        };
        for (i, (start, end, text)) in edits.iter().enumerate() {
            buffer.edit(*start..*end, text);
            check(&mut buffer, &mut syntax, &format!("edit {i}"));
        }
        // several edits between two updates, and undo
        buffer.edit(0..0, "// head\n");
        buffer.edit(3..4, "");
        check(&mut buffer, &mut syntax, "two edits at once");
        buffer.undo();
        check(&mut buffer, &mut syntax, "undo");
    }

    #[test]
    fn an_incremental_update_gives_what_a_full_parse_gives() {
        let source = "use std::fmt;\n\nfn a() {\n    let s = \"h\u{e9}llo\";\n    if s.len() > 1 {\n        run(s);\n    }\n}\n\nstruct P { x: u8 }\n";
        assert_incremental_matches_full(
            "a.rs",
            source,
            &[
                (30, 30, "x"),                      // type inside a line
                (14, 14, "\nfn b() {\n    1\n}\n"), // new lines, a new block
                (40, 70, ""),                       // delete across lines
                (0, 3, "pub"),                      // replace
                (20, 21, "{"),                      // unbalance the braces
                (22, 23, "\u{f1}\n\u{f1}"),         // multibyte and a newline
            ],
        );
    }

    #[test]
    fn an_incremental_update_gives_what_a_full_parse_gives_in_tsx() {
        let source =
            "import a from 'a';\nconst C = () => {\n  return <div className=\"x\">{a}</div>;\n};\n";
        assert_incremental_matches_full(
            "a.tsx",
            source,
            &[(60, 60, " id=\"y\""), (19, 19, "\n// c\n"), (30, 45, "")],
        );
    }

    #[test]
    fn updating_without_edits_keeps_the_tree() {
        let language = language_for_path("a.rs").unwrap();
        let mut syntax = Syntax::new(language, "fn a() {}").unwrap();
        syntax.update("fn a() {}", &[]);
        assert_eq!(
            syntax.fold_regions("fn a() {}"),
            fold_regions(language, "fn a() {}")
        );
    }
}
