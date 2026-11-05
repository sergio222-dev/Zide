//! File editor view: line numbers, syntax colors, a block cursor and Vim keys.

use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
    time::{Duration, Instant},
};

use gpui_kit::*;
use zide_syntax::{Class, LanguageId, Span, highlight, language_for_path};
use zide_text::{Buffer, FindQuery, Finder, Position, Revision};
use zide_vim::{Key, Mode, Vim};

/// Files larger than this are not opened.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TAB_WIDTH: usize = 4;
const ROW_HEIGHT: f32 = 20.;
const EDITOR_FONT: &str = "monospace";

struct Line {
    /// Char offset of the line in the buffer, and its length in chars.
    start: usize,
    chars: usize,
    text: String,
    colors: Vec<(Range<usize>, HighlightStyle)>,
    /// Display byte of each `char` of the line (and of its end), when that is not the identity
    /// (tabs, multi-byte characters).
    cols: Option<Vec<usize>>,
}

impl Line {
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
    /// Display lines of `buffer` as of `built_at`.
    lines: Arc<Vec<Line>>,
    built_at: Revision,
    scroll: UniformListScrollHandle,
    /// A `z` was typed and its second key (`t`, `z`, `b`...) is awaited.
    z_pending: bool,
    anim: Option<ScrollAnim>,
    /// Advance of one monospace cell in pixels, measured on the first frame.
    cell_width: Option<f32>,
}

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
            lines: Arc::default(),
            built_at: buffer.revision(),
            buffer,
            scroll: UniformListScrollHandle::new(),
            z_pending: false,
            anim: None,
            cell_width: None,
        };
        viewer.rebuild();
        viewer
    }

    /// Puts the cursor on 1-based `line` and brings it into view.
    pub fn go_to_line(&mut self, line: u32) {
        let row = (line as usize).saturating_sub(1).min(self.lines.len() - 1);
        let offset = self.buffer.line_start(row);
        self.vim.set_cursor(&self.buffer, offset);
        self.vim.handle(&mut self.buffer, Key::Char('^')); // first non-blank of the line
        self.scroll.scroll_to_item(row, ScrollStrategy::Top);
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
    pub fn save(&mut self) {
        if !self.is_dirty() {
            return;
        }
        if disk_stamp(&self.full_path) != self.disk {
            self.notice = Some("not saved: the file changed on disk".into());
            return;
        }
        match fs::write(&self.full_path, self.buffer.text()) {
            Ok(()) => {
                self.saved_at = self.buffer.revision();
                self.disk = disk_stamp(&self.full_path);
                self.notice = None;
            }
            Err(e) => self.notice = Some(format!("not saved: {e}")),
        }
    }

    /// `(first visible row, visible rows)` as of the last layout, if there was one.
    fn view_metrics(&self) -> Option<(usize, usize)> {
        let state = self.scroll.0.borrow();
        let viewport = f32::from(state.last_item_size?.item.height);
        let top = (-f32::from(state.base_handle.offset().y) / ROW_HEIGHT).max(0.) as usize;
        let rows = (viewport / ROW_HEIGHT) as usize;
        Some((top, rows.max(1)))
    }

    /// `(viewport height, current top)` in pixels, once the list has been laid out.
    fn view_pixels(&self) -> Option<(f32, f32)> {
        let state = self.scroll.0.borrow();
        let viewport = f32::from(state.last_item_size?.item.height);
        Some((viewport, -f32::from(state.base_handle.offset().y)))
    }

    /// Glides the view so `top` (in pixels, clamped to the document) is its first row.
    /// Without a layout there is nothing to glide on: `fallback` scrolls instead.
    fn glide_to(&mut self, top: impl FnOnce(f32) -> f32, fallback: impl FnOnce(&Self)) {
        let Some((viewport, current)) = self.view_pixels() else {
            return fallback(self);
        };
        let max = (self.buffer.line_count() as f32 * ROW_HEIGHT - viewport).max(0.);
        self.anim = Some(ScrollAnim {
            from: current,
            to: top(viewport).clamp(0., max),
            start: Instant::now(),
        });
    }

    /// Advances the glide one frame; asks for another while it lasts.
    pub fn tick(&mut self, window: &mut Window) {
        if self.cell_width.is_none() {
            let text_system = window.text_system();
            let size = window.text_style().font_size.to_pixels(window.rem_size());
            let id = text_system.resolve_font(&font(EDITOR_FONT));
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
        let line = self.buffer.position_of(self.vim.cursor()).line;
        let y = line as f32 * ROW_HEIGHT;
        self.glide_to(
            |viewport| match strategy {
                ScrollStrategy::Center => y - (viewport - ROW_HEIGHT) / 2.,
                ScrollStrategy::Bottom => y + ROW_HEIGHT - viewport,
                _ => y,
            },
            |this| this.scroll.scroll_to_item_strict(line, strategy),
        );
    }

    /// `Ctrl-D` / `Ctrl-U`: the cursor and the view move half a page.
    fn scroll_half_page(&mut self, down: bool) {
        let (top, rows) = self.view_metrics().unwrap_or((0, 20));
        let half = (rows / 2).max(1);
        let line = self.buffer.position_of(self.vim.cursor()).line;
        let last = self.buffer.line_count().saturating_sub(1);
        let line = if down {
            (line + half).min(last)
        } else {
            line.saturating_sub(half)
        };
        self.vim.move_to_line(&self.buffer, line);
        let shift = half as f32 * ROW_HEIGHT;
        let top = top as f32 * ROW_HEIGHT;
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
        self.vim.handle(&mut self.buffer, key);
        if self.buffer.revision() != self.built_at {
            self.rebuild();
        }
        let line = self.buffer.position_of(self.vim.cursor()).line;
        self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
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
        let line = self.buffer.position_of(start).line;
        self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
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

    /// Re-highlights the whole text. Fine for ordinary files; large ones will need a debounce.
    fn rebuild(&mut self) {
        let spans = self
            .language
            .map(|language| highlight(language, &self.buffer.text()))
            .unwrap_or_default();
        self.lines = Arc::new(build_lines(&self.buffer, &spans));
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
        (position.line, map[byte]..map[end].max(map[byte] + 1))
    }

    pub fn render(&self) -> Div {
        let lines = self.lines.clone();
        let digits = lines.len().to_string().len();
        let (cursor_line, cursor_cell) = self.cursor_cell();
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
        let cell_width = self.cell_width.unwrap_or(8.);
        let list = uniform_list("viewer", lines.len(), move |range, _, _| {
            range
                .map(|row| {
                    let line = &lines[row];
                    let text = line.text.clone();
                    let mut colors = line.colors.clone();
                    if let Some(matches) = &matches {
                        let first = matches.partition_point(|m| m.end <= line.start);
                        for found in &matches[first..] {
                            if found.start >= line.start + line.chars {
                                break;
                            }
                            let from = found.start.max(line.start) - line.start;
                            let to = found.end.min(line.start + line.chars) - line.start;
                            let color = if Some(found.start) == current_match {
                                0xffc766
                            } else {
                                0xfff1a8
                            };
                            let range = line.display_byte(from)..line.display_byte(to);
                            colors = paint(&colors, range, rgb(color).into());
                        }
                    }
                    if let Some(range) = selection.and_then(|s| s.on_line(row, line)) {
                        colors = paint(&colors, range, rgb(0xcfe3ff).into());
                    }
                    // The caret is drawn on the cell grid, so it is as wide on a space or
                    // a thin letter as on any other character.
                    let caret = (row == cursor_line).then(|| {
                        let column = text[..cursor_cell.start.min(text.len())].chars().count();
                        let color = if insert { 0xb3d4ff } else { 0xb0b0b0 };
                        let gutter = 12. * digits as f32 + 16.;
                        div()
                            .absolute()
                            .top_0()
                            .left(px(gutter + column as f32 * cell_width))
                            .w(px(cell_width))
                            .h(px(ROW_HEIGHT))
                            .bg(rgba(color << 8 | 0x99))
                    });
                    div()
                        .relative()
                        .flex()
                        .h(px(ROW_HEIGHT))
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(12. * digits as f32 + 16.))
                                .pr_3()
                                .text_right()
                                .text_color(rgb(0x888888))
                                .child((row + 1).to_string()),
                        )
                        .child(StyledText::new(text).with_highlights(colors))
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
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(rgb(0x555555))
                    .child(self.path.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(EDITOR_FONT)
                    .whitespace_nowrap()
                    .child(list),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .gap_3()
                    .text_color(rgb(0x555555))
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
    if range.is_empty() {
        return colors.to_vec();
    }
    let with_bg = |style: HighlightStyle| HighlightStyle {
        background_color: Some(bg),
        ..style
    };
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

fn style(class: Class) -> HighlightStyle {
    let color = match class {
        Class::Keyword => 0xa626a4,
        Class::String => 0x50a14f,
        Class::Number | Class::Constant => 0xb76b01,
        Class::Comment => 0x9a9a9a,
        Class::Function => 0x4078f2,
        Class::Type | Class::Tag => 0xc18401,
        Class::Property | Class::Attribute => 0x986801,
        Class::Variable => 0xe45649,
        Class::Operator | Class::Punctuation => 0x555555,
    };
    HighlightStyle {
        color: Some(rgb(color).into()),
        ..Default::default()
    }
}

/// Splits the text into display lines (tabs expanded) with their colors, in line coordinates.
fn build_lines(buffer: &Buffer, spans: &[Span]) -> Vec<Line> {
    let mut next_span = 0;
    (0..buffer.line_count())
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
            Line {
                start: buffer.line_start(index),
                chars: buffer.line_len(index),
                text,
                colors,
                cols,
            }
        })
        .collect()
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
        let lines = build_lines(&buffer, &spans);
        assert_eq!(lines[0].colors[0].0, 0..4);
        assert_eq!(lines[1].colors[0].0, 0..5);
        assert!(lines[1].colors.len() == 1);
    }

    fn temp_file(name: &str, content: &str) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("zide-viewer-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), content).unwrap();
        (dir, "a.txt".into())
    }

    fn type_keys(viewer: &mut Viewer, keys: &str) {
        for c in keys.chars() {
            viewer.handle_key(Key::Char(c));
        }
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
        }
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
}
