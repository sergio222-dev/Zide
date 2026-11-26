//! Which Python syntax nodes declare stubs. Names starting with `_` are not public.

use tree_sitter::Node;

use crate::{
    ImportTarget, Modifiers, StubKind,
    extract::{Item, Step},
};

pub(crate) fn classify(node: Node, source: &str, parent: Option<StubKind>) -> Step {
    match node.kind() {
        // containers to look through
        "module" | "decorated_definition" => Step::Through,
        // the body of a class holds declarations; the body of a function does not
        "block" if parent == Some(StubKind::Class) => Step::Through,
        "expression_statement" if matches!(parent, None | Some(StubKind::Class)) => Step::Through,

        "function_definition" => {
            let kind = if parent == Some(StubKind::Class) {
                StubKind::Method
            } else {
                StubKind::Function
            };
            declare(node, source, kind, false)
        }
        "class_definition" => declare(node, source, StubKind::Class, true),
        // module and class constants: `MAX_SIZE = 10`
        "assignment" => constant(node, source),
        "import_statement" => imports(node, source),
        "import_from_statement" => imports_from(node, source),
        _ => Step::Skip,
    }
}

fn text(node: Node, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_string)
}

fn visibility(name: &str) -> Modifiers {
    if name.starts_with('_') {
        Modifiers::default()
    } else {
        Modifiers::PUBLIC
    }
}

fn declare(node: Node, source: &str, kind: StubKind, descend: bool) -> Step {
    let Some(name_node) = node.child_by_field_name("name") else {
        return Step::Skip;
    };
    let name = text(name_node, source);
    let mut modifiers = name.as_deref().map(visibility).unwrap_or_default();
    let mut cursor = node.walk();
    if node.children(&mut cursor).any(|c| c.kind() == "async") {
        modifiers = modifiers.with(Modifiers::ASYNC);
    }
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

/// `NAME = value` with an ALL_CAPS name; other assignments are variables, not declarations.
fn constant(node: Node, source: &str) -> Step {
    let Some(left) = node
        .child_by_field_name("left")
        .filter(|n| n.kind() == "identifier")
    else {
        return Step::Skip;
    };
    let name = text(left, source).unwrap_or_default();
    let is_constant = name.chars().any(char::is_alphabetic)
        && name
            .chars()
            .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_');
    if !is_constant {
        return Step::Skip;
    }
    Step::Declare {
        items: vec![Item {
            kind: StubKind::Const,
            modifiers: visibility(&name),
            name: Some(name),
            name_range: left.start_byte()..left.end_byte(),
            import: None,
            signature_end: None,
        }],
        descend: false,
    }
}

fn import_item(target: ImportTarget) -> Item {
    let name = target
        .alias
        .as_deref()
        .or(target.member.as_deref())
        .or_else(|| target.path.rsplit('.').next())
        .map(str::to_string);
    Item {
        kind: StubKind::Import,
        name,
        name_range: 0..0,
        modifiers: Modifiers::default(),
        import: Some(target),
        signature_end: None,
    }
}

/// `import a.b`, `import a.b as c`, `import a, b`.
fn imports(node: Node, source: &str) -> Step {
    let mut cursor = node.walk();
    let items: Vec<Item> = node
        .children_by_field_name("name", &mut cursor)
        .filter_map(|name| {
            let (path, alias) = match name.kind() {
                "aliased_import" => (
                    name.child_by_field_name("name")?,
                    name.child_by_field_name("alias"),
                ),
                _ => (name, None),
            };
            Some(import_item(ImportTarget {
                path: text(path, source)?.into(),
                member: None,
                alias: alias.and_then(|a| text(a, source)).map(Into::into),
                glob: false,
            }))
        })
        .collect();
    declared(items)
}

/// `from a import b`, `from a import b as c`, `from . import x`, `from a import *`.
fn imports_from(node: Node, source: &str) -> Step {
    let Some(module) = node
        .child_by_field_name("module_name")
        .and_then(|m| text(m, source))
    else {
        return Step::Skip;
    };
    let target = |member: Option<String>, alias: Option<String>, glob: bool| ImportTarget {
        path: module.as_str().into(),
        member: member.map(Into::into),
        alias: alias.map(Into::into),
        glob,
    };
    let mut items = Vec::new();
    let mut cursor = node.walk();
    for name in node.children_by_field_name("name", &mut cursor) {
        let (member, alias) = match name.kind() {
            "aliased_import" => (
                name.child_by_field_name("name")
                    .and_then(|n| text(n, source)),
                name.child_by_field_name("alias")
                    .and_then(|n| text(n, source)),
            ),
            _ => (text(name, source), None),
        };
        items.push(import_item(target(member, alias, false)));
    }
    let mut cursor = node.walk();
    if node
        .children(&mut cursor)
        .any(|c| c.kind() == "wildcard_import")
    {
        items.push(import_item(target(None, None, true)));
    }
    declared(items)
}

fn declared(items: Vec<Item>) -> Step {
    if items.is_empty() {
        Step::Skip
    } else {
        Step::Declare {
            items,
            descend: false,
        }
    }
}
