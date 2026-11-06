//! Where the editor can fold: blocks, runs of imports and comments that span several lines.

use tree_sitter::{Node, Parser};

use crate::{LanguageId, grammars};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    /// Anything between a pair of brackets (or a JSX element), or a whole declaration body.
    Block,
    /// Consecutive import statements, folded together.
    Imports,
    /// A block comment, or consecutive line comments.
    Comment,
}

/// Lines `start_line..=end_line` (0-based) can be folded into the first one: the region
/// hides `start_line + 1..=end_line`, so `end_line > start_line` always holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRegion {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldKind,
}

/// What a grammar folds, by node kind.
struct Rules {
    blocks: &'static [&'static str],
    imports: &'static [&'static str],
    /// A comment node that is a single line (`//`), foldable only in runs.
    line_comment: fn(&str, Node) -> bool,
    /// A comment node that may span several lines (`/* */`).
    block_comment: fn(&str, Node) -> bool,
}

fn rules_for(extension: &str) -> Option<Rules> {
    match extension {
        "rs" => Some(Rules {
            blocks: &[
                "block",
                "declaration_list",
                "field_declaration_list",
                "field_initializer_list",
                "enum_variant_list",
                "match_block",
                "token_tree",
            ],
            imports: &["use_declaration"],
            line_comment: |_, node| node.kind() == "line_comment",
            block_comment: |_, node| node.kind() == "block_comment",
        }),
        "js" | "ts" | "tsx" => Some(Rules {
            blocks: &[
                "statement_block",
                "class_body",
                "object",
                "array",
                "switch_body",
                "object_type",
                "interface_body",
                "enum_body",
                "named_imports",
                "jsx_element",
            ],
            imports: &["import_statement"],
            line_comment: |source, node| {
                node.kind() == "comment" && source[node.start_byte()..].starts_with("//")
            },
            block_comment: |source, node| {
                node.kind() == "comment" && source[node.start_byte()..].starts_with("/*")
            },
        }),
        "go" => Some(Rules {
            blocks: &[
                "block",
                "field_declaration_list",
                "interface_type",
                "literal_value",
                "import_spec_list",
                "var_declaration",
                "const_declaration",
            ],
            imports: &["import_declaration"],
            line_comment: |source, node| {
                node.kind() == "comment" && source[node.start_byte()..].starts_with("//")
            },
            block_comment: |source, node| {
                node.kind() == "comment" && source[node.start_byte()..].starts_with("/*")
            },
        }),
        _ => None,
    }
}

/// The foldable regions of `source`, by start line (the outermost one when several start on
/// the same line). Empty for a language without folding rules.
pub fn fold_regions(language: LanguageId, source: &str) -> Vec<FoldRegion> {
    let grammar = &grammars()[language.0];
    let Some(rules) = grammar.extensions.first().and_then(|e| rules_for(e)) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&grammar.language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let mut regions = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        visit(&rules, source, node, &mut regions);
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return outermost(regions);
            }
        }
    }
}

fn visit(rules: &Rules, source: &str, node: Node, regions: &mut Vec<FoldRegion>) {
    let kind = node.kind();
    if rules.blocks.contains(&kind) {
        push(regions, node, closing_line(source, node), FoldKind::Block);
    } else if (rules.block_comment)(source, node) {
        push(regions, node, closing_line(source, node), FoldKind::Comment);
    }
    runs(rules, source, node, regions);
}

/// Consecutive children that are imports, or line comments on consecutive lines.
fn runs(rules: &Rules, source: &str, parent: Node, regions: &mut Vec<FoldRegion>) {
    let mut run: Option<(FoldKind, Node, Node)> = None; // kind, first, last
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        let kind = if rules.imports.contains(&child.kind()) {
            Some(FoldKind::Imports)
        } else if (rules.line_comment)(source, child) {
            Some(FoldKind::Comment)
        } else {
            None
        };
        run = match (run, kind) {
            (Some((k, first, last)), Some(next))
                if k == next
                    && (k == FoldKind::Imports
                        || child.start_position().row == last_row(last) + 1) =>
            {
                Some((k, first, child))
            }
            (previous, next) => {
                if let Some((k, first, last)) = previous {
                    push(regions, first, last_row(last), k);
                }
                next.map(|k| (k, child, child))
            }
        };
    }
    if let Some((k, first, last)) = run {
        push(regions, first, last_row(last), k);
    }
}

/// The row a node's text ends on; a Rust doc comment swallows its newline and so ends at the
/// start of the next row.
fn last_row(node: Node) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row - 1
    } else {
        end.row
    }
}

fn push(regions: &mut Vec<FoldRegion>, node: Node, end_line: usize, kind: FoldKind) {
    let start_line = node.start_position().row;
    if end_line > start_line {
        regions.push(FoldRegion {
            start_line,
            end_line,
            kind,
        });
    }
}

/// The last line a fold of `node` hides: the one with its closing bracket, unless a word
/// follows it there (`} else {`), which has to stay visible.
fn closing_line(source: &str, node: Node) -> usize {
    let end = node.end_byte();
    let rest = source[end..].split('\n').next().unwrap_or_default();
    let row = node.end_position().row;
    if rest.trim_start().starts_with(|c: char| c.is_alphabetic()) {
        row.saturating_sub(1)
    } else {
        row
    }
}

/// One region per start line: the one that reaches furthest, so a fold never hides only part
/// of another region that starts on its first line. Sorted by start line.
fn outermost(mut regions: Vec<FoldRegion>) -> Vec<FoldRegion> {
    regions.sort_by_key(|r| (r.start_line, std::cmp::Reverse(r.end_line)));
    regions.dedup_by_key(|r| r.start_line);
    regions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_for_path;

    fn regions(path: &str, source: &str) -> Vec<(usize, usize, FoldKind)> {
        fold_regions(language_for_path(path).unwrap(), source)
            .into_iter()
            .map(|r| (r.start_line, r.end_line, r.kind))
            .collect()
    }

    #[test]
    fn rust_folds_imports_comments_and_blocks() {
        let source = "use a;\nuse b;\n\n/// doc\n/// more\nfn f() {\n    if x {\n        1\n    } else {\n        2\n    }\n}\nstruct S {\n    a: u8,\n}\n";
        assert_eq!(
            regions("a.rs", source),
            [
                (0, 1, FoldKind::Imports),
                (3, 4, FoldKind::Comment),
                (5, 11, FoldKind::Block),
                (6, 7, FoldKind::Block), // `} else {` stays visible
                (8, 10, FoldKind::Block),
                (12, 14, FoldKind::Block),
            ]
        );
    }

    #[test]
    fn typescript_folds_imports_comments_classes_and_literals() {
        let source = "import { a } from \"a\";\nimport b from \"b\";\n\n/* one\n two */\nclass C {\n  m() {\n    return { x: 1,\n      y: 2 };\n  }\n}\n";
        assert_eq!(
            regions("a.ts", source),
            [
                (0, 1, FoldKind::Imports),
                (3, 4, FoldKind::Comment),
                (5, 10, FoldKind::Block),
                (6, 9, FoldKind::Block),
                (7, 8, FoldKind::Block),
            ]
        );
    }

    #[test]
    fn jsx_elements_fold_in_tsx() {
        let source = "const a = (\n  <div>\n    <b/>\n  </div>\n);\n";
        assert_eq!(regions("a.tsx", source), [(1, 3, FoldKind::Block)]);
    }

    #[test]
    fn a_closing_line_with_punctuation_is_hidden_too() {
        let source = "f(() => {\n  g({\n    a: 1,\n  });\n});\n";
        assert_eq!(
            regions("a.js", source),
            [(0, 4, FoldKind::Block), (1, 3, FoldKind::Block)]
        );
    }

    #[test]
    fn of_two_regions_starting_on_one_line_the_outer_one_stays() {
        let source = "const v = { a: [\n  1,\n  2,\n], z: 1,\n};\n";
        assert_eq!(regions("a.js", source), [(0, 4, FoldKind::Block)]);
    }

    #[test]
    fn single_lines_and_lone_comments_do_not_fold() {
        let source = "// a\nfn g() {}\nconst x = [1, 2];\n";
        assert_eq!(regions("a.rs", "// a\nfn g() {}\n"), []);
        assert_eq!(regions("a.js", source), []);
    }

    #[test]
    fn line_comments_fold_only_on_consecutive_lines() {
        let source = "// a\n// b\n// c\nx();\n// d\n\n// e\n";
        assert_eq!(regions("a.js", source), [(0, 2, FoldKind::Comment)]);
    }

    #[test]
    fn a_language_without_rules_has_no_regions() {
        assert_eq!(regions("a.py", "def f():\n    pass\n"), []);
    }

    #[test]
    fn go_folds_imports_types_and_function_bodies() {
        let source = "package main\n\nimport (\n\t\"a\"\n\t\"b\"\n)\n\ntype S struct {\n\tx int\n}\n\nfunc f() {\n\tif x {\n\t\ty()\n\t}\n}\n";
        assert_eq!(
            regions("a.go", source),
            [
                (2, 5, FoldKind::Imports),
                (7, 9, FoldKind::Block),
                (11, 15, FoldKind::Block),
                (12, 14, FoldKind::Block),
            ]
        );
    }
}
