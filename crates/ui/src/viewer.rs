//! File editor view: line numbers, syntax colors, a block cursor and Vim keys.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::SystemTime,
    time::{Duration, Instant},
};

use gpui_kit::*;
use zide_git::{BlameLine, LineChange};
use zide_lsp::{
    Diagnostic, DiagnosticSeverity, NumberOrString, ParameterHint, char_column, uri_to_path,
};
use zide_stubs::{
    StubKind, StubNode, extract_nodes, extract_nodes_in, language_for_path as stub_language,
};
use zide_syntax::{Class, FoldRegion, LanguageId, Span, Syntax, language_for_path};
use zide_text::{Buffer, FindQuery, Finder, Position, Revision};
use zide_vim::{Key, Mode, Vim};

use crate::{
    indent,
    rows::Rows,
    sticky::{MAX_STICKY, Scope, sticky_headers},
    theme,
};

/// Files larger than this are not opened.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TAB_WIDTH: usize = 4;
/// Row height as a multiple of the font size (20px at the default 16px).
const ROW_RATIO: f32 = 1.25;
pub const DEFAULT_FONT_SIZE: f32 = 16.;
pub const MIN_FONT_SIZE: f32 = 8.;
pub const MAX_FONT_SIZE: f32 = 48.;
/// Monospace families, in order of preference. GPUI does not resolve the generic `monospace`.
const EDITOR_FONTS: &[&str] = &[
    "JetBrains Mono",
    "Iosevka Nerd Font Mono",
    "0xProto Nerd Font Mono",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Adwaita Mono",
    "Liberation Mono",
    "Menlo",
    "Consolas",
];

/// Called with the start line of the region whose chevron was clicked.
pub type FoldClick = Rc<dyn Fn(usize, &mut App)>;

/// Called with the line and window position the mouse is over, or `None` when it left the
/// editor.
pub type HoverMove = Rc<dyn Fn(Option<(usize, Point<Pixels>)>, &mut App)>;

/// Called with the line and window position of a click on the text.
pub type TextClick = Rc<dyn Fn(usize, Point<Pixels>, &mut App)>;

/// Per line, the parameter name hints to show: the column they go before and their text.
type LineHints = HashMap<usize, Vec<(usize, String)>>;

/// Parameter names a language server gave for the text as of `revision`, as char offsets.
struct HintSet {
    revision: Revision,
    /// The text they were given for, to follow the edits made since.
    text: Arc<String>,
    hints: Vec<(usize, String)>,
}

/// Per line, the diagnostics to underline and their display bytes.
type Squiggles = HashMap<usize, Vec<(usize, Range<usize>)>>;

/// The error popup: which diagnostics are under the mouse and where to show them, relative to
/// the editor.
struct Hover {
    diagnostics: Vec<usize>,
    at: Point<Pixels>,
}

struct Line {
    /// Char offset of the line in the buffer, and its length in chars.
    start: usize,
    chars: usize,
    text: String,
    colors: Vec<(Range<usize>, HighlightStyle)>,
    /// Display byte of each `char` of the line (and of its end), when that is not the identity
    /// (tabs, multi-byte characters).
    cols: Option<Vec<usize>>,
    /// Parameter name hints shown in the line: where each was put (a display byte of the line
    /// without hints) and how many bytes it takes, in order.
    inserts: Vec<(usize, usize)>,
    /// How many indent bands the line has, each `step` columns wide.
    level: usize,
    step: usize,
}

impl Line {
    /// Where the display byte `without_hints` of the line without hints is once they are in.
    fn with_hints(&self, without_hints: usize) -> usize {
        let before = self
            .inserts
            .iter()
            .take_while(|(at, _)| *at <= without_hints);
        without_hints + before.map(|(_, size)| size).sum::<usize>()
    }

    fn display_byte(&self, column: usize) -> usize {
        match &self.cols {
            Some(cols) => cols[column.min(cols.len() - 1)],
            None => column.min(self.text.len()),
        }
    }
}

/// The part of the buffer that Visual mode selected, in lines and columns.
#[derive(Clone, Copy)]
struct Selection {
    start: Position,
    /// Exclusive.
    end: Position,
    linewise: bool,
}

impl Selection {
    /// The display bytes of `row` that are selected, if any.
    fn on_line(&self, row: usize, line: &Line) -> Option<Range<usize>> {
        let last_row = if self.end.column == 0 && self.end.line > self.start.line {
            self.end.line - 1
        } else {
            self.end.line
        };
        if row < self.start.line || row > last_row {
            return None;
        }
        if self.linewise {
            return Some(0..line.text.len());
        }
        let from = if row == self.start.line {
            self.start.column
        } else {
            0
        };
        let to = if row == self.end.line {
            line.display_byte(self.end.column)
        } else {
            line.text.len()
        };
        Some(line.display_byte(from)..to)
    }
}

/// What the file looked like on disk when it was read or last written.
type DiskStamp = (SystemTime, u64);

fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// What the editor asks the workspace to do, beyond changing its own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    /// `/` was typed: open the find panel.
    OpenFind,
}

/// A search over the buffer: the query, and where it matches (char ranges).
struct FindState {
    finder: Option<Finder>,
    error: Option<String>,
    matches: Arc<Vec<Range<usize>>>,
    built_at: Option<Revision>,
    /// Matches are painted only while the panel is open; `n`/`N` keep working after it closes.
    visible: bool,
}

/// What the find panel shows about the current search.
pub struct FindStatus {
    pub total: usize,
    /// 1-based index of the match the cursor is on.
    pub current: Option<usize>,
    pub error: Option<String>,
}

/// A scroll in flight: the view's top, in pixels, glides from `from` to `to`.
struct ScrollAnim {
    from: f32,
    to: f32,
    start: Instant,
}

const SCROLL_DURATION: Duration = Duration::from_millis(160);

/// What a formatter is asked for, see [`Viewer::format_request`].
pub struct FormatRequest {
    pub revision: Revision,
    pub text: String,
    pub range: Option<zide_lsp::Range>,
}

/// The selections Extend Selection went through, so Shrink Selection can walk back.
struct SelectionTrail {
    revision: Revision,
    /// What the last extend selected; anything else selected since breaks the trail.
    current: Range<usize>,
    /// What was selected before each step (`None`: nothing, only the caret), and the caret.
    history: Vec<(Option<Range<usize>>, usize)>,
}

/// Revision of the text and where the highlighted word starts.
pub type HighlightKey = (Revision, usize);

pub struct HighlightTarget {
    pub key: HighlightKey,
    pub line: u32,
    pub character: u32,
}

pub struct Viewer {
    find: Option<FindState>,
    path: String,
    full_path: PathBuf,
    /// False when the file could not be read: the buffer then holds an error message.
    editable: bool,
    saved_at: Revision,
    disk: Option<DiskStamp>,
    /// Last thing worth telling the user (a failed save).
    notice: Option<String>,
    buffer: Buffer,
    vim: Vim,
    language: Option<LanguageId>,
    /// The syntax tree of `buffer` as of `built_at`, updated by each rebuild.
    syntax: Option<Syntax>,
    trail: Option<SelectionTrail>,
    /// Usages of the word at `key` that a language server found.
    semantic: Option<(HighlightKey, Arc<Vec<Range<usize>>>)>,
    /// Display lines of `buffer` as of `built_at`.
    lines: Arc<Vec<Line>>,
    /// Which of those lines the list shows, one per row.
    rows: Arc<Rows>,
    /// Regions of the text that can fold, by start line, as of `built_at`.
    folds: Vec<FoldRegion>,
    /// Start lines of the regions folded away; the region must still exist to count.
    collapsed: BTreeSet<usize>,
    built_at: Revision,
    scroll: UniformListScrollHandle,
    /// A `z` was typed and its second key (`t`, `z`, `b`...) is awaited.
    z_pending: bool,
    anim: Option<ScrollAnim>,
    /// Advance of one monospace cell in pixels, measured on the first frame.
    cell_width: Option<f32>,
    font_size: f32,
    /// Scroll position (px) as of the last frame, before any wheel event of the next one.
    settled_top: f32,
    font_family: SharedString,
    /// Declarations of the file as of `built_at`, for the breadcrumb.
    outline: Vec<StubNode>,
    /// The declarations of `outline` that span several lines, for sticky scroll.
    scopes: Vec<Scope>,
    /// Name of the project folder, first crumb of the breadcrumb.
    root_name: String,
    /// Who last changed each line of the file on disk, when git knows it.
    blame: Option<Arc<Vec<BlameLine>>>,
    /// The file as of the last commit, to mark what changed since.
    head: Option<Arc<String>>,
    /// The lines that differ from `head`, as of the current text.
    changes: Arc<HashMap<usize, LineChange>>,
    /// What the language server reported about the file.
    diagnostics: Arc<Vec<Diagnostic>>,
    /// The errors of `diagnostics` to underline, by line: the diagnostic and its display bytes.
    squiggles: Arc<Squiggles>,
    hover: Option<Hover>,
    /// The parameter names shown before arguments.
    hints: Option<HintSet>,
    /// Where the editor was drawn last, to turn a mouse position into a line and column.
    editor_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The two ends of the pair under the caret (char ranges), for the revision and caret
    /// they were found at.
    pair: RefCell<Option<(PairKey, Pair)>>,
}

/// Revision, caret and whether it is inserting: what `pair` depends on.
type PairKey = (Revision, usize, bool);
type Pair = Arc<Vec<Range<usize>>>;

impl Viewer {
    /// Opens `root/relative`. Errors are shown as the file's content, not propagated.
    pub fn open(root: &Path, relative: &str) -> Self {
        let full = root.join(relative);
        let disk = disk_stamp(&full);
        let text = match fs::metadata(&full) {
            Ok(m) if m.len() > MAX_BYTES => Err(format!("{relative}: file too large to open")),
            Ok(_) => fs::read_to_string(&full).map_err(|e| format!("{relative}: {e}")),
            Err(e) => Err(format!("{relative}: {e}")),
        };
        let editable = text.is_ok();
        let text = text.unwrap_or_else(|message| message);
        let buffer = Buffer::new(&text);
        let language = language_for_path(relative);
        let mut viewer = Self {
            find: None,
            path: relative.to_string(),
            full_path: full,
            editable,
            saved_at: Revision::default(),
            disk,
            notice: None,
            vim: Vim::default(),
            language,
            syntax: None,
            trail: None,
            semantic: None,
            lines: Arc::default(),
            rows: Arc::default(),
            folds: Vec::new(),
            collapsed: BTreeSet::new(),
            built_at: buffer.revision(),
            buffer,
            scroll: UniformListScrollHandle::new(),
            z_pending: false,
            anim: None,
            cell_width: None,
            font_size: DEFAULT_FONT_SIZE,
            settled_top: 0.,
            font_family: EDITOR_FONTS[0].into(),
            outline: Vec::new(),
            scopes: Vec::new(),
            root_name: root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            blame: None,
            head: None,
            diagnostics: Arc::default(),
            squiggles: Arc::default(),
            hover: None,
            hints: None,
            editor_bounds: Rc::default(),
            changes: Arc::default(),
            pair: RefCell::new(None),
        };
        viewer.rebuild();
        viewer
    }

    /// Records who last changed each line of the file on disk (`None`: git has no answer).
    pub fn set_blame(&mut self, blame: Option<Vec<BlameLine>>) {
        self.blame = blame.map(Arc::new);
    }

    /// Records the parameter names a language server gave for the text as of `revision`.
    /// Dropped when the text has changed meanwhile (the next answer is for the newer text).
    /// True when what is shown changed.
    pub fn set_hints(&mut self, hints: Vec<ParameterHint>, revision: Revision) -> bool {
        if revision != self.buffer.revision() {
            return false;
        }
        let mut offsets: Vec<(usize, String)> = hints
            .into_iter()
            .filter(|hint| (hint.position.line as usize) < self.buffer.line_count())
            .map(|hint| {
                let line = hint.position.line as usize;
                let column = char_column(&self.buffer.line(line), hint.position.character);
                let at = self.buffer.line_start(line) + column.min(self.buffer.line_len(line));
                (at, hint.label)
            })
            .collect();
        offsets.sort_by_key(|(at, _)| *at);
        let before = self.hints_by_line();
        self.hints = Some(HintSet {
            revision,
            text: Arc::new(self.buffer.text()),
            hints: offsets,
        });
        if self.hints_by_line() == before {
            return false;
        }
        self.rebuild();
        true
    }

    /// The hints to show, by line. Hints given for older text move with the edits made since
    /// (as one edit: what was typed between the text they were given for and now).
    fn hints_by_line(&self) -> LineHints {
        let mut by_line = LineHints::new();
        let Some(set) = &self.hints else {
            return by_line;
        };
        let current;
        let hints = if set.revision == self.buffer.revision() {
            &set.hints
        } else {
            current = follow_edit(&set.text, &self.buffer.text(), &set.hints);
            &current
        };
        for (at, label) in hints {
            let position = self.buffer.position_of(*at);
            by_line
                .entry(position.line)
                .or_default()
                .push((position.column, label.clone()));
        }
        by_line
    }

    /// Records what the language server reports about the file. True when it is not what was
    /// known before.
    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) -> bool {
        if *self.diagnostics == diagnostics {
            return false;
        }
        self.diagnostics = Arc::new(diagnostics);
        self.hover = None;
        self.refresh_squiggles();
        true
    }

    pub fn revision(&self) -> Revision {
        self.buffer.revision()
    }

    /// Finds the display bytes of every error on each of its lines. Positions are the ones the
    /// server saw, so after an edit they can be off until it answers again, but never out of
    /// the file.
    fn refresh_squiggles(&mut self) {
        let mut squiggles = Squiggles::new();
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if diagnostic
                .severity
                .is_some_and(|severity| severity != DiagnosticSeverity::ERROR)
            {
                continue;
            }
            let (start, end) = (diagnostic.range.start, diagnostic.range.end);
            for row in start.line as usize..=end.line as usize {
                let Some(line) = self.lines.get(row).filter(|line| line.chars > 0) else {
                    continue;
                };
                let raw = self.buffer.line(row);
                let from = match row == start.line as usize {
                    true => char_column(&raw, start.character),
                    false => 0,
                }
                .min(line.chars - 1);
                let to = match row == end.line as usize {
                    true => char_column(&raw, end.character),
                    false => line.chars,
                }
                .min(line.chars)
                .max(from + 1); // an empty range still gets a character to underline
                squiggles
                    .entry(row)
                    .or_default()
                    .push((index, line.display_byte(from)..line.display_byte(to)));
            }
        }
        self.squiggles = Arc::new(squiggles);
    }

    /// Width of the line numbers column.
    fn gutter_width(&self) -> f32 {
        let digits = self.lines.len().to_string().len();
        (12. * digits as f32 + 28.) * self.font_size / DEFAULT_FONT_SIZE
    }

    /// The cell of the line text at window position `x`, `None` over the line numbers. With
    /// `nearest` the edge closest to `x` is meant (where a caret goes), else the cell it is in.
    fn cell_under(&self, x: Pixels, nearest: bool) -> Option<usize> {
        let x = f32::from(x - self.editor_bounds.get().origin.x) - self.gutter_width();
        let cells = x / self.cell_width.unwrap_or(8.);
        (x >= 0.).then_some((if nearest { cells + 0.5 } else { cells }) as usize)
    }

    /// A click at window position `at` on `row`: the caret goes to that character. True when it
    /// was on the text.
    pub fn click(&mut self, row: usize, at: Point<Pixels>) -> bool {
        let Some(line) = self.lines.get(row) else {
            return false;
        };
        let inserting = self.vim.mode() == Mode::Insert;
        let Some(cell) = self.cell_under(at.x, inserting) else {
            return false;
        };
        // the display byte of that cell, then the character it belongs to (a tab spans several)
        let byte = line
            .text
            .char_indices()
            .nth(cell)
            .map_or(line.text.len(), |(byte, _)| byte);
        let column = match &line.cols {
            Some(cols) => cols.iter().rposition(|&shown| shown <= byte).unwrap_or(0),
            None => byte,
        }
        .min(line.chars);
        let offset = self.buffer.line_start(row) + column;
        self.anim = None;
        self.z_pending = false;
        self.vim.click(&mut self.buffer, offset);
        true
    }

    /// The mouse is at `at` (window coordinates) over `row`, or left the editor. True when the
    /// popup must change.
    pub fn set_hover(&mut self, over: Option<(usize, Point<Pixels>)>) -> bool {
        let found = over.and_then(|(row, at)| {
            let bounds = self.editor_bounds.get();
            let line = self.lines.get(row)?;
            let column = self.cell_under(at.x, false)?;
            let byte = line.text.char_indices().nth(column)?.0;
            let under: Vec<usize> = self
                .squiggles
                .get(&row)?
                .iter()
                .filter(|(_, range)| range.contains(&byte))
                .map(|(index, _)| *index)
                .collect();
            (!under.is_empty()).then(|| {
                let shown = point(at.x - bounds.origin.x, at.y - bounds.origin.y + px(12.));
                (under, shown)
            })
        });
        match (found, &self.hover) {
            (None, None) => false,
            (Some((under, _)), Some(hover)) if under == hover.diagnostics => false,
            (found, _) => {
                self.hover = found.map(|(diagnostics, at)| Hover { diagnostics, at });
                true
            }
        }
    }

    /// The popup with the errors under the mouse.
    fn hover_popup(&self) -> Option<Div> {
        let hover = self.hover.as_ref()?;
        let room = f32::from(self.editor_bounds.get().size.width);
        let left = f32::from(hover.at.x).min((room - 540.).max(0.));
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(hover.at.y)
                .max_w(px(520.))
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(theme::CONTROL_BORDER))
                .bg(rgb(theme::POPUP))
                .text_sm()
                .whitespace_normal()
                .flex()
                .flex_col()
                .gap_2()
                .children(
                    hover
                        .diagnostics
                        .iter()
                        .filter_map(|&index| self.diagnostics.get(index))
                        .map(|diagnostic| {
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_color(rgb(theme::STRONG))
                                        .child(headline(diagnostic)),
                                )
                                .children(related_lines(diagnostic).into_iter().map(|line| {
                                    div().text_color(rgb(theme::CARET_BAR)).child(line)
                                }))
                        }),
                ),
        )
    }

    /// Records the file as of the last commit (`None`: nothing to compare with) and marks the
    /// lines that differ from it.
    pub fn set_head(&mut self, head: Option<String>) {
        self.head = head.map(Arc::new);
        self.refresh_changes();
    }

    fn refresh_changes(&mut self) {
        self.changes = Arc::new(match &self.head {
            Some(head) if self.editable => zide_git::line_changes(head, &self.buffer.text()),
            _ => HashMap::new(),
        });
    }

    /// The blame of the cursor's line, in gray after the code. Only while the buffer is what is
    /// on disk: after an edit the lines no longer match what git saw.
    fn blame_label(&self) -> Option<String> {
        if self.is_dirty() {
            return None;
        }
        Some(self.blame.as_ref()?.get(self.cursor_line())?.label())
    }

    /// Both ends of the bracket, JSX tag or Python block under the caret (the character before
    /// it also counts while inserting), so you can see where it closes. Cached per caret.
    fn pair_highlights(&self) -> Pair {
        let caret = self.vim.cursor();
        let inserting = self.vim.mode() == Mode::Insert;
        let key = (self.buffer.revision(), caret, inserting);
        if let Some((cached, found)) = &*self.pair.borrow()
            && *cached == key
        {
            return found.clone();
        }
        let found = Arc::new(self.find_pair(caret, inserting).unwrap_or_default());
        *self.pair.borrow_mut() = Some((key, found.clone()));
        found
    }

    fn find_pair(&self, caret: usize, inserting: bool) -> Option<Vec<Range<usize>>> {
        let language = self.language?;
        let before = if inserting {
            caret.checked_sub(1)
        } else {
            None
        };
        let at = before.into_iter().chain([caret]).find(|&at| {
            self.buffer
                .slice(at..at + 1)
                .chars()
                .next()
                .is_some_and(|c| zide_syntax::is_pair_candidate(language, c))
        })?;
        let text = self.buffer.text();
        let ends = zide_syntax::matching_pair(language, &text, self.buffer.byte_of(at))?;
        Some(
            ends.into_iter()
                .map(|r| self.buffer.offset_of_byte(r.start)..self.buffer.offset_of_byte(r.end))
                .collect(),
        )
    }

    /// Puts the cursor on 1-based `line` and brings it into view.
    pub fn go_to_line(&mut self, line: u32) {
        let row = (line as usize).saturating_sub(1).min(self.lines.len() - 1);
        let offset = self.buffer.line_start(row);
        self.vim.set_cursor(&self.buffer, offset);
        self.vim.handle(&mut self.buffer, Key::Char('^')); // first non-blank of the line
        self.unfold_hiding(row);
        self.scroll
            .scroll_to_item(self.rows.row_of(row), ScrollStrategy::Top);
    }

    /// Shows `notice` in the status line (`None` clears it).
    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// True when the file changed on disk and there is nothing of ours to lose by re-reading it.
    pub fn is_stale(&self) -> bool {
        !self.is_dirty() && disk_stamp(&self.full_path) != self.disk
    }

    pub fn is_dirty(&self) -> bool {
        self.editable && self.buffer.revision() != self.saved_at
    }

    /// Writes the buffer to its file when it has unsaved changes. Refuses to overwrite a file
    /// that changed on disk since it was read: the notice says so and the edits stay in memory.
    /// True when it wrote the file (its blame is then out of date).
    pub fn save(&mut self) -> bool {
        if !self.is_dirty() {
            return false;
        }
        if disk_stamp(&self.full_path) != self.disk {
            self.notice = Some("not saved: the file changed on disk".into());
            return false;
        }
        match fs::write(&self.full_path, self.buffer.text()) {
            Ok(()) => {
                self.saved_at = self.buffer.revision();
                self.disk = disk_stamp(&self.full_path);
                self.notice = None;
                self.blame = None;
                true
            }
            Err(e) => {
                self.notice = Some(format!("not saved: {e}"));
                false
            }
        }
    }

    /// Where the file is now: the first line on screen and the caret. `None` before the first
    /// layout, when the scroll is not known.
    pub fn view(&self) -> Option<crate::session::View> {
        self.view_metrics()?;
        let at = self.buffer.position_of(self.vim.cursor());
        Some(crate::session::View {
            top: self
                .rows
                .line((self.settled_top / self.row_height()) as usize),
            line: at.line,
            column: at.column,
        })
    }

    /// Goes back to where the file was left (what it has now may be shorter).
    pub fn restore_view(&mut self, view: crate::session::View) {
        let line = view.line.min(self.lines.len() - 1);
        let offset = self.buffer.line_start(line) + view.column;
        self.vim.set_cursor(&self.buffer, offset);
        let top = view.top.min(self.lines.len() - 1);
        // strict: a line that already fits on screen from the top of the file still goes to the
        // top of the view
        self.scroll
            .scroll_to_item_strict(self.rows.row_of(top), ScrollStrategy::Top);
    }

    /// `(first visible row, visible rows)` as of the last layout, if there was one.
    fn view_metrics(&self) -> Option<(usize, usize)> {
        let state = self.scroll.0.borrow();
        let viewport = f32::from(state.last_item_size?.item.height);
        let top = (-f32::from(state.base_handle.offset().y) / self.row_height()).max(0.) as usize;
        let rows = (viewport / self.row_height()) as usize;
        Some((top, rows.max(1)))
    }

    /// Where the word under the caret appears in the rows on screen (and a screenful around
    /// them, which absorbs a scroll glide), as char ranges: IntelliJ's usage highlighting,
    /// by text. Nothing while selecting or searching, when it would only add noise.
    fn word_highlights(&self) -> Arc<Vec<Range<usize>>> {
        if !self.highlights_apply() {
            return Arc::default();
        }
        let Some(range) = self.word_range_at_cursor() else {
            return Arc::default();
        };
        if let Some((key, found)) = &self.semantic
            && *key == (self.buffer.revision(), range.start)
        {
            return found.clone();
        }
        let word = self.buffer.slice(range);
        let (top, rows) = self.view_metrics().unwrap_or((0, 60));
        let first = self.rows.line(top.saturating_sub(rows));
        let last = (self.rows.line(top + 2 * rows) + 1).min(self.buffer.line_count());
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut found = Vec::new();
        for row in first..last {
            let line = self.buffer.line(row);
            let start = self.buffer.line_start(row);
            for (at, _) in line.match_indices(&word) {
                let before = line[..at].chars().next_back();
                let after = line[at + word.len()..].chars().next();
                if before.is_some_and(is_word) || after.is_some_and(is_word) {
                    continue;
                }
                let from = start + line[..at].chars().count();
                found.push(from..from + word.chars().count());
            }
        }
        Arc::new(found)
    }

    /// `(viewport height, current top)` in pixels, once the list has been laid out.
    fn view_pixels(&self) -> Option<(f32, f32)> {
        let state = self.scroll.0.borrow();
        let viewport = f32::from(state.last_item_size?.item.height);
        Some((viewport, -f32::from(state.base_handle.offset().y)))
    }

    fn row_height(&self) -> f32 {
        self.font_size * ROW_RATIO
    }

    /// Changes the editor font size (clamped), keeping the first visible row where it was
    /// at the last frame. A Ctrl+wheel zoom also reaches the list, which scrolls by it;
    /// anchoring on the last frame takes that back, even when the size no longer changes.
    pub fn set_font_size(&mut self, size: f32) {
        let top_row = self.settled_top / self.row_height();
        self.font_size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let state = self.scroll.0.borrow();
        let x = state.base_handle.offset().x;
        state
            .base_handle
            .set_offset(point(x, px(-top_row * self.row_height())));
        drop(state);
        self.anim = None;
        self.cell_width = None; // measured again with the new size on the next tick
    }

    /// Glides the view so `top` (in pixels, clamped to the document) is its first row.
    /// Without a layout there is nothing to glide on: `fallback` scrolls instead.
    fn glide_to(&mut self, top: impl FnOnce(f32) -> f32, fallback: impl FnOnce(&Self)) {
        let Some((viewport, current)) = self.view_pixels() else {
            return fallback(self);
        };
        let max = (self.rows.len() as f32 * self.row_height() - viewport).max(0.);
        self.anim = Some(ScrollAnim {
            from: current,
            to: top(viewport).clamp(0., max),
            start: Instant::now(),
        });
    }

    /// Advances the glide one frame; asks for another while it lasts.
    pub fn tick(&mut self, window: &mut Window) {
        self.settled_top = -f32::from(self.scroll.0.borrow().base_handle.offset().y);
        if self.cell_width.is_none() {
            let text_system = window.text_system();
            let size = px(self.font_size);
            let installed = text_system.all_font_names();
            if let Some(family) = EDITOR_FONTS
                .iter()
                .find(|family| installed.iter().any(|name| name == *family))
            {
                self.font_family = (*family).into();
            }
            let id = text_system.resolve_font(&font(self.font_family.clone()));
            self.cell_width = text_system
                .advance(id, size, 'm')
                .ok()
                .map(|advance| f32::from(advance.width));
        }
        let Some(anim) = &self.anim else { return };
        let t = (anim.start.elapsed().as_secs_f32() / SCROLL_DURATION.as_secs_f32()).min(1.);
        let eased = 1. - (1. - t).powi(3);
        let top = anim.from + (anim.to - anim.from) * eased;
        let state = self.scroll.0.borrow();
        let x = state.base_handle.offset().x;
        state.base_handle.set_offset(point(x, px(-top)));
        drop(state);
        if t >= 1. {
            self.anim = None;
        } else {
            window.request_animation_frame();
        }
    }

    /// `zt`, `zz`, `zb`: puts the cursor line at the top, middle or bottom of the view.
    fn place_cursor_line(&mut self, strategy: ScrollStrategy) {
        let line = self
            .rows
            .row_of(self.buffer.position_of(self.vim.cursor()).line);
        let row = self.row_height();
        let y = line as f32 * row;
        self.glide_to(
            |viewport| match strategy {
                ScrollStrategy::Center => y - (viewport - row) / 2.,
                ScrollStrategy::Bottom => y + row - viewport,
                _ => y,
            },
            |this| this.scroll.scroll_to_item_strict(line, strategy),
        );
    }

    /// `Ctrl-D` / `Ctrl-U`: the cursor and the view move half a page.
    fn scroll_half_page(&mut self, down: bool) {
        let (top, rows) = self.view_metrics().unwrap_or((0, 20));
        let half = (rows / 2).max(1);
        let row = self
            .rows
            .row_of(self.buffer.position_of(self.vim.cursor()).line);
        let last = self.rows.len().saturating_sub(1);
        let row = if down {
            (row + half).min(last)
        } else {
            row.saturating_sub(half)
        };
        self.vim.move_to_line(&self.buffer, self.rows.line(row));
        let shift = half as f32 * self.row_height();
        let top = top as f32 * self.row_height();
        self.glide_to(|_| if down { top + shift } else { top - shift }, |_| {});
    }

    /// True when the next key starts a Normal-mode command (nothing half typed).
    pub fn awaits_command(&self) -> bool {
        self.vim.mode() == Mode::Normal && self.vim.pending().is_empty() && !self.z_pending
    }

    /// What is typed so far of an unfinished command, for the status bar.
    fn pending_keys(&self) -> String {
        format!(
            "{}{}",
            if self.z_pending { "z" } else { "" },
            self.vim.pending()
        )
    }

    pub fn handle_key(&mut self, key: Key) -> Effect {
        self.anim = None; // a key press takes over from a glide
        if self.z_pending {
            self.z_pending = false;
            if let Key::Char(command @ ('a' | 'o' | 'c' | 'M' | 'R')) = key {
                self.fold_command(command);
                return Effect::None;
            }
            let strategy = match key {
                Key::Char('t') | Key::Enter => Some(ScrollStrategy::Top),
                Key::Char('z' | '.') => Some(ScrollStrategy::Center),
                Key::Char('b' | '-') => Some(ScrollStrategy::Bottom),
                _ => None, // Esc or anything else cancels
            };
            if let Some(strategy) = strategy {
                if matches!(key, Key::Enter | Key::Char('.' | '-')) {
                    self.vim.handle(&mut self.buffer, Key::Char('^'));
                }
                self.place_cursor_line(strategy);
            }
            return Effect::None;
        }
        let navigating = matches!(
            self.vim.mode(),
            Mode::Normal | Mode::Visual | Mode::VisualLine
        ) && self.vim.pending().is_empty();
        if navigating {
            match key {
                Key::Char('z') if self.vim.mode() == Mode::Normal => {
                    self.z_pending = true;
                    return Effect::None;
                }
                Key::Ctrl('d') => {
                    self.scroll_half_page(true);
                    return Effect::None;
                }
                Key::Ctrl('u') => {
                    self.scroll_half_page(false);
                    return Effect::None;
                }
                _ => {}
            }
        }
        if self.vim.mode() == Mode::Normal && self.vim.pending().is_empty() {
            match key {
                Key::Char('/') => return Effect::OpenFind,
                Key::Char('n') if self.find.is_some() => {
                    self.jump(true, false);
                    return Effect::None;
                }
                Key::Char('N') if self.find.is_some() => {
                    self.jump(false, false);
                    return Effect::None;
                }
                _ => {}
            }
        }
        if !self.editable
            && !matches!(
                key,
                Key::Char('h' | 'j' | 'k' | 'l' | 'w' | 'b' | 'e' | 'G' | 'g' | '0' | '$' | '^')
            )
        {
            return Effect::None; // an error message is not a document
        }
        let before = self.cursor_line();
        self.vim.handle(&mut self.buffer, key);
        if self.buffer.revision() != self.built_at {
            self.rebuild();
        }
        self.snap_cursor(before);
        self.reveal_cursor();
        Effect::None
    }

    /// Starts (or updates) the search. The cursor goes to the first match at or after it.
    pub fn set_find(&mut self, query: &FindQuery) {
        let (finder, error) = match query.compile() {
            Ok(finder) => (finder, None),
            Err(message) => (None, Some(message)),
        };
        self.find = Some(FindState {
            finder,
            error,
            matches: Arc::default(),
            built_at: None,
            visible: true,
        });
        self.refresh_matches();
        self.jump(true, true);
    }

    /// Hides the highlights; `n` and `N` still use the last search.
    pub fn hide_find(&mut self) {
        if let Some(find) = &mut self.find {
            find.visible = false;
        }
    }

    /// Whether the caret is at the start of a match of the current search.
    pub fn cursor_on_match(&mut self) -> bool {
        self.refresh_matches();
        self.match_at_cursor().is_some()
    }

    /// What F3 searches when nothing is being searched at the caret: the selected word, or the
    /// identifier under the caret.
    pub fn word_to_search(&self) -> Option<String> {
        self.selected_text().or_else(|| self.word_at_cursor())
    }

    /// Searches `word` as a whole word, case-sensitively, and moves to its next (or previous)
    /// occurrence. The highlights stay hidden; `n`, `N` and F3 go on with it.
    pub fn search_word(&mut self, word: &str, forward: bool) {
        // start from the word itself, wherever in it the caret is, so "next" means the one after
        let start = match self.vim.selection(&self.buffer) {
            Some((range, _)) => Some(range.start),
            None => self.word_range_at_cursor().map(|range| range.start),
        };
        if let Some(start) = start {
            if self.vim.mode() != Mode::Normal {
                self.vim.handle(&mut self.buffer, Key::Esc);
            }
            self.vim.set_cursor(&self.buffer, start);
        }
        self.set_find(&FindQuery {
            pattern: word.to_string(),
            case_sensitive: true,
            whole_word: true,
            regex: false,
        });
        self.hide_find();
        self.jump(forward, false);
    }

    pub fn has_find(&self) -> bool {
        self.find.is_some()
    }

    pub fn find_status(&self) -> Option<FindStatus> {
        let find = self.find.as_ref()?;
        Some(FindStatus {
            total: find.matches.len(),
            current: self.match_at_cursor().map(|i| i + 1),
            error: find.error.clone(),
        })
    }

    /// The text selected in Visual mode, when it is a single line: what the panel starts with.
    /// The selection in chars: the Visual one, or the empty range at the caret.
    fn current_selection(&self) -> Range<usize> {
        self.vim
            .selection(&self.buffer)
            .map_or(self.vim.cursor()..self.vim.cursor(), |(range, _)| range)
    }

    /// Ctrl+W: selects the word at the caret, then each enclosing construct, up to the file.
    pub fn extend_selection(&mut self) {
        let current = self.current_selection();
        let text = self.buffer.text();
        let bytes = self.buffer.byte_of(current.start)..self.buffer.byte_of(current.end);
        let Some(next) = zide_syntax::extend_selection(self.language, &text, bytes) else {
            return;
        };
        let next = self.buffer.offset_of_byte(next.start)..self.buffer.offset_of_byte(next.end);
        let revision = self.buffer.revision();
        let mut trail = self
            .trail
            .take()
            .filter(|t| t.revision == revision && t.current == current)
            .unwrap_or(SelectionTrail {
                revision,
                current: current.clone(),
                history: Vec::new(),
            });
        let before = self.vim.selection(&self.buffer).map(|_| current);
        trail.history.push((before, self.vim.cursor()));
        trail.current = next.clone();
        self.trail = Some(trail);
        self.vim.select(&self.buffer, next);
    }

    /// Ctrl+Shift+W: undoes the last Extend Selection.
    pub fn shrink_selection(&mut self) {
        let current = self.current_selection();
        let Some(mut trail) = self
            .trail
            .take()
            .filter(|t| t.revision == self.buffer.revision() && t.current == current)
        else {
            return;
        };
        let Some((before, cursor)) = trail.history.pop() else {
            return;
        };
        match before {
            Some(range) => {
                trail.current = range.clone();
                self.vim.select(&self.buffer, range);
                self.trail = Some(trail).filter(|t| !t.history.is_empty());
            }
            None => {
                self.vim.handle(&mut self.buffer, Key::Esc);
                self.vim.set_cursor(&self.buffer, cursor);
            }
        }
    }

    /// The cursor as a language server wants it: 0-based line and UTF-16 column.
    pub fn lsp_position(&self) -> (u32, u32) {
        let position = self.lsp_position_at(self.vim.cursor());
        (position.line, position.character)
    }

    fn lsp_position_at(&self, offset: usize) -> zide_lsp::Position {
        let position = self.buffer.position_of(offset);
        let line = self.buffer.line(position.line);
        zide_lsp::Position::new(
            position.line as u32,
            zide_lsp::utf16_column(&line, position.column),
        )
    }

    /// What to ask a formatter: the Visual selection, or the whole document when nothing is
    /// selected (IntelliJ's Reformat Code). `None` when the file cannot be edited.
    pub fn format_request(&self) -> Option<FormatRequest> {
        if !self.editable {
            return None;
        }
        let range = self.vim.selection(&self.buffer).map(|(range, _)| {
            zide_lsp::Range::new(
                self.lsp_position_at(range.start),
                self.lsp_position_at(range.end),
            )
        });
        Some(FormatRequest {
            revision: self.buffer.revision(),
            text: self.buffer.text(),
            range,
        })
    }

    /// Applies the edits a formatter answered for the text of `revision` as one undo step, and
    /// leaves Visual mode. Returns false (touching nothing) when the text changed meanwhile.
    pub fn apply_format(&mut self, revision: Revision, edits: Vec<zide_lsp::TextEdit>) -> bool {
        if self.buffer.revision() != revision {
            return false;
        }
        let cursor = self.buffer.position_of(self.vim.cursor());
        let mut edits: Vec<(usize, Range<usize>, String)> = edits
            .into_iter()
            .enumerate()
            .map(|(order, edit)| {
                let start = self.offset_of_lsp(edit.range.start);
                let end = self.offset_of_lsp(edit.range.end).max(start);
                (order, start..end, edit.new_text)
            })
            .collect();
        // from the end, so earlier offsets stay valid; inserts at one place keep their order
        edits.sort_by_key(|(order, range, _)| std::cmp::Reverse((range.start, *order)));
        if edits
            .iter()
            .all(|(_, range, text)| self.buffer.slice(range.clone()) == *text)
        {
            return true; // already formatted
        }
        if matches!(self.vim.mode(), Mode::Visual | Mode::VisualLine) {
            self.vim.handle(&mut self.buffer, Key::Esc);
        }
        for (_, range, text) in &edits {
            self.buffer.edit(range.clone(), text);
        }
        self.buffer.commit();
        self.rebuild();
        let line = cursor.line.min(self.buffer.line_count().saturating_sub(1));
        let column = cursor.column.min(self.buffer.line_len(line));
        let offset = self.buffer.offset_of(Position { line, column });
        self.vim.set_cursor(&self.buffer, offset);
        true
    }

    fn offset_of_lsp(&self, position: zide_lsp::Position) -> usize {
        let line = position.line as usize;
        if line >= self.buffer.line_count() {
            return self.buffer.len();
        }
        let column = zide_lsp::char_column(&self.buffer.line(line), position.character);
        self.buffer.offset_of(Position {
            line,
            column: column.min(self.buffer.line_len(line)),
        })
    }

    /// The whole text, as of now (saved or not).
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// The identifier under the cursor, if it is on one.
    pub fn word_at_cursor(&self) -> Option<String> {
        self.word_range_at_cursor()
            .map(|range| self.buffer.slice(range))
    }

    /// Where the identifier under the cursor is, in chars.
    fn word_range_at_cursor(&self) -> Option<Range<usize>> {
        let position = self.buffer.position_of(self.vim.cursor());
        let line: Vec<char> = self.buffer.line(position.line).chars().collect();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if !line.get(position.column).copied().is_some_and(is_word) {
            return None;
        }
        let start = line[..position.column]
            .iter()
            .rposition(|&c| !is_word(c))
            .map_or(0, |i| i + 1);
        let end = line[position.column..]
            .iter()
            .position(|&c| !is_word(c))
            .map_or(line.len(), |i| position.column + i);
        let first = self.buffer.line_start(position.line);
        Some(first + start..first + end)
    }

    /// Whether usage highlighting applies now: not while selecting or searching, when it would
    /// only add noise.
    fn highlights_apply(&self) -> bool {
        let searching = self.find.as_ref().is_some_and(|find| find.visible);
        !searching && self.vim.selection(&self.buffer).is_none()
    }

    /// What a language server is asked to highlight, when the caret is on a word.
    pub fn highlight_target(&self) -> Option<HighlightTarget> {
        if !self.highlights_apply() {
            return None;
        }
        let word = self.word_range_at_cursor()?;
        let at = self.lsp_position_at(word.start);
        Some(HighlightTarget {
            key: (self.buffer.revision(), word.start),
            line: at.line,
            character: at.character,
        })
    }

    /// Stores what a language server answered for `key`; ignored when the text moved on.
    pub fn set_semantic(&mut self, key: HighlightKey, ranges: Vec<zide_lsp::Range>) {
        if self.buffer.revision() != key.0 {
            return;
        }
        let mut found: Vec<Range<usize>> = ranges
            .into_iter()
            .map(|r| self.offset_of_lsp(r.start)..self.offset_of_lsp(r.end))
            .collect();
        found.sort_by_key(|r| r.start);
        self.semantic = Some((key, Arc::new(found)));
    }

    pub fn selected_text(&self) -> Option<String> {
        let (range, linewise) = self.vim.selection(&self.buffer)?;
        let text = self.buffer.slice(range);
        (!linewise && !text.is_empty() && !text.contains('\n')).then_some(text)
    }

    /// Moves to the next (or previous) match, wrapping around. `inclusive` accepts one that
    /// starts at the cursor.
    pub fn jump(&mut self, forward: bool, inclusive: bool) {
        self.refresh_matches();
        let Some(find) = &self.find else {
            return;
        };
        let matches = &find.matches;
        if matches.is_empty() {
            return;
        }
        let cursor = self.vim.cursor();
        let index = if forward {
            let next = matches.partition_point(|m| {
                if inclusive {
                    m.start < cursor
                } else {
                    m.start <= cursor
                }
            });
            if next == matches.len() { 0 } else { next }
        } else {
            matches
                .partition_point(|m| m.start < cursor)
                .checked_sub(1)
                .unwrap_or(matches.len() - 1)
        };
        let start = matches[index].start;
        self.vim.set_cursor(&self.buffer, start);
        self.reveal_cursor();
    }

    fn match_at_cursor(&self) -> Option<usize> {
        let find = self.find.as_ref()?;
        let cursor = self.vim.cursor();
        find.matches.binary_search_by_key(&cursor, |m| m.start).ok()
    }

    /// Recomputes the matches when the text changed since they were found.
    fn refresh_matches(&mut self) {
        let revision = self.buffer.revision();
        let Some(find) = &mut self.find else {
            return;
        };
        if find.built_at == Some(revision) {
            return;
        }
        let text = self.buffer.text();
        let matches = find.finder.as_ref().map_or_else(Vec::new, |finder| {
            finder
                .find_all(&text)
                .into_iter()
                .map(|r| self.buffer.offset_of_byte(r.start)..self.buffer.offset_of_byte(r.end))
                .collect()
        });
        find.matches = Arc::new(matches);
        find.built_at = Some(revision);
    }

    /// Replaces the match the cursor is on and moves to the next one; with the cursor
    /// elsewhere it only moves to the next match.
    pub fn replace_current(&mut self, template: &str) {
        if !self.editable {
            return;
        }
        self.refresh_matches();
        let Some(index) = self.match_at_cursor() else {
            return self.jump(true, true);
        };
        let Some(find) = &self.find else {
            return;
        };
        let range = find.matches[index].clone();
        let Some(finder) = &find.finder else {
            return;
        };
        let text = self.buffer.text();
        let bytes = self.buffer.byte_of(range.start)..self.buffer.byte_of(range.end);
        let replacement = finder.replacement(&text, bytes, template);
        let end = range.start + replacement.chars().count();
        self.buffer.edit(range, &replacement);
        self.buffer.commit();
        self.rebuild();
        self.vim.set_cursor(&self.buffer, end);
        self.jump(true, true);
    }

    /// Replaces every match; one undo step. Returns how many there were.
    pub fn replace_all(&mut self, template: &str) -> usize {
        if !self.editable {
            return 0;
        }
        self.refresh_matches();
        let Some(find) = &self.find else {
            return 0;
        };
        let Some(finder) = &find.finder else {
            return 0;
        };
        let text = self.buffer.text();
        let edits: Vec<_> = find
            .matches
            .iter()
            .map(|range| {
                let bytes = self.buffer.byte_of(range.start)..self.buffer.byte_of(range.end);
                (range.clone(), finder.replacement(&text, bytes, template))
            })
            .collect();
        // from the end, so earlier offsets stay valid
        for (range, replacement) in edits.iter().rev() {
            self.buffer.edit(range.clone(), replacement);
        }
        self.buffer.commit();
        self.rebuild();
        self.refresh_matches();
        let cursor = self.vim.cursor();
        self.vim.set_cursor(&self.buffer, cursor);
        edits.len()
    }

    /// Alt+Down / Alt+Up: moves the cursor to the name of the next (or previous) function,
    /// method, class or interface, wherever the cursor is. With none left it goes to the end
    /// (or the start) of the file.
    pub fn next_method(&mut self, forward: bool) {
        let from = self.buffer.byte_of(self.vim.cursor());
        let offset = match declaration_beyond(&self.outline, from, forward) {
            Some(byte) => self.buffer.offset_of_byte(byte),
            None if forward => self.buffer.len(),
            None => 0,
        };
        self.vim.set_cursor(&self.buffer, offset);
        self.reveal_cursor();
    }

    /// Scrolls the least that brings the cursor's row into view.
    fn reveal_cursor(&mut self) {
        self.unfold_hiding(self.cursor_line());
        let row = self.rows.row_of(self.cursor_line());
        // a cursor under the pinned declarations is out of sight: scroll until it is right
        // below them
        if let Some((top, _)) = self.view_metrics()
            && row < top + self.pinned_at(top).len()
        {
            let mut first = row;
            for _ in 0..MAX_STICKY {
                first = row.saturating_sub(self.pinned_at(first).len());
            }
            self.scroll.scroll_to_item(first, ScrollStrategy::Top);
            return;
        }
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
    }

    /// The header lines pinned above the code when `top` is the first row on screen.
    fn pinned_at(&self, top: usize) -> Vec<usize> {
        sticky_headers(&self.scopes, |row| self.rows.line(row), top)
    }

    fn cursor_line(&self) -> usize {
        self.buffer.position_of(self.vim.cursor()).line
    }

    /// The cursor never rests on a hidden line: one that a movement left on one goes to the
    /// line after the fold when it moved down (`from` is where it was) and to the fold's
    /// first line when it moved up.
    fn snap_cursor(&mut self, from: usize) {
        let line = self.cursor_line();
        if self.rows.shows(line) {
            return;
        }
        let header = self.rows.row_of(line);
        let row = if line > from {
            (header + 1).min(self.rows.len() - 1)
        } else {
            header
        };
        self.vim.move_to_line(&self.buffer, self.rows.line(row));
    }

    /// Rebuilds the rows from the regions that are folded.
    fn rebuild_rows(&mut self) {
        let mut hidden: Vec<Range<usize>> = Vec::new();
        for region in self
            .folds
            .iter()
            .filter(|r| self.collapsed.contains(&r.start_line))
        {
            let range = region.start_line + 1..region.end_line + 1;
            match hidden.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => hidden.push(range),
            }
        }
        self.rows = Arc::new(Rows::hiding(self.lines.len(), &hidden));
    }

    /// Opens every folded region that hides `line`.
    fn unfold_hiding(&mut self, line: usize) {
        let before = self.collapsed.len();
        for region in &self.folds {
            if region.start_line < line && line <= region.end_line {
                self.collapsed.remove(&region.start_line);
            }
        }
        if self.collapsed.len() != before {
            self.rebuild_rows();
        }
    }

    /// Folds the region that starts on `line`, or opens it when it is folded (a click on its
    /// chevron). The cursor goes to the fold's first line if it was inside.
    pub fn toggle_fold(&mut self, line: usize) {
        if !self.collapsed.remove(&line) && self.folds.iter().any(|r| r.start_line == line) {
            self.collapsed.insert(line);
        }
        self.rebuild_rows();
        self.snap_cursor(usize::MAX);
    }

    /// `za`, `zo`, `zc`, `zM`, `zR`.
    pub fn fold_command(&mut self, command: char) {
        let line = self.cursor_line();
        let folded = self.collapsed.contains(&line);
        match command {
            'o' | 'a' if folded => {
                self.collapsed.remove(&line);
            }
            'c' | 'a' => {
                // the innermost region around the cursor that is still open
                let region = self
                    .folds
                    .iter()
                    .filter(|r| r.start_line <= line && line <= r.end_line)
                    .filter(|r| !self.collapsed.contains(&r.start_line))
                    .max_by_key(|r| r.start_line);
                if let Some(region) = region {
                    self.collapsed.insert(region.start_line);
                }
            }
            'M' => self.collapsed = self.folds.iter().map(|r| r.start_line).collect(),
            'R' => self.collapsed.clear(),
            _ => {}
        }
        self.rebuild_rows();
        self.snap_cursor(usize::MAX); // inside a new fold: on its first line
        self.reveal_cursor();
    }

    /// Re-highlights the whole text. Fine for ordinary files; large ones will need a debounce.
    fn rebuild(&mut self) {
        // one tree for the highlight, the folds and the outline, reparsed only where edited
        let edits = self.buffer.take_edits();
        let text = self.buffer.text();
        match (&mut self.syntax, self.language) {
            (Some(syntax), _) => syntax.update(&text, &edits),
            (None, Some(language)) => self.syntax = Syntax::new(language, &text),
            (None, None) => {}
        }
        let spans = self
            .syntax
            .as_ref()
            .map(|syntax| syntax.highlight(&text))
            .unwrap_or_default();
        let hints = self.hints_by_line();
        self.lines = Arc::new(build_lines(&self.buffer, &spans, &hints));
        self.folds = match &self.syntax {
            Some(syntax) if self.editable => syntax.fold_regions(&text),
            _ => Vec::new(),
        };
        let starts: BTreeSet<usize> = self.folds.iter().map(|r| r.start_line).collect();
        self.collapsed.retain(|line| starts.contains(line));
        self.rebuild_rows();
        self.refresh_changes();
        self.refresh_squiggles();
        self.outline = match (stub_language(&self.path), &self.syntax) {
            (Some(language), Some(syntax)) if self.editable => {
                extract_nodes_in(language, syntax.tree(), &text)
            }
            (Some(language), None) if self.editable => extract_nodes(language, &text),
            _ => Vec::new(),
        };
        let line_of = |byte: u32| {
            let offset = self.buffer.offset_of_byte(byte as usize);
            self.buffer.position_of(offset).line
        };
        self.scopes = self
            .outline
            .iter()
            .filter(|n| {
                !matches!(
                    n.kind,
                    StubKind::Import
                        | StubKind::Field
                        | StubKind::Variant
                        | StubKind::Const
                        | StubKind::TypeAlias
                        | StubKind::Macro
                )
            })
            .map(|n| Scope {
                start: line_of(n.range.start),
                end: line_of(n.range.end.saturating_sub(1)),
            })
            .filter(|s| s.end > s.start)
            .collect();
        self.built_at = self.buffer.revision();
    }

    /// The cursor line and, in display bytes of that line, the cell the cursor covers.
    fn cursor_cell(&self) -> (usize, Range<usize>) {
        let position = self.buffer.position_of(self.vim.cursor());
        let raw = self.buffer.line(position.line);
        let (_, map) = expand_tabs(&raw);
        let byte = raw
            .char_indices()
            .nth(position.column)
            .map_or(raw.len(), |(b, _)| b);
        let end = raw[byte..]
            .chars()
            .next()
            .map_or(byte, |c| byte + c.len_utf8());
        let start = self
            .lines
            .get(position.line)
            .map_or(map[byte], |line| line.with_hints(map[byte]));
        (position.line, start..start + (map[end] - map[byte]).max(1))
    }

    /// The declarations around the cursor, outermost first.
    fn enclosing_symbols(&self) -> Vec<&StubNode> {
        enclosing(&self.outline, self.buffer.byte_of(self.vim.cursor()))
    }

    /// Project folder, directories, file and the symbols around the cursor.
    fn breadcrumb(&self) -> Div {
        let separator = || div().text_color(rgb(theme::DIM)).child("›");
        let mut parts: Vec<&str> = vec![self.root_name.as_str()];
        parts.extend(self.path.split('/'));
        let file = parts.pop().unwrap_or_default();
        let mut path = div()
            .flex()
            .gap_1p5()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap();
        for part in parts.into_iter().filter(|p| !p.is_empty()) {
            path = path.child(part.to_string()).child(separator());
        }
        let path = path.child(div().text_color(rgb(theme::STRONG)).child(file.to_string()));
        let mut symbols = div().flex().gap_1p5().flex_shrink_0().whitespace_nowrap();
        for symbol in self.enclosing_symbols() {
            let name = symbol.name.as_deref().unwrap_or("");
            let callable = matches!(symbol.kind, StubKind::Function | StubKind::Method);
            symbols = symbols.child(separator()).child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(theme::SYMBOL))
                            .child(symbol_glyph(symbol.kind)),
                    )
                    .child(if callable {
                        format!("{name}()")
                    } else {
                        name.to_string()
                    }),
            );
        }
        div()
            .flex()
            .gap_1p5()
            .px_3()
            .py_1()
            .text_color(rgb(theme::MUTED))
            .child(path)
            .child(symbols)
    }

    /// `on_fold` is called with the start line of the region whose chevron was clicked.
    pub fn render(&self, on_fold: FoldClick, on_hover: HoverMove, on_click: TextClick) -> Div {
        let lines = self.lines.clone();
        let rows = self.rows.clone();
        let folded: BTreeSet<usize> = self
            .collapsed
            .iter()
            .copied()
            .filter(|line| rows.shows(*line))
            .collect();
        let starts: BTreeSet<usize> = self.folds.iter().map(|r| r.start_line).collect();
        let digits = lines.len().to_string().len();
        let (cursor_line, cursor_cell) = self.cursor_cell();
        let blame = self.blame_label();
        let changes = self.changes.clone();
        let squiggles = self.squiggles.clone();
        let editor_bounds = self.editor_bounds.clone();
        let leave = on_hover.clone();
        let popup = self.hover_popup();
        let insert = self.vim.mode() == Mode::Insert;
        let selection = self
            .vim
            .selection(&self.buffer)
            .map(|(range, linewise)| Selection {
                start: self.buffer.position_of(range.start),
                end: self.buffer.position_of(range.end),
                linewise,
            });
        let matches = self
            .find
            .as_ref()
            .filter(|find| find.visible)
            .map(|find| find.matches.clone());
        let current_match = self
            .match_at_cursor()
            .and_then(|i| matches.as_ref().map(|m| m[i].start));
        let word_matches = self.word_highlights();
        let pair = self.pair_highlights();
        let cell_width = self.cell_width.unwrap_or(8.);
        let row_height = self.row_height();
        let scale = self.font_size / DEFAULT_FONT_SIZE;
        let pinned: Vec<_> = self
            .pinned_at((self.settled_top / row_height) as usize)
            .into_iter()
            .filter_map(|line| Some((line, self.lines.get(line)?)))
            .collect();
        let sticky = (!pinned.is_empty()).then(|| {
            let gutter = (12. * digits as f32 + 28.) * scale;
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .bg(rgb(theme::BACKGROUND))
                .border_b_1()
                .border_color(rgb(theme::BORDER))
                .children(pinned.into_iter().map(|(line, shown)| {
                    div()
                        .flex()
                        .h(px(row_height))
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(gutter))
                                .pr_3()
                                .text_right()
                                .text_color(rgb(theme::DIM))
                                .child((line + 1).to_string()),
                        )
                        .child(
                            StyledText::new(shown.text.clone())
                                .with_highlights(shown.colors.clone()),
                        )
                }))
        });
        let list = uniform_list("viewer", rows.len(), move |range, _, _| {
            range
                .map(|row| {
                    let row = rows.line(row);
                    let line = &lines[row];
                    let text = line.text.clone();
                    let mut colors = line.colors.clone();
                    let first = word_matches.partition_point(|m| m.end <= line.start);
                    for found in &word_matches[first..] {
                        if found.start >= line.start + line.chars {
                            break;
                        }
                        let from = found.start - line.start;
                        let to = found.end - line.start;
                        let range = line.display_byte(from)..line.display_byte(to);
                        colors = paint(&colors, range, rgb(theme::WORD).into());
                    }
                    for (_, range) in squiggles.get(&row).into_iter().flatten() {
                        colors = restyle(&colors, range.clone(), |style| HighlightStyle {
                            underline: Some(UnderlineStyle {
                                color: Some(rgb(theme::ERROR).into()),
                                thickness: px(1.),
                                wavy: true,
                            }),
                            ..style
                        });
                    }
                    for end in pair.iter() {
                        if end.start >= line.start && end.start < line.start + line.chars {
                            let from = end.start - line.start;
                            let to = end.end.min(line.start + line.chars) - line.start;
                            let range = line.display_byte(from)..line.display_byte(to);
                            colors = paint(&colors, range, rgb(theme::BRACKET).into());
                        }
                    }
                    if let Some(matches) = &matches {
                        let first = matches.partition_point(|m| m.end <= line.start);
                        for found in &matches[first..] {
                            if found.start >= line.start + line.chars {
                                break;
                            }
                            let from = found.start.max(line.start) - line.start;
                            let to = found.end.min(line.start + line.chars) - line.start;
                            let color = if Some(found.start) == current_match {
                                theme::FIND_CURRENT
                            } else {
                                theme::FIND_MATCH
                            };
                            let range = line.display_byte(from)..line.display_byte(to);
                            colors = paint(&colors, range, rgb(color).into());
                        }
                    }
                    if let Some(range) = selection.and_then(|s| s.on_line(row, line)) {
                        colors = paint(&colors, range, rgb(theme::SELECTION).into());
                    }
                    // The caret is drawn on the cell grid, so it is as wide on a space or
                    // a thin letter as on any other character.
                    let gutter = (12. * digits as f32 + 28.) * scale;
                    let caret = (row == cursor_line).then(|| {
                        let column = text[..cursor_cell.start.min(text.len())].chars().count();
                        let left = px(gutter + column as f32 * cell_width);
                        let caret = div().absolute().top_0().left(left).h(px(row_height));
                        if insert {
                            caret.w(px(2.)).bg(rgb(theme::CARET_BAR)) // a bar, like a terminal's
                        } else {
                            caret.w(px(cell_width)).bg(rgba(0xff7a0099)) // a block you can see
                        }
                    });
                    let bands = (0..line.level).map(|i| {
                        let left = gutter + (i * line.step) as f32 * cell_width;
                        div()
                            .absolute()
                            .top_0()
                            .left(px(left))
                            .w(px(line.step as f32 * cell_width))
                            .h(px(row_height))
                            .bg(rgba(theme::INDENT_BANDS[i % theme::INDENT_BANDS.len()]))
                    });
                    let dots = indent::dots(&line.text).into_iter().map(|column| {
                        let left = gutter + (column as f32 + 0.5) * cell_width - 1.;
                        div()
                            .absolute()
                            .top(px(row_height / 2. - 1.))
                            .left(px(left))
                            .w(px(2.))
                            .h(px(2.))
                            .rounded_full()
                            .bg(rgba(theme::WHITESPACE))
                    });
                    let hover = on_hover.clone();
                    let click = on_click.clone();
                    div()
                        .relative()
                        .flex()
                        .h(px(row_height))
                        .cursor_text()
                        .on_mouse_move(move |event, _, cx| hover(Some((row, event.position)), cx))
                        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                            click(row, event.position, cx)
                        })
                        .children(bands)
                        .child(
                            div()
                                .relative()
                                .cursor_default()
                                .flex_shrink_0()
                                .w(px(gutter))
                                .pr_3()
                                .text_right()
                                .text_color(rgb(theme::DIM))
                                .child((row + 1).to_string())
                                .children(
                                    changes.get(&row).map(|change| {
                                        change_mark(*change, gutter, scale, row_height)
                                    }),
                                )
                                .children(starts.contains(&row).then(|| {
                                    let on_fold = on_fold.clone();
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left(px(2. * scale))
                                        .w(px(14. * scale))
                                        .h(px(row_height))
                                        .text_center()
                                        .cursor_pointer()
                                        .child(if folded.contains(&row) { "▸" } else { "▾" })
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            cx.stop_propagation(); // not a click on the text
                                            on_fold(row, cx)
                                        })
                                })),
                        )
                        .child(StyledText::new(text).with_highlights(colors))
                        .children(blame.clone().filter(|_| row == cursor_line).map(|label| {
                            div()
                                .flex_shrink_0()
                                .ml_6()
                                .whitespace_nowrap()
                                .italic()
                                .text_color(rgb(theme::DIM))
                                .child(label)
                        }))
                        .children(folded.contains(&row).then(|| {
                            div()
                                .px_1()
                                .mx_1()
                                .rounded_sm()
                                .bg(rgb(theme::BORDER))
                                .text_color(rgb(theme::DIM))
                                .child("...")
                        }))
                        .children(dots)
                        .children(caret)
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(rgb(theme::BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .child(self.breadcrumb())
            .child(
                div()
                    .id("editor")
                    .cursor_text()
                    .flex_1()
                    .relative()
                    .font_family(self.font_family.clone())
                    .text_size(px(self.font_size))
                    .whitespace_nowrap()
                    .on_hover(move |hovered, _, cx| {
                        if !*hovered {
                            leave(None, cx)
                        }
                    })
                    .child(
                        canvas(
                            move |bounds, _, _| editor_bounds.set(bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(list)
                    .children(sticky)
                    .children(popup),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .gap_3()
                    .text_color(rgb(theme::MUTED))
                    .child(match self.vim.mode() {
                        Mode::Normal => "NORMAL",
                        Mode::Insert => "INSERT",
                        Mode::Visual => "VISUAL",
                        Mode::VisualLine => "VISUAL LINE",
                    })
                    .child(self.pending_keys())
                    .child(if self.is_dirty() { "[+]" } else { "" })
                    .child(self.notice.clone().unwrap_or_default())
                    .child(div().ml_auto().child({
                        let at = self.buffer.position_of(self.vim.cursor());
                        format!("{}:{}", at.line + 1, at.column + 1)
                    })),
            )
    }
}

/// `colors` with `bg` as the background of `range`; the rest is unchanged.
fn paint(
    colors: &[(Range<usize>, HighlightStyle)],
    range: Range<usize>,
    bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    restyle(colors, range, |style| HighlightStyle {
        background_color: Some(bg),
        ..style
    })
}

/// `colors` with `with` applied to the style of every part of `range`, gaps included; the rest
/// is unchanged.
fn restyle(
    colors: &[(Range<usize>, HighlightStyle)],
    range: Range<usize>,
    with: impl Fn(HighlightStyle) -> HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if range.is_empty() {
        return colors.to_vec();
    }
    let with_bg = with;
    let mut out = Vec::with_capacity(colors.len() + 3);
    let mut painted_to = range.start;
    for (span, style) in colors {
        if span.end <= range.start || span.start >= range.end {
            out.push((span.clone(), *style));
            continue;
        }
        if span.start < range.start {
            out.push((span.start..range.start, *style));
        }
        let middle = span.start.max(range.start)..span.end.min(range.end);
        if middle.start > painted_to {
            out.push((painted_to..middle.start, with_bg(HighlightStyle::default())));
        }
        painted_to = middle.end;
        out.push((middle, with_bg(*style)));
        if span.end > range.end {
            out.push((range.end..span.end, *style));
        }
    }
    if painted_to < range.end {
        out.push((painted_to..range.end, with_bg(HighlightStyle::default())));
    }
    out.sort_by_key(|(span, _)| span.start);
    out
}

/// A line with hints in it: its text, colors, byte of each character and where hints are.
struct Injected {
    text: String,
    colors: Vec<(Range<usize>, HighlightStyle)>,
    cols: Vec<usize>,
    inserts: Vec<(usize, usize)>,
}

/// The line `text` with each hint put before the character at its column, with the same
/// treatment of everything that is measured in display bytes: the `colors` are split around the
/// hints and moved, and the byte of every character (`cols`) is where it ends up. Returns
/// those and where hints were put.
fn inject_hints(
    raw: &str,
    map: &[usize],
    text: String,
    colors: Vec<(Range<usize>, HighlightStyle)>,
    hints: &[(usize, String)],
) -> Injected {
    // the name, then a space that is not part of the chip
    let inserts: Vec<(usize, String)> = hints
        .iter()
        .map(|(column, label)| {
            let byte = raw
                .char_indices()
                .nth(*column)
                .map_or(raw.len(), |(b, _)| b);
            (map[byte], format!("{label} "))
        })
        .collect();
    let total = |below: &dyn Fn(usize) -> bool| -> usize {
        inserts
            .iter()
            .filter(|(at, _)| below(*at))
            .map(|(_, shown)| shown.len())
            .sum()
    };
    // a character at a hint is after it, an end at a hint is before it
    let start_of = |at: usize| at + total(&|hint| hint <= at);
    let end_of = |at: usize| at + total(&|hint| hint < at);

    let mut shown = String::with_capacity(text.len() + total(&|_| true));
    let mut copied = 0;
    for (at, hint) in &inserts {
        shown.push_str(&text[copied..*at]);
        shown.push_str(hint);
        copied = *at;
    }
    shown.push_str(&text[copied..]);

    let mut moved = Vec::with_capacity(colors.len() + inserts.len());
    for (span, style) in colors {
        let mut from = span.start;
        let inside: BTreeSet<usize> = inserts
            .iter()
            .map(|(at, _)| *at)
            .filter(|at| *at > span.start && *at < span.end)
            .collect();
        for at in inside {
            moved.push((start_of(from)..end_of(at), style));
            from = at;
        }
        if from < span.end {
            moved.push((start_of(from)..end_of(span.end), style));
        }
    }
    let chip = HighlightStyle {
        color: Some(rgb(theme::HINT_TEXT).into()),
        background_color: Some(rgb(theme::HINT_BG).into()),
        ..Default::default()
    };
    let mut put = 0;
    for (at, hint) in &inserts {
        let start = at + put;
        moved.push((start..start + hint.len() - 1, chip)); // not the space after it
        put += hint.len();
    }
    moved.sort_by_key(|(span, _)| span.start);

    let cols = raw
        .char_indices()
        .map(|(byte, _)| start_of(map[byte]))
        .chain(std::iter::once(start_of(map[raw.len()])))
        .collect();
    let sizes = inserts.iter().map(|(at, hint)| (*at, hint.len())).collect();
    Injected {
        text: shown,
        colors: moved,
        cols,
        inserts: sizes,
    }
}

/// Where char offsets of `old` are in `new`, taking the difference as one edit: what lies
/// between their common start and common end. Offsets inside that are dropped.
fn follow_edit(old: &str, new: &str, hints: &[(usize, String)]) -> Vec<(usize, String)> {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - start;
    let end = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let old_end = old.len() - end;
    hints
        .iter()
        .filter_map(|(at, label)| {
            let moved = if *at <= start {
                *at
            } else if *at >= old_end {
                *at + new.len() - old.len()
            } else {
                return None;
            };
            Some((moved, label.clone()))
        })
        .collect()
}

/// What the popup says first: `TS2554: Expected 8 arguments, but got 7`.
fn headline(diagnostic: &Diagnostic) -> String {
    let code = match (&diagnostic.code, diagnostic.source.as_deref()) {
        (Some(NumberOrString::Number(n)), Some("ts")) => Some(format!("TS{n}")),
        (Some(NumberOrString::Number(n)), _) => Some(n.to_string()),
        (Some(NumberOrString::String(code)), _) => Some(code.clone()),
        (None, _) => None,
    };
    match code {
        Some(code) => format!("{code}: {}", diagnostic.message),
        None => diagnostic.message.clone(),
    }
}

/// The places a diagnostic points at besides its own: `card.ts(38, 5): An argument for
/// maxCopies was not provided.`
fn related_lines(diagnostic: &Diagnostic) -> Vec<String> {
    diagnostic
        .related_information
        .iter()
        .flatten()
        .map(|related| {
            let file = uri_to_path(related.location.uri.as_str())
                .and_then(|path| Some(path.file_name()?.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let at = related.location.range.start;
            format!(
                "{file}({}, {}): {}",
                at.line + 1,
                at.character + 1,
                related.message
            )
        })
        .collect()
}

fn style(class: Class) -> HighlightStyle {
    let color = match class {
        Class::Keyword => 0xc678dd,
        Class::String => 0x98c379,
        Class::Number | Class::Constant => 0xd19a66,
        Class::Comment => 0x7f848e,
        Class::Function => 0x61afef,
        Class::Type | Class::Tag => 0xe5c07b,
        Class::Property | Class::Attribute => 0xd19a66,
        Class::Variable => 0xe06c75,
        Class::Operator | Class::Punctuation => 0xabb2bf,
    };
    HighlightStyle {
        color: Some(rgb(color).into()),
        ..Default::default()
    }
}

/// Splits the text into display lines (tabs expanded) with their colors, in line coordinates.
fn build_lines(buffer: &Buffer, spans: &[Span], hints: &LineHints) -> Vec<Line> {
    let mut next_span = 0;
    let mut lines: Vec<Line> = (0..buffer.line_count())
        .map(|index| {
            let raw = buffer.line(index);
            let start = buffer.byte_of(buffer.line_start(index));
            let end = start + raw.len();
            while next_span < spans.len() && spans[next_span].range.end <= start {
                next_span += 1;
            }
            let (text, map) = expand_tabs(&raw);
            let mut colors = Vec::new();
            for span in &spans[next_span..] {
                if span.range.start >= end {
                    break;
                }
                let from = span.range.start.max(start) - start;
                let to = span.range.end.min(end) - start;
                colors.push((map[from]..map[to], style(span.class)));
            }
            let cols =
                (text.len() != raw.len() || !raw.is_ascii() || raw.contains('\t')).then(|| {
                    raw.char_indices()
                        .map(|(byte, _)| map[byte])
                        .chain(std::iter::once(map[raw.len()]))
                        .collect()
                });
            let (text, colors, cols, inserts) = match hints.get(&index) {
                Some(hints) => {
                    let line = inject_hints(&raw, &map, text, colors, hints);
                    (line.text, line.colors, Some(line.cols), line.inserts)
                }
                None => (text, colors, cols, Vec::new()),
            };
            Line {
                start: buffer.line_start(index),
                chars: buffer.line_len(index),
                text,
                colors,
                cols,
                inserts,
                level: 0,
                step: 1,
            }
        })
        .collect();
    let (step, levels) = {
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        indent::levels(&texts)
    };
    for (line, level) in lines.iter_mut().zip(levels) {
        line.level = level;
        line.step = step;
    }
    lines
}

/// The byte where the nearest function, method, class or interface name starts, strictly
/// after (`forward`) or before `from`. Structs and traits count as classes and interfaces.
fn declaration_beyond(outline: &[StubNode], from: usize, forward: bool) -> Option<usize> {
    let stops = outline
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                StubKind::Function
                    | StubKind::Method
                    | StubKind::Class
                    | StubKind::Interface
                    | StubKind::Struct
                    | StubKind::Trait
            )
        })
        .map(|node| match node.name_range.is_empty() {
            true => node.range.start as usize,
            false => node.name_range.start as usize,
        });
    if forward {
        stops.filter(|&byte| byte > from).min()
    } else {
        stops.filter(|&byte| byte < from).max()
    }
}

/// The declarations of `outline` (in source order) that contain `byte`, outermost first.
/// Imports are not shown: they are declarations only for the search.
fn enclosing(outline: &[StubNode], byte: usize) -> Vec<&StubNode> {
    // nested declarations come after their parent, so the last one containing the byte is
    // the innermost
    let Some(innermost) = outline
        .iter()
        .rposition(|node| node.kind != StubKind::Import && node.range.contains(&(byte as u32)))
    else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut at = Some(innermost);
    while let Some(index) = at {
        chain.push(&outline[index]);
        at = outline[index].parent.map(|p| p as usize);
    }
    chain.reverse();
    chain
}

fn symbol_glyph(kind: StubKind) -> &'static str {
    match kind {
        StubKind::Function | StubKind::Method | StubKind::Macro => "ƒ",
        StubKind::Struct
        | StubKind::Class
        | StubKind::Interface
        | StubKind::Enum
        | StubKind::Trait
        | StubKind::Impl
        | StubKind::TypeAlias => "C",
        StubKind::Module => "M",
        StubKind::Const => "c",
        StubKind::Field | StubKind::Variant | StubKind::Import => "•",
    }
}

/// `line` with tabs as spaces, and for each old byte offset (plus the end) its new offset.
fn expand_tabs(line: &str) -> (String, Vec<usize>) {
    let mut text = String::with_capacity(line.len());
    let mut map = vec![0; line.len() + 1];
    let mut column = 0;
    for (byte, c) in line.char_indices() {
        for offset in 0..c.len_utf8() {
            map[byte + offset] = text.len();
        }
        if c == '\t' {
            let width = TAB_WIDTH - column % TAB_WIDTH;
            text.push_str(&" ".repeat(width));
            column += width;
        } else {
            text.push(c);
            column += 1;
        }
    }
    map[line.len()] = text.len();
    (text, map)
}

// gpui's glob import shadows `test`, hence the explicit attribute path below.
/// The bar in the gutter, between the line number and the code, that says how the line differs
/// from the last commit; deletions are a short notch on the line edge where the lines were.
fn change_mark(change: LineChange, gutter: f32, scale: f32, row_height: f32) -> Div {
    let (color, top, height) = match change {
        LineChange::Added => (theme::CHANGE_ADDED, 0., row_height),
        LineChange::Modified => (theme::CHANGE_MODIFIED, 0., row_height),
        LineChange::Removed => (theme::CHANGE_REMOVED, 0., 4. * scale),
        LineChange::RemovedBelow => (theme::CHANGE_REMOVED, row_height - 4. * scale, 4. * scale),
    };
    div()
        .absolute()
        .top(px(top))
        .left(px(gutter - 6. * scale))
        .w(px(3. * scale))
        .h(px(height))
        .bg(rgb(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn tabs_expand_to_the_next_stop_and_offsets_follow() {
        let (text, map) = expand_tabs("a\tb");
        assert_eq!(text, "a   b");
        assert_eq!(map, [0, 1, 4, 5]);
    }

    #[::core::prelude::v1::test]
    fn spans_are_clipped_to_each_line() {
        let source = "/* a\nb */ x";
        let buffer = Buffer::new(source);
        let spans = [Span {
            range: 0..10,
            class: Class::Comment,
        }];
        let lines = build_lines(&buffer, &spans, &LineHints::new());
        assert_eq!(lines[0].colors[0].0, 0..4);
        assert_eq!(lines[1].colors[0].0, 0..5);
        assert!(lines[1].colors.len() == 1);
    }

    fn temp_file(name: &str, content: &str) -> (PathBuf, String) {
        temp_file_as(name, "a.txt", content)
    }

    fn temp_file_as(name: &str, file: &str, content: &str) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("zide-viewer-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), content).unwrap();
        (dir, file.into())
    }

    fn type_keys(viewer: &mut Viewer, keys: &str) {
        for c in keys.chars() {
            viewer.handle_key(Key::Char(c));
        }
    }

    #[::core::prelude::v1::test]
    fn the_breadcrumb_lists_the_declarations_around_the_cursor() {
        let source = "struct P;\nimpl P {\n    fn go(&self) {\n        1;\n    }\n}\n";
        let outline = extract_nodes(stub_language("a.rs").unwrap(), source);
        let names = |byte: usize| -> Vec<String> {
            enclosing(&outline, byte)
                .iter()
                .map(|n| n.name.as_deref().unwrap_or("").to_string())
                .collect()
        };
        assert_eq!(names(source.find("1;").unwrap()), ["P", "go"]);
        assert_eq!(names(source.find("struct").unwrap()), ["P"]);
        assert!(names(source.len() - 1).is_empty()); // after the last declaration
    }

    #[::core::prelude::v1::test]
    fn the_word_under_the_cursor_is_an_identifier() {
        let (dir, file) = temp_file("word", "let foo_bar = baz;\n");
        let mut viewer = Viewer::open(&dir, &file);
        assert_eq!(viewer.word_at_cursor().as_deref(), Some("let"));
        type_keys(&mut viewer, "w");
        viewer.handle_key(Key::Char('l'));
        assert_eq!(viewer.word_at_cursor().as_deref(), Some("foo_bar"));
        type_keys(&mut viewer, "e");
        type_keys(&mut viewer, "l"); // on the space
        assert_eq!(viewer.word_at_cursor(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    fn edit_at(line: u32, from: u32, to: u32, text: &str) -> zide_lsp::TextEdit {
        zide_lsp::TextEdit {
            range: zide_lsp::Range::new(
                zide_lsp::Position::new(line, from),
                zide_lsp::Position::new(line, to),
            ),
            new_text: text.into(),
        }
    }

    #[::core::prelude::v1::test]
    fn formatting_without_a_selection_asks_for_everything() {
        let (dir, file) = temp_file("fmt-all", "a\nb\n");
        let viewer = Viewer::open(&dir, &file);
        let request = viewer.format_request().unwrap();
        assert!(request.range.is_none());
        assert_eq!(request.text, "a\nb\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn formatting_a_selection_asks_for_its_range() {
        let (dir, file) = temp_file("fmt-sel", "one\ntwo\nthree\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "jVj");
        let range = viewer.format_request().unwrap().range.unwrap();
        // whole lines 1 and 2: from the start of "two" to the start of the line after "three"
        assert_eq!(
            (
                range.start.line,
                range.start.character,
                range.end.line,
                range.end.character
            ),
            (1, 0, 3, 0)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn formatted_edits_apply_as_one_undo_step_and_leave_visual() {
        let (dir, file) = temp_file("fmt-apply", "a  =1\nb  =2\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "Vj");
        let request = viewer.format_request().unwrap();
        let edits = vec![edit_at(0, 0, 5, "a = 1"), edit_at(1, 0, 5, "b = 2")];
        assert!(viewer.apply_format(request.revision, edits));
        assert_eq!(viewer.text(), "a = 1\nb = 2\n");
        assert_eq!(viewer.vim.mode(), Mode::Normal);
        type_keys(&mut viewer, "u");
        assert_eq!(viewer.text(), "a  =1\nb  =2\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_format_answer_for_older_text_is_dropped() {
        let (dir, file) = temp_file("fmt-stale", "a\n");
        let mut viewer = Viewer::open(&dir, &file);
        let request = viewer.format_request().unwrap();
        type_keys(&mut viewer, "ix");
        viewer.handle_key(Key::Esc);
        assert!(!viewer.apply_format(request.revision, vec![edit_at(0, 0, 1, "zzz")]));
        assert_eq!(viewer.text(), "xa\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn inserts_at_the_same_place_keep_the_answer_order() {
        let (dir, file) = temp_file("fmt-order", "ab\n");
        let mut viewer = Viewer::open(&dir, &file);
        let revision = viewer.format_request().unwrap().revision;
        let edits = vec![edit_at(0, 1, 1, "1"), edit_at(0, 1, 1, "2")];
        assert!(viewer.apply_format(revision, edits));
        assert_eq!(viewer.text(), "a12b\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn extend_selection_grows_and_shrink_walks_back() {
        let (dir, file) = temp_file_as("extend", "a.rs", "fn m() { f(one, two); }\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "lllllllllll"); // on the `o` of "one"
        let selected = |v: &Viewer| v.selected_text();
        viewer.extend_selection();
        assert_eq!(selected(&viewer).as_deref(), Some("one"));
        viewer.extend_selection();
        assert_eq!(selected(&viewer).as_deref(), Some("one, two"));
        viewer.shrink_selection();
        assert_eq!(selected(&viewer).as_deref(), Some("one"));
        viewer.shrink_selection();
        assert_eq!(viewer.vim.mode(), Mode::Normal);
        assert_eq!(viewer.vim.cursor(), 11); // the `o` it started on
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn shrink_does_nothing_after_the_selection_was_changed_by_hand() {
        let (dir, file) = temp_file_as("shrink-broken", "a.rs", "fn m() { f(one, two); }\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "lllllllllll"); // on the `o` of "one"
        viewer.extend_selection();
        type_keys(&mut viewer, "l"); // moves the end of the selection
        viewer.shrink_selection();
        assert_ne!(viewer.selected_text().as_deref(), Some("o"));
        assert_eq!(viewer.vim.mode(), Mode::Visual);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn the_word_under_the_caret_is_highlighted_where_it_stands_alone() {
        let (dir, file) = temp_file("word-hl", "foo foobar foo_x (foo)\nfoo\n");
        let mut viewer = Viewer::open(&dir, &file);
        // caret on the first `foo`: whole words only, on every line
        assert_eq!(
            *viewer.word_highlights(),
            [0..3, 18..21, 23..26] // "foo", "(foo)" and the one on line 2
        );
        type_keys(&mut viewer, "v"); // selecting: no highlighting
        assert!(viewer.word_highlights().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    #[allow(clippy::single_range_in_vec_init)]
    fn a_server_answer_replaces_the_textual_highlights_while_it_fits() {
        let (dir, file) = temp_file("semantic-hl", "foo foo\n");
        let mut viewer = Viewer::open(&dir, &file);
        let target = viewer.highlight_target().unwrap();
        let only_second =
            zide_lsp::Range::new(zide_lsp::Position::new(0, 4), zide_lsp::Position::new(0, 7));
        viewer.set_semantic(target.key, vec![only_second]);
        assert_eq!(*viewer.word_highlights(), [4..7]);
        // after an edit ("oo foo") the old answer no longer applies: back to the text
        type_keys(&mut viewer, "x");
        assert_eq!(*viewer.word_highlights(), [0..2]);
        // an answer for a revision that is gone is ignored
        viewer.set_semantic(target.key, vec![only_second]);
        assert_eq!(*viewer.word_highlights(), [0..2]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn searching_the_word_under_the_caret_steps_to_the_next_one() {
        let (dir, file) = temp_file("f3", "foo bar foo foobar foo\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "l"); // inside the word, not at its start
        let word = viewer.word_to_search().unwrap();
        viewer.search_word(&word, true);
        assert_eq!(viewer.vim.cursor(), 8); // whole words only: `foobar` is skipped
        assert!(viewer.cursor_on_match());
        viewer.jump(true, false);
        assert_eq!(viewer.vim.cursor(), 19);
        viewer.jump(false, false);
        assert_eq!(viewer.vim.cursor(), 8);
        viewer.search_word("foo", false); // Shift+F3 from the second one goes back
        assert_eq!(viewer.vim.cursor(), 0);
        // moved off the matches: a different word would be searched instead
        type_keys(&mut viewer, "l");
        assert!(!viewer.cursor_on_match());
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn edits_count_utf16_columns() {
        let (dir, file) = temp_file("fmt-utf16", "😀x\n");
        let mut viewer = Viewer::open(&dir, &file);
        let revision = viewer.format_request().unwrap().revision;
        // the emoji is two UTF-16 units: column 2 is the `x`
        assert!(viewer.apply_format(revision, vec![edit_at(0, 2, 3, "y")]));
        assert_eq!(viewer.text(), "😀y\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn z_waits_for_its_second_key_and_escape_cancels() {
        let (dir, file) = temp_file("z", "a\nb\nc\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "z");
        assert_eq!(viewer.pending_keys(), "z");
        viewer.handle_key(Key::Esc);
        assert_eq!(viewer.pending_keys(), "");
        type_keys(&mut viewer, "zz");
        assert_eq!(viewer.pending_keys(), "");
        assert_eq!(viewer.vim.cursor(), 0); // scrolling does not move the cursor
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn ctrl_d_and_ctrl_u_move_the_cursor_half_a_page() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (dir, file) = temp_file("page", &text);
        let mut viewer = Viewer::open(&dir, &file);
        let line = |v: &Viewer| v.buffer.position_of(v.vim.cursor()).line;
        viewer.handle_key(Key::Ctrl('d')); // no layout yet: 20 rows, half is 10
        assert_eq!(line(&viewer), 10);
        viewer.handle_key(Key::Ctrl('d'));
        assert_eq!(line(&viewer), 20);
        viewer.handle_key(Key::Ctrl('u'));
        assert_eq!(line(&viewer), 10);
        for _ in 0..5 {
            viewer.handle_key(Key::Ctrl('u'));
        }
        assert_eq!(line(&viewer), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn save_writes_only_changes_and_clears_the_dirty_mark() {
        let (dir, file) = temp_file("save", "one\ntwo\n");
        let mut viewer = Viewer::open(&dir, &file);
        viewer.save(); // nothing to write
        assert!(!viewer.is_dirty());
        type_keys(&mut viewer, "dd");
        assert!(viewer.is_dirty());
        viewer.save();
        assert!(!viewer.is_dirty());
        assert_eq!(fs::read_to_string(dir.join(&file)).unwrap(), "two\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn lines_that_differ_from_the_last_commit_are_marked_and_follow_the_edits() {
        let (dir, file) = temp_file("changes", "one\ntwo\nthree\n");
        let mut viewer = Viewer::open(&dir, &file);
        assert!(viewer.changes.is_empty()); // git has not answered yet
        viewer.set_head(Some("one\ntwo\nthree\n".into()));
        assert!(viewer.changes.is_empty()); // same as the commit
        type_keys(&mut viewer, "jdd");
        assert_eq!(viewer.changes.get(&1), Some(&LineChange::Removed));
        type_keys(&mut viewer, "Onew");
        viewer.handle_key(Key::Esc);
        assert_eq!(viewer.changes.get(&1), Some(&LineChange::Modified));
        viewer.set_head(None); // untracked: nothing to compare with
        assert!(viewer.changes.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    fn diagnostic(range: ((u32, u32), (u32, u32)), message: &str) -> Diagnostic {
        Diagnostic {
            range: zide_lsp::Range::new(
                zide_lsp::Position::new(range.0.0, range.0.1),
                zide_lsp::Position::new(range.1.0, range.1.1),
            ),
            severity: Some(DiagnosticSeverity::ERROR),
            message: message.into(),
            ..Default::default()
        }
    }

    #[::core::prelude::v1::test]
    fn a_click_puts_the_caret_on_the_character_under_the_mouse() {
        let (dir, file) = temp_file("click", "hello\n\tab\n");
        let mut viewer = Viewer::open(&dir, &file);
        let gutter = viewer.gutter_width();
        // without a layout the cells are 8 px wide and the editor starts at the window's corner
        let at = |cell: f32| point(px(gutter + cell * 8. + 1.), px(0.));
        let cursor = |v: &Viewer| v.buffer.position_of(v.vim.cursor());
        assert!(viewer.click(0, at(2.)));
        assert_eq!((cursor(&viewer).line, cursor(&viewer).column), (0, 2));
        assert!(viewer.click(0, at(30.))); // past the end: the last character
        assert_eq!(cursor(&viewer).column, 4);
        assert!(viewer.click(1, at(4.))); // the tab fills 4 cells: `a` is the next one
        assert_eq!((cursor(&viewer).line, cursor(&viewer).column), (1, 1));
        assert!(viewer.click(1, at(2.))); // inside the tab
        assert_eq!(cursor(&viewer).column, 0);
        assert!(!viewer.click(0, point(px(3.), px(0.)))); // on the line numbers
        assert_eq!(cursor(&viewer).column, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    fn hint(line: u32, character: u32, label: &str) -> ParameterHint {
        ParameterHint {
            position: zide_lsp::Position::new(line, character),
            label: label.into(),
        }
    }

    #[::core::prelude::v1::test]
    fn parameter_names_are_part_of_the_line_shown_and_everything_measured_in_it_follows() {
        let (dir, file) = temp_file("hints", "f(a, bb);\n\tg(c);\n");
        let mut viewer = Viewer::open(&dir, &file);
        let revision = viewer.revision();
        assert!(viewer.set_hints(
            vec![hint(0, 2, "x:"), hint(0, 5, "yy:"), hint(1, 3, "z:")],
            revision
        ));
        assert!(!viewer.set_hints(
            vec![hint(0, 2, "x:"), hint(0, 5, "yy:"), hint(1, 3, "z:")],
            revision
        )); // the same
        assert_eq!(viewer.lines[0].text, "f(x: a, yy: bb);");
        assert_eq!(viewer.lines[1].text, "    g(z: c);"); // the tab is 4 cells
        let chips: Vec<_> = viewer.lines[0]
            .colors
            .iter()
            .map(|(r, _)| r.clone())
            .collect();
        assert_eq!(chips, [2..4, 8..11]); // the names, not the space after them
        // the characters are where they end up: `a`, the second `b`, `)` and the end of the line
        assert_eq!(viewer.lines[0].display_byte(2), 5);
        assert_eq!(viewer.lines[0].display_byte(5), 12);
        assert_eq!(viewer.lines[0].display_byte(8), 15);
        assert_eq!(viewer.lines[0].display_byte(9), 16);
        // the caret is on the character, not on the hint before it
        viewer.vim.set_cursor(&viewer.buffer, 5); // `b`
        let (_, cell) = viewer.cursor_cell();
        assert_eq!(cell, 12..13);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn hints_for_older_text_are_dropped_and_hints_follow_the_edits_since() {
        let (dir, file) = temp_file("hints-edit", "f(a);\ng(b);\n");
        let mut viewer = Viewer::open(&dir, &file);
        let revision = viewer.revision();
        viewer.set_hints(vec![hint(0, 2, "x:"), hint(1, 2, "y:")], revision);
        type_keys(&mut viewer, "Onew");
        viewer.handle_key(Key::Esc); // a line above: both hints go one line down
        assert_eq!(viewer.lines[1].text, "f(x: a);");
        assert_eq!(viewer.lines[2].text, "g(y: b);");
        assert!(!viewer.set_hints(vec![hint(0, 2, "stale:")], revision)); // for the old text
        assert_eq!(viewer.lines[1].text, "f(x: a);");
        assert_eq!(
            follow_edit("f(a, b)", "f(a, b, c)", &[(2, "x".into()), (5, "y".into())]),
            [(2, "x".to_string()), (5, "y".to_string())]
        );
        // an edit that rewrites the place of a hint removes it; the start of the edit keeps it
        assert_eq!(
            follow_edit(
                "f(abc, b)",
                "f(zz, b)",
                &[(2, "x".into()), (3, "w".into()), (7, "y".into())]
            ),
            [(2, "x".to_string()), (6, "y".to_string())]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn the_place_a_file_was_left_at_comes_back_within_what_the_file_still_has() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (dir, file) = temp_file("restore", &text);
        let mut viewer = Viewer::open(&dir, &file);
        assert_eq!(viewer.view(), None); // nothing drawn yet: the scroll is not known
        let place = |top, line, column| crate::session::View { top, line, column };
        let cursor = |v: &Viewer| {
            let at = v.buffer.position_of(v.vim.cursor());
            (at.line, at.column)
        };
        viewer.restore_view(place(40, 45, 3));
        assert_eq!(cursor(&viewer), (45, 3));
        viewer.restore_view(place(900, 900, 900)); // the file shrank: the end of it
        assert_eq!(cursor(&viewer).0, 100);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn errors_are_underlined_where_the_server_put_them() {
        let (dir, file) = temp_file("squiggles", "let a = 1;\n\tlet b = c;\n");
        let mut viewer = Viewer::open(&dir, &file);
        let mut warning = diagnostic(((0, 0), (0, 3)), "unused");
        warning.severity = Some(DiagnosticSeverity::WARNING);
        let diagnostics = vec![
            diagnostic(((1, 9), (1, 10)), "cannot find `c`"),
            diagnostic(((0, 4), (0, 4)), "empty range"),
            diagnostic(((0, 9), (1, 1)), "spans lines"),
            warning,
        ];
        assert!(viewer.set_diagnostics(diagnostics.clone()));
        assert!(!viewer.set_diagnostics(diagnostics)); // nothing new
        let on = |row: usize| -> Vec<(usize, Range<usize>)> { viewer.squiggles[&row].clone() };
        // the tab is shown as spaces, so the display bytes are not the characters
        let tab = viewer.lines[1].display_byte(1);
        assert!(tab > 1);
        assert_eq!(
            on(1)[0],
            (
                0,
                viewer.lines[1].display_byte(9)..viewer.lines[1].display_byte(10)
            )
        );
        assert_eq!(on(0)[0], (1, 4..5)); // an empty range still gets one character
        assert_eq!(on(0)[1], (2, 9..10)); // multi-line: to the end of the first line
        assert_eq!(on(1)[1], (2, 0..tab)); // and from the start of the next
        assert!(
            !viewer
                .squiggles
                .values()
                .flatten()
                .any(|(index, _)| *index == 3)
        ); // not a warning
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn the_popup_names_the_error_with_its_code_and_related_places() {
        let mut d = diagnostic(((0, 0), (0, 1)), "Expected 8 arguments, but got 7");
        assert_eq!(headline(&d), "Expected 8 arguments, but got 7");
        d.code = Some(NumberOrString::Number(2554));
        d.source = Some("ts".into());
        assert_eq!(headline(&d), "TS2554: Expected 8 arguments, but got 7");
        d.source = Some("rustc".into());
        assert_eq!(headline(&d), "2554: Expected 8 arguments, but got 7");
        d.related_information = Some(vec![zide_lsp::DiagnosticRelatedInformation {
            location: zide_lsp::Location::new(
                "file:///app/card.ts".parse().unwrap(),
                zide_lsp::Range::new(
                    zide_lsp::Position::new(37, 4),
                    zide_lsp::Position::new(37, 9),
                ),
            ),
            message: "An argument for maxCopies was not provided.".into(),
        }]);
        assert_eq!(
            related_lines(&d),
            ["card.ts(38, 5): An argument for maxCopies was not provided."]
        );
    }

    #[::core::prelude::v1::test]
    fn blame_shows_for_the_cursor_line_until_the_buffer_is_edited_or_saved() {
        let (dir, file) = temp_file("blame", "one\ntwo\n");
        let mut viewer = Viewer::open(&dir, &file);
        assert_eq!(viewer.blame_label(), None); // git has not answered yet
        let entry = |summary: &str| BlameLine {
            author: "Sergio Molina".into(),
            time: 0,
            tz_minutes: 0,
            summary: summary.into(),
            committed: true,
        };
        viewer.set_blame(Some(vec![entry("first"), entry("second")]));
        assert_eq!(
            viewer.blame_label().as_deref(),
            Some("Molina, 1/1/70, 12:00 AM \u{2022} first")
        );
        type_keys(&mut viewer, "j");
        assert!(viewer.blame_label().unwrap().ends_with("second"));
        type_keys(&mut viewer, "x");
        assert_eq!(viewer.blame_label(), None); // edited: the lines no longer match
        assert!(viewer.save());
        assert_eq!(viewer.blame_label(), None); // saved: waits for the new blame
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn the_pair_under_the_caret_is_found_and_follows_it() {
        let (dir, file) = temp_file_as("pair", "a.rs", "fn m() { f(1); }\n");
        let mut viewer = Viewer::open(&dir, &file);
        assert!(viewer.pair_highlights().is_empty()); // on `f`, no bracket
        type_keys(&mut viewer, "lllllll"); // on the `{`
        assert_eq!(*viewer.pair_highlights(), vec![7..8, 15..16]);
        type_keys(&mut viewer, "lll"); // on the `(`
        assert_eq!(*viewer.pair_highlights(), vec![10..11, 12..13]);
        type_keys(&mut viewer, "l");
        assert!(viewer.pair_highlights().is_empty());
        type_keys(&mut viewer, "l"); // on the `)`: its partner is the `(`
        assert_eq!(*viewer.pair_highlights(), vec![12..13, 10..11]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn declarations_scrolled_out_of_view_are_pinned_outermost_first() {
        let text = "struct S;\nimpl S {\n    fn a(&self) {\n        let x = 1;\n        let y = 2;\n    }\n    fn b(&self) {}\n}\n";
        let (dir, file) = temp_file_as("sticky", "a.rs", text);
        let viewer = Viewer::open(&dir, &file);
        assert!(viewer.pinned_at(0).is_empty());
        assert!(viewer.pinned_at(1).is_empty()); // the impl line is the top row itself
        assert_eq!(viewer.pinned_at(3), vec![1, 2]); // inside `a`, in the impl
        assert_eq!(viewer.pinned_at(6), vec![1]); // `b` fits in one line: not a scope
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn save_refuses_to_overwrite_a_file_changed_on_disk() {
        let (dir, file) = temp_file("conflict", "one\n");
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "x");
        fs::write(dir.join(&file), "changed elsewhere, longer\n").unwrap();
        viewer.save();
        assert!(viewer.is_dirty());
        assert!(viewer.notice.is_some());
        assert_eq!(
            fs::read_to_string(dir.join(&file)).unwrap(),
            "changed elsewhere, longer\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn an_unreadable_file_is_never_written() {
        let dir = std::env::temp_dir().join(format!("zide-viewer-{}-missing", std::process::id()));
        let mut viewer = Viewer::open(&dir, "nope.txt");
        type_keys(&mut viewer, "x");
        assert!(!viewer.is_dirty());
        viewer.save();
        assert!(!dir.join("nope.txt").exists());
    }

    fn plain(text: &str) -> Line {
        let (text, _) = expand_tabs(text);
        Line {
            start: 0,
            chars: text.chars().count(),
            text,
            colors: Vec::new(),
            cols: None,
            inserts: Vec::new(),
            level: 0,
            step: 1,
        }
    }

    #[::core::prelude::v1::test]
    fn the_font_size_is_clamped_and_the_rows_follow_it() {
        let (dir, name) = temp_file("zoom.txt", "a\nb\n");
        let mut viewer = Viewer::open(&dir, &name);
        assert_eq!(viewer.row_height(), 20.);
        viewer.set_font_size(1000.);
        assert_eq!(viewer.row_height(), MAX_FONT_SIZE * ROW_RATIO);
        viewer.set_font_size(0.);
        assert_eq!(viewer.row_height(), MIN_FONT_SIZE * ROW_RATIO);
    }

    #[::core::prelude::v1::test]
    fn next_and_previous_method_walk_the_declarations() {
        let source = "struct P;\nconst X: u8 = 1;\nimpl P {\n    fn one(&self) {}\n    fn two(&self) {}\n}\nfn free() {}\ntrait T {}\n";
        let (dir, file) = temp_file_as("methods", "a.rs", source);
        let mut viewer = Viewer::open(&dir, &file);
        let word = |v: &Viewer| v.word_at_cursor();
        let mut forward = Vec::new();
        for _ in 0..5 {
            viewer.next_method(true);
            forward.push(word(&viewer));
        }
        // consts and impl blocks are not stops
        assert_eq!(
            forward,
            ["P", "one", "two", "free", "T"].map(|n| Some(n.to_string()))
        );
        // with nothing left the cursor goes to the end of the file, and stays there
        let mut end = Viewer::open(&dir, &file);
        end.vim.set_cursor(&end.buffer, end.buffer.len());
        viewer.next_method(true);
        assert_eq!(viewer.vim.cursor(), end.vim.cursor());
        viewer.next_method(true);
        assert_eq!(viewer.vim.cursor(), end.vim.cursor());
        let mut backward = Vec::new();
        for _ in 0..5 {
            viewer.next_method(false);
            backward.push(word(&viewer));
        }
        assert_eq!(
            backward,
            ["T", "free", "two", "one", "P"].map(|n| Some(n.to_string()))
        );
        // and with nothing before, to the start
        viewer.next_method(false);
        assert_eq!(viewer.vim.cursor(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn selection_covers_the_right_part_of_each_line() {
        let selection = Selection {
            start: Position { line: 1, column: 2 },
            end: Position { line: 3, column: 1 },
            linewise: false,
        };
        let line = plain("abcdef");
        assert_eq!(selection.on_line(0, &line), None);
        assert_eq!(selection.on_line(1, &line), Some(2..6));
        assert_eq!(selection.on_line(2, &line), Some(0..6));
        assert_eq!(selection.on_line(3, &line), Some(0..1));
        assert_eq!(selection.on_line(4, &line), None);
    }

    #[::core::prelude::v1::test]
    fn a_selection_ending_at_a_line_start_leaves_that_line_out() {
        let selection = Selection {
            start: Position { line: 0, column: 0 },
            end: Position { line: 2, column: 0 },
            linewise: true,
        };
        let line = plain("abc");
        assert_eq!(selection.on_line(1, &line), Some(0..3));
        assert_eq!(selection.on_line(2, &line), None);
    }

    #[::core::prelude::v1::test]
    fn paint_splits_spans_and_fills_gaps() {
        let red = HighlightStyle {
            color: Some(rgb(0xff0000).into()),
            ..Default::default()
        };
        let colors = vec![(0..4, red), (6..8, red)];
        let painted = paint(&colors, 2..7, rgb(0x00ff00).into());
        let ranges: Vec<_> = painted.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..2, 2..4, 4..6, 6..7, 7..8]);
        assert!(painted[0].1.background_color.is_none());
        assert!(painted[1].1.background_color.is_some());
        assert_eq!(painted[1].1.color, red.color); // keeps its color
        assert!(painted[2].1.background_color.is_some()); // the gap is painted too
        assert!(painted[4].1.background_color.is_none());
    }

    fn query(pattern: &str) -> FindQuery {
        FindQuery {
            pattern: pattern.into(),
            ..Default::default()
        }
    }

    #[::core::prelude::v1::test]
    fn find_moves_the_cursor_and_n_steps_through_matches() {
        let (dir, file) = temp_file("find", "foo bar\nfoo baz\nfoo");
        let mut viewer = Viewer::open(&dir, &file);
        viewer.set_find(&query("foo"));
        assert_eq!(viewer.find_status().unwrap().total, 3);
        assert_eq!(viewer.find_status().unwrap().current, Some(1));
        viewer.handle_key(Key::Char('n'));
        assert_eq!(viewer.find_status().unwrap().current, Some(2));
        viewer.handle_key(Key::Char('N'));
        viewer.handle_key(Key::Char('N'));
        assert_eq!(viewer.find_status().unwrap().current, Some(3)); // wraps backwards
        viewer.handle_key(Key::Char('n'));
        assert_eq!(viewer.find_status().unwrap().current, Some(1)); // and forwards
        assert_eq!(viewer.handle_key(Key::Char('/')), Effect::OpenFind);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn replace_current_replaces_one_match_and_moves_on() {
        let (dir, file) = temp_file("replace-one", "a a a");
        let mut viewer = Viewer::open(&dir, &file);
        viewer.set_find(&query("a"));
        viewer.replace_current("bb");
        assert_eq!(viewer.buffer.text(), "bb a a");
        assert_eq!(viewer.find_status().unwrap().total, 2);
        assert_eq!(viewer.find_status().unwrap().current, Some(1));
        viewer.replace_current("c");
        assert_eq!(viewer.buffer.text(), "bb c a");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn replace_all_is_one_undo_step() {
        let (dir, file) = temp_file("replace-all", "x1 x2\nx3");
        let mut viewer = Viewer::open(&dir, &file);
        viewer.set_find(&FindQuery {
            pattern: r"x(\d)".into(),
            regex: true,
            ..Default::default()
        });
        assert_eq!(viewer.replace_all("y$1"), 3);
        assert_eq!(viewer.buffer.text(), "y1 y2\ny3");
        viewer.handle_key(Key::Char('u'));
        assert_eq!(viewer.buffer.text(), "x1 x2\nx3");
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn matches_follow_edits_and_invalid_patterns_report_an_error() {
        let (dir, file) = temp_file("follow", "foo foo");
        let mut viewer = Viewer::open(&dir, &file);
        viewer.set_find(&query("foo"));
        viewer.handle_key(Key::Char('d'));
        viewer.handle_key(Key::Char('w')); // deletes the first foo
        viewer.jump(true, true);
        assert_eq!(viewer.find_status().unwrap().total, 1);
        viewer.set_find(&FindQuery {
            pattern: "(".into(),
            regex: true,
            ..Default::default()
        });
        let status = viewer.find_status().unwrap();
        assert!(status.error.is_some());
        assert_eq!(status.total, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_parked_file_is_stale_only_if_clean_and_changed_on_disk() {
        let (dir, file) = temp_file("stale", "one\n");
        let mut viewer = Viewer::open(&dir, &file);
        assert!(!viewer.is_stale());
        fs::write(dir.join(&file), "two, longer\n").unwrap();
        assert!(viewer.is_stale());
        // with unsaved edits the file is kept, so nothing of ours is lost
        type_keys(&mut viewer, "x");
        assert!(!viewer.is_stale());
        fs::remove_dir_all(dir).unwrap();
    }

    const FOLDABLE: &str = "fn a() {\n    one();\n    two();\n}\nfn b() {\n    three();\n}\n";

    fn shown(viewer: &Viewer) -> Vec<usize> {
        (0..viewer.rows.len())
            .map(|r| viewer.rows.line(r))
            .collect()
    }

    fn cursor_line(viewer: &Viewer) -> usize {
        viewer.buffer.position_of(viewer.vim.cursor()).line
    }

    #[::core::prelude::v1::test]
    fn zc_folds_the_block_around_the_cursor_and_za_toggles_it() {
        let (dir, file) = temp_file_as("fold-zc", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        assert_eq!(shown(&viewer).len(), 8);
        type_keys(&mut viewer, "jzc");
        assert_eq!(shown(&viewer), [0, 4, 5, 6, 7]);
        assert_eq!(cursor_line(&viewer), 0); // the cursor was inside: it goes to the header
        type_keys(&mut viewer, "zo");
        assert_eq!(shown(&viewer).len(), 8);
        type_keys(&mut viewer, "za");
        assert_eq!(shown(&viewer), [0, 4, 5, 6, 7]);
        type_keys(&mut viewer, "za");
        assert_eq!(shown(&viewer).len(), 8);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn zm_folds_everything_and_zr_opens_everything() {
        let (dir, file) = temp_file_as("fold-zm", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "zM");
        assert_eq!(shown(&viewer), [0, 4, 7]);
        type_keys(&mut viewer, "zR");
        assert_eq!(shown(&viewer).len(), 8);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn j_and_k_step_over_a_folded_block() {
        let (dir, file) = temp_file_as("fold-jk", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "zcj");
        assert_eq!(cursor_line(&viewer), 4);
        type_keys(&mut viewer, "k");
        assert_eq!(cursor_line(&viewer), 0);
        type_keys(&mut viewer, "G");
        assert!(viewer.rows.shows(cursor_line(&viewer)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_jump_into_a_folded_block_opens_it() {
        let (dir, file) = temp_file_as("fold-jump", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "zM");
        viewer.go_to_line(3); // `two();`
        assert_eq!(shown(&viewer), [0, 1, 2, 3, 4, 7]);
        assert_eq!(cursor_line(&viewer), 2);
        type_keys(&mut viewer, "zM");
        viewer.next_method(false); // back to `a`, then forward again into `b`
        viewer.next_method(true);
        assert_eq!(cursor_line(&viewer), 4);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn a_fold_survives_an_edit_elsewhere() {
        let (dir, file) = temp_file_as("fold-edit", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "zcjjjjx"); // fold `a`, delete a char in `b`
        assert_eq!(shown(&viewer), [0, 4, 5, 6, 7]);
        assert!(viewer.is_dirty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn clicking_a_chevron_toggles_the_region_that_starts_on_its_line() {
        let (dir, file) = temp_file_as("fold-click", "a.rs", FOLDABLE);
        let mut viewer = Viewer::open(&dir, &file);
        type_keys(&mut viewer, "j");
        viewer.toggle_fold(0);
        assert_eq!(shown(&viewer), [0, 4, 5, 6, 7]);
        assert_eq!(cursor_line(&viewer), 0); // it was inside the fold
        viewer.toggle_fold(0);
        assert_eq!(shown(&viewer).len(), 8);
        viewer.toggle_fold(1); // no region starts on this line
        assert_eq!(shown(&viewer).len(), 8);
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn folds_and_outline_follow_edits_as_a_fresh_parse_would() {
        let source = "use a::b;\n\nfn one() {\n    let x = 1;\n}\n\nstruct S {\n    f: u8,\n}\n\nfn two() {\n    if true {\n        run();\n    }\n}\n";
        let (dir, file) = temp_file_as("incremental", "a.rs", source);
        let mut viewer = Viewer::open(&dir, &file);
        let fresh = |viewer: &Viewer| {
            let text = viewer.buffer.text();
            let language = viewer.language.unwrap();
            (
                zide_syntax::fold_regions(language, &text),
                extract_nodes(stub_language("a.rs").unwrap(), &text),
            )
        };
        let steps = [
            "3jo// new\n", // a line inside a block
            "ggdd",        // delete the first line
            "5Gx",         // delete one char
            "Gddkdd",      // delete lines at the end
            "ggOfn zero() {}\n",
        ];
        for keys in steps {
            type_keys(&mut viewer, keys);
            viewer.handle_key(Key::Esc);
            let (folds, outline) = fresh(&viewer);
            assert_eq!(viewer.folds, folds, "folds after {keys:?}");
            assert_eq!(viewer.outline, outline, "outline after {keys:?}");
        }
        type_keys(&mut viewer, "u");
        let (folds, outline) = fresh(&viewer);
        assert_eq!(viewer.folds, folds, "folds after undo");
        assert_eq!(viewer.outline, outline, "outline after undo");
        fs::remove_dir_all(dir).unwrap();
    }
}
