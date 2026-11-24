//! Completion: what the server offers at the cursor, narrowed down here while typing goes on,
//! and the edit that accepting one makes.
//!
//! The server is asked once per word; each key typed after that filters and ranks the same
//! items locally (unless the server said the list was incomplete), so the popup follows
//! typing without a round trip per key.

use lsp_types::{
    CompletionItemKind, CompletionResponse, CompletionTextEdit, InsertTextFormat, Position, Range,
    TextEdit,
};
use serde_json::Value;
use zide_matcher::{Matcher, MatchingMode, MinusculeMatcher};

use crate::text::utf16_column;

/// Why the server is asked, which it may use to decide what to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTrigger {
    /// Typing a word, or asked for by hand.
    Invoked,
    /// The character just typed is one the server listed (`.`, `:`).
    Character(char),
    /// Typing went on over a list the server said was incomplete.
    Incomplete,
}

/// A server's answer: the items, and whether typing more needs a new question.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
    pub items: Vec<CompletionItem>,
    /// The server left items out: typing more must ask again instead of only filtering.
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    /// Shown right after the label, usually a signature (`(a: u8)`).
    pub label_detail: Option<String>,
    /// Shown apart from the label: a type, or where the item comes from.
    pub detail: Option<String>,
    pub kind: Option<CompletionItemKind>,
    /// What typing is matched against: the label unless the server says otherwise.
    pub filter_text: String,
    /// The server's own order, which breaks ties between items that match equally well.
    pub sort_text: String,
    /// The server's pick for the row selected first.
    pub preselect: bool,
    text: String,
    /// Where `text` goes, in the document as it was when asked; `None` is the word before the
    /// cursor.
    range: Option<Range>,
    /// Where the cursor ends up inside `text`, in chars.
    cursor: usize,
    /// Edits elsewhere that come with the item, such as the import it needs.
    additional: Vec<TextEdit>,
}

impl CompletionItem {
    /// An item that puts its label in place of the word typed, for completion made without a
    /// server (the words of the file, the project's symbols).
    pub fn plain(
        label: impl Into<String>,
        kind: Option<CompletionItemKind>,
        detail: Option<String>,
    ) -> Self {
        let label = label.into();
        Self {
            label_detail: None,
            detail,
            kind,
            filter_text: label.clone(),
            sort_text: label.clone(),
            preselect: false,
            cursor: label.chars().count(),
            text: label.clone(),
            range: None,
            additional: Vec::new(),
            label,
        }
    }

    /// The edit that puts this item in place of what was typed, with the cursor at char
    /// `column` of `line`, whose text is `line_text`. The server's range is stretched to the
    /// cursor, since typing may have gone on since the question; without one the word before
    /// the cursor is replaced.
    pub fn edit(&self, line: u32, line_text: &str, column: usize) -> TextEdit {
        let cursor = Position::new(line, utf16_column(line_text, column));
        let range = match self.range {
            Some(range) if range.end.line == line && range.end.character < cursor.character => {
                Range::new(range.start, cursor)
            }
            Some(range) => range,
            None => {
                let start = utf16_column(line_text, word_start(line_text, column));
                Range::new(Position::new(line, start), cursor)
            }
        };
        TextEdit {
            range,
            new_text: self.text.clone(),
        }
    }

    /// Where the cursor goes after accepting, in chars from the start of [`Self::edit`]'s
    /// text: a snippet's first stop, or the end.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Edits away from the cursor that accepting also makes (an import). They never overlap
    /// the main edit, and their positions are in the document before any edit.
    pub fn additional_edits(&self) -> &[TextEdit] {
        &self.additional
    }
}

/// A `textDocument/completion` answer: null, a list of items, or a `CompletionList`.
pub fn parse_completion(result: Value) -> Result<Completion, serde_json::Error> {
    if result.is_null() {
        return Ok(Completion::default());
    }
    let (items, incomplete) = match serde_json::from_value::<CompletionResponse>(result)? {
        CompletionResponse::Array(items) => (items, false),
        CompletionResponse::List(list) => (list.items, list.is_incomplete),
    };
    Ok(Completion {
        items: items.into_iter().map(item).collect(),
        incomplete,
    })
}

fn item(raw: lsp_types::CompletionItem) -> CompletionItem {
    let (text, range) = match raw.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => (edit.new_text, Some(edit.range)),
        // accepting inserts, as Enter does elsewhere: the rest of the word after the cursor stays
        Some(CompletionTextEdit::InsertAndReplace(edit)) => (edit.new_text, Some(edit.insert)),
        None => (raw.insert_text.unwrap_or_else(|| raw.label.clone()), None),
    };
    // we say we take no snippets, but a server may send them anyway
    let (text, cursor) = if raw.insert_text_format == Some(InsertTextFormat::SNIPPET) {
        plain_snippet(&text)
    } else {
        let end = text.chars().count();
        (text, end)
    };
    let (label_detail, description) = raw
        .label_details
        .map_or((None, None), |d| (d.detail, d.description));
    CompletionItem {
        filter_text: raw.filter_text.unwrap_or_else(|| raw.label.clone()),
        sort_text: raw.sort_text.unwrap_or_else(|| raw.label.clone()),
        preselect: raw.preselect.unwrap_or(false),
        label: raw.label,
        label_detail: label_detail.filter(|d| !d.is_empty()),
        detail: raw.detail.or(description).filter(|d| !d.is_empty()),
        kind: raw.kind,
        text,
        range,
        cursor,
        additional: raw.additional_text_edits.unwrap_or_default(),
    }
}

/// A snippet as plain text: a placeholder keeps its default (`${1:name}` is `name`), a choice
/// its first option, tab stops and variables go, and `\$`, `\}`, `\\` are the character.
/// Also returns where the cursor goes, in chars: the first stop, else `$0`, else the end.
fn plain_snippet(snippet: &str) -> (String, usize) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut text = String::new();
    let mut length = 0;
    let mut stops: Vec<(u32, usize)> = Vec::new();
    // placeholders whose default text is being copied, waiting for their `}`
    let mut open = 0;
    fn push(text: &mut String, length: &mut usize, c: char) {
        text.push(c);
        *length += 1;
    }
    // a tab stop is digits only (`$0b` is stop 0 then `b`), a variable a word
    let name_at = |from: usize| -> String {
        let rest = &chars[from.min(chars.len())..];
        if rest.first().is_some_and(char::is_ascii_digit) {
            rest.iter().take_while(|c| c.is_ascii_digit()).collect()
        } else {
            rest.iter()
                .take_while(|c| c.is_alphanumeric() || **c == '_')
                .collect()
        }
    };
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if matches!(chars.get(i + 1), Some('$' | '}' | '\\')) => {
                push(&mut text, &mut length, chars[i + 1]);
                i += 2;
            }
            '}' if open > 0 => {
                open -= 1;
                i += 1;
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let name = name_at(i + 2);
                let after = i + 2 + name.chars().count();
                if let Ok(number) = name.parse() {
                    stops.push((number, length));
                }
                match chars.get(after) {
                    Some(':') => {
                        open += 1;
                        i = after + 1;
                    }
                    Some('|') => {
                        let options: String = chars[after + 1..]
                            .iter()
                            .take_while(|c| **c != '|')
                            .collect();
                        for c in options.split(',').next().unwrap_or("").chars() {
                            push(&mut text, &mut length, c);
                        }
                        i = after + 1 + options.chars().count() + 2; // past `|}`
                    }
                    Some('}') => i = after + 1,
                    _ => {
                        push(&mut text, &mut length, '$');
                        i += 1;
                    }
                }
            }
            '$' => {
                let name = name_at(i + 1);
                if name.is_empty() {
                    push(&mut text, &mut length, '$');
                    i += 1;
                    continue;
                }
                if let Ok(number) = name.parse() {
                    stops.push((number, length));
                }
                i += 1 + name.chars().count();
            }
            c => {
                push(&mut text, &mut length, c);
                i += 1;
            }
        }
    }
    let first = stops.iter().filter(|(n, _)| *n > 0).min_by_key(|(n, _)| *n);
    let last = stops.iter().find(|(n, _)| *n == 0);
    let cursor = first.or(last).map_or(length, |(_, at)| *at);
    (text, cursor)
}

/// The char column where the word ending at char `column` of `line` starts: the part already
/// typed, which accepting an item replaces.
pub fn word_start(line: &str, column: usize) -> usize {
    let before: Vec<char> = line.chars().take(column).collect();
    let word = before
        .iter()
        .rev()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
        .count();
    before.len() - word
}

/// The items worth showing once `typed` (the word before the cursor) is typed, best first, as
/// indices into `items`. Matching is IntelliJ's, on word starts and camel humps (`gBN` finds
/// `getByName`), ignoring case; among equally good matches the whole name typed leads, then
/// the server's order. With nothing typed every item stays, in the server's order with its
/// preselected one first.
pub fn rank(items: &[CompletionItem], typed: &str) -> Vec<usize> {
    let server_order = |a: usize, b: usize| {
        items[b]
            .preselect
            .cmp(&items[a].preselect)
            .then_with(|| items[a].sort_text.cmp(&items[b].sort_text))
            .then_with(|| a.cmp(&b))
    };
    if typed.is_empty() {
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by(|&a, &b| server_order(a, b));
        return order;
    }
    let matcher = MinusculeMatcher::new(typed, MatchingMode::IgnoreCase, "");
    let first = typed.chars().next();
    let mut scored: Vec<(i32, bool, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let name: Vec<char> = item.filter_text.chars().collect();
            let fragments = matcher.match_chars(&name)?;
            let same_case = name.first().copied() == first;
            let score = matcher.degree_for_fragments(&name, same_case, Some(&fragments));
            Some((score, item.filter_text.eq_ignore_ascii_case(typed), index))
        })
        .collect();
    // the whole name typed wins over longer ones that start the same: `push` before
    // `push_mut`, which some servers' sort texts put first
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| server_order(a.2, b.2))
    });
    scored.into_iter().map(|(_, _, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn items(answer: Value) -> Vec<CompletionItem> {
        parse_completion(answer).unwrap().items
    }

    fn named(labels: &[&str]) -> Vec<CompletionItem> {
        items(json!(
            labels
                .iter()
                .map(|label| json!({"label": label}))
                .collect::<Vec<_>>()
        ))
    }

    fn ranked(items: &[CompletionItem], typed: &str) -> Vec<String> {
        rank(items, typed)
            .into_iter()
            .map(|i| items[i].label.clone())
            .collect()
    }

    #[test]
    fn an_answer_may_be_null_a_list_of_items_or_a_completion_list() {
        assert_eq!(
            parse_completion(Value::Null).unwrap(),
            Completion::default()
        );
        let array = parse_completion(json!([{"label": "a"}])).unwrap();
        assert_eq!((array.items.len(), array.incomplete), (1, false));
        let list = parse_completion(
            json!({"isIncomplete": true, "items": [{"label": "a"}, {"label": "b"}]}),
        )
        .unwrap();
        assert_eq!((list.items.len(), list.incomplete), (2, true));
    }

    #[test]
    fn a_plain_item_replaces_the_word_with_its_label() {
        let item = CompletionItem::plain("length", Some(CompletionItemKind::TEXT), None);
        let edit = item.edit(0, "x.le", 4);
        assert_eq!(
            edit.range,
            Range::new(Position::new(0, 2), Position::new(0, 4))
        );
        assert_eq!((edit.new_text.as_str(), item.cursor()), ("length", 6));
        assert!(item.additional_edits().is_empty());
    }

    #[test]
    fn an_item_keeps_what_the_popup_shows() {
        let found = items(json!([{
            "label": "push",
            "kind": 2,
            "detail": "fn(&mut self, value: T)",
            "labelDetails": {"detail": "(value)", "description": "Vec<T>"},
            "sortText": "0001",
            "filterText": "push_back",
            "preselect": true,
        }, {
            "label": "len",
            "labelDetails": {"description": "Vec<T>"},
        }]));
        assert_eq!(found[0].label_detail.as_deref(), Some("(value)"));
        assert_eq!(found[0].detail.as_deref(), Some("fn(&mut self, value: T)"));
        assert_eq!(found[0].kind, Some(CompletionItemKind::METHOD));
        assert_eq!(
            (found[0].filter_text.as_str(), found[0].sort_text.as_str()),
            ("push_back", "0001")
        );
        assert!(found[0].preselect);
        // without a detail, the description stands in; without the rest, the label does
        assert_eq!(found[1].detail.as_deref(), Some("Vec<T>"));
        assert_eq!(
            (found[1].filter_text.as_str(), found[1].sort_text.as_str()),
            ("len", "len")
        );
    }

    #[test]
    fn without_a_range_the_item_replaces_the_word_before_the_cursor() {
        let found = items(json!([{"label": "length", "insertText": "len()"}]));
        let edit = found[0].edit(3, "    v.le", 8);
        assert_eq!(
            (edit.range.start, edit.range.end, edit.new_text.as_str()),
            (Position::new(3, 6), Position::new(3, 8), "len()")
        );
        assert_eq!(found[0].cursor(), 5);
        // the label is the text when there is nothing else
        assert_eq!(named(&["x"])[0].edit(0, "", 0).new_text, "x");
    }

    #[test]
    fn the_server_range_stretches_to_where_typing_went_on() {
        let found = items(json!([{
            "label": "length",
            "textEdit": {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 3}},
                         "newText": "length"},
        }]));
        // asked after `l`, accepted after `len`
        let edit = found[0].edit(1, "v.len", 5);
        assert_eq!(
            edit.range,
            Range::new(Position::new(1, 2), Position::new(1, 5))
        );
        assert_eq!(edit.new_text, "length");
    }

    #[test]
    fn an_insert_or_replace_edit_inserts() {
        let found = items(json!([{
            "label": "length",
            "textEdit": {
                "newText": "length",
                "insert": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}},
                "replace": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 7}},
            },
        }]));
        let edit = found[0].edit(0, "v.lexxx", 4);
        assert_eq!(edit.range.end, Position::new(0, 4));
    }

    #[test]
    fn columns_are_utf16_like_the_rest_of_the_protocol() {
        let found = named(&["über"]);
        // `𝓍` is two UTF-16 units: the word starts at unit 3, the cursor is at unit 5
        let edit = found[0].edit(0, "𝓍 üb", 4);
        assert_eq!(
            edit.range,
            Range::new(Position::new(0, 3), Position::new(0, 5))
        );
    }

    #[test]
    fn additional_edits_come_along() {
        let found = items(json!([{
            "label": "HashMap",
            "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                                     "newText": "use std::collections::HashMap;\n"}],
        }]));
        assert_eq!(found[0].additional_edits().len(), 1);
        assert!(named(&["x"])[0].additional_edits().is_empty());
    }

    #[test]
    fn a_snippet_is_taken_as_plain_text_with_the_cursor_at_its_first_stop() {
        let snippet = |text: &str| {
            let found = items(json!([{"label": "x", "insertText": text, "insertTextFormat": 2}]));
            (found[0].edit(0, "", 0).new_text, found[0].cursor())
        };
        assert_eq!(snippet("push($1)$0"), ("push()".to_string(), 5));
        assert_eq!(
            snippet("fn ${1:name}($2) {$0}"),
            ("fn name() {}".to_string(), 3)
        );
        assert_eq!(snippet("${1|one,two|} x"), ("one x".to_string(), 0));
        assert_eq!(snippet("a$0b"), ("ab".to_string(), 1));
        assert_eq!(
            snippet("cost \\$5 $TM_FILENAME"),
            ("cost $5 ".to_string(), 8)
        );
        assert_eq!(
            snippet("${1:outer ${2:inner}}!"),
            ("outer inner!".to_string(), 0)
        );
        assert_eq!(snippet("$ alone"), ("$ alone".to_string(), 7));
        // plain text is left alone, `$` and all
        let found = items(json!([{"label": "x", "insertText": "a$1"}]));
        assert_eq!(found[0].edit(0, "", 0).new_text, "a$1");
    }

    #[test]
    fn the_word_before_the_cursor_is_letters_digits_underscores_and_dollars() {
        assert_eq!(word_start("let x = foo_bar2", 16), 8);
        assert_eq!(word_start("a.$el", 5), 2);
        assert_eq!(word_start("a.", 2), 2);
        assert_eq!(word_start("", 0), 0);
        assert_eq!(word_start("señal", 5), 0);
    }

    #[test]
    fn typing_narrows_the_items_down_by_word_starts_and_humps() {
        let found = named(&["getByName", "getName", "toString", "get"]);
        assert_eq!(ranked(&found, "gBN"), ["getByName"]);
        assert_eq!(ranked(&found, "ts"), ["toString"]);
        assert!(ranked(&found, "xyz").is_empty());
        // case does not keep an item out
        assert_eq!(ranked(&found, "TOSTR"), ["toString"]);
    }

    #[test]
    fn better_matches_come_first_and_ties_keep_the_server_order() {
        let found = items(json!([
            {"label": "setNumber", "sortText": "1"},
            {"label": "number", "sortText": "2"},
            {"label": "numbers", "sortText": "0"},
        ]));
        // both start with `num`: the server put `numbers` first
        assert_eq!(ranked(&found, "num")[..2], ["numbers", "number"]);
        // the whole name typed comes first, whatever the server's order
        assert_eq!(ranked(&found, "number")[..2], ["number", "numbers"]);
    }

    #[test]
    fn with_nothing_typed_the_server_order_stays_and_its_pick_leads() {
        let found = items(json!([
            {"label": "b", "sortText": "2"},
            {"label": "a", "sortText": "1"},
            {"label": "c", "sortText": "3", "preselect": true},
        ]));
        assert_eq!(ranked(&found, ""), ["c", "a", "b"]);
    }
}
