//! The other end of the construct under the caret: a bracket's partner, the closing JSX tag of
//! an opening one, the last line of the block a Python `:` opens.

use std::ops::Range;

use tree_sitter::{Node, Tree};

use crate::{LanguageId, Syntax, grammars};

/// Whether a caret on `c` can be at one end of a pair, so that the text need not be parsed for
/// the many positions where it cannot. Identifier characters only count in languages with JSX.
pub fn is_pair_candidate(language: LanguageId, c: char) -> bool {
    matches!(c, '{' | '}' | '(' | ')' | '[' | ']' | ':')
        || (has_jsx(language) && (c.is_alphanumeric() || matches!(c, '_' | '.' | '$')))
}

fn has_jsx(language: LanguageId) -> bool {
    grammars()[language.0]
        .extensions
        .iter()
        .any(|e| matches!(*e, "jsx" | "tsx"))
}

/// The two ends of the pair the caret at byte `at` of `source` belongs to, as byte ranges
/// (the one under the caret first). `None` when it is on no pair, or its other end is missing.
pub fn matching_pair(language: LanguageId, source: &str, at: usize) -> Option<[Range<usize>; 2]> {
    if !is_pair_candidate(language, source.get(at..)?.chars().next()?) {
        return None; // not worth a parse
    }
    matching_pair_in(language, Syntax::new(language, source)?.tree(), source, at)
}

/// [`matching_pair`] on the syntax tree of `source`, already parsed.
pub(crate) fn matching_pair_in(
    language: LanguageId,
    tree: &Tree,
    source: &str,
    at: usize,
) -> Option<[Range<usize>; 2]> {
    let c = source.get(at..)?.chars().next()?;
    if !is_pair_candidate(language, c) {
        return None;
    }
    let leaf = tree
        .root_node()
        .descendant_for_byte_range(at, at + c.len_utf8())?;
    if !leaf.is_named() && bracket_pair(leaf.kind()).is_some() {
        let other = other_bracket(leaf)?;
        return Some([range(leaf), range(other)]);
    }
    if leaf.kind() == ":" {
        let block = leaf.next_sibling().filter(|n| n.kind() == "block")?;
        return Some([range(leaf), last_line(source, leaf, block)?]);
    }
    other_tag(leaf)
}

fn range(node: Node) -> Range<usize> {
    node.start_byte()..node.end_byte()
}

/// `(open, close, is the token the opening one)`.
fn bracket_pair(kind: &str) -> Option<(&'static str, &'static str, bool)> {
    match kind {
        "{" => Some(("{", "}", true)),
        "}" => Some(("{", "}", false)),
        "(" => Some(("(", ")", true)),
        ")" => Some(("(", ")", false)),
        "[" => Some(("[", "]", true)),
        "]" => Some(("[", "]", false)),
        _ => None,
    }
}

/// The bracket among `leaf`'s siblings that closes (or opens) it, nesting counted.
fn other_bracket(leaf: Node) -> Option<Node> {
    let (open, close, opening) = bracket_pair(leaf.kind())?;
    let parent = leaf.parent()?;
    let mut cursor = parent.walk();
    let mut siblings: Vec<Node> = parent.children(&mut cursor).collect();
    let at = siblings.iter().position(|n| n.id() == leaf.id())?;
    let (mine, theirs) = if opening {
        (open, close)
    } else {
        (close, open)
    };
    let rest: Vec<Node> = if opening {
        siblings.split_off(at + 1)
    } else {
        siblings.truncate(at);
        siblings.into_iter().rev().collect()
    };
    let mut depth = 1;
    for node in rest {
        if node.is_named() {
            continue;
        }
        if node.kind() == mine {
            depth += 1;
        } else if node.kind() == theirs {
            depth -= 1;
            if depth == 0 {
                return Some(node);
            }
        }
    }
    None
}

/// The text of the last line of `block`, without its indentation: where the block that the
/// `colon` opens ends. `None` for a body on the line of its header.
fn last_line(source: &str, colon: Node, block: Node) -> Option<Range<usize>> {
    if block.start_position().row == colon.start_position().row {
        return None;
    }
    let end = block.end_byte();
    let line = source[..end].rfind('\n').map_or(0, |i| i + 1);
    let indent = source[line..end].len() - source[line..end].trim_start().len();
    Some(line + indent..end)
}

/// For a caret on the name of a JSX tag, the name in the tag at the other end of the element.
fn other_tag(leaf: Node) -> Option<[Range<usize>; 2]> {
    let mut name = leaf;
    while matches!(
        name.parent()?.kind(),
        "member_expression" | "nested_identifier" | "jsx_namespace_name"
    ) {
        name = name.parent()?;
    }
    let tag = name.parent()?;
    let field = match tag.kind() {
        "jsx_opening_element" => "close_tag",
        "jsx_closing_element" => "open_tag",
        _ => return None,
    };
    if tag.child_by_field_name("name")?.id() != name.id() {
        return None;
    }
    let other = tag.parent()?.child_by_field_name(field)?;
    Some([range(name), range(other.child_by_field_name("name")?)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_for_path;

    fn pair(path: &str, source: &str, marker: &str) -> Option<[String; 2]> {
        let at = source.find(marker).unwrap();
        let language = language_for_path(path).unwrap();
        matching_pair(language, source, at).map(|[a, b]| [source[a].into(), source[b].into()])
    }

    fn ends(path: &str, source: &str, marker: &str) -> Option<[usize; 2]> {
        let at = source.find(marker).unwrap();
        let language = language_for_path(path).unwrap();
        matching_pair(language, source, at).map(|[a, b]| [a.start, b.start])
    }

    #[test]
    fn an_opening_brace_finds_its_closing_one_through_nested_pairs() {
        let source = "fn a() { if x { y } }";
        assert_eq!(ends("a.rs", source, "{ if"), Some([7, 20]));
        assert_eq!(ends("a.rs", source, "{ y"), Some([14, 18]));
    }

    #[test]
    fn a_closing_bracket_finds_its_opening_one() {
        let source = "fn a() { f(one, [1, 2]); }";
        assert_eq!(ends("a.rs", source, ");"), Some([22, 10]));
        assert_eq!(ends("a.rs", source, "]"), Some([21, 16]));
    }

    #[test]
    fn brackets_inside_strings_and_comments_are_not_pairs() {
        let source = "fn a() { let s = \"(}\"; } // )";
        assert_eq!(ends("a.rs", source, "{ let"), Some([7, 23]));
        assert_eq!(ends("a.rs", source, "(}"), None); // inside the string
    }

    #[test]
    fn works_in_every_bracket_language() {
        assert_eq!(
            ends("a.ts", "const f = (a: number) => { return a; };", "("),
            Some([10, 20])
        );
        assert_eq!(
            ends("a.go", "func f() {\n\treturn\n}\n", "{"),
            Some([9, 19])
        );
        assert_eq!(ends("a.py", "x = [1, (2, 3)]\n", "(2"), Some([8, 13]));
    }

    #[test]
    fn an_unmatched_bracket_and_other_characters_have_no_pair() {
        assert_eq!(ends("a.rs", "fn a() { ", "{"), None);
        assert_eq!(ends("a.rs", "fn a() {}", "a"), None);
    }

    #[test]
    fn a_python_colon_points_to_the_last_line_of_its_block() {
        let source = "def f(x):\n    if x:\n        a = 1\n        return a\n\ny = 2\n";
        assert_eq!(
            pair("a.py", source, "x):"),
            None // the caret is on `x`, not on the colon
        );
        let colon = source.find("):").unwrap() + 1;
        let language = language_for_path("a.py").unwrap();
        let [here, there] = matching_pair(language, source, colon).unwrap();
        assert_eq!(&source[here], ":");
        assert_eq!(&source[there], "return a");
        let inner = source.find("x:\n").unwrap() + 1;
        let [_, there] = matching_pair(language, source, inner).unwrap();
        assert_eq!(&source[there], "return a");
    }

    #[test]
    fn a_python_colon_that_is_not_a_block_has_no_pair() {
        assert_eq!(ends("a.py", "d = {1: 2}\n", ":"), None);
        assert_eq!(ends("a.py", "if x: pass\n", ":"), None); // body on the header's line
    }

    #[test]
    fn a_jsx_tag_name_finds_the_name_in_the_other_tag() {
        let source = "const a = <Box.Item id=\"1\"><span>x</span></Box.Item>;";
        assert_eq!(
            pair("a.tsx", source, "span>x"),
            Some(["span".into(), "span".into()])
        );
        assert_eq!(
            pair("a.tsx", source, "Item id"),
            Some(["Box.Item".into(), "Box.Item".into()])
        );
        assert_eq!(pair("a.tsx", source, "id="), None); // an attribute is not a tag name
        assert_eq!(pair("a.tsx", "const a = <br />;", "br"), None); // self-closing
    }
}
