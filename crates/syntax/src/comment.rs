//! How each language writes comments, and toggling them on a run of lines.

use crate::{LanguageId, grammars};

/// The comment markers of a language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentSyntax {
    /// Marker of a comment that runs to the end of the line (`//`, `#`).
    pub line: Option<&'static str>,
    /// Opening and closing markers, for languages without a line comment.
    pub block: Option<(&'static str, &'static str)>,
    /// How a comment goes between JSX tags, where `//` would be text.
    pub jsx: Option<(&'static str, &'static str)>,
}

impl CommentSyntax {
    pub(crate) const SLASHES: Self = Self {
        line: Some("//"),
        block: Some(("/*", "*/")),
        jsx: None,
    };
    pub(crate) const JSX: Self = Self {
        jsx: Some(("{/*", "*/}")),
        ..Self::SLASHES
    };
    pub(crate) const HASH: Self = Self {
        line: Some("#"),
        block: None,
        jsx: None,
    };
    pub(crate) const HTML: Self = Self {
        line: None,
        block: Some(("<!--", "-->")),
        jsx: None,
    };
}

pub fn comment_syntax(language: LanguageId) -> CommentSyntax {
    grammars()[language.0].comment
}

/// Toggles comments on `lines`: when every non-blank line is a comment the markers come off,
/// otherwise they go on all of them, at the shallowest indentation among them. A language
/// without a line comment wraps each line in its block markers; `markup` (every line is JSX
/// markup) wraps them in the JSX ones. Blank lines are left alone.
pub fn toggle_comment(syntax: CommentSyntax, lines: &[String], markup: bool) -> Vec<String> {
    let marked: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if marked.is_empty() {
        return lines.to_vec();
    }
    let line = syntax.line.map(|open| (open, ""));
    let candidates = [syntax.jsx, line, syntax.block];
    let commented = |(open, close): (&str, &str), line: &str| {
        let text = line.trim();
        text.starts_with(open) && text.ends_with(close)
    };
    let uncomment = candidates
        .into_iter()
        .flatten()
        .find(|&markers| marked.iter().all(|l| commented(markers, l)));
    let comment = if markup { syntax.jsx } else { None }
        .or(line)
        .or(syntax.block);
    let Some((open, close)) = uncomment.or(comment) else {
        return lines.to_vec();
    };
    let indent = marked
        .iter()
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return line.clone();
            }
            if uncomment.is_some() {
                let own = line.chars().take_while(|c| c.is_whitespace()).count();
                let (head, rest) = split_indent(line, own);
                let rest = rest.trim_end().strip_prefix(open).unwrap_or(rest);
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                let rest = rest.strip_suffix(close).unwrap_or(rest);
                let rest = if close.is_empty() {
                    rest
                } else {
                    rest.strip_suffix(' ').unwrap_or(rest)
                };
                format!("{head}{rest}")
            } else {
                let (head, rest) = split_indent(line, indent);
                if close.is_empty() {
                    format!("{head}{open} {rest}")
                } else {
                    format!("{head}{open} {} {close}", rest.trim_end())
                }
            }
        })
        .collect()
}

/// True when the text at `byte` is JSX markup (tags and their children), where a comment
/// is `{/* */}`; inside `{ }` or between the attributes of a tag it is plain JavaScript.
pub fn in_jsx_markup(tree: &tree_sitter::Tree, byte: usize) -> bool {
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte + 1);
    while let Some(n) = node {
        match n.kind() {
            "jsx_expression" => return false,
            "jsx_opening_element" | "jsx_self_closing_element" | "jsx_closing_element" => {
                return n.start_byte() == byte;
            }
            "jsx_element" | "jsx_fragment" => return true,
            _ => node = n.parent(),
        }
    }
    false
}

/// `line` cut after its first `chars` characters.
fn split_indent(line: &str, chars: usize) -> (&str, &str) {
    let at = line
        .char_indices()
        .nth(chars)
        .map_or(line.len(), |(i, _)| i);
    line.split_at(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn comments_at_the_shallowest_indent_and_skips_blank_lines() {
        let out = toggle_comment(
            CommentSyntax::SLASHES,
            &lines(&["    a", "", "      b"]),
            false,
        );
        assert_eq!(out, lines(&["    // a", "", "    //   b"]));
    }

    #[test]
    fn uncomments_when_every_line_is_a_comment() {
        let out = toggle_comment(
            CommentSyntax::SLASHES,
            &lines(&["    // a", "    //b"]),
            false,
        );
        assert_eq!(out, lines(&["    a", "    b"]));
    }

    #[test]
    fn one_plain_line_makes_the_run_get_commented() {
        let out = toggle_comment(CommentSyntax::HASH, &lines(&["# a", "b"]), false);
        assert_eq!(out, lines(&["# # a", "# b"]));
    }

    #[test]
    fn toggling_twice_restores_the_text() {
        for syntax in [
            CommentSyntax::SLASHES,
            CommentSyntax::HASH,
            CommentSyntax::HTML,
        ] {
            let original = lines(&["  fn a() {", "", "\t  b", "  }"]);
            let commented = toggle_comment(syntax, &original, false);
            assert_ne!(commented, original);
            assert_eq!(toggle_comment(syntax, &commented, false), original);
        }
    }

    #[test]
    fn languages_without_a_line_comment_wrap_each_line() {
        let out = toggle_comment(CommentSyntax::HTML, &lines(&["# Title", "text"]), false);
        assert_eq!(out, lines(&["<!-- # Title -->", "<!-- text -->"]));
    }

    #[test]
    fn blank_lines_only_stay_as_they_are() {
        let blank = lines(&["", "  "]);
        assert_eq!(toggle_comment(CommentSyntax::SLASHES, &blank, false), blank);
    }

    #[test]
    fn markup_lines_get_jsx_comments_and_they_come_off_again() {
        let original = lines(&["  <div>", "    text", "  </div>"]);
        let commented = toggle_comment(CommentSyntax::JSX, &original, true);
        assert_eq!(
            commented,
            lines(&["  {/* <div> */}", "  {/*   text */}", "  {/* </div> */}"])
        );
        assert_eq!(
            toggle_comment(CommentSyntax::JSX, &commented, false),
            original
        );
        let code = toggle_comment(CommentSyntax::JSX, &lines(&["a();"]), false);
        assert_eq!(code, lines(&["// a();"]));
    }
}
