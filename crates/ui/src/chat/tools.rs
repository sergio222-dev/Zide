//! Tool calls as cards: a line that says what Claude is doing and how it went, which opens to
//! show what it did (the change an edit makes, the command a shell runs) and what came out.
//! Edits and writes start open, since the change is the point; the rest start folded.

use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::*;
use zide_claude::{Block, ToolCall, ToolOutput, Value};

use super::{Chat, summary};
use crate::markdown::TextView;
use crate::{Workspace, theme};

/// Monospace for code and output; GPUI falls back to its default font when it is missing.
pub(super) const MONO: &str = "JetBrains Mono";

/// Lines of output shown before the rest is folded away.
const OUTPUT_LINES: usize = 30;

/// Lines of a written file shown.
const WRITE_LINES: usize = 30;

/// Rows the diff of an edit shows before it scrolls.
const DIFF_ROWS: usize = 16;

/// The diff of one edit, ready to draw.
pub(super) struct EditDiff {
    state: Entity<DiffState>,
    /// How many rows it has, for its height.
    rows: usize,
}

/// A unified patch for replacing `old` with `new` in the file `path`.
fn unified(path: &str, old: &str, new: &str) -> String {
    let terminated = |text: &str| {
        if text.is_empty() || text.ends_with('\n') {
            text.to_owned()
        } else {
            format!("{text}\n")
        }
    };
    let (old, new) = (terminated(old), terminated(new));
    similar::TextDiff::from_lines(old.as_str(), new.as_str())
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// The patch an edit tool makes, `None` for any other tool or an edit that changes nothing.
/// Line numbers count from the edited text, not from the file: Claude does not say where in the
/// file it was.
fn patch(name: &str, input: &Value) -> Option<String> {
    let path = input["file_path"].as_str()?;
    let patch = match name {
        "Edit" => unified(
            path,
            input["old_string"].as_str()?,
            input["new_string"].as_str()?,
        ),
        "MultiEdit" => input["edits"]
            .as_array()?
            .iter()
            .filter_map(|edit| {
                Some(unified(
                    path,
                    edit["old_string"].as_str()?,
                    edit["new_string"].as_str()?,
                ))
            })
            .collect(),
        "Write" => unified(path, "", input["content"].as_str()?),
        _ => return None,
    };
    patch.contains("\n@@").then_some(patch)
}

/// One line of a change.
#[derive(Debug, PartialEq)]
enum Change {
    Removed(String),
    Added(String),
}

/// What replacing `old` with `new` removes and adds. Lines the two share at the start and the
/// end are left out: only what differs is shown.
fn diff(old: &str, new: &str) -> Vec<Change> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let shared_end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old[start..old.len() - shared_end].iter();
    let added = new[start..new.len() - shared_end].iter();
    removed
        .map(|line| Change::Removed((*line).to_owned()))
        .chain(added.map(|line| Change::Added((*line).to_owned())))
        .collect()
}

/// The first `max` lines of `text`, and how many were left out.
fn clip(text: &str, max: usize) -> (Vec<&str>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let hidden = lines.len().saturating_sub(max);
    (lines.into_iter().take(max).collect(), hidden)
}

/// Whether a tool's card is open before the user touches it.
fn starts_open(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write")
}

fn mono_block() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .bg(rgb(theme::BACKGROUND))
        .px_3()
        .py_2()
        .font_family(MONO)
        .text_sm()
}

fn change_lines(changes: Vec<Change>) -> Div {
    mono_block().children(changes.into_iter().map(|change| {
        let (sign, text, color) = match change {
            Change::Removed(text) => ("-", text, theme::ERROR),
            Change::Added(text) => ("+", text, theme::OK),
        };
        div()
            .flex()
            .gap_2()
            .text_color(rgb(color))
            .child(sign)
            .child(text)
    }))
}

fn file_label(input: &Value) -> Option<Div> {
    let path = input["file_path"].as_str()?;
    Some(
        div()
            .text_xs()
            .text_color(rgb(theme::DIM))
            .child(path.to_owned()),
    )
}

/// What a tool is going to do, when its input says more than its one-line summary: the change
/// of an edit, the whole command of a shell. Also shown on the permission card.
pub(super) fn input_detail(name: &str, input: &Value) -> Option<Div> {
    let column = || div().flex().flex_col().gap_1();
    match name {
        "Edit" => {
            let changes = diff(input["old_string"].as_str()?, input["new_string"].as_str()?);
            Some(
                column()
                    .children(file_label(input))
                    .child(change_lines(changes)),
            )
        }
        "MultiEdit" => {
            let edits = input["edits"].as_array()?;
            Some(
                column()
                    .children(file_label(input))
                    .children(edits.iter().filter_map(|edit| {
                        let old = edit["old_string"].as_str()?;
                        let new = edit["new_string"].as_str()?;
                        Some(change_lines(diff(old, new)))
                    })),
            )
        }
        "Write" => {
            let (lines, hidden) = clip(input["content"].as_str()?, WRITE_LINES);
            let mut changes: Vec<Change> = lines
                .into_iter()
                .map(|line| Change::Added(line.to_owned()))
                .collect();
            let more = (hidden > 0).then(|| {
                div()
                    .text_xs()
                    .text_color(rgb(theme::DIM))
                    .child(format!("… {hidden} more lines"))
            });
            changes.shrink_to_fit();
            Some(
                column()
                    .children(file_label(input))
                    .child(change_lines(changes))
                    .children(more),
            )
        }
        "Bash" => {
            let command = input["command"].as_str()?;
            Some(mono_block().children(command.lines().map(|line| format!("$ {line}"))))
        }
        _ => None,
    }
}

/// The diff of an edit under its file name.
fn edit_diff(input: &Value, edit: &EditDiff) -> Div {
    let height = px(22.) * edit.rows.min(DIFF_ROWS) as f32 + px(8.);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(file_label(input))
        .child(
            div().h(height).rounded_md().overflow_hidden().child(
                Diff::new(&edit.state)
                    .header_visible(false)
                    .line_number(false),
            ),
        )
}

/// What came out of the tool, cut after `OUTPUT_LINES` lines.
fn output_view(output: &ToolOutput) -> Option<Div> {
    if output.text.trim().is_empty() {
        return None;
    }
    let (lines, hidden) = clip(&output.text, OUTPUT_LINES);
    let color = if output.is_error {
        theme::ERROR
    } else {
        theme::MUTED
    };
    Some(
        mono_block()
            .text_color(rgb(color))
            .children(lines.into_iter().map(|line| line.to_owned()))
            .children((hidden > 0).then(|| {
                div()
                    .text_color(rgb(theme::DIM))
                    .child(format!("… {hidden} more lines"))
            })),
    )
}

impl Workspace {
    /// Gives each edit of the active chat its diff, once.
    pub(super) fn ensure_diffs(&mut self, cx: &mut Context<Self>) {
        let Some(chats) = &self.claude else { return };
        let chat = chats.active();
        let missing: Vec<(String, Vec<DiffFile>)> = chat
            .transcript
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Tool(call) if !chat.diffs.contains_key(&call.id) => {
                    let patch = patch(&call.name, &call.input)?;
                    Some((call.id.clone(), DiffFile::parse(&patch).ok()?))
                }
                _ => None,
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        let built: Vec<(String, EditDiff)> = missing
            .into_iter()
            .map(|(id, files)| {
                // the changed lines, and about as many of context as a hunk has around them
                let rows = files
                    .iter()
                    .map(|file| file.additions() + file.deletions() + 7)
                    .sum();
                let state = cx.new(|cx| DiffState::new(files, cx));
                (id, EditDiff { state, rows })
            })
            .collect();
        if let Some(chats) = &mut self.claude {
            chats.active_mut().diffs.extend(built);
        }
    }

    pub(super) fn claude_toggle_tool(
        &mut self,
        chat_id: u64,
        tool_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(chat) = self.claude.as_mut().and_then(|c| c.find(chat_id))
            && !chat.toggled.remove(tool_id)
        {
            chat.toggled.insert(tool_id.to_owned());
        }
        cx.notify();
    }

    /// A `/context` answer: its head, then every section folded under its title.
    pub(super) fn report_view(
        &self,
        chat: &Chat,
        index: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(report) = super::report::parse(text) else {
            return div().child(text.to_owned()).into_any_element();
        };
        let part = |n: usize| {
            chat.report_md
                .get(&(index, n))
                .map(|state| TextView::new(state).selectable(true))
        };
        let chat_id = chat.id;
        let sections = report.sections.iter().enumerate().map(|(n, section)| {
            let key = format!("report-{index}-{n}");
            let open = super::report::starts_open(n) != chat.toggled.contains(&key);
            let toggle = key.clone();
            let header =
                div()
                    .id(SharedString::from(key))
                    .flex()
                    .gap_2()
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.claude_toggle_tool(chat_id, &toggle, cx)
                    }))
                    .child(div().text_color(rgb(theme::DIM)).child(if open {
                        "▾"
                    } else {
                        "▸"
                    }))
                    .child(
                        div()
                            .text_color(rgb(theme::STRONG))
                            .child(section.title.clone()),
                    );
            Collapsible::new()
                .open(open)
                .child(header)
                .content(div().pl_5().children(part(n + 1)))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(part(0))
            .children(sections)
            .into_any_element()
    }

    /// A tool call: its line, opening to the change it makes and what it printed.
    pub(super) fn tool_card(&self, chat: &Chat, call: &ToolCall, cx: &mut Context<Self>) -> Div {
        let (mark, color) = match &call.output {
            None => ("…", theme::DIM),
            Some(output) if output.is_error => ("✗", theme::ERROR),
            Some(_) => ("✓", theme::OK),
        };
        let open = starts_open(&call.name) != chat.toggled.contains(&call.id);
        let (chat_id, tool_id) = (chat.id, call.id.clone());
        let header = div()
            .id(SharedString::from(format!("tool-{}", call.id)))
            .flex()
            .gap_2()
            .text_sm()
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, _, cx| this.claude_toggle_tool(chat_id, &tool_id, cx)),
            )
            .child(
                div()
                    .text_color(rgb(theme::DIM))
                    .child(if open { "▾" } else { "▸" }),
            )
            .child(div().text_color(rgb(color)).child(mark))
            .child(
                div()
                    .text_color(rgb(theme::STRONG))
                    .child(call.name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(theme::MUTED))
                    .child(summary(&call.input)),
            );
        let detail = div()
            .flex()
            .flex_col()
            .gap_2()
            .pl_5()
            .pt_1()
            .children(match chat.diffs.get(&call.id) {
                Some(edit) => Some(edit_diff(&call.input, edit)),
                None => input_detail(&call.name, &call.input),
            })
            .children(call.output.as_ref().and_then(output_view));
        div().child(Collapsible::new().open(open).child(header).content(detail))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::diff::DiffFile;
    use zide_claude::Value;

    use super::{Change, clip, diff, patch, starts_open};

    fn removed(text: &str) -> Change {
        Change::Removed(text.to_owned())
    }

    fn added(text: &str) -> Change {
        Change::Added(text.to_owned())
    }

    #[test]
    fn a_change_shows_only_the_lines_that_differ() {
        let old = "fn a() {\n    one();\n    two();\n}";
        let new = "fn a() {\n    one();\n    three();\n    four();\n}";
        assert_eq!(
            diff(old, new),
            [
                removed("    two();"),
                added("    three();"),
                added("    four();")
            ]
        );
    }

    #[test]
    fn a_change_to_nothing_removes_everything_and_the_reverse_adds_it() {
        assert_eq!(diff("a\nb", ""), [removed("a"), removed("b")]);
        assert_eq!(diff("", "a"), [added("a")]);
        assert!(diff("same", "same").is_empty());
    }

    #[test]
    fn long_text_is_cut_and_the_rest_is_counted() {
        let (shown, hidden) = clip("1\n2\n3\n4\n5", 3);
        assert_eq!(shown, ["1", "2", "3"]);
        assert_eq!(hidden, 2);
        assert_eq!(clip("1\n2", 3), (vec!["1", "2"], 0));
    }

    #[test]
    fn edits_start_open_and_the_rest_folded() {
        assert!(starts_open("Edit"));
        assert!(starts_open("Write"));
        assert!(!starts_open("Bash"));
        assert!(!starts_open("Read"));
    }

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn an_edit_becomes_a_patch_the_diff_component_reads() {
        let input = json(
            r#"{"file_path":"src/a.rs","old_string":"one\ntwo","new_string":"one\nthree\nfour"}"#,
        );
        let patch = patch("Edit", &input).unwrap();
        let files = DiffFile::parse(&patch).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].additions(), files[0].deletions()), (2, 1));
    }

    #[test]
    fn a_write_is_all_additions_and_an_empty_change_has_no_patch() {
        let write = json(r#"{"file_path":"new.txt","content":"a\nb"}"#);
        let files = DiffFile::parse(&patch("Write", &write).unwrap()).unwrap();
        assert_eq!((files[0].additions(), files[0].deletions()), (2, 0));
        let same = json(r#"{"file_path":"a","old_string":"x","new_string":"x"}"#);
        assert_eq!(patch("Edit", &same), None);
        assert_eq!(patch("Bash", &json(r#"{"command":"ls"}"#)), None);
    }

    #[test]
    fn a_multi_edit_has_a_file_per_edit() {
        let input = json(
            r#"{"file_path":"a","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d"}]}"#,
        );
        let files = DiffFile::parse(&patch("MultiEdit", &input).unwrap()).unwrap();
        assert_eq!(files.len(), 2);
    }
}
