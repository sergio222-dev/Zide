//! The merge editor: a conflicted file in three panes, yours on the left, theirs on the right and
//! the result in the middle, which starts as the common ancestor. The arrow of a change puts it in
//! the result (both sides, one under the other); the cross leaves it out. *Merge…* of the
//! conflict dialog opens it.

use std::cell::Cell as Shared;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::merge::{Change, Handling, Kind, Merge, Segment, Which};

use crate::viewer::EDITOR_FONTS;
use crate::{Workspace, theme};

const ROW_HEIGHT: f32 = 22.;
/// Width of the column of a side where its arrow, cross and line numbers are.
const GUTTER: f32 = 88.;
const NUMBER: f32 = 36.;

/// One line of a pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Cell {
    number: Option<usize>,
    text: String,
    tint: Option<u32>,
    /// A change that was applied or discarded: shown, but out of the way.
    dim: bool,
}

/// One row of the three panes: the panes of a change are as tall as the tallest, so that what
/// lines up stays level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Row {
    left: Cell,
    mid: Cell,
    right: Cell,
    /// The segment of the change that starts at this row, with the sides that offer a choice.
    start_of: Option<(usize, bool, bool)>,
    /// The segment and the line of it that the middle cell is, where the caret can be.
    mid_at: Option<(usize, usize)>,
}

/// Where the caret is in the result: a line of a segment, and a character of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Caret {
    segment: usize,
    line: usize,
    col: usize,
}

/// What a key does to the text of a segment.
enum Op<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// The lines of a segment after `op` at `caret`, and where the caret goes (line, character).
/// A line is never joined to the next segment: the last line of the segment keeps its break
/// (unless it is the end of the file), and an empty edge line is what Backspace and Delete can
/// remove there.
fn edit_text(
    lines: &[String],
    caret: Caret,
    op: Op,
    last: bool,
) -> Option<(Vec<String>, usize, usize)> {
    let mut text = lines.concat();
    let content = lines
        .get(caret.line)
        .map_or("", |line| line.trim_end_matches('\n'));
    let byte = content
        .char_indices()
        .nth(caret.col)
        .map_or(content.len(), |(at, _)| at);
    let mut at = lines[..caret.line.min(lines.len())]
        .iter()
        .map(String::len)
        .sum::<usize>()
        + byte;
    match op {
        Op::Insert(typed) => {
            text.insert_str(at, typed);
            at += typed.len();
        }
        Op::Backspace if at == 0 => {
            if !text.starts_with('\n') {
                return None;
            }
            text.remove(0); // the empty first line
        }
        Op::Backspace => {
            let before = text[..at].char_indices().next_back()?.0;
            text.replace_range(before..at, "");
            at = before;
        }
        Op::Delete => {
            let ch = text[at..].chars().next()?;
            let ends_a_line_of_text =
                ch == '\n' && at + 1 == text.len() && at > 0 && text.as_bytes()[at - 1] != b'\n';
            if ends_a_line_of_text {
                return None;
            }
            text.replace_range(at..at + ch.len_utf8(), "");
        }
    }
    if !last && !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if at == text.len() && text.ends_with('\n') {
        lines.push(String::new()); // the caret is on a new last line at the end of the file
    }
    let (mut line, mut col, mut start) = (0, 0, 0);
    for (index, text_line) in lines.iter().enumerate() {
        let content = text_line.trim_end_matches('\n').len();
        if at <= start + content {
            (line, col) = (index, text_line[..at - start].chars().count());
            break;
        }
        start += text_line.len();
    }
    Some((lines, line, col))
}

/// How many cells a line takes up to its `col`-th character.
fn cells_before(line: &str, col: usize) -> usize {
    line.chars()
        .take(col)
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// The character of `line` the caret goes before when the click is `cells` cells from its start:
/// a click on the left half of a character goes before it.
fn col_at(line: &str, cells: usize) -> usize {
    let line = line.trim_end_matches('\n');
    let mut used = 0;
    for (col, c) in line.chars().enumerate() {
        let width = if c == '\t' { 4 } else { 1 };
        if cells * 2 < used * 2 + width {
            return col;
        }
        used += width;
    }
    line.chars().count()
}

pub struct MergeEditor {
    path: String,
    merge: Merge,
    pub yours: String,
    pub theirs: String,
    caret: Caret,
    /// The merge before each step, to undo them.
    history: Vec<Merge>,
    /// The last step was typing: the next key joins it.
    typing: bool,
    family: SharedString,
    /// Width of a character of the font.
    cell: f32,
    /// Where the text of the result starts, on screen.
    origin: Rc<Shared<f32>>,
    rows: Arc<Vec<Row>>,
    /// The row where each change starts.
    starts: Vec<usize>,
    /// The change the arrows of the toolbar are on.
    at: usize,
    pub busy: bool,
    scroll: UniformListScrollHandle,
}

/// What a line shows: no line break, tabs as spaces.
fn shown(line: &str) -> String {
    line.trim_end_matches(['\n', '\r']).replace('\t', "    ")
}

/// The color of the lines a side changed: gray when it removed them, green when it added them
/// to nothing, blue when it replaced others, red when the other side changed them too.
fn tint(change: &Change, side: Which) -> Option<u32> {
    if !change.kind.changed_by(side) || change.handling(side) != Handling::Pending {
        return None;
    }
    let lines = match side {
        Which::Yours => change.yours.len(),
        Which::Theirs => change.theirs.len(),
    };
    Some(if change.kind == Kind::Conflict {
        theme::DIFF_CONFLICT
    } else if change.base.is_empty() {
        theme::DIFF_ADDED
    } else if lines == 0 {
        theme::DIFF_REMOVED
    } else {
        theme::DIFF_CHANGED
    })
}

/// The rows of the three panes for the merge as it stands.
fn build_rows(merge: &Merge) -> (Vec<Row>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut starts = Vec::new();
    let (mut left, mut mid, mut right) = (0, 0, 0);
    let number = |counter: &mut usize, text: &str, tint: Option<u32>, dim: bool| {
        *counter += 1;
        Cell {
            number: Some(*counter),
            text: shown(text),
            tint,
            dim,
        }
    };
    for (index, segment) in merge.segments().iter().enumerate() {
        let result = merge.result_of(index, segment);
        let (yours, theirs, tints, dims, choice) = match segment {
            Segment::Common(range) => {
                let lines = &merge.base()[range.clone()];
                (
                    lines.to_vec(),
                    lines.to_vec(),
                    [None; 3],
                    [false; 2],
                    (false, false),
                )
            }
            Segment::Change(change) => {
                let handled = |side: Which| {
                    change.kind.changed_by(side) && change.handling(side) != Handling::Pending
                };
                // the result keeps the color of the change: red while it is a conflict, else green
                // for lines added, blue for lines replaced, once something was put there
                let touched = change.handling(Which::Yours) == Handling::Applied
                    || change.handling(Which::Theirs) == Handling::Applied
                    || merge.is_edited(index);
                let mid_tint = if !change.resolved() && change.kind == Kind::Conflict {
                    Some(theme::DIFF_CONFLICT)
                } else if touched {
                    Some(if change.base.is_empty() {
                        theme::DIFF_ADDED
                    } else {
                        theme::DIFF_CHANGED
                    })
                } else {
                    None
                };
                let pending = |side: Which| {
                    change.kind.changed_by(side) && change.handling(side) == Handling::Pending
                };
                (
                    merge.yours()[change.yours.clone()].to_vec(),
                    merge.theirs()[change.theirs.clone()].to_vec(),
                    [
                        tint(change, Which::Yours),
                        mid_tint,
                        tint(change, Which::Theirs),
                    ],
                    [handled(Which::Yours), handled(Which::Theirs)],
                    (pending(Which::Yours), pending(Which::Theirs)),
                )
            }
        };
        let is_change = matches!(segment, Segment::Change(_));
        let height = if is_change {
            yours.len().max(theirs.len()).max(result.len()).max(1)
        } else {
            result.len()
        };
        if is_change {
            starts.push(rows.len());
        }
        for line in 0..height {
            let mut row = Row::default();
            // padding keeps the color of the change, so it reads as one block
            row.left.tint = tints[0];
            row.mid.tint = tints[1];
            row.right.tint = tints[2];
            if let Some(text) = yours.get(line) {
                row.left = number(&mut left, text, tints[0], dims[0]);
            }
            if let Some(text) = result.get(line) {
                row.mid = number(&mut mid, text, tints[1], false);
                row.mid_at = Some((index, line));
            } else if line == 0 && is_change {
                row.mid_at = Some((index, 0)); // nothing in the result yet: a line can be typed
            }
            if let Some(text) = theirs.get(line) {
                row.right = number(&mut right, text, tints[2], dims[1]);
            }
            if line == 0 && is_change {
                row.start_of = Some((index, choice.0, choice.1));
            }
            rows.push(row);
        }
    }
    (rows, starts)
}

impl MergeEditor {
    pub fn new(
        path: String,
        yours: String,
        theirs: String,
        merge: Merge,
        family: SharedString,
        cell: f32,
    ) -> Self {
        let mut editor = Self {
            path,
            merge,
            yours,
            theirs,
            caret: Caret::default(),
            history: Vec::new(),
            typing: false,
            family,
            cell,
            origin: Rc::new(Shared::new(0.)),
            rows: Arc::new(Vec::new()),
            starts: Vec::new(),
            at: 0,
            busy: false,
            scroll: UniformListScrollHandle::new(),
        };
        editor.rebuild();
        editor.settle_caret();
        // the caret starts on the first change
        if let Some(at) = editor
            .starts
            .first()
            .and_then(|row| editor.rows[*row].mid_at)
        {
            editor.caret = Caret {
                segment: at.0,
                line: at.1,
                col: 0,
            };
        }
        editor.go_to(0);
        editor
    }

    /// Remembers the merge before a step. Keys typed one after another are one step.
    fn record(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.history.push(self.merge.clone());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.typing = typing;
    }

    pub fn undo(&mut self) {
        if let Some(merge) = self.history.pop() {
            self.merge = merge;
            self.typing = false;
            self.rebuild();
            self.reveal();
        }
    }

    fn row_of(&self, caret: Caret) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.mid_at == Some((caret.segment, caret.line)))
    }

    /// Puts the caret on a row that can hold it, if the one it was on is gone.
    fn settle_caret(&mut self) {
        if self.row_of(self.caret).is_some() {
            let len = self
                .line_text(self.caret.segment, self.caret.line)
                .chars()
                .count();
            self.caret.col = self.caret.col.min(len);
            return;
        }
        let found = self
            .rows
            .iter()
            .find_map(|row| {
                row.mid_at
                    .filter(|(segment, _)| *segment >= self.caret.segment)
            })
            .or_else(|| self.rows.iter().rev().find_map(|row| row.mid_at));
        if let Some((segment, line)) = found {
            self.caret = Caret {
                segment,
                line,
                col: 0,
            };
        }
    }

    fn line_text(&self, segment: usize, line: usize) -> String {
        self.merge
            .result_lines(segment)
            .get(line)
            .map(|text| text.trim_end_matches('\n').to_string())
            .unwrap_or_default()
    }

    /// Scrolls so the caret is on screen.
    fn reveal(&mut self) {
        if let Some(row) = self.row_of(self.caret) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }

    fn edit(&mut self, op: Op) -> bool {
        let lines = self.merge.result_lines(self.caret.segment);
        let last = self.caret.segment + 1 == self.merge.segments().len();
        let Some((lines, line, col)) = edit_text(&lines, self.caret, op, last) else {
            return false;
        };
        self.record(true);
        self.merge.set_result(self.caret.segment, lines);
        self.caret.line = line;
        self.caret.col = col;
        self.rebuild();
        self.settle_caret();
        self.reveal();
        true
    }

    /// Types `text` (which may hold line breaks) at the caret.
    pub fn type_text(&mut self, text: &str) {
        self.edit(Op::Insert(&text.replace("\r\n", "\n")));
    }

    pub fn backspace(&mut self) {
        self.edit(Op::Backspace);
    }

    pub fn delete(&mut self) {
        self.edit(Op::Delete);
    }

    pub fn move_caret(&mut self, direction: Move) {
        self.typing = false;
        let Some(row) = self.row_of(self.caret) else {
            return;
        };
        let editable = |rows: &[Row], from: usize, down: bool| -> Option<(usize, usize)> {
            if down {
                rows.get(from + 1..)?.iter().find_map(|row| row.mid_at)
            } else {
                rows.get(..from)?.iter().rev().find_map(|row| row.mid_at)
            }
        };
        let len = self
            .line_text(self.caret.segment, self.caret.line)
            .chars()
            .count();
        let to = |this: &mut Self, at: (usize, usize), col: Option<usize>| {
            this.caret = Caret {
                segment: at.0,
                line: at.1,
                col: 0,
            };
            let len = this.line_text(at.0, at.1).chars().count();
            this.caret.col = col.map_or(len, |col| col.min(len));
        };
        match direction {
            Move::Home => self.caret.col = 0,
            Move::End => self.caret.col = len,
            Move::Left if self.caret.col > 0 => self.caret.col -= 1,
            Move::Right if self.caret.col < len => self.caret.col += 1,
            Move::Left => {
                if let Some(at) = editable(&self.rows, row, false) {
                    to(self, at, None);
                }
            }
            Move::Right => {
                if let Some(at) = editable(&self.rows, row, true) {
                    to(self, at, Some(0));
                }
            }
            Move::Up => {
                if let Some(at) = editable(&self.rows, row, false) {
                    to(self, at, Some(self.caret.col));
                }
            }
            Move::Down => {
                if let Some(at) = editable(&self.rows, row, true) {
                    to(self, at, Some(self.caret.col));
                }
            }
        }
        self.reveal();
    }

    /// A click on the middle pane at row `row`, `x` pixels from the left of the window.
    pub fn click(&mut self, row: usize, x: f32) {
        let Some((segment, line)) = self.rows.get(row).and_then(|row| row.mid_at) else {
            return;
        };
        self.typing = false;
        let cells = ((x - self.origin.get()) / self.cell).round().max(0.) as usize;
        let text = self.line_text(segment, line);
        self.caret = Caret {
            segment,
            line,
            col: col_at(&text, cells),
        };
    }

    /// Pixels from the start of the text to the caret, and the row it is on.
    fn caret_position(&self) -> Option<(usize, f32)> {
        let row = self.row_of(self.caret)?;
        let text = self.line_text(self.caret.segment, self.caret.line);
        Some((row, cells_before(&text, self.caret.col) as f32 * self.cell))
    }

    fn rebuild(&mut self) {
        let (rows, starts) = build_rows(&self.merge);
        (self.rows, self.starts) = (Arc::new(rows), starts);
    }

    fn go_to(&mut self, change: usize) {
        self.at = change;
        if let Some(row) = self.starts.get(change) {
            self.scroll.scroll_to_item(*row, ScrollStrategy::Top);
        }
    }

    /// Moves to the next (or previous) change, wrapping around.
    pub fn step(&mut self, delta: isize) {
        let count = self.starts.len();
        if count > 0 {
            let next = (self.at as isize + delta).rem_euclid(count as isize) as usize;
            self.go_to(next);
        }
    }

    /// Runs a step over the merge that can be undone.
    fn change(&mut self, step: impl FnOnce(&mut Merge)) {
        self.record(false);
        step(&mut self.merge);
        self.rebuild();
        self.settle_caret();
    }

    pub fn apply(&mut self, segment: usize, side: Which) {
        self.change(|merge| merge.apply(segment, side));
    }

    pub fn discard(&mut self, segment: usize, side: Which) {
        self.change(|merge| merge.discard(segment, side));
    }

    pub fn apply_non_conflicting(&mut self, only: Option<Which>) {
        self.change(|merge| merge.apply_non_conflicting(only));
    }

    pub fn accept(&mut self, side: Which) {
        self.change(|merge| merge.accept(side));
    }

    /// Every conflict has a choice: the file can be saved.
    pub fn can_apply(&self) -> bool {
        !self.busy && self.merge.conflicts_left() == 0
    }

    /// The text to save: what was chosen, plus the changes that needed no choice.
    pub fn finish(&mut self) -> String {
        self.apply_non_conflicting(None);
        self.merge.result()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// "No changes. 1 conflict."
    fn summary(&self) -> String {
        let (changes, conflicts) = (self.merge.changes_left(), self.merge.conflicts_left());
        let count = |n: usize, word: &str| match n {
            0 => format!("No {word}s"),
            1 => format!("1 {word}"),
            n => format!("{n} {word}s"),
        };
        format!(
            "{}. {}.",
            count(changes, "change"),
            count(conflicts, "conflict")
        )
    }
}

impl Workspace {
    /// *Merge…*: opens the selected conflicted file in the merge editor.
    pub(crate) fn open_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conflict) = self
            .overlay
            .conflicts()
            .and_then(|popup| popup.selected_conflict().cloned())
        else {
            return;
        };
        self.autosave(cx);
        let root = self.root.clone();
        let (yours, theirs) = self
            .overlay
            .conflicts()
            .map(|popup| popup.names())
            .unwrap_or_default();
        let (family, cell) = measure_font(window);
        cx.spawn_in(window, async move |this, cx| {
            let path = conflict.path.clone();
            let stages = cx
                .background_executor()
                .spawn(async move { zide_git::stages(&root, &path) })
                .await;
            this.update_in(cx, |this, window, cx| match stages {
                Ok(stages) => {
                    let merge = Merge::new(&stages.base, &stages.yours, &stages.theirs);
                    this.overlay.set_merge(MergeEditor::new(
                        conflict.path,
                        yours,
                        theirs,
                        merge,
                        family,
                        cell,
                    ));
                    this.focus.focus(window, cx);
                    cx.notify();
                }
                Err(why) => this.notify(Notification::error(why).title("Could not open it"), cx),
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn close_merge(&mut self, cx: &mut Context<Self>) {
        self.overlay.close_merge();
        cx.notify();
    }

    /// Writes the result over the file and marks it resolved.
    pub(crate) fn save_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.overlay.merge_mut() else {
            return;
        };
        if !editor.can_apply() {
            return;
        }
        editor.busy = true;
        let (path, text) = (editor.path().to_string(), editor.finish());
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let resolved = zide_git::resolve_with(&root, &path, &text);
                    (resolved, zide_git::conflicts(&root))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let saved = outcome.0.is_ok();
                if saved {
                    this.overlay.close_merge();
                } else if let Some(editor) = this.overlay.merge_mut() {
                    editor.busy = false;
                }
                this.finish_resolve(outcome, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Keys while the merge editor is open: Esc closes it, F7 and Shift+F7 move between changes,
    /// the rest edit the result (typing, Backspace, Delete, arrows, Ctrl+V, Ctrl+Z).
    pub(crate) fn merge_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let (shift, held) = (
            keystroke.modifiers.shift,
            keystroke.modifiers.control || keystroke.modifiers.platform,
        );
        let key = keystroke.key.as_str();
        if key == "escape" {
            return self.close_merge(cx);
        }
        let pasted = (held && key == "v")
            .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
            .flatten();
        let Some(editor) = self.overlay.merge_mut() else {
            return;
        };
        match key {
            _ if held && key == "z" => editor.undo(),
            _ if held => {
                if let Some(text) = pasted {
                    editor.type_text(&text);
                }
            }
            "f7" => editor.step(if shift { -1 } else { 1 }),
            "left" => editor.move_caret(Move::Left),
            "right" => editor.move_caret(Move::Right),
            "up" => editor.move_caret(Move::Up),
            "down" => editor.move_caret(Move::Down),
            "home" => editor.move_caret(Move::Home),
            "end" => editor.move_caret(Move::End),
            "backspace" => editor.backspace(),
            "delete" => editor.delete(),
            "enter" => editor.type_text("\n"),
            "tab" => editor.type_text("\t"),
            _ => {
                if let Some(text) = keystroke
                    .key_char
                    .as_ref()
                    .filter(|_| !keystroke.modifiers.alt)
                {
                    editor.type_text(text);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn merge_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let editor = self.overlay.merge().expect("the editor is open");
        let workspace = cx.entity();
        let rows = editor.rows.clone();
        let scroll = editor.scroll.clone();
        let (can_apply, busy) = (editor.can_apply(), editor.busy);
        let font = editor.family.clone();
        let caret = editor.caret_position();
        let origin = editor.origin.clone();
        let file_name = editor.path.rsplit('/').next().unwrap_or(&editor.path);

        let tool = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .h(px(20.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .text_color(rgb(theme::MUTED))
                .hover(|d| {
                    d.bg(rgb(theme::TREE_SELECTED))
                        .text_color(rgb(theme::STRONG))
                })
                .child(glyph)
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(30.))
            .px_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .child(tool("merge-prev", "↑").on_click({
                let workspace = workspace.clone();
                move |_, _, cx| edit(&workspace, cx, |editor| editor.step(-1))
            }))
            .child(tool("merge-next", "↓").on_click({
                let workspace = workspace.clone();
                move |_, _, cx| edit(&workspace, cx, |editor| editor.step(1))
            }))
            .child(
                div()
                    .ml_2()
                    .text_sm()
                    .text_color(rgb(theme::MUTED))
                    .child("Apply non-conflicting changes:"),
            )
            .child(tool("merge-all-yours", "»").on_click({
                let workspace = workspace.clone();
                move |_, _, cx| {
                    edit(&workspace, cx, |editor| {
                        editor.apply_non_conflicting(Some(Which::Yours))
                    })
                }
            }))
            .child(tool("merge-all-theirs", "«").on_click({
                let workspace = workspace.clone();
                move |_, _, cx| {
                    edit(&workspace, cx, |editor| {
                        editor.apply_non_conflicting(Some(Which::Theirs))
                    })
                }
            }))
            .child(tool("merge-all", "»«").on_click({
                let workspace = workspace.clone();
                move |_, _, cx| edit(&workspace, cx, |editor| editor.apply_non_conflicting(None))
            }))
            .child(
                div()
                    .ml_auto()
                    .text_sm()
                    .text_color(rgb(theme::MUTED))
                    .child(editor.summary()),
            );

        let name = |text: &str| {
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(theme::STRONG))
                .child(text.to_string())
        };
        let heading = |side: &str, who: &str| {
            div()
                .w_1_3()
                .flex()
                .gap_1()
                .px_2()
                .whitespace_nowrap()
                .child(div().text_color(rgb(theme::MUTED)).child(side.to_string()))
                .child(name(who))
        };
        let headings = div()
            .flex()
            .items_center()
            .h(px(24.))
            .flex_shrink_0()
            .child(heading("Changes from", &editor.yours))
            .child(
                div()
                    .w_1_3()
                    .flex()
                    .gap_2()
                    .px_2()
                    .child(name("Result"))
                    .child(
                        div()
                            .text_color(rgb(theme::DIM))
                            .child(file_name.to_string()),
                    ),
            )
            .child(heading("Changes from", &editor.theirs));

        let list = {
            let workspace = workspace.clone();
            let font = font.clone();
            uniform_list("merge-rows", rows.len(), move |range, _, _| {
                range
                    .map(|position| {
                        let caret = caret.filter(|(row, _)| *row == position).map(|(_, x)| x);
                        row_view(&rows[position], position, &workspace, &font, caret, &origin)
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(&scroll)
        };

        let (accept_yours, accept_theirs, apply, cancel) = (
            workspace.clone(),
            workspace.clone(),
            workspace.clone(),
            workspace,
        );
        div()
            .id("merge-overlay")
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p_6()
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(theme::POPUP))
                    .text_color(rgb(theme::TEXT))
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .rounded_md()
                    .overflow_hidden()
                    .child(toolbar)
                    .child(headings)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .bg(rgb(theme::BACKGROUND))
                            .child(list)
                            .child(Scrollbar::new(&editor.scroll)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(44.))
                            .px_3()
                            .flex_shrink_0()
                            .border_t_1()
                            .border_color(rgb(theme::BORDER))
                            .child(
                                Button::new("merge-accept-left")
                                    .label("Accept Left")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        accept_yours.update(cx, |this, cx| {
                                            if let Some(editor) = this.overlay.merge_mut() {
                                                editor.accept(Which::Yours);
                                            }
                                            this.save_merge(window, cx);
                                        })
                                    }),
                            )
                            .child(
                                Button::new("merge-accept-right")
                                    .label("Accept Right")
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        accept_theirs.update(cx, |this, cx| {
                                            if let Some(editor) = this.overlay.merge_mut() {
                                                editor.accept(Which::Theirs);
                                            }
                                            this.save_merge(window, cx);
                                        })
                                    }),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("merge-apply")
                                            .primary()
                                            .label("Apply")
                                            .disabled(!can_apply)
                                            .on_click(move |_, window, cx| {
                                                apply.update(cx, |this, cx| {
                                                    this.save_merge(window, cx)
                                                })
                                            }),
                                    )
                                    .child(Button::new("merge-cancel").label("Cancel").on_click(
                                        move |_, _, cx| {
                                            cancel.update(cx, |this, cx| this.close_merge(cx))
                                        },
                                    )),
                            ),
                    ),
            )
    }
}

/// The first editor font that is installed, and the width of one of its characters.
fn measure_font(window: &Window) -> (SharedString, f32) {
    let text_system = window.text_system();
    let installed = text_system.all_font_names();
    let family: SharedString = EDITOR_FONTS
        .iter()
        .find(|family| installed.iter().any(|name| name == *family))
        .unwrap_or(&EDITOR_FONTS[0])
        .to_string()
        .into();
    let id = text_system.resolve_font(&font(family.clone()));
    let cell = text_system
        .advance(id, px(13.), 'm')
        .ok()
        .map_or(8., |advance| f32::from(advance.width));
    (family, cell)
}

/// Changes the merge being edited.
fn edit(workspace: &Entity<Workspace>, cx: &mut App, change: impl FnOnce(&mut MergeEditor)) {
    workspace.update(cx, |this, cx| {
        if let Some(editor) = this.overlay.merge_mut() {
            change(editor);
        }
        cx.notify();
    })
}

/// One row: yours | result | theirs, with the arrow and cross of a change on its first row.
fn row_view(
    row: &Row,
    position: usize,
    workspace: &Entity<Workspace>,
    font: &SharedString,
    caret: Option<f32>,
    origin: &Rc<Shared<f32>>,
) -> AnyElement {
    let text = |cell: &Cell| {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .px_1()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(rgb(if cell.dim { theme::DIM } else { theme::TEXT }))
            .child(cell.text.clone())
    };
    let number = |cell: &Cell, width: f32| {
        div()
            .w(px(width))
            .flex_shrink_0()
            .flex()
            .justify_end()
            .px_1()
            .text_color(rgb(theme::DIM))
            .children(cell.number.map(|n| n.to_string()))
    };
    let control = |id: &'static str, glyph: &'static str, side: Which, discard: bool, segment| {
        let workspace = workspace.clone();
        div()
            .id((id, position))
            .w(px(18.))
            .flex()
            .justify_center()
            .cursor_pointer()
            .text_color(rgb(theme::MUTED))
            .hover(|d| d.text_color(rgb(theme::STRONG)))
            .child(glyph)
            .on_click(move |_, _, cx| {
                workspace.update(cx, |this, cx| {
                    if let Some(editor) = this.overlay.merge_mut() {
                        if discard {
                            editor.discard(segment, side)
                        } else {
                            editor.apply(segment, side)
                        }
                    }
                    cx.notify();
                })
            })
    };
    let (segment, yours_pending, theirs_pending) = row.start_of.unwrap_or((0, false, false));

    // the gutter of a side holds its controls (when it has something to choose) and its numbers
    let left_gutter = div()
        .w(px(GUTTER))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .when(row.start_of.is_some() && yours_pending, |d| {
            d.child(control(
                "merge-drop-yours",
                "✕",
                Which::Yours,
                true,
                segment,
            ))
            .child(control(
                "merge-take-yours",
                "»",
                Which::Yours,
                false,
                segment,
            ))
        })
        .child(number(&row.left, 32.));
    let right_gutter = div()
        .w(px(GUTTER))
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(number(&row.right, 32.))
        .when(row.start_of.is_some() && theirs_pending, |d| {
            d.child(control(
                "merge-take-theirs",
                "«",
                Which::Theirs,
                false,
                segment,
            ))
            .child(control(
                "merge-drop-theirs",
                "✕",
                Which::Theirs,
                true,
                segment,
            ))
        });
    let pane = |tint: Option<u32>| {
        div()
            .w_1_3()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .when_some(tint, |d, tint| d.bg(rgb(tint)))
    };
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .font_family(font.clone())
        .text_size(px(13.))
        .child(
            pane(row.left.tint)
                .child(text(&row.left))
                .child(left_gutter),
        )
        .child(
            pane(row.mid.tint)
                .border_l_1()
                .border_r_1()
                .border_color(rgb(theme::BORDER))
                .when(row.mid_at.is_some(), |d| d.cursor_text())
                .child(number(&row.mid, NUMBER))
                .child(
                    text(&row.mid)
                        .relative()
                        .child({
                            let origin = origin.clone();
                            canvas(
                                move |bounds, _, _| origin.set(f32::from(bounds.origin.x)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .left(px(4.))
                            .top_0()
                            .size_full()
                        })
                        .when_some(caret, |d, x| {
                            d.child(
                                div()
                                    .absolute()
                                    .left(px(4. + x))
                                    .top_0()
                                    .h_full()
                                    .w(px(2.))
                                    .bg(rgb(theme::CARET_BAR)),
                            )
                        }),
                )
                .on_mouse_down(MouseButton::Left, {
                    let workspace = workspace.clone();
                    move |event, _, cx| {
                        let x = f32::from(event.position.x);
                        edit(&workspace, cx, |editor| editor.click(position, x))
                    }
                }),
        )
        .child(
            pane(row.right.tint)
                .child(right_gutter)
                .child(text(&row.right)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(base: &str, yours: &str, theirs: &str) -> MergeEditor {
        MergeEditor::new(
            "file.txt".into(),
            "main".into(),
            "feature".into(),
            Merge::new(base, yours, theirs),
            "monospace".into(),
            8.,
        )
    }

    fn texts(editor: &MergeEditor) -> Vec<(String, String, String)> {
        editor
            .rows
            .iter()
            .map(|r| {
                (
                    r.left.text.clone(),
                    r.mid.text.clone(),
                    r.right.text.clone(),
                )
            })
            .collect()
    }

    #[::core::prelude::v1::test]
    fn a_conflict_shows_the_base_in_the_middle_and_both_sides_level() {
        let editor = editor("A\n", "C\n", "B\n");
        assert_eq!(texts(&editor), [("C".into(), "A".into(), "B".into())]);
        let row = &editor.rows[0];
        assert_eq!(row.start_of, Some((0, true, true)));
        assert_eq!(row.left.tint, Some(theme::DIFF_CONFLICT));
        assert_eq!(row.mid.tint, Some(theme::DIFF_CONFLICT));
        assert_eq!(editor.summary(), "No changes. 1 conflict.");
        assert!(!editor.can_apply());
    }

    #[::core::prelude::v1::test]
    fn applying_both_sides_stacks_them_and_the_panes_stay_level() {
        let mut editor = editor("a\nA\nz\n", "a\nC\nz\n", "a\nB\nz\n");
        editor.apply(1, Which::Theirs);
        editor.apply(1, Which::Yours);
        assert_eq!(
            texts(&editor),
            [
                ("a".into(), "a".into(), "a".into()),
                ("C".into(), "B".into(), "B".into()),
                ("".into(), "C".into(), "".into()),
                ("z".into(), "z".into(), "z".into()),
            ]
        );
        // what was applied has no controls left and is out of the way
        assert_eq!(editor.rows[1].start_of, Some((1, false, false)));
        assert!(editor.rows[1].left.dim && editor.rows[1].left.tint.is_none());
        assert!(editor.can_apply());
    }

    #[::core::prelude::v1::test]
    fn a_discarded_change_is_shown_without_controls_and_leaves_the_result_alone() {
        let mut editor = editor("A\n", "C\n", "B\n");
        editor.discard(0, Which::Yours);
        let row = &editor.rows[0];
        assert_eq!(row.mid.text, "A");
        assert_eq!(row.start_of, Some((0, false, true)));
        assert!(row.left.dim);
    }

    #[::core::prelude::v1::test]
    fn additions_are_green_removals_gray_and_replacements_blue() {
        let editor = editor("a\nb\nc\n", "a\nnew\nb\nc\n", "a\nb\n");
        let tints: Vec<_> = editor
            .rows
            .iter()
            .map(|r| (r.left.tint, r.right.tint))
            .collect();
        assert_eq!(tints[1], (Some(theme::DIFF_ADDED), None));
        assert_eq!(tints[3], (None, Some(theme::DIFF_REMOVED)));
        let editor = self::editor("a\nb\n", "a\nB\n", "a\nb\n");
        assert_eq!(editor.rows[1].left.tint, Some(theme::DIFF_CHANGED));
    }

    #[::core::prelude::v1::test]
    fn finishing_applies_what_needed_no_choice() {
        let mut editor = editor("a\nb\nc\nd\n", "A\nb\nc\nd\n", "a\nb\nc\nD\n");
        assert_eq!(editor.summary(), "2 changes. No conflicts.");
        assert_eq!(editor.finish(), "A\nb\nc\nD\n");
        assert_eq!(editor.summary(), "No changes. No conflicts.");
    }

    #[::core::prelude::v1::test]
    fn the_arrows_of_the_toolbar_go_round_the_changes() {
        let mut editor = editor("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n", "a\nb\nc\nd\ne\n");
        assert_eq!(editor.starts, [0, 4]);
        editor.step(1);
        assert_eq!(editor.at, 1);
        editor.step(1);
        assert_eq!(editor.at, 0);
        editor.step(-1);
        assert_eq!(editor.at, 1);
    }

    fn caret_at(segment: usize, line: usize, col: usize) -> Caret {
        Caret { segment, line, col }
    }

    fn lines(text: &str) -> Vec<String> {
        text.split_inclusive('\n').map(str::to_string).collect()
    }

    #[::core::prelude::v1::test]
    fn typing_splits_and_joins_lines_inside_a_segment() {
        let start = lines("ab\ncd\n");
        let (typed, line, col) =
            edit_text(&start, caret_at(0, 0, 1), Op::Insert("X\nY"), false).unwrap();
        assert_eq!((typed.concat(), line, col), ("aX\nYb\ncd\n".into(), 1, 1));
        let (joined, line, col) =
            edit_text(&lines("aX\nYb\n"), caret_at(0, 1, 0), Op::Backspace, false).unwrap();
        assert_eq!((joined.concat(), line, col), ("aXYb\n".into(), 0, 2));
        let (gone, line, col) =
            edit_text(&lines("ab\ncd\n"), caret_at(0, 0, 2), Op::Delete, false).unwrap();
        assert_eq!((gone.concat(), line, col), ("abcd\n".into(), 0, 2));
    }

    #[::core::prelude::v1::test]
    fn the_break_that_ends_a_segment_stays_unless_its_line_is_empty() {
        // Delete at the end of the last line would glue it to the next segment
        assert!(edit_text(&lines("ab\n"), caret_at(0, 0, 2), Op::Delete, false).is_none());
        // an empty first line goes with Backspace, an empty last line with Delete
        let (rest, ..) =
            edit_text(&lines("\nab\n"), caret_at(0, 0, 0), Op::Backspace, false).unwrap();
        assert_eq!(rest.concat(), "ab\n");
        let (rest, ..) = edit_text(&lines("ab\n\n"), caret_at(0, 1, 0), Op::Delete, false).unwrap();
        assert_eq!(rest.concat(), "ab\n");
        // nothing before the first character of a line of text
        assert!(edit_text(&lines("ab\n"), caret_at(0, 0, 0), Op::Backspace, false).is_none());
    }

    #[::core::prelude::v1::test]
    fn text_typed_into_an_empty_segment_gets_its_break() {
        let (typed, line, col) =
            edit_text(&[], caret_at(1, 0, 0), Op::Insert("new"), false).unwrap();
        assert_eq!((typed, line, col), (vec!["new\n".to_string()], 0, 3));
        // at the end of the file it does not
        let (typed, ..) = edit_text(&[], caret_at(1, 0, 0), Op::Insert("new"), true).unwrap();
        assert_eq!(typed, ["new"]);
    }

    #[::core::prelude::v1::test]
    fn typing_in_the_result_keeps_the_change_open_and_can_be_undone() {
        let mut editor = editor("a\nA\nz\n", "a\nC\nz\n", "a\nB\nz\n");
        assert_eq!(editor.caret, caret_at(1, 0, 0));
        editor.type_text("my ");
        editor.type_text("own ");
        assert_eq!(editor.merge.result(), "a\nmy own A\nz\n");
        // still a conflict: red, with the arrows of both sides
        assert!(!editor.can_apply());
        assert_eq!(editor.rows[1].mid.text, "my own A");
        assert_eq!(editor.rows[1].mid.tint, Some(theme::DIFF_CONFLICT));
        assert_eq!(editor.rows[1].start_of, Some((1, true, true)));
        editor.discard(1, Which::Yours);
        editor.discard(1, Which::Theirs);
        assert!(editor.can_apply());
        assert_eq!(editor.rows[1].mid.tint, Some(theme::DIFF_CHANGED));
        // two keys typed in a row are one step
        editor.undo();
        editor.undo();
        editor.undo();
        assert_eq!(editor.merge.result(), "a\nA\nz\n");
    }

    #[::core::prelude::v1::test]
    fn the_caret_walks_over_the_lines_of_the_result_and_skips_padding() {
        // yours adds two lines, so the result has padding rows in that change
        let mut editor = editor("a\nz\n", "a\nx\ny\nz\n", "a\nz\n");
        // the result of the change is empty: its first row can take a line
        assert_eq!(editor.caret, caret_at(1, 0, 0));
        editor.move_caret(Move::Down);
        assert_eq!(editor.caret, caret_at(2, 0, 0));
        editor.move_caret(Move::End);
        editor.move_caret(Move::Right);
        assert_eq!(editor.caret, caret_at(2, 0, 1));
        editor.move_caret(Move::Right); // end of the file
        assert_eq!(editor.caret, caret_at(2, 0, 1));
        editor.move_caret(Move::Left);
        assert_eq!(editor.caret, caret_at(2, 0, 0));
        editor.move_caret(Move::Left);
        assert_eq!(editor.caret, caret_at(1, 0, 0));
    }

    #[::core::prelude::v1::test]
    fn a_click_puts_the_caret_on_the_nearest_character() {
        let mut editor = editor("abc\n", "abc\n", "abc\n");
        editor.origin.set(100.);
        editor.click(0, 100. + 2. * 8. + 1.);
        assert_eq!(editor.caret, caret_at(0, 0, 2));
        editor.click(0, 100. + 50. * 8.);
        assert_eq!(editor.caret, caret_at(0, 0, 3));
        assert_eq!(col_at("\tx", 4), 1);
        assert_eq!(cells_before("\tx", 2), 5);
    }

    #[::core::prelude::v1::test]
    fn lines_show_without_breaks_and_with_spaces_for_tabs() {
        assert_eq!(shown("\tx\r\n"), "    x");
    }
}
