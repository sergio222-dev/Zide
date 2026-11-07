//! The change an edit makes, drawn in the chat: rows with the old and new lines on a red and green
//! band, numbered as in the file. Built from the tool's input, so it needs no process and no
//! diff component.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use similar::{ChangeTag, TextDiff};
use zide_claude::Value;

use super::tools::MONO;
use crate::theme;

/// Rows shown for one change before the rest is counted, as the Claude CLI does for an edit.
const MAX_ROWS: usize = 40;

/// Lines of the file around a change, as in a unified diff.
const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Context,
    Added,
    Removed,
    /// Where two groups of changes are apart.
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Row {
    pub kind: Kind,
    /// Line number in the file, when it is known.
    pub number: Option<usize>,
    pub text: String,
}

/// One edit: replacing `old` with `new`, the first line of which sits at line `start` of the file
/// (1-based) when that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Edit {
    pub rows: Vec<Row>,
    pub added: usize,
    pub removed: usize,
}

/// The rows of replacing `old` with `new`.
pub(super) fn edit(old: &str, new: &str, start: Option<usize>) -> Edit {
    let diff = TextDiff::from_lines(old, new);
    let mut rows = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for (group, ops) in diff.grouped_ops(CONTEXT).iter().enumerate() {
        if group > 0 {
            rows.push(Row {
                kind: Kind::Gap,
                number: None,
                text: String::new(),
            });
        }
        for op in ops {
            for change in diff.iter_changes(op) {
                let (kind, index) = match change.tag() {
                    ChangeTag::Equal => (Kind::Context, change.new_index()),
                    ChangeTag::Insert => {
                        added += 1;
                        (Kind::Added, change.new_index())
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        (Kind::Removed, change.old_index())
                    }
                };
                rows.push(Row {
                    kind,
                    number: start.zip(index).map(|(start, index)| start + index),
                    text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
                });
            }
        }
    }
    Edit {
        rows,
        added,
        removed,
    }
}

/// The edits a tool call makes: one for `Edit` and `Write`, one per entry for `MultiEdit`.
/// `starts` has the line each begins at, when the file could be read.
pub(super) fn edits(name: &str, input: &Value, starts: Option<&[usize]>) -> Vec<Edit> {
    let start = |at: usize| starts.and_then(|starts| starts.get(at).copied());
    fn pair(value: &Value) -> Option<(&str, &str)> {
        Some((value["old_string"].as_str()?, value["new_string"].as_str()?))
    }
    let found: Vec<Edit> = match name {
        "Edit" => pair(input)
            .map(|(old, new)| edit(old, new, start(0)))
            .into_iter()
            .collect(),
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(at, value)| {
                let (old, new) = pair(value)?;
                Some(edit(old, new, start(at)))
            })
            .collect(),
        "Write" => input["content"]
            .as_str()
            .map(|content| edit("", content, Some(1)))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    found
        .into_iter()
        .filter(|edit| edit.added + edit.removed > 0)
        .collect()
}

/// The line (1-based) a change sits at in `file`: where the new text is once the edit is made, else
/// where the old text still is.
pub(super) fn line_in(file: &str, old: &str, new: &str) -> Option<usize> {
    let at = [new, old]
        .into_iter()
        .filter(|text| !text.is_empty())
        .find_map(|text| file.find(text))?;
    Some(file[..at].matches('\n').count() + 1)
}

/// The lines each edit of a call begins at, read from the file; empty when it cannot be read.
pub(super) fn starts(name: &str, input: &Value) -> Vec<usize> {
    let Some(path) = input["file_path"].as_str() else {
        return Vec::new();
    };
    let Ok(file) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let pairs: Vec<(&str, &str)> = match name {
        "Edit" => vec![(
            input["old_string"].as_str().unwrap_or_default(),
            input["new_string"].as_str().unwrap_or_default(),
        )],
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|edit| {
                (
                    edit["old_string"].as_str().unwrap_or_default(),
                    edit["new_string"].as_str().unwrap_or_default(),
                )
            })
            .collect(),
        _ => return Vec::new(),
    };
    let found: Vec<Option<usize>> = pairs
        .into_iter()
        .map(|(old, new)| line_in(&file, old, new))
        .collect();
    // all or nothing: a number that is only known for some edits would mislead
    found
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

fn band(kind: Kind) -> Option<Rgba> {
    match kind {
        Kind::Added => Some(rgba(0x98c37926)),
        Kind::Removed => Some(rgba(0xe06c7526)),
        Kind::Context | Kind::Gap => None,
    }
}

/// The edit as rows on bands, under a line with its counts.
pub(super) fn view(edit: &Edit, numbered: bool) -> Div {
    let counts = div()
        .flex()
        .gap_2()
        .text_xs()
        .child(
            div()
                .text_color(rgb(theme::OK))
                .child(format!("+{}", edit.added)),
        )
        .child(
            div()
                .text_color(rgb(theme::ERROR))
                .child(format!("-{}", edit.removed)),
        );
    let shown = edit.rows.iter().take(MAX_ROWS).map(|row| {
        let sign = match row.kind {
            Kind::Added => "+",
            Kind::Removed => "-",
            _ => " ",
        };
        let text = if row.kind == Kind::Gap {
            "⋯".to_owned()
        } else if row.text.is_empty() {
            "\u{a0}".to_owned() // keeps the row's height
        } else {
            row.text.clone()
        };
        div()
            .flex()
            .w_full()
            .gap_2()
            .px_2()
            .when_some(band(row.kind), |row, color| row.bg(color))
            .text_color(rgb(if row.kind == Kind::Gap {
                theme::DIM
            } else {
                theme::TEXT
            }))
            .children(numbered.then(|| {
                div()
                    .flex_none()
                    .w(px(36.))
                    .text_right()
                    .text_color(rgb(theme::DIM))
                    .child(row.number.map(|n| n.to_string()).unwrap_or_default())
            }))
            .child(div().flex_none().w(px(10.)).child(sign))
            .child(div().flex_1().min_w_0().child(text))
    });
    let hidden = edit.rows.len().saturating_sub(MAX_ROWS);
    div().flex().flex_col().gap_1().child(counts).child(
        div()
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(theme::BACKGROUND))
            .py_1()
            .font_family(MONO)
            .text_sm()
            .children(shown)
            .children((hidden > 0).then(|| {
                div()
                    .px_2()
                    .text_xs()
                    .text_color(rgb(theme::DIM))
                    .child(format!("… {hidden} more lines"))
            })),
    )
}

#[cfg(test)]
mod tests {
    use zide_claude::Value;

    use super::{Edit, Kind, edit, edits, line_in};

    fn rows(edit: &Edit) -> Vec<(Kind, Option<usize>, &str)> {
        edit.rows
            .iter()
            .map(|row| (row.kind, row.number, row.text.as_str()))
            .collect()
    }

    #[test]
    fn a_replaced_line_is_removed_then_added_at_the_same_number() {
        let change = edit("a\ntwo\nc\n", "a\n2\nc\n", Some(160));
        assert_eq!(
            rows(&change),
            [
                (Kind::Context, Some(160), "a"),
                (Kind::Removed, Some(161), "two"),
                (Kind::Added, Some(161), "2"),
                (Kind::Context, Some(162), "c"),
            ]
        );
        assert_eq!((change.added, change.removed), (1, 1));
    }

    #[test]
    fn without_a_known_start_rows_have_no_number() {
        let change = edit("x\n", "y\n", None);
        assert!(change.rows.iter().all(|row| row.number.is_none()));
    }

    #[test]
    fn far_apart_changes_are_split_by_a_gap() {
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old.replace("2\n", "two\n").replace("19\n", "nineteen\n");
        let change = edit(&old, &new, Some(1));
        assert_eq!(
            change.rows.iter().filter(|r| r.kind == Kind::Gap).count(),
            1
        );
    }

    #[test]
    fn a_write_is_all_additions_from_line_one() {
        let input: Value = serde_json::from_str(r#"{"file_path":"n","content":"a\nb"}"#).unwrap();
        let found = edits("Write", &input, None);
        assert_eq!(found.len(), 1);
        assert_eq!(
            rows(&found[0]),
            [(Kind::Added, Some(1), "a"), (Kind::Added, Some(2), "b")]
        );
    }

    #[test]
    fn an_edit_that_changes_nothing_shows_nothing() {
        let input: Value =
            serde_json::from_str(r#"{"file_path":"n","old_string":"x","new_string":"x"}"#).unwrap();
        assert!(edits("Edit", &input, None).is_empty());
        assert!(edits("Bash", &input, None).is_empty());
    }

    #[test]
    fn the_line_of_a_change_is_found_after_or_before_the_edit() {
        let file = "one\ntwo\nthree\n";
        assert_eq!(line_in(file, "two", "2"), Some(2)); // not applied yet
        assert_eq!(line_in("one\n2\nthree\n", "two", "2"), Some(2)); // applied
        assert_eq!(line_in(file, "nope", "nada"), None);
    }
}
