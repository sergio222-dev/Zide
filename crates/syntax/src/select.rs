//! Extend Selection (IntelliJ's Ctrl+W): the next larger range around the selection, from the
//! word under the caret up through the syntax tree to the whole file.
//!
//! Like IntelliJ's `SelectWordUtil`, every enclosing construct offers its candidate ranges and
//! the smallest one that strictly contains the current selection wins.

use std::ops::Range;

use tree_sitter::Parser;

use crate::{LanguageId, grammars};

/// The range to select next, in bytes of `source`. `selection` is what is selected now (empty
/// at the caret when nothing is). `None` when the whole text is selected already.
pub fn extend_selection(
    language: Option<LanguageId>,
    source: &str,
    selection: Range<usize>,
) -> Option<Range<usize>> {
    let mut candidates = vec![word_at(source, selection.start)];
    match language.and_then(|language| syntax_ranges(language, source, selection.clone())) {
        Some(ranges) => candidates.extend(ranges.into_iter().map(Some)),
        None => candidates.push(line_at(source, selection.start)),
    }
    candidates.push(Some(0..source.len()));
    candidates
        .into_iter()
        .flatten()
        .filter(|c| {
            !c.is_empty() && c.start <= selection.start && selection.end <= c.end && *c != selection
        })
        .min_by_key(|c| c.len())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier the caret is on or right after.
fn word_at(source: &str, caret: usize) -> Option<Range<usize>> {
    let at = |offset: usize| source[offset..].chars().next().filter(|&c| is_word(c));
    let on = at(caret).is_some();
    let after = source[..caret].chars().next_back().is_some_and(is_word);
    if !on && !after {
        return None;
    }
    let start = source[..caret]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(caret, |(i, _)| i);
    let end = caret
        + source[caret..]
            .char_indices()
            .take_while(|&(_, c)| is_word(c))
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
    Some(start..end)
}

/// The line's text without its indentation and line break.
fn line_at(source: &str, caret: usize) -> Option<Range<usize>> {
    let start = source[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = source[caret..]
        .find('\n')
        .map_or(source.len(), |i| caret + i);
    trim(source, start..end)
}

fn trim(source: &str, range: Range<usize>) -> Option<Range<usize>> {
    let text = &source[range.clone()];
    let trimmed = text.trim_start();
    let start = range.start + (text.len() - trimmed.len());
    let end = start + trimmed.trim_end().len();
    (start < end).then_some(start..end)
}

/// What each syntax node around the selection offers: its inside (between its brackets or
/// quotes) and itself.
fn syntax_ranges(
    language: LanguageId,
    source: &str,
    selection: Range<usize>,
) -> Option<Vec<Range<usize>>> {
    let mut parser = Parser::new();
    parser.set_language(&grammars()[language.0].language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(selection.start, selection.end)?;
    let mut ranges = Vec::new();
    loop {
        if node.is_named() {
            if let Some(inside) = inside_delimiters(source, node.start_byte()..node.end_byte()) {
                ranges.push(inside);
            }
            ranges.push(node.start_byte()..node.end_byte());
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => return Some(ranges),
        }
    }
}

/// The text between a pair of brackets or quotes that open and close `range`, trimmed.
fn inside_delimiters(source: &str, range: Range<usize>) -> Option<Range<usize>> {
    let text = &source[range.clone()];
    let first = text.chars().next()?;
    let last = text.chars().next_back()?;
    let closes = match first {
        '{' => '}',
        '(' => ')',
        '[' => ']',
        '"' | '\'' | '`' => first,
        _ => return None,
    };
    if last != closes || text.len() < first.len_utf8() + last.len_utf8() {
        return None;
    }
    trim(
        source,
        range.start + first.len_utf8()..range.end - last.len_utf8(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_for_path;

    /// Repeatedly extends from `caret` and returns the selected texts, in order.
    fn chain(path: &str, source: &str, caret: usize) -> Vec<String> {
        let language = language_for_path(path);
        let mut selection = caret..caret;
        let mut texts = Vec::new();
        while let Some(next) = extend_selection(language, source, selection) {
            texts.push(source[next.clone()].to_string());
            selection = next;
        }
        texts
    }

    #[test]
    fn grows_from_the_word_through_the_syntax_tree() {
        let source = "fn main() {\n    let total = add(one, two);\n}\n";
        let caret = source.find("one").unwrap() + 1;
        let steps = chain("a.rs", source, caret);
        assert_eq!(steps[0], "one");
        assert_eq!(steps[1], "one, two");
        assert_eq!(steps[2], "(one, two)");
        assert_eq!(steps[3], "add(one, two)");
        assert!(steps.contains(&"let total = add(one, two);".to_string()));
        assert_eq!(steps.last().unwrap(), source);
    }

    #[test]
    fn a_string_is_selected_without_then_with_its_quotes() {
        let source = "const s = \"hello world\";\n";
        let caret = source.find("world").unwrap();
        let steps = chain("a.ts", source, caret);
        assert_eq!(&steps[..3], ["world", "hello world", "\"hello world\""]);
    }

    #[test]
    fn a_block_is_selected_without_then_with_its_braces() {
        let source = "fn f() {\n    one();\n    two();\n}\n";
        let caret = source.find("two").unwrap();
        let steps = chain("a.rs", source, caret);
        let inside = steps
            .iter()
            .position(|s| s == "one();\n    two();")
            .unwrap();
        let braces = steps
            .iter()
            .position(|s| s == "{\n    one();\n    two();\n}")
            .unwrap();
        assert!(inside < braces);
    }

    #[test]
    fn the_caret_right_after_a_word_still_selects_it() {
        assert_eq!(chain("a.rs", "let abc = 1;", 7)[0], "abc");
    }

    #[test]
    fn multibyte_text_does_not_split_characters() {
        let source = "let ñandú = \"ü\";";
        let steps = chain("a.rs", source, source.find('ñ').unwrap());
        assert_eq!(steps[0], "ñandú");
        let steps = chain("a.rs", source, source.find('ü').unwrap());
        assert_eq!(&steps[..2], ["ü", "\"ü\""]);
    }

    #[test]
    fn files_without_a_grammar_grow_by_word_line_and_file() {
        let source = "first line\n  second one\nthird\n";
        let caret = source.find("one").unwrap();
        let steps = chain("notes.txt", source, caret);
        assert_eq!(steps, ["one", "second one", source]);
    }

    #[test]
    fn nothing_is_left_once_everything_is_selected() {
        let source = "x";
        assert_eq!(extend_selection(None, source, 0..1), None);
        assert_eq!(extend_selection(None, "", 0..0), None);
    }
}
