//! The completion popup of the editor: what the language server offered for the word being
//! typed, narrowed down as typing goes on, with one row selected to accept.
//!
//! The state is plain data so it can be tested; the viewer draws it at the caret and the
//! workspace asks the server (see `Workspace::request_completion`).

use std::ops::Range;

use gpui_kit::{prelude::FluentBuilder, *};
use zide_lsp::{Completion, CompletionItem, CompletionItemKind, rank};

use crate::theme;

/// How many rows the popup shows at once; the selection scrolls the rest into view.
pub const VISIBLE_ROWS: usize = 10;

#[derive(Debug)]
pub struct CompletionPopup {
    items: Vec<CompletionItem>,
    /// The server left items out: typing more must ask it again.
    pub incomplete: bool,
    /// Char offset where the word being completed starts. Moving before it, or off its line,
    /// ends the popup.
    pub start: usize,
    /// The matching items, best first, as indices into `items`.
    shown: Vec<usize>,
    /// Index into `shown`.
    selected: usize,
    /// The first of the rows on screen, as an index into `shown`.
    first: usize,
}

impl CompletionPopup {
    /// The popup for the server's answer, with `typed` already typed after `start`; `None`
    /// when nothing matches.
    pub fn new(completion: Completion, start: usize, typed: &str) -> Option<Self> {
        let mut popup = Self {
            items: completion.items,
            incomplete: completion.incomplete,
            start,
            shown: Vec::new(),
            selected: 0,
            first: 0,
        };
        popup.refilter(typed).then_some(popup)
    }

    /// Narrows the items down to what matches `typed` now, keeping the selected item selected
    /// when it still matches. False when nothing does.
    pub fn refilter(&mut self, typed: &str) -> bool {
        let kept = self.shown.get(self.selected).copied();
        self.shown = rank(&self.items, typed);
        self.selected = kept
            .and_then(|item| self.shown.iter().position(|&i| i == item))
            .unwrap_or(0);
        self.first = 0;
        self.keep_visible();
        !self.shown.is_empty()
    }

    /// Moves the selection by `step` rows, wrapping around at either end.
    pub fn move_selection(&mut self, step: isize) {
        let len = self.shown.len() as isize;
        if len > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(len) as usize;
        }
        self.keep_visible();
    }

    /// Scrolls the rows on screen the least that shows the selection.
    fn keep_visible(&mut self) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + VISIBLE_ROWS {
            self.first = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    pub fn selected(&self) -> Option<&CompletionItem> {
        self.items.get(*self.shown.get(self.selected)?)
    }

    /// The rows on screen: up to [`VISIBLE_ROWS`], the selection among them.
    fn window(&self) -> Range<usize> {
        self.first..(self.first + VISIBLE_ROWS).min(self.shown.len())
    }

    /// How many rows the popup shows.
    pub fn rows_shown(&self) -> usize {
        self.window().len()
    }

    /// The popup's box, to be placed at the caret: one row per item with its kind, label,
    /// signature and detail, the selected one highlighted.
    pub fn view(&self, row_height: f32) -> Div {
        let window = self.window();
        let rows = self.shown[window.clone()]
            .iter()
            .enumerate()
            .map(|(at, &i)| {
                let item = &self.items[i];
                let selected = window.start + at == self.selected;
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(row_height))
                    .px_2()
                    .when(selected, |row| row.bg(rgb(theme::ACTIVE_BG)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(row_height * 2.))
                            .text_color(rgb(theme::SYMBOL))
                            .child(kind_label(item.kind)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_color(rgb(theme::STRONG))
                                    .child(SharedString::from(item.label.clone())),
                            )
                            .children(item.label_detail.clone().map(|signature| {
                                div().text_color(rgb(theme::DIM)).child(signature)
                            })),
                    )
                    .children(item.detail.clone().map(|detail| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_right()
                            .text_color(rgb(theme::DIM))
                            .child(detail)
                    }))
            });
        div()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .max_w(px(640.))
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::CONTROL_BORDER))
            .bg(rgb(theme::POPUP))
            .whitespace_nowrap()
            .overflow_hidden()
            .children(rows)
    }
}

/// A short tag for what an item is, shown before its label.
pub fn kind_label(kind: Option<CompletionItemKind>) -> &'static str {
    let Some(kind) = kind else { return "" };
    match kind {
        CompletionItemKind::METHOD => "method",
        CompletionItemKind::FUNCTION | CompletionItemKind::CONSTRUCTOR => "fn",
        CompletionItemKind::FIELD | CompletionItemKind::PROPERTY => "field",
        CompletionItemKind::VARIABLE => "var",
        CompletionItemKind::CLASS => "class",
        CompletionItemKind::STRUCT => "struct",
        CompletionItemKind::INTERFACE => "trait",
        CompletionItemKind::MODULE => "mod",
        CompletionItemKind::ENUM => "enum",
        CompletionItemKind::ENUM_MEMBER => "variant",
        CompletionItemKind::CONSTANT => "const",
        CompletionItemKind::KEYWORD => "kw",
        CompletionItemKind::SNIPPET => "snip",
        CompletionItemKind::TYPE_PARAMETER => "type",
        CompletionItemKind::FILE | CompletionItemKind::FOLDER => "file",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(labels: &[&str], incomplete: bool) -> Completion {
        let items: Vec<_> = labels
            .iter()
            .map(|label| serde_json::json!({"label": label}))
            .collect();
        let mut completion: Completion =
            zide_lsp::parse_completion(serde_json::json!(items)).unwrap();
        completion.incomplete = incomplete;
        completion
    }

    fn shown(popup: &CompletionPopup) -> Vec<&str> {
        popup
            .shown
            .iter()
            .map(|&i| popup.items[i].label.as_str())
            .collect()
    }

    #[::core::prelude::v1::test]
    fn the_popup_opens_only_when_something_matches() {
        assert!(CompletionPopup::new(answer(&["len"], false), 0, "x").is_none());
        let popup = CompletionPopup::new(answer(&["len", "push"], true), 4, "pu").unwrap();
        assert_eq!(shown(&popup), ["push"]);
        assert_eq!((popup.start, popup.incomplete), (4, true));
    }

    #[::core::prelude::v1::test]
    fn typing_keeps_the_selected_item_when_it_still_matches() {
        let mut popup =
            CompletionPopup::new(answer(&["len", "length", "last"], false), 0, "l").unwrap();
        popup.move_selection(1);
        let picked = popup.selected().unwrap().label.clone();
        assert!(popup.refilter("le"));
        assert_eq!(popup.selected().unwrap().label, picked);
        // gone from the list: the first one is selected
        assert!(popup.refilter("la"));
        assert_eq!(popup.selected().unwrap().label, "last");
        assert!(!popup.refilter("zz"));
        assert!(popup.selected().is_none());
    }

    #[::core::prelude::v1::test]
    fn the_selection_wraps_around() {
        let mut popup = CompletionPopup::new(answer(&["a1", "a2", "a3"], false), 0, "").unwrap();
        popup.move_selection(-1);
        assert_eq!(popup.selected().unwrap().label, "a3");
        popup.move_selection(1);
        assert_eq!(popup.selected().unwrap().label, "a1");
    }

    #[::core::prelude::v1::test]
    fn the_rows_on_screen_follow_the_selection() {
        let labels: Vec<String> = (0..25).map(|i| format!("item{i:02}")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut popup = CompletionPopup::new(answer(&labels, false), 0, "").unwrap();
        assert_eq!(popup.window(), 0..VISIBLE_ROWS);
        popup.move_selection(VISIBLE_ROWS as isize);
        assert_eq!(popup.window(), 1..VISIBLE_ROWS + 1);
        // back up within the rows on screen: they stay
        popup.move_selection(-1);
        assert_eq!(popup.window(), 1..VISIBLE_ROWS + 1);
        popup.move_selection(-9);
        assert_eq!(popup.window(), 0..VISIBLE_ROWS);
        popup.move_selection(-1); // wraps to the last
        assert_eq!(popup.window(), 25 - VISIBLE_ROWS..25);
        // narrowing down starts again from the top
        popup.refilter("item0");
        assert_eq!(popup.window(), 0..VISIBLE_ROWS);
    }

    #[::core::prelude::v1::test]
    fn kinds_have_short_tags() {
        assert_eq!(kind_label(Some(CompletionItemKind::METHOD)), "method");
        assert_eq!(kind_label(Some(CompletionItemKind::CONSTRUCTOR)), "fn");
        assert_eq!(kind_label(None), "");
    }
}
