//! Builds stubs with one iterative pass over the syntax tree, like IntelliJ's
//! `LightStubBuilder`: a language says per node whether it is a declaration, a transparent
//! container to look through, or something to skip (bodies, expressions).

use std::{ops::Range, sync::OnceLock};

use tree_sitter::{Node, Parser};

use crate::{
    ImportTarget, Modifiers, StubKind, StubNode,
    rust::{self},
};

/// Bump when any language's rules change: stubs built with other rules are rebuilt.
pub const RULES_VERSION: u32 = 1;

const MAX_SIGNATURE_CHARS: usize = 200;

/// One stub a node declares. A `use` with a list declares several.
pub(crate) struct Item {
    pub kind: StubKind,
    pub name: Option<String>,
    pub name_range: Range<usize>,
    pub modifiers: Modifiers,
    pub import: Option<ImportTarget>,
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
        vec![Language {
            extensions: &["rs"],
            grammar: tree_sitter_rust::LANGUAGE.into(),
            classify: rust::classify,
            signature_ends_at: &["body", "value"],
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

/// Stubs of `source` in source order. `language` comes from [`language_for_path`].
pub fn extract_nodes(language: usize, source: &str) -> Vec<StubNode> {
    let language = &languages()[language];
    let mut parser = Parser::new();
    if parser.set_language(&language.grammar).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

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
fn signature(node: Node, source: &str, end_fields: &[&str], is_import: bool) -> Option<Box<str>> {
    if is_import {
        return None;
    }
    let end = end_fields
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .map_or(node.end_byte(), |body| body.start_byte());
    let text = source.get(node.start_byte()..end)?;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.trim_end_matches(['{', '=', ' ']).to_string();
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

    #[test]
    fn language_is_chosen_by_extension() {
        assert!(language_for_path("src/main.rs").is_some());
        assert!(language_for_path("README.md").is_none());
        assert!(language_for_path("Makefile").is_none());
    }

    #[test]
    fn broken_source_still_yields_what_parses() {
        let found = outline("fn ok() {}\nfn broken( {");
        assert!(found.iter().any(|(_, n, _)| n == "ok"));
    }
}
