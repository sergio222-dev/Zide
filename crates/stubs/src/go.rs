//! Which Go syntax nodes declare stubs. Exported names (capitalized) are public.

use tree_sitter::Node;

use crate::{
    ImportTarget, Modifiers, StubKind,
    extract::{Item, Step},
};

pub(crate) fn classify(node: Node, source: &str, parent: Option<StubKind>) -> Step {
    match node.kind() {
        // containers to look through
        "source_file"
        | "type_declaration"
        | "const_declaration"
        | "var_declaration"
        | "var_spec_list"
        | "import_declaration"
        | "import_spec_list"
        | "struct_type"
        | "field_declaration_list"
        | "interface_type" => Step::Through,

        "function_declaration" => declare(node, source, StubKind::Function, false),
        "method_declaration" => declare(node, source, StubKind::Method, false),
        "type_spec" | "type_alias" => type_spec(node, source),
        "const_spec" | "var_spec" => specs(node, source, StubKind::Const),
        "field_declaration" if parent == Some(StubKind::Struct) => {
            specs(node, source, StubKind::Field)
        }
        // interface methods; embedded interfaces and type unions have no `name`
        "method_elem" if parent == Some(StubKind::Interface) => {
            declare(node, source, StubKind::Method, false)
        }
        "import_spec" => import(node, source),
        _ => Step::Skip,
    }
}

fn text(node: Node, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_string)
}

fn visibility(name: &str) -> Modifiers {
    if name.chars().next().is_some_and(char::is_uppercase) {
        Modifiers::PUBLIC
    } else {
        Modifiers::default()
    }
}

fn declare(node: Node, source: &str, kind: StubKind, descend: bool) -> Step {
    let Some(name_node) = node.child_by_field_name("name") else {
        return Step::Skip;
    };
    let name = text(name_node, source);
    let modifiers = name.as_deref().map(visibility).unwrap_or_default();
    Step::Declare {
        items: vec![Item {
            kind,
            name,
            name_range: name_node.start_byte()..name_node.end_byte(),
            modifiers,
            import: None,
            signature_end: None,
        }],
        descend,
    }
}

/// `type Point struct {...}`, `type Reader interface {...}`, `type ID int`, `type A = B`.
fn type_spec(node: Node, source: &str) -> Step {
    let Some(name_node) = node.child_by_field_name("name") else {
        return Step::Skip;
    };
    let definition = node.child_by_field_name("type");
    let (kind, descend, signature_end) = match definition.map(|d| d.kind()) {
        Some("struct_type") => (StubKind::Struct, true, keyword_end(definition)),
        Some("interface_type") => (StubKind::Interface, true, keyword_end(definition)),
        _ => (StubKind::TypeAlias, false, None),
    };
    let name = text(name_node, source);
    let modifiers = name.as_deref().map(visibility).unwrap_or_default();
    Step::Declare {
        items: vec![Item {
            kind,
            name,
            name_range: name_node.start_byte()..name_node.end_byte(),
            modifiers,
            import: None,
            signature_end,
        }],
        descend,
    }
}

/// The signature of a struct or interface ends after its keyword, before the body.
fn keyword_end(definition: Option<Node>) -> Option<usize> {
    definition?.child(0).map(|keyword| keyword.end_byte())
}

/// `const A, B = 1, 2` and `X, Y int` declare one stub per name.
fn specs(node: Node, source: &str, kind: StubKind) -> Step {
    let mut cursor = node.walk();
    let items: Vec<Item> = node
        .children_by_field_name("name", &mut cursor)
        .filter(|n| n.kind() != "blank_identifier")
        .map(|name_node| {
            let name = text(name_node, source);
            let modifiers = name.as_deref().map(visibility).unwrap_or_default();
            Item {
                kind,
                name,
                name_range: name_node.start_byte()..name_node.end_byte(),
                modifiers,
                import: None,
                signature_end: None,
            }
        })
        .collect();
    if items.is_empty() {
        Step::Skip
    } else {
        Step::Declare {
            items,
            descend: false,
        }
    }
}

/// `import "fmt"`, `import f "fmt"`, `import . "fmt"`, `import _ "embed"`.
fn import(node: Node, source: &str) -> Step {
    let Some(path_node) = node.child_by_field_name("path") else {
        return Step::Skip;
    };
    let path = text(path_node, source)
        .unwrap_or_default()
        .trim_matches(['"', '`'])
        .to_string();
    let alias = node
        .child_by_field_name("name")
        .and_then(|n| text(n, source));
    let glob = alias.as_deref() == Some(".");
    let alias = alias.filter(|a| a != "." && a != "_");
    let name = alias
        .clone()
        .or_else(|| path.rsplit('/').next().map(str::to_string));
    Step::Declare {
        items: vec![Item {
            kind: StubKind::Import,
            name,
            name_range: 0..0,
            modifiers: Modifiers::default(),
            import: Some(ImportTarget {
                path: path.into(),
                member: None,
                alias: alias.map(Into::into),
                glob,
            }),
            signature_end: None,
        }],
        descend: false,
    }
}
