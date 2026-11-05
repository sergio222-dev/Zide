//! File editor view: line numbers, syntax colors, a block cursor and Vim keys.

use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use gpui_kit::*;
use zide_syntax::{Class, LanguageId, Span, highlight, language_for_path};
use zide_text::{Buffer, Position, Revision};
use zide_vim::{Key, Mode, Vim};

/// Files larger than this are not opened.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TAB_WIDTH: usize = 4;
const ROW_HEIGHT: f32 = 20.;

struct Line {
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

pub struct Viewer {
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

    pub fn handle_key(&mut self, key: Key) {
        if !self.editable
            && !matches!(
                key,
                Key::Char('h' | 'j' | 'k' | 'l' | 'w' | 'b' | 'e' | 'G' | 'g' | '0' | '$' | '^')
            )
        {
            return; // an error message is not a document
        }
        self.vim.handle(&mut self.buffer, key);
        if self.buffer.revision() != self.built_at {
            self.rebuild();
        }
        let line = self.buffer.position_of(self.vim.cursor()).line;
        self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
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
        let list = uniform_list("viewer", lines.len(), move |range, _, _| {
            range
                .map(|row| {
                    let line = &lines[row];
                    let mut text = line.text.clone();
                    let mut colors = line.colors.clone();
                    if let Some(range) = selection.and_then(|s| s.on_line(row, line)) {
                        colors = paint(&colors, range, rgb(0xcfe3ff).into());
                    }
                    if row == cursor_line {
                        if cursor_cell.start >= text.len() {
                            text.push(' '); // the cursor at the end of the line needs a cell
                        }
                        let bg = if insert { 0xb3d4ff } else { 0xb0b0b0 };
                        colors = paint(&colors, cursor_cell.clone(), rgb(bg).into());
                    }
                    div()
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
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .flex()
            .flex_col()
            .size_full()
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
                    .font_family("monospace")
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
                    .child(self.vim.pending())
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
            Line { text, colors, cols }
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
}
