//! Which Rust syntax nodes declare stubs.

use tree_sitter::Node;

use crate::{
    ImportTarget, Modifiers, StubKind,
    extract::{Item, Step},
};

pub(crate) fn classify(node: Node, source: &str, parent: Option<StubKind>) -> Step {
    let kind = match node.kind() {
        // containers to look through
        "source_file" | "declaration_list" | "field_declaration_list" | "enum_variant_list" => {
            return Step::Through;
        }
        "function_item" | "function_signature_item" => {
            if matches!(parent, Some(StubKind::Impl | StubKind::Trait)) {
                StubKind::Method
            } else {
                StubKind::Function
            }
        }
        "struct_item" | "union_item" => StubKind::Struct,
        "enum_item" => StubKind::Enum,
        "enum_variant" => StubKind::Variant,
        "field_declaration" => StubKind::Field,
        "trait_item" => StubKind::Trait,
        "impl_item" => StubKind::Impl,
        "mod_item" => StubKind::Module,
        "type_item" => StubKind::TypeAlias,
        "const_item" | "static_item" => StubKind::Const,
        "macro_definition" => StubKind::Macro,
        "use_declaration" => return imports(node, source),
        _ => return Step::Skip,
    };

    // an impl is named after the type it implements for
    let name_node = if kind == StubKind::Impl {
        node.child_by_field_name("type")
    } else {
        node.child_by_field_name("name")
    };
    let name = name_node.and_then(|n| text(n, source));
    // inline modules have a body to look into; `mod x;` and bodiless items do not
    let descend = matches!(
        kind,
        StubKind::Struct | StubKind::Enum | StubKind::Trait | StubKind::Impl | StubKind::Module
    );
    Step::Declare {
        items: vec![Item {
            kind,
            name,
            name_range: name_node.map_or(0..0, |n| n.start_byte()..n.end_byte()),
            modifiers: modifiers(node, source),
            import: None,
        }],
        descend,
    }
}

fn text(node: Node, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_string)
}

fn modifiers(node: Node, source: &str) -> Modifiers {
    let mut modifiers = Modifiers::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "visibility_modifier" => modifiers = modifiers.with(Modifiers::PUBLIC),
            "function_modifiers" => {
                let words = text(child, source).unwrap_or_default();
                if words.split_whitespace().any(|w| w == "async") {
                    modifiers = modifiers.with(Modifiers::ASYNC);
                }
                if words.split_whitespace().any(|w| w == "unsafe") {
                    modifiers = modifiers.with(Modifiers::UNSAFE);
                }
            }
            _ => {}
        }
    }
    modifiers
}

/// One import per leaf of the `use` tree: `use a::{b, c as d}` imports `a::b` and `a::c`.
fn imports(node: Node, source: &str) -> Step {
    let Some(argument) = node.child_by_field_name("argument") else {
        return Step::Skip;
    };
    let public = modifiers(node, source);
    let mut leaves = Vec::new();
    use_leaves(argument, source, "", &mut leaves);
    let items = leaves
        .into_iter()
        .map(|target| {
            let name = target
                .alias
                .as_deref()
                .or_else(|| target.path.rsplit("::").next())
                .map(str::to_string);
            Item {
                kind: StubKind::Import,
                name,
                name_range: 0..0,
                modifiers: public,
                import: Some(target),
            }
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        Step::Skip
    } else {
        Step::Declare {
            items,
            descend: false,
        }
    }
}

fn join(prefix: &str, rest: &str) -> String {
    match (prefix.is_empty(), rest) {
        (true, _) => rest.to_string(),
        (false, "self") => prefix.to_string(),
        (false, _) => format!("{prefix}::{rest}"),
    }
}

fn use_leaves(node: Node, source: &str, prefix: &str, out: &mut Vec<ImportTarget>) {
    let target = |path: String, alias: Option<String>, glob: bool| ImportTarget {
        path: path.into(),
        alias: alias.map(Into::into),
        glob,
    };
    match node.kind() {
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                use_leaves(child, source, prefix, out);
            }
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .and_then(|n| text(n, source));
            let prefix = path.map_or(prefix.to_string(), |p| join(prefix, &p));
            if let Some(list) = node.child_by_field_name("list") {
                use_leaves(list, source, &prefix, out);
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let path = node
                .named_children(&mut cursor)
                .next()
                .and_then(|n| text(n, source))
                .map_or(prefix.to_string(), |p| join(prefix, &p));
            out.push(target(path, None, true));
        }
        "use_as_clause" => {
            let path = node
                .child_by_field_name("path")
                .and_then(|n| text(n, source));
            let alias = node
                .child_by_field_name("alias")
                .and_then(|n| text(n, source));
            if let Some(path) = path {
                out.push(target(join(prefix, &path), alias, false));
            }
        }
        _ => {
            if let Some(path) = text(node, source) {
                out.push(target(join(prefix, &path), None, false));
            }
        }
    }
}
