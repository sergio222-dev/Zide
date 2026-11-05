use std::sync::OnceLock;

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Macro,
    Type,
    Constant,
}

impl SymbolKind {
    /// From the suffix of a tags capture, e.g. `definition.function`.
    fn from_capture(name: &str) -> Option<Self> {
        Some(match name.strip_prefix("definition.")? {
            "function" => Self::Function,
            "method" => Self::Method,
            "class" => Self::Class,
            "interface" => Self::Interface,
            "module" => Self::Module,
            "macro" => Self::Macro,
            "type" => Self::Type,
            "constant" => Self::Constant,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based.
    pub line: u32,
}

struct Language {
    extensions: &'static [&'static str],
    grammar: tree_sitter::Language,
    query: Query,
}

fn languages() -> &'static [Language] {
    static LANGUAGES: OnceLock<Vec<Language>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        let rust: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        vec![Language {
            extensions: &["rs"],
            query: Query::new(&rust, tree_sitter_rust::TAGS_QUERY).expect("valid Rust tags query"),
            grammar: rust,
        }]
    })
}

/// The language of the file at `path`, by extension, as an index into the supported languages.
pub fn language_for_path(path: &str) -> Option<usize> {
    let extension = path.rsplit_once('.')?.1;
    languages()
        .iter()
        .position(|l| l.extensions.contains(&extension))
}

/// Definitions found in `source`, in file order. `language` comes from [`language_for_path`].
pub fn extract_symbols(language: usize, source: &str) -> Vec<Symbol> {
    let language = &languages()[language];
    let mut parser = Parser::new();
    if parser.set_language(&language.grammar).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let names = language.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&language.query, tree.root_node(), source.as_bytes());
    let mut symbols = Vec::new();
    while let Some(m) = matches.next() {
        let mut kind = None;
        let mut name = None;
        for capture in m.captures {
            let capture_name = names[capture.index as usize];
            if capture_name == "name" {
                name = capture.node.utf8_text(source.as_bytes()).ok();
            } else if let Some(k) = SymbolKind::from_capture(capture_name) {
                kind = Some(k);
            }
        }
        if let (Some(kind), Some(name)) = (kind, name) {
            let line = m.captures[0].node.start_position().row as u32 + 1;
            symbols.push(Symbol {
                name: name.to_string(),
                kind,
                line,
            });
        }
    }
    symbols.sort_by_key(|s| s.line);
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust(source: &str) -> Vec<(String, SymbolKind, u32)> {
        let language = language_for_path("a.rs").unwrap();
        extract_symbols(language, source)
            .into_iter()
            .map(|s| (s.name, s.kind, s.line))
            .collect()
    }

    #[test]
    fn finds_rust_definitions_with_their_lines() {
        let found = rust(
            "struct Point;\n\nfn main() {}\n\ntrait Shape { fn area(&self); }\n\nmod util {}\n\nenum Color { Red }\n",
        );
        let has = |name: &str, kind| found.iter().any(|(n, k, _)| n == name && *k == kind);
        assert!(has("Point", SymbolKind::Class), "{found:?}");
        assert!(has("main", SymbolKind::Function), "{found:?}");
        assert!(has("Shape", SymbolKind::Interface), "{found:?}");
        assert!(has("util", SymbolKind::Module), "{found:?}");
        assert!(found.contains(&("main".to_string(), SymbolKind::Function, 3)));
    }

    #[test]
    fn methods_inside_impl_blocks_are_found() {
        let found = rust("struct A;\nimpl A {\n    fn go(&self) {}\n}\n");
        assert!(
            found.iter().any(|(n, _, l)| n == "go" && *l == 3),
            "{found:?}"
        );
    }

    #[test]
    fn language_is_chosen_by_extension() {
        assert!(language_for_path("src/main.rs").is_some());
        assert!(language_for_path("README.md").is_none());
        assert!(language_for_path("Makefile").is_none());
    }

    #[test]
    fn broken_source_still_yields_what_parses() {
        let found = rust("fn ok() {}\nfn broken( {");
        assert!(found.iter().any(|(n, _, _)| n == "ok"));
    }
}
