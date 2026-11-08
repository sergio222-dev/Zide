//! The merge editor: a conflicted file in three panes, yours on the left, theirs on the right and
//! the result in the middle, which starts as the common ancestor. The arrow of a change puts it in
//! the result (both sides, one under the other); the cross leaves it out. *Merge…* of the
//! conflict dialog opens it.

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
}

pub struct MergeEditor {
    path: String,
    merge: Merge,
    pub yours: String,
    pub theirs: String,
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
        let result = merge.result_of(segment);
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
                let mid_tint = if !change.resolved() && change.kind == Kind::Conflict {
                    Some(theme::DIFF_CONFLICT)
                } else if change.handling(Which::Yours) == Handling::Applied
                    || change.handling(Which::Theirs) == Handling::Applied
                {
                    Some(theme::DIFF_CHANGED)
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
    pub fn new(path: String, yours: String, theirs: String, merge: Merge) -> Self {
        let mut editor = Self {
            path,
            merge,
            yours,
            theirs,
            rows: Arc::new(Vec::new()),
            starts: Vec::new(),
            at: 0,
            busy: false,
            scroll: UniformListScrollHandle::new(),
        };
        editor.rebuild();
        editor.go_to(0);
        editor
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

    pub fn apply(&mut self, segment: usize, side: Which) {
        self.merge.apply(segment, side);
        self.rebuild();
    }

    pub fn discard(&mut self, segment: usize, side: Which) {
        self.merge.discard(segment, side);
        self.rebuild();
    }

    pub fn apply_non_conflicting(&mut self, only: Option<Which>) {
        self.merge.apply_non_conflicting(only);
        self.rebuild();
    }

    pub fn accept(&mut self, side: Which) {
        self.merge.accept(side);
        self.rebuild();
    }

    /// Every conflict has a choice: the file can be saved.
    pub fn can_apply(&self) -> bool {
        !self.busy && self.merge.conflicts_left() == 0
    }

    /// The text to save: what was chosen, plus the changes that needed no choice.
    pub fn finish(&mut self) -> String {
        self.merge.apply_non_conflicting(None);
        self.rebuild();
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
            .conflicts
            .as_ref()
            .and_then(|popup| popup.selected_conflict().cloned())
        else {
            return;
        };
        self.autosave(cx);
        let root = self.root.clone();
        let (yours, theirs) = self
            .conflicts
            .as_ref()
            .map(|popup| popup.names())
            .unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let path = conflict.path.clone();
            let stages = cx
                .background_executor()
                .spawn(async move { zide_git::stages(&root, &path) })
                .await;
            this.update_in(cx, |this, window, cx| match stages {
                Ok(stages) => {
                    let merge = Merge::new(&stages.base, &stages.yours, &stages.theirs);
                    this.merge = Some(MergeEditor::new(conflict.path, yours, theirs, merge));
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
        self.merge = None;
        cx.notify();
    }

    /// Writes the result over the file and marks it resolved.
    pub(crate) fn save_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.merge else {
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
                    this.merge = None;
                } else if let Some(editor) = &mut this.merge {
                    editor.busy = false;
                }
                this.finish_resolve(outcome, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Keys while the merge editor is open: Esc closes it, F7 and Shift+F7 move between changes.
    pub(crate) fn merge_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let shift = event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "escape" => self.close_merge(cx),
            "f7" => {
                if let Some(editor) = &mut self.merge {
                    editor.step(if shift { -1 } else { 1 });
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn merge_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let editor = self.merge.as_ref().expect("the editor is open");
        let workspace = cx.entity();
        let rows = editor.rows.clone();
        let scroll = editor.scroll.clone();
        let (can_apply, busy) = (editor.can_apply(), editor.busy);
        let font: SharedString = EDITOR_FONTS[0].into();
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
                    .map(|position| row_view(&rows[position], position, &workspace, &font))
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
                                            if let Some(editor) = &mut this.merge {
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
                                            if let Some(editor) = &mut this.merge {
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

/// Changes the merge being edited.
fn edit(workspace: &Entity<Workspace>, cx: &mut App, change: impl FnOnce(&mut MergeEditor)) {
    workspace.update(cx, |this, cx| {
        if let Some(editor) = &mut this.merge {
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
                    if let Some(editor) = &mut this.merge {
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
                .child(number(&row.mid, NUMBER))
                .child(text(&row.mid)),
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

    #[::core::prelude::v1::test]
    fn lines_show_without_breaks_and_with_spaces_for_tabs() {
        assert_eq!(shown("\tx\r\n"), "    x");
    }
}
