//! Which JavaScript, TypeScript and TSX syntax nodes declare stubs. The three grammars share
//! their node kinds, so they share these rules. React needs nothing extra: a component is a
//! function (or a class) with a capitalized name, and JSX is just syntax inside bodies we skip.

use tree_sitter::Node;

use crate::{
    ImportTarget, Modifiers, StubKind,
    extract::{Item, Step},
};

pub(crate) fn classify(node: Node, source: &str, parent: Option<StubKind>) -> Step {
    let kind = match node.kind() {
        // containers to look through
        "program"
        | "export_statement"
        | "ambient_declaration"
        | "lexical_declaration"
        | "variable_declaration"
        | "class_body"
        | "interface_body"
        | "enum_body" => {
            return Step::Through;
        }
        "object_type" if parent == Some(StubKind::Interface) => return Step::Through,
        // the body of a namespace holds declarations; the body of a function does not
        "statement_block" if parent == Some(StubKind::Module) => return Step::Through,

        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            StubKind::Function
        }
        "class_declaration" | "abstract_class_declaration" => StubKind::Class,
        "interface_declaration" => StubKind::Interface,
        "type_alias_declaration" => StubKind::TypeAlias,
        "enum_declaration" => StubKind::Enum,
        "internal_module" | "module" => StubKind::Module,
        "method_definition" | "method_signature" | "abstract_method_signature" => StubKind::Method,
        "public_field_definition" | "field_definition" | "property_signature" => StubKind::Field,
        "property_identifier" | "enum_assignment" if parent == Some(StubKind::Enum) => {
            StubKind::Variant
        }
        "variable_declarator" => return variable(node, source),
        "import_statement" => return imports(node, source),
        _ => return Step::Skip,
    };

    let name_node = match node.kind() {
        "field_definition" => node.child_by_field_name("property"),
        "enum_assignment" => node.child_by_field_name("name"),
        "property_identifier" => Some(node),
        _ => node.child_by_field_name("name"),
    };
    // computed names (`[Symbol.iterator]() {}`) and the like are not findable by name
    let Some(name_node) = name_node.filter(|n| is_name(n.kind())) else {
        return Step::Skip;
    };
    let in_class_like = matches!(parent, Some(StubKind::Class | StubKind::Interface));
    let mut modifiers = if in_class_like {
        member_visibility(node, source, name_node)
    } else if is_exported(node) {
        Modifiers::PUBLIC
    } else {
        Modifiers::default()
    };
    if has_child(node, "async") {
        modifiers = modifiers.with(Modifiers::ASYNC);
    }
    Step::Declare {
        items: vec![Item {
            kind,
            name: text(name_node, source),
            name_range: name_node.start_byte()..name_node.end_byte(),
            modifiers,
            import: None,
            signature_end: None,
        }],
        descend: matches!(
            kind,
            StubKind::Class | StubKind::Interface | StubKind::Enum | StubKind::Module
        ),
    }
}

fn is_name(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "property_identifier"
            | "private_property_identifier"
            | "nested_identifier"
            | "string"
    )
}

fn text(node: Node, source: &str) -> Option<String> {
    let text = source.get(node.start_byte()..node.end_byte())?;
    Some(text.trim_matches(['"', '\'', '`']).to_string())
}

fn has_child(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == kind)
}

/// `export function f() {}` and `export const f = ...`.
fn is_exported(node: Node) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        match n.kind() {
            "lexical_declaration" | "variable_declaration" => ancestor = n.parent(),
            "export_statement" => return true,
            _ => return false,
        }
    }
    false
}

/// Members are public unless TypeScript says `private`/`protected` or the name is `#private`.
fn member_visibility(node: Node, source: &str, name: Node) -> Modifiers {
    let mut cursor = node.walk();
    let hidden = node.children(&mut cursor).any(|c| {
        c.kind() == "accessibility_modifier"
            && text(c, source).is_some_and(|t| t == "private" || t == "protected")
    });
    if hidden || name.kind() == "private_property_identifier" {
        Modifiers::default()
    } else {
        Modifiers::PUBLIC
    }
}

/// `const Foo = () => ...` declares a function; `const LIMIT = 10` a constant. A capitalized
/// name wrapping a function (`memo(() => ...)`, `forwardRef(function ...)`) is a React
/// component, so it is a function too.
fn variable(node: Node, source: &str) -> Step {
    let Some(name) = node
        .child_by_field_name("name")
        .filter(|n| n.kind() == "identifier")
    else {
        return Step::Skip; // destructuring
    };
    let value = node.child_by_field_name("value");
    let function = value.filter(|v| {
        matches!(
            v.kind(),
            "arrow_function" | "function_expression" | "function" | "generator_function"
        )
    });
    let capitalized = source
        .get(name.start_byte()..name.end_byte())
        .is_some_and(|n| n.starts_with(|c: char| c.is_uppercase()));
    let wrapped_function = value.is_some_and(|v| v.kind() == "call_expression")
        && capitalized
        && value.is_some_and(wraps_function);

    let (kind, signature_end, async_fn) = if let Some(function) = function {
        let end = function.child_by_field_name("body").map(|b| b.start_byte());
        (StubKind::Function, end, has_child(function, "async"))
    } else if wrapped_function {
        (StubKind::Function, None, false)
    } else if value.is_some_and(|v| v.kind() == "class") {
        (StubKind::Class, None, false)
    } else {
        (StubKind::Const, None, false)
    };
    let mut modifiers = if is_exported(node) {
        Modifiers::PUBLIC
    } else {
        Modifiers::default()
    };
    if async_fn {
        modifiers = modifiers.with(Modifiers::ASYNC);
    }
    Step::Declare {
        items: vec![Item {
            kind,
            name: text(name, source),
            name_range: name.start_byte()..name.end_byte(),
            modifiers,
            import: None,
            signature_end,
        }],
        descend: false,
    }
}

/// A call with a function among its arguments: `memo(() => ...)`.
fn wraps_function(call: Node) -> bool {
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return false;
    };
    let mut cursor = arguments.walk();
    arguments.named_children(&mut cursor).any(|a| {
        matches!(a.kind(), "arrow_function" | "function_expression" | "function")
            // `forwardRef(...)` wrapped in `memo(...)`
            || (a.kind() == "call_expression" && wraps_function(a))
    })
}

/// One import per name: `import a, { b as c } from "m"` imports `m#default` and `m#b`.
fn imports(node: Node, source: &str) -> Step {
    let Some(module) = node
        .child_by_field_name("source")
        .and_then(|n| text(n, source))
    else {
        return Step::Skip;
    };
    let target = |member: Option<String>, alias: Option<String>, glob: bool| ImportTarget {
        path: module.clone().into(),
        member: member.map(Into::into),
        alias: alias.map(Into::into),
        glob,
    };

    let mut targets = Vec::new();
    let mut cursor = node.walk();
    let clause = node
        .children(&mut cursor)
        .find(|c| c.kind() == "import_clause");
    if let Some(clause) = clause {
        let mut cursor = clause.walk();
        for child in clause.named_children(&mut cursor) {
            match child.kind() {
                "identifier" => {
                    targets.push(target(Some("default".into()), text(child, source), false));
                }
                "namespace_import" => {
                    let mut cursor = child.walk();
                    let alias = child
                        .named_children(&mut cursor)
                        .find(|c| c.kind() == "identifier")
                        .and_then(|c| text(c, source));
                    targets.push(target(None, alias, true));
                }
                "named_imports" => {
                    let mut cursor = child.walk();
                    for specifier in child.named_children(&mut cursor) {
                        let member = specifier
                            .child_by_field_name("name")
                            .and_then(|n| text(n, source));
                        let alias = specifier
                            .child_by_field_name("alias")
                            .and_then(|n| text(n, source));
                        targets.push(target(member, alias, false));
                    }
                }
                _ => {}
            }
        }
    } else {
        targets.push(target(None, None, false)); // `import "./polyfill"`
    }

    let items = targets
        .into_iter()
        .map(|target| Item {
            kind: StubKind::Import,
            name: target
                .alias
                .as_deref()
                .or(target.member.as_deref())
                .or_else(|| target.path.rsplit('/').next())
                .map(str::to_string),
            name_range: 0..0,
            modifiers: Modifiers::default(),
            import: Some(target),
            signature_end: None,
        })
        .collect();
    Step::Declare {
        items,
        descend: false,
    }
}

#[cfg(test)]
mod tests {
    use crate::{StubKind::*, StubNode, extract_nodes, language_for_path};

    fn nodes(path: &str, source: &str) -> Vec<StubNode> {
        let language = language_for_path(path).expect("language");
        extract_nodes(language, source)
    }

    fn summary(nodes: &[StubNode]) -> Vec<(crate::StubKind, String)> {
        nodes
            .iter()
            .map(|n| (n.kind, n.name.as_deref().unwrap_or("").to_string()))
            .collect()
    }

    #[test]
    fn react_components_are_functions() {
        let source = r#"
import React, { useState as useS } from "react";
export const Button = ({ label }: Props) => <button>{label}</button>;
const Card = memo(forwardRef((p, r) => <div ref={r} />));
export default function App() { return <Button label="x" />; }
const LIMIT = 10;
"#;
        let n = nodes("a.tsx", source);
        let s = summary(&n);
        assert!(s.contains(&(Function, "Button".into())));
        assert!(s.contains(&(Function, "Card".into())));
        assert!(s.contains(&(Function, "App".into())));
        assert!(s.contains(&(Const, "LIMIT".into())));
        let button = n
            .iter()
            .find(|n| n.name.as_deref() == Some("Button"))
            .unwrap();
        assert!(button.modifiers.contains(crate::Modifiers::PUBLIC));
        assert_eq!(
            button.signature.as_deref(),
            Some("Button = ({ label }: Props) =>")
        );
        let card = n
            .iter()
            .find(|n| n.name.as_deref() == Some("Card"))
            .unwrap();
        assert!(!card.modifiers.contains(crate::Modifiers::PUBLIC));
    }

    #[test]
    fn imports_are_one_per_name() {
        let n = nodes(
            "a.ts",
            "import a, { b as c, d } from \"m\";\nimport * as ns from \"n\";\nimport \"./side\";",
        );
        let imports: Vec<_> = n.iter().filter_map(|n| n.import.as_ref()).collect();
        assert_eq!(imports.len(), 5);
        assert_eq!(imports[0].member.as_deref(), Some("default"));
        assert_eq!(imports[1].member.as_deref(), Some("b"));
        assert_eq!(imports[1].alias.as_deref(), Some("c"));
        assert!(imports[3].glob);
        assert_eq!(&*imports[4].path, "./side");
    }

    #[test]
    fn classes_and_interfaces_hold_members() {
        let source = r#"
export class Repo extends Base {
    private cache = 1;
    public name: string;
    async load(id: number): Promise<void> {}
    #secret() {}
}
interface Props { label: string; onClick(): void }
type Id = string;
enum Color { Red, Green = 2 }
"#;
        let s = summary(&nodes("a.ts", source));
        for expected in [
            (Class, "Repo"),
            (Field, "cache"),
            (Field, "name"),
            (Method, "load"),
            (Method, "#secret"),
            (Interface, "Props"),
            (Field, "label"),
            (Method, "onClick"),
            (TypeAlias, "Id"),
            (Enum, "Color"),
            (Variant, "Red"),
            (Variant, "Green"),
        ] {
            assert!(
                s.contains(&(expected.0, expected.1.to_string())),
                "missing {expected:?} in {s:?}"
            );
        }
    }

    #[test]
    fn member_visibility_follows_typescript_modifiers() {
        let n = nodes(
            "a.ts",
            "class A { private x = 1; y = 2; #z = 3; protected m() {} }",
        );
        let public = |name: &str| {
            n.iter()
                .find(|n| n.name.as_deref() == Some(name))
                .unwrap()
                .modifiers
                .contains(crate::Modifiers::PUBLIC)
        };
        assert!(!public("x"));
        assert!(public("y"));
        assert!(!public("#z"));
        assert!(!public("m"));
    }

    #[test]
    fn function_bodies_are_pruned() {
        let s = summary(&nodes(
            "a.js",
            "function outer() { function inner() {} const local = 1; }",
        ));
        assert_eq!(s, [(Function, "outer".to_string())]);
    }
}
