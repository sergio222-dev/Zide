//! The completion popup of the editor: what the language server offered for the word being
//! typed, narrowed down as typing goes on, with one row selected to accept.
//!
//! The state is plain data so it can be tested; the viewer draws it at the caret and the
//! workspace asks the server (see `Workspace::request_completion`).

use std::ops::Range;

use std::{collections::HashSet, rc::Rc, sync::atomic::AtomicBool};

use gpui_kit::{prelude::FluentBuilder, *};
use zide_index::FileIndex;
use zide_lsp::{Completion, CompletionItem, CompletionItemKind, Documentation, rank};
use zide_stubs::{StubKind, StubStore, language_for_path, search_symbols};

use crate::{markdown, theme};

/// Width of the documentation beside the popup.
const DOCS_WIDTH: f32 = 420.;

/// A click on a row of the completion popup, by its index among the rows shown.
pub type CompletionPick = Rc<dyn Fn(usize, &mut App)>;

/// Width of the kind column, in editor cells: the longest tag (`variant`, `method`) and a space,
/// so a tag never runs into the label after it.
const KIND_CELLS: f32 = 8.;

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
    /// Whether an item was picked on purpose (moved to or clicked): only then Enter takes it,
    /// otherwise Enter is a new line.
    chosen: bool,
    /// The first of the rows on screen, as an index into `shown`.
    first: usize,
    /// Items (indices into `items`) the server already filled in, or had nothing more for.
    resolved: HashSet<usize>,
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
            chosen: false,
            first: 0,
            resolved: HashSet::new(),
        };
        popup.refilter(typed).then_some(popup)
    }

    /// Narrows the items down to what matches `typed` now, keeping the selected item selected
    /// when it still matches. False when nothing does.
    pub fn chosen(&self) -> bool {
        self.chosen
    }

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
            self.chosen = true;
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

    /// Selects the `index`-th row shown (from the top of the list, not of the screen).
    pub fn select(&mut self, index: usize) {
        if index < self.shown.len() {
            self.selected = index;
            self.chosen = true;
            self.keep_visible();
        }
    }

    /// The selected item, with its index, when the server has not filled it in yet.
    pub fn to_resolve(&self) -> Option<(usize, CompletionItem)> {
        let index = *self.shown.get(self.selected)?;
        (!self.resolved.contains(&index)).then(|| (index, self.items[index].clone()))
    }

    /// Takes what the server filled in for the `index`-th item (`None`: nothing more), unless
    /// the popup holds other items by now.
    pub fn resolved(&mut self, index: usize, item: Option<CompletionItem>, label: &str) {
        let Some(target) = self.items.get_mut(index).filter(|i| i.label == label) else {
            return;
        };
        if let Some(item) = item {
            target.absorb(item);
        }
        self.resolved.insert(index);
    }

    /// The selected item's documentation, with its detail on top, to show beside the popup;
    /// `None` when it has none.
    pub fn docs_view(&self, max_height: f32) -> Option<Div> {
        let item = self.selected()?;
        let documentation = item.documentation.as_ref()?;
        let body = match documentation {
            Documentation::Markdown(text) => div().child(
                markdown::TextView::markdown(
                    SharedString::from(format!("completion-docs-{}", item.label)),
                    text.clone(),
                )
                .inline_code_style(markdown::inline_code()),
            ),
            Documentation::Plain(text) => div().whitespace_normal().child(text.clone()),
        };
        Some(
            div()
                .w(px(DOCS_WIDTH))
                .max_h(px(max_height))
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(theme::CONTROL_BORDER))
                .bg(rgb(theme::POPUP))
                .text_sm()
                .whitespace_normal()
                .children(
                    item.detail
                        .clone()
                        .map(|detail| div().text_color(rgb(theme::STRONG)).child(detail)),
                )
                .child(body),
        )
    }

    /// Room the documentation beside the popup takes, in pixels (none without any).
    pub fn docs_width(&self) -> f32 {
        let shown = self.selected().is_some_and(|i| i.documentation.is_some());
        if shown { DOCS_WIDTH + 8. } else { 0. }
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
    pub fn view(&self, row_height: f32, cell_width: f32, on_pick: CompletionPick) -> Div {
        let window = self.window();
        let rows = self.shown[window.clone()]
            .iter()
            .enumerate()
            .map(|(at, &i)| {
                let item = &self.items[i];
                let index = window.start + at;
                let selected = index == self.selected;
                let pick = on_pick.clone();
                div()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation(); // not a click on the text under it
                        pick(index, cx);
                    })
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(row_height))
                    .px_2()
                    .when(selected, |row| row.bg(rgb(theme::ACTIVE_BG)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(KIND_CELLS * cell_width))
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

/// How many project symbols local completion looks at for one word.
const SYMBOL_LIMIT: usize = 200;

/// Completion without a language server: the project's symbols of the file's language whose
/// names match `typed`, then the other words of `text` (the word at the cursor, char range
/// `typing`, left out). Always incomplete: the symbols depend on what is typed, so each key
/// asks again.
pub fn local_completion(
    text: &str,
    typing: Range<usize>,
    typed: &str,
    path: &str,
    project: Option<(&StubStore, &FileIndex)>,
) -> Completion {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    if let Some((store, index)) = project.filter(|_| !typed.is_empty()) {
        let language = language_for_path(path);
        let hits = search_symbols(store, typed, SYMBOL_LIMIT, &AtomicBool::new(false));
        for hit in hits.unwrap_or_default() {
            let symbol = store.symbol(hit.entry);
            let file = index.paths().get(symbol.file).map_or("", String::as_str);
            let (Some(name), true) = (&symbol.node.name, language_for_path(file) == language)
            else {
                continue;
            };
            if seen.insert(name.to_string()) {
                let detail = symbol
                    .container
                    .map(str::to_string)
                    .or_else(|| Some(file.rsplit('/').next().unwrap_or(file).to_string()));
                let kind = symbol_kind(symbol.node.kind);
                items.push(CompletionItem::plain(name.to_string(), kind, detail));
            }
        }
    }
    for word in buffer_words(text, typing) {
        if seen.insert(word.clone()) {
            items.push(CompletionItem::plain(
                word,
                Some(CompletionItemKind::TEXT),
                None,
            ));
        }
    }
    Completion {
        items,
        incomplete: true,
    }
}

/// The identifiers of `text` (two characters or more), first appearance first, leaving out
/// the one overlapping the char range `skip`: the word being typed is no suggestion for itself.
fn buffer_words(text: &str, skip: Range<usize>) -> Vec<String> {
    let word_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '$');
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut start = 0;
    let chars = text.chars().chain(std::iter::once(' '));
    for (at, c) in chars.enumerate() {
        if word_char(c) {
            if word.is_empty() {
                start = at;
            }
            word.push(c);
            continue;
        }
        let overlaps = !skip.is_empty() && start < skip.end && skip.start < at;
        let identifier = !word.starts_with(|c: char| c.is_ascii_digit());
        if word.chars().nth(1).is_some() && identifier && !overlaps && seen.insert(word.clone()) {
            words.push(std::mem::take(&mut word));
        }
        word.clear();
    }
    words
}

/// The completion kind of a declaration, for its tag in the popup.
fn symbol_kind(kind: StubKind) -> Option<CompletionItemKind> {
    Some(match kind {
        StubKind::Function | StubKind::Macro => CompletionItemKind::FUNCTION,
        StubKind::Method => CompletionItemKind::METHOD,
        StubKind::Struct => CompletionItemKind::STRUCT,
        StubKind::Class => CompletionItemKind::CLASS,
        StubKind::Interface | StubKind::Trait => CompletionItemKind::INTERFACE,
        StubKind::Enum => CompletionItemKind::ENUM,
        StubKind::Variant => CompletionItemKind::ENUM_MEMBER,
        StubKind::Field => CompletionItemKind::FIELD,
        StubKind::Module => CompletionItemKind::MODULE,
        StubKind::TypeAlias => CompletionItemKind::TYPE_PARAMETER,
        StubKind::Const => CompletionItemKind::CONSTANT,
        StubKind::Impl | StubKind::Import => return None,
    })
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
        CompletionItemKind::TEXT => "word",
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
    fn an_item_is_chosen_only_once_moved_to_or_clicked() {
        let answer = || answer(&["a1", "a2", "a3"], false);
        let mut popup = CompletionPopup::new(answer(), 0, "").unwrap();
        assert!(!popup.chosen()); // the first is selected, not chosen
        assert!(popup.refilter("a"));
        assert!(!popup.chosen());
        popup.move_selection(1);
        assert!(popup.chosen());
        let mut popup = CompletionPopup::new(answer(), 0, "").unwrap();
        popup.select(2);
        assert!(popup.chosen());
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
    fn the_words_of_the_file_are_offered_once_without_the_one_being_typed() {
        let text = "let total = totals + 1;\nlet x = to";
        let typing = 32..34; // the `to` at the end
        assert_eq!(&text.chars().skip(32).collect::<String>(), "to");
        assert_eq!(
            buffer_words(text, typing.clone()),
            ["let", "total", "totals"]
        );
        let completion = local_completion(text, typing, "to", "a.rs", None);
        assert!(completion.incomplete);
        let popup = CompletionPopup::new(completion, 32, "to").unwrap();
        assert_eq!(shown(&popup), ["total", "totals"]);
        assert_eq!(
            popup.selected().unwrap().kind,
            Some(CompletionItemKind::TEXT)
        );
        // numbers and one-letter words are no suggestions; `$` and `_` are word characters
        assert_eq!(buffer_words("a 12 x9 $el _id", 0..0), ["x9", "$el", "_id"]);
    }

    #[::core::prelude::v1::test]
    fn project_symbols_of_the_same_language_come_first() {
        let dir = std::env::temp_dir().join(format!("zide-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "struct Totals;\nfn total_count() {}\n").unwrap();
        std::fs::write(dir.join("b.py"), "def total_py():\n    pass\n").unwrap();
        let index = std::sync::Arc::new(FileIndex::new(vec!["a.rs".into(), "b.py".into()]));
        let store = StubStore::build(index.clone(), &dir, None, &AtomicBool::new(false)).unwrap();
        let completion = local_completion(
            "let total = to",
            12..14,
            "to",
            "c.rs",
            Some((&store, &index)),
        );
        let labels: Vec<_> = completion.items.iter().map(|i| i.label.as_str()).collect();
        // the Python one is left out; the file's own `total` comes after the symbols
        assert_eq!(labels, ["total_count", "Totals", "let", "total"]);
        assert_eq!(completion.items[0].kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(completion.items[0].detail.as_deref(), Some("a.rs"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[::core::prelude::v1::test]
    fn a_word_right_after_the_cursor_is_still_offered() {
        // with nothing typed yet, the word that follows the cursor is not the one being typed
        assert_eq!(buffer_words("ab\ncd", 3..3), ["ab", "cd"]);
        assert_eq!(buffer_words("ab cd", 3..3), ["ab", "cd"]);
    }

    #[::core::prelude::v1::test]
    fn each_selected_item_is_resolved_once() {
        let mut popup = CompletionPopup::new(answer(&["a1", "a2"], false), 0, "").unwrap();
        let (index, item) = popup.to_resolve().unwrap();
        assert_eq!((index, item.label.as_str()), (0, "a1"));
        // still asked for until the answer comes: a question given up on is asked again
        assert!(popup.to_resolve().is_some());
        let filled = zide_lsp::parse_completion(serde_json::json!([
            {"label": "a1", "documentation": "the first"}
        ]))
        .unwrap()
        .items
        .remove(0);
        popup.resolved(0, Some(filled), "a1");
        assert!(popup.to_resolve().is_none());
        assert_eq!(
            popup.selected().unwrap().documentation,
            Some(Documentation::Plain("the first".into()))
        );
        assert!(popup.docs_width() > 0.);
        popup.move_selection(1);
        assert_eq!(popup.to_resolve().unwrap().0, 1);
        assert_eq!(popup.docs_width(), 0.);
        // nothing more to say is remembered too
        popup.resolved(1, None, "a2");
        assert!(popup.to_resolve().is_none());
        // an answer meant for other items is dropped
        popup.resolved(0, None, "zz");
    }

    #[::core::prelude::v1::test]
    fn kinds_have_short_tags() {
        // every tag fits its column with a space to spare, whatever the kind
        for kind in 1..=25 {
            let kind: CompletionItemKind = serde_json::from_value(serde_json::json!(kind)).unwrap();
            let tag = kind_label(Some(kind));
            assert!((tag.chars().count() as f32) < KIND_CELLS, "{tag}");
        }
        assert_eq!(kind_label(Some(CompletionItemKind::METHOD)), "method");
        assert_eq!(kind_label(Some(CompletionItemKind::CONSTRUCTOR)), "fn");
        assert_eq!(kind_label(None), "");
    }
}
