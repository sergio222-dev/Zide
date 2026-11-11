//! Tool calls as cards: a line that says what Claude is doing and how it went, which opens to
//! show what it did (the change an edit makes, the command a shell runs) and what came out.
//! Edits and writes start open, since the change is the point; the rest start folded.

use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::*;
use zide_claude::{Block, ToolCall, ToolOutput, Value};

use super::{Chat, changes, summary};
use crate::markdown::TextView;
use crate::{Workspace, theme};

/// Monospace for code and output; GPUI falls back to its default font when it is missing.
pub(super) const MONO: &str = "JetBrains Mono";

/// Lines of output shown before the rest is folded away.
const OUTPUT_LINES: usize = 30;

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
pub(super) fn input_detail(name: &str, input: &Value, starts: Option<&[usize]>) -> Option<Div> {
    let column = || div().flex().flex_col().gap_1();
    match name {
        "Edit" | "MultiEdit" | "Write" => {
            let found = changes::edits(name, input, starts);
            (!found.is_empty()).then(|| {
                column().children(file_label(input)).children(
                    found
                        .iter()
                        .map(|edit| changes::view(edit, starts.is_some() || name == "Write")),
                )
            })
        }
        "Bash" => {
            let command = input["command"].as_str()?;
            Some(mono_block().children(command.lines().map(|line| format!("$ {line}"))))
        }
        _ => None,
    }
}

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
    /// Finds where in its file each edit of the active chat lies, once, to number the rows.
    pub(super) fn ensure_diffs(&mut self) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let viewed = chats
            .viewing
            .as_ref()
            .filter(|(id, _)| *id == chats.list[chats.active].id)
            .map(|(_, tool)| tool.clone());
        let chat = chats.active_mut();
        // a worker's edits are numbered too; tool call ids never repeat, so they share the map
        let worker = viewed.and_then(|tool| chat.workers.get(&tool));
        let worker_blocks = worker.map_or(&[][..], |w| w.transcript.blocks());
        let missing: Vec<(String, Vec<usize>)> = chat
            .transcript
            .blocks()
            .iter()
            .chain(worker_blocks)
            .filter_map(|block| match block {
                Block::Tool(call)
                    if matches!(call.name.as_str(), "Edit" | "MultiEdit")
                        && !chat.starts.contains_key(&call.id) =>
                {
                    Some((call.id.clone(), changes::starts(&call.name, &call.input)))
                }
                _ => None,
            })
            .collect();
        chat.starts.extend(missing);
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
            chat.report_md.get(&(index, n)).map(|state| {
                TextView::new(state)
                    .inline_code_style(crate::markdown::inline_code())
                    .selectable(true)
            })
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
            .children(input_detail(
                &call.name,
                &call.input,
                chat.starts
                    .get(&call.id)
                    .filter(|starts| !starts.is_empty())
                    .map(Vec::as_slice),
            ))
            .children(call.output.as_ref().and_then(output_view));
        div().child(Collapsible::new().open(open).child(header).content(detail))
    }
}

#[cfg(test)]
mod tests {

    use super::{clip, starts_open};

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
}
