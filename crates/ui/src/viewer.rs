//! File editor view: line numbers, syntax colors, a block cursor and Vim keys.

use std::{fs, ops::Range, path::Path, sync::Arc};

use gpui_kit::*;
use zide_syntax::{Class, LanguageId, Span, highlight, language_for_path};
use zide_text::{Buffer, Revision};
use zide_vim::{Key, Mode, Vim};

/// Files larger than this are not opened.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TAB_WIDTH: usize = 4;
const ROW_HEIGHT: f32 = 20.;

struct Line {
    text: String,
    colors: Vec<(Range<usize>, HighlightStyle)>,
}

pub struct Viewer {
    path: String,
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
        let text = match fs::metadata(&full) {
            Ok(m) if m.len() > MAX_BYTES => Err(format!("{relative}: file too large to open")),
            Ok(_) => fs::read_to_string(&full).map_err(|e| format!("{relative}: {e}")),
            Err(e) => Err(format!("{relative}: {e}")),
        };
        let text = text.unwrap_or_else(|message| message);
        let buffer = Buffer::new(&text);
        let language = language_for_path(relative);
        let mut viewer = Self {
            path: relative.to_string(),
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

    pub fn handle_key(&mut self, key: Key) {
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
        let list = uniform_list("viewer", lines.len(), move |range, _, _| {
            range
                .map(|row| {
                    let line = &lines[row];
                    let (text, colors) = if row == cursor_line {
                        with_cursor(line, cursor_cell.clone(), insert)
                    } else {
                        (line.text.clone(), line.colors.clone())
                    };
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
                    })
                    .child(self.vim.pending())
                    .child(div().ml_auto().child({
                        let at = self.buffer.position_of(self.vim.cursor());
                        format!("{}:{}", at.line + 1, at.column + 1)
                    })),
            )
    }
}

/// The text of `line` with the cursor cell painted over its colors. At the end of the line
/// the cell is an extra space.
fn with_cursor(
    line: &Line,
    cell: Range<usize>,
    insert: bool,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let mut text = line.text.clone();
    if cell.start >= text.len() {
        text.push(' ');
    }
    let paint = |mut style: HighlightStyle| {
        style.background_color = Some(rgb(if insert { 0xb3d4ff } else { 0xc8c8c8 }).into());
        style
    };
    let mut colors = Vec::new();
    let mut under = HighlightStyle::default();
    let mut right = None;
    for (range, style) in &line.colors {
        if range.end <= cell.start || range.start >= cell.end {
            colors.push((range.clone(), *style));
            continue;
        }
        under = *style;
        if range.start < cell.start {
            colors.push((range.start..cell.start, *style));
        }
        if range.end > cell.end {
            right = Some((cell.end..range.end, *style));
        }
    }
    colors.push((cell.start..cell.end, paint(under)));
    colors.extend(right);
    colors.sort_by_key(|(range, _)| range.start);
    (text, colors)
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
            Line { text, colors }
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
}
