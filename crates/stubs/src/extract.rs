//! Builds stubs with one iterative pass over the syntax tree, like IntelliJ's
//! `LightStubBuilder`: a language says per node whether it is a declaration, a transparent
//! container to look through, or something to skip (bodies, expressions).

use std::{ops::Range, sync::OnceLock};

use tree_sitter::{Node, Parser, Tree};

use crate::{ImportTarget, Modifiers, StubKind, StubNode, go, js, python, rust};

/// Bump when any language's rules change: stubs built with other rules are rebuilt.
pub const RULES_VERSION: u32 = 3;

const MAX_SIGNATURE_CHARS: usize = 200;

/// One stub a node declares. A `use` with a list declares several.
pub(crate) struct Item {
    pub kind: StubKind,
    pub name: Option<String>,
    pub name_range: Range<usize>,
    pub modifiers: Modifiers,
    pub import: Option<ImportTarget>,
    /// Where the signature ends, when it is not at the start of the body.
    pub signature_end: Option<usize>,
}

pub(crate) enum Step {
    /// Not a declaration and not worth looking into.
    Skip,
    /// Not a declaration, but its children may hold some.
    Through,
    /// Declares `items`; the first one is the parent of declarations inside `node`.
    Declare { items: Vec<Item>, descend: bool },
}

struct Language {
    extensions: &'static [&'static str],
    grammar: tree_sitter::Language,
    classify: fn(Node, &str, Option<StubKind>) -> Step,
    /// Fields whose start ends the signature (the body, or the value of a constant).
    signature_ends_at: &'static [&'static str],
}

fn languages() -> &'static [Language] {
    static LANGUAGES: OnceLock<Vec<Language>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        let js = |extensions, grammar| Language {
            extensions,
            grammar,
            classify: js::classify,
            signature_ends_at: &["body", "value"],
        };
        vec![
            Language {
                extensions: &["rs"],
                grammar: tree_sitter_rust::LANGUAGE.into(),
                classify: rust::classify,
                signature_ends_at: &["body", "value"],
            },
            js(
                &["js", "mjs", "cjs", "jsx"],
                tree_sitter_javascript::LANGUAGE.into(),
            ),
            js(
                &["ts", "mts", "cts"],
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            ),
            js(&["tsx"], tree_sitter_typescript::LANGUAGE_TSX.into()),
            Language {
                extensions: &["go"],
                grammar: tree_sitter_go::LANGUAGE.into(),
                classify: go::classify,
                signature_ends_at: &["body", "value"],
            },
            Language {
                extensions: &["py", "pyi"],
                grammar: tree_sitter_python::LANGUAGE.into(),
                classify: python::classify,
                signature_ends_at: &["body"],
            },
        ]
    })
}

/// The language of the file at `path`, by extension, as an index into the supported languages.
pub fn language_for_path(path: &str) -> Option<usize> {
    let extension = path.rsplit_once('.')?.1;
    languages()
        .iter()
        .position(|l| l.extensions.contains(&extension))
}

/// Stubs of `source` in source order. `language` comes from [`language_for_path`].
pub fn extract_nodes(language: usize, source: &str) -> Vec<StubNode> {
    let mut parser = Parser::new();
    if parser.set_language(&languages()[language].grammar).is_err() {
        return Vec::new();
    }
    match parser.parse(source, None) {
        Some(tree) => extract_nodes_in(language, &tree, source),
        None => Vec::new(),
    }
}

/// Stubs of `source` in source order, read from `tree`, its syntax tree. A tree of another
/// grammar (the editor's tree of a file this crate reads differently) is parsed again.
pub fn extract_nodes_in(language: usize, tree: &Tree, source: &str) -> Vec<StubNode> {
    if *tree.language() != languages()[language].grammar {
        return extract_nodes(language, source);
    }
    let language = &languages()[language];
    let mut nodes: Vec<StubNode> = Vec::new();
    // (syntax node, index of the stub it hangs from); popped in source order
    let mut pending = vec![(tree.root_node(), None::<u32>)];
    while let Some((node, parent)) = pending.pop() {
        let (items, descend) =
            match (language.classify)(node, source, parent.map(|p| nodes[p as usize].kind)) {
                Step::Skip => continue,
                Step::Through => (Vec::new(), true),
                Step::Declare { items, descend } => (items, descend),
            };

        let mut child_parent = parent;
        for (i, item) in items.into_iter().enumerate() {
            let index = nodes.len() as u32;
            if i == 0 {
                child_parent = Some(index);
            }
            nodes.push(StubNode {
                kind: item.kind,
                parent,
                name: item.name.map(Into::into),
                range: node.start_byte() as u32..node.end_byte() as u32,
                name_range: item.name_range.start as u32..item.name_range.end as u32,
                line: node.start_position().row as u32 + 1,
                modifiers: item.modifiers,
                signature: signature(
                    node,
                    source,
                    language.signature_ends_at,
                    item.signature_end,
                    item.import.is_some(),
                ),
                import: item.import,
            });
        }
        if descend {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            // reversed, so the stack yields them in source order
            pending.extend(children.into_iter().rev().map(|c| (c, child_parent)));
        }
    }
    nodes
}

/// The declaration text from its start to its body (or constant value), whitespace collapsed.
fn signature(
    node: Node,
    source: &str,
    end_fields: &[&str],
    end_override: Option<usize>,
    is_import: bool,
) -> Option<Box<str>> {
    if is_import {
        return None;
    }
    let end = end_override.unwrap_or_else(|| {
        end_fields
            .iter()
            .find_map(|field| node.child_by_field_name(field))
            .map_or(node.end_byte(), |body| body.start_byte())
    });
    let text = source.get(node.start_byte()..end)?;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed
        .trim_end_matches(['{', '=', ' ', ';', ','])
        .to_string();
    let signature: String = collapsed.chars().take(MAX_SIGNATURE_CHARS).collect();
    (!signature.is_empty()).then(|| signature.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stubs(source: &str) -> Vec<StubNode> {
        extract_nodes(language_for_path("a.rs").unwrap(), source)
    }

    /// `(kind, name, container name)` of each stub.
    fn outline(source: &str) -> Vec<(StubKind, String, Option<String>)> {
        let nodes = stubs(source);
        nodes
            .iter()
            .map(|n| {
                let container = n.parent.and_then(|p| nodes[p as usize].name.clone());
                (
                    n.kind,
                    n.name.clone().unwrap_or_default().to_string(),
                    container.map(|c| c.to_string()),
                )
            })
            .collect()
    }

    #[test]
    fn declarations_know_their_container() {
        let found = outline(
            "struct Point { x: i32 }\nimpl Point {\n    fn new() -> Self { todo!() }\n}\nmod util { fn helper() {} }\n",
        );
        use StubKind::*;
        let has = |kind, name: &str, container: Option<&str>| {
            found.contains(&(kind, name.to_string(), container.map(str::to_string)))
        };
        assert!(has(Struct, "Point", None), "{found:?}");
        assert!(has(Field, "x", Some("Point")), "{found:?}");
        assert!(has(Impl, "Point", None), "{found:?}");
        assert!(has(Method, "new", Some("Point")), "{found:?}");
        assert!(has(Module, "util", None), "{found:?}");
        assert!(has(Function, "helper", Some("util")), "{found:?}");
    }

    #[test]
    fn function_bodies_are_never_visited() {
        let found = outline("fn outer() {\n    fn inner() {}\n    struct Local;\n}\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].1, "outer");
    }

    #[test]
    fn enums_have_variants_and_traits_have_methods() {
        let found = outline("enum Color { Red, Green }\ntrait Shape { fn area(&self) -> f64; }\n");
        use StubKind::*;
        let has = |kind, name: &str, container: &str| {
            found.contains(&(kind, name.to_string(), Some(container.to_string())))
        };
        assert!(has(Variant, "Red", "Color"), "{found:?}");
        assert!(has(Variant, "Green", "Color"), "{found:?}");
        assert!(has(Method, "area", "Shape"), "{found:?}");
    }

    #[test]
    fn signatures_stop_at_the_body_and_modifiers_are_kept() {
        let source =
            "pub async fn go<T>(\n    x: T,\n) -> u8 {\n    1\n}\nconst LIMIT: usize = 10;\n";
        let nodes = stubs(source);
        assert_eq!(
            nodes[0].signature.as_deref(),
            Some("pub async fn go<T>( x: T, ) -> u8")
        );
        assert!(nodes[0].modifiers.contains(Modifiers::PUBLIC));
        assert!(nodes[0].modifiers.contains(Modifiers::ASYNC));
        assert!(!nodes[0].modifiers.contains(Modifiers::UNSAFE));
        assert_eq!(nodes[1].signature.as_deref(), Some("const LIMIT: usize"));
    }

    #[test]
    fn ranges_and_lines_point_at_the_declaration() {
        let source = "\n\nfn target() {}\n";
        let node = &stubs(source)[0];
        assert_eq!(node.line, 3);
        assert_eq!(
            &source[node.name_range.start as usize..node.name_range.end as usize],
            "target"
        );
        assert_eq!(
            &source[node.range.start as usize..node.range.end as usize],
            "fn target() {}"
        );
    }

    #[test]
    fn use_trees_flatten_into_one_import_per_leaf() {
        let nodes = stubs(
            "use std::collections::{HashMap, hash_map::Entry as E};\nuse std::io::{self, Read};\nuse crate::prelude::*;\n",
        );
        let imports: Vec<_> = nodes
            .iter()
            .map(|n| n.import.as_ref().unwrap())
            .map(|i| {
                (
                    i.path.to_string(),
                    i.alias.as_deref().map(str::to_string),
                    i.glob,
                )
            })
            .collect();
        assert_eq!(
            imports,
            [
                ("std::collections::HashMap".to_string(), None, false),
                (
                    "std::collections::hash_map::Entry".to_string(),
                    Some("E".to_string()),
                    false
                ),
                ("std::io".to_string(), None, false),
                ("std::io::Read".to_string(), None, false),
                ("crate::prelude".to_string(), None, true),
            ]
        );
        assert!(
            nodes
                .iter()
                .all(|n| n.kind == StubKind::Import && n.signature.is_none())
        );
    }

    /// `(kind, name, container name)` of the stubs of `source`, whose language comes from `path`.
    fn outline_of(path: &str, source: &str) -> Vec<(StubKind, String, Option<String>)> {
        let nodes = extract_nodes(language_for_path(path).unwrap(), source);
        nodes
            .iter()
            .map(|n| {
                let container = n.parent.and_then(|p| nodes[p as usize].name.clone());
                (
                    n.kind,
                    n.name.clone().unwrap_or_default().to_string(),
                    container.map(|c| c.to_string()),
                )
            })
            .collect()
    }

    fn has(
        found: &[(StubKind, String, Option<String>)],
        kind: StubKind,
        name: &str,
        container: Option<&str>,
    ) -> bool {
        found.contains(&(kind, name.to_string(), container.map(str::to_string)))
    }

    #[test]
    fn go_declarations_and_their_containers() {
        let found = outline_of(
            "main.go",
            "package main\n\nimport (\n\t\"fmt\"\n\tio \"os/io\"\n)\n\n\
             const Max, min = 1, 2\n\n\
             type Point struct {\n\tX, Y int\n\tname string\n}\n\
             type Shape interface {\n\tArea() float64\n}\n\
             type ID int\n\n\
             func (p Point) Dist() float64 { return 0 }\n\
             func helper() { type local int }\n",
        );
        use StubKind::*;
        assert!(has(&found, Const, "Max", None), "{found:?}");
        assert!(has(&found, Const, "min", None), "{found:?}");
        assert!(has(&found, Struct, "Point", None), "{found:?}");
        assert!(has(&found, Field, "X", Some("Point")), "{found:?}");
        assert!(has(&found, Field, "Y", Some("Point")), "{found:?}");
        assert!(has(&found, Field, "name", Some("Point")), "{found:?}");
        assert!(has(&found, Interface, "Shape", None), "{found:?}");
        assert!(has(&found, Method, "Area", Some("Shape")), "{found:?}");
        assert!(has(&found, TypeAlias, "ID", None), "{found:?}");
        assert!(has(&found, Method, "Dist", None), "{found:?}");
        assert!(has(&found, Function, "helper", None), "{found:?}");
        assert!(!found.iter().any(|(_, n, _)| n == "local"), "{found:?}");
    }

    #[test]
    fn go_exports_imports_and_signatures() {
        let source = "package p\n\nimport (\n\t\"net/http\"\n\tf \"fmt\"\n\t. \"strings\"\n)\n\n\
                      type Point struct {\n\tX int\n}\nfunc add(a int) int { return a }\nfunc Add() {}\n";
        let nodes = extract_nodes(language_for_path("p.go").unwrap(), source);
        let named = |name: &str| {
            nodes
                .iter()
                .find(|n| n.name.as_deref() == Some(name))
                .unwrap()
        };
        assert!(named("Add").modifiers.contains(Modifiers::PUBLIC));
        assert!(!named("add").modifiers.contains(Modifiers::PUBLIC));
        assert_eq!(
            named("add").signature.as_deref(),
            Some("func add(a int) int")
        );
        assert_eq!(named("Point").signature.as_deref(), Some("Point struct"));
        let imports: Vec<_> = nodes
            .iter()
            .filter_map(|n| n.import.as_ref())
            .map(|i| {
                (
                    i.path.to_string(),
                    i.alias.as_deref().map(str::to_string),
                    i.glob,
                )
            })
            .collect();
        assert_eq!(
            imports,
            [
                ("net/http".to_string(), None, false),
                ("fmt".to_string(), Some("f".to_string()), false),
                ("strings".to_string(), None, true),
            ]
        );
    }

    #[test]
    fn python_declarations_and_their_containers() {
        let found = outline_of(
            "app.py",
            "MAX_SIZE = 10\nlimit = 3\n\n@decorator\nclass Service(Base):\n    TIMEOUT = 5\n    \n    def run(self):\n        def inner(): pass\n        class Local: pass\n\n    async def fetch(self): ...\n\ndef helper(x): return x\n",
        );
        use StubKind::*;
        assert!(has(&found, Const, "MAX_SIZE", None), "{found:?}");
        assert!(!found.iter().any(|(_, n, _)| n == "limit"), "{found:?}");
        assert!(has(&found, Class, "Service", None), "{found:?}");
        assert!(has(&found, Const, "TIMEOUT", Some("Service")), "{found:?}");
        assert!(has(&found, Method, "run", Some("Service")), "{found:?}");
        assert!(has(&found, Method, "fetch", Some("Service")), "{found:?}");
        assert!(has(&found, Function, "helper", None), "{found:?}");
        assert!(
            !found.iter().any(|(_, n, _)| n == "inner" || n == "Local"),
            "{found:?}"
        );
    }

    #[test]
    fn python_modifiers_signatures_and_imports() {
        let source = "import os.path as p\nimport sys, re\nfrom a.b import c, d as e\nfrom . import f\nfrom x import *\n\nasync def go(a, b):\n    return a\ndef _hidden(): pass\n";
        let nodes = extract_nodes(language_for_path("m.py").unwrap(), source);
        let named = |name: &str| {
            nodes
                .iter()
                .find(|n| n.name.as_deref() == Some(name))
                .unwrap()
        };
        assert!(named("go").modifiers.contains(Modifiers::ASYNC));
        assert!(named("go").modifiers.contains(Modifiers::PUBLIC));
        assert!(!named("_hidden").modifiers.contains(Modifiers::PUBLIC));
        assert_eq!(
            named("go").signature.as_deref(),
            Some("async def go(a, b):")
        );
        let imports: Vec<_> = nodes
            .iter()
            .filter_map(|n| n.import.as_ref())
            .map(|i| {
                (
                    i.path.to_string(),
                    i.member.as_deref().map(str::to_string),
                    i.alias.as_deref().map(str::to_string),
                    i.glob,
                )
            })
            .collect();
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            imports,
            [
                ("os.path".to_string(), None, s("p"), false),
                ("sys".to_string(), None, None, false),
                ("re".to_string(), None, None, false),
                ("a.b".to_string(), s("c"), None, false),
                ("a.b".to_string(), s("d"), s("e"), false),
                (".".to_string(), s("f"), None, false),
                ("x".to_string(), None, None, true),
            ]
        );
    }

    #[test]
    fn language_is_chosen_by_extension() {
        assert!(language_for_path("a.go").is_some());
        assert!(language_for_path("a.py").is_some());
        assert!(language_for_path("a.pyi").is_some());
        assert!(language_for_path("src/main.rs").is_some());
        assert!(language_for_path("README.md").is_none());
        assert!(language_for_path("Makefile").is_none());
    }

    #[test]
    fn broken_source_still_yields_what_parses() {
        let found = outline("fn ok() {}\nfn broken( {");
        assert!(found.iter().any(|(_, n, _)| n == "ok"));
    }

    #[test]
    fn nodes_read_from_a_given_tree_are_those_of_parsing_the_source() {
        let source = "struct A;\nimpl A {\n    fn go(&self) {}\n}\n";
        let language = language_for_path("a.rs").unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let nodes = extract_nodes_in(language, &tree, source);
        assert!(!nodes.is_empty());
        assert_eq!(nodes, extract_nodes(language, source));
    }

    #[test]
    fn a_tree_of_another_grammar_is_ignored_and_the_source_parsed_again() {
        let source = "fn go() {}\n";
        let language = language_for_path("a.rs").unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let python_tree = parser.parse(source, None).unwrap();
        assert_eq!(
            extract_nodes_in(language, &python_tree, source),
            extract_nodes(language, source)
        );
    }
}
