//! The minimap: the whole file at the right of the editor, each line a row of colored bars (its
//! tokens), with the lines that changed, have errors or hold the cursor painted across.

use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

use gpui_kit::*;

use crate::{rows::Rows, theme};

/// The width of the minimap, and the padding before the first column.
pub const WIDTH: f32 = 100.;
const PADDING: f32 = 4.;
/// A column of text is one pixel wide; a row is at most this tall, fewer when the file is long.
const MAX_PITCH: f32 = 2.;

/// A run of text of one color, in columns of the line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub col: usize,
    pub len: usize,
    pub color: Hsla,
}

/// How tall each row is so that `rows` fit in `height`.
pub fn pitch(rows: usize, height: f32) -> f32 {
    if rows == 0 {
        return MAX_PITCH;
    }
    (height / rows as f32).min(MAX_PITCH)
}

/// The row at `y` pixels from the top of the minimap.
pub fn row_at(y: f32, pitch: f32, rows: usize) -> usize {
    ((y.max(0.) / pitch) as usize).min(rows.saturating_sub(1))
}

/// The bars of a line: its words, each in the color `colors` gives it (`default` where none
/// does). `colors` is sorted by position and does not overlap.
pub fn bars(text: &str, colors: &[(Range<usize>, Hsla)], default: Hsla) -> Vec<Bar> {
    let columns = (WIDTH - PADDING) as usize;
    let mut found: Vec<Bar> = Vec::new();
    let mut next = 0;
    for (col, (byte, c)) in text.char_indices().enumerate().take(columns) {
        if c.is_whitespace() {
            continue;
        }
        while next < colors.len() && colors[next].0.end <= byte {
            next += 1;
        }
        let color = colors
            .get(next)
            .filter(|(range, _)| range.start <= byte)
            .map_or(default, |(_, color)| *color);
        match found.last_mut() {
            Some(last) if last.col + last.len == col && last.color == color => last.len += 1,
            _ => found.push(Bar { col, len: 1, color }),
        }
    }
    found
}

/// A row painted across, whatever its text.
pub struct Mark {
    pub row: usize,
    pub color: Hsla,
}

pub struct Minimap {
    pub rows: Arc<Rows>,
    /// The bars of a buffer line.
    pub bars: Box<dyn Fn(usize) -> Vec<Bar>>,
    /// In painting order: what is painted last shows.
    pub marks: Vec<Mark>,
    pub scroll: UniformListScrollHandle,
    pub row_height: f32,
    /// Where the editor is, for how much of the file it shows.
    pub editor: Rc<Cell<Bounds<Pixels>>>,
    /// Where the minimap is; it sets it as it is laid out.
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// Scrolls the editor so that the row under `y` is in the middle.
fn jump(
    scroll: &UniformListScrollHandle,
    bounds: &Cell<Bounds<Pixels>>,
    rows: usize,
    y: Pixels,
    window: &mut Window,
) {
    let bounds = bounds.get();
    let pitch = pitch(rows, f32::from(bounds.size.height));
    let row = row_at(f32::from(y - bounds.origin.y), pitch, rows);
    scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
    window.refresh();
}

impl Minimap {
    pub fn render(self) -> impl IntoElement {
        let (rows, scroll, bounds) = (self.rows.len(), self.scroll.clone(), self.bounds.clone());
        let (on_move_scroll, on_move_bounds) = (scroll.clone(), bounds.clone());
        let laid_out = bounds.clone();
        div()
            .id("minimap")
            .w(px(WIDTH))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(theme::BORDER))
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                jump(&scroll, &bounds, rows, event.position.y, window);
            })
            .on_mouse_move(move |event, window, _| {
                if event.pressed_button == Some(MouseButton::Left) {
                    jump(
                        &on_move_scroll,
                        &on_move_bounds,
                        rows,
                        event.position.y,
                        window,
                    );
                }
            })
            .child(
                canvas(
                    move |bounds, _, _| laid_out.set(bounds),
                    move |bounds, _, window, _| self.paint(bounds, window),
                )
                .size_full(),
            )
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let rows = self.rows.len();
        let height = f32::from(bounds.size.height);
        let pitch = pitch(rows, height);
        let (left, top) = (
            f32::from(bounds.origin.x) + PADDING,
            f32::from(bounds.origin.y),
        );
        let quad = |window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: Hsla| {
            let area = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
            window.paint_quad(fill(area, color));
        };
        let across = |window: &mut Window, row: usize, color: Hsla| {
            let y = top + row as f32 * pitch;
            quad(
                window,
                f32::from(bounds.origin.x),
                y,
                WIDTH,
                pitch.max(2.),
                color,
            );
        };
        // what is visible: a long file has more rows than pixels, so a row is drawn per pixel
        let bar = (pitch * 0.75).max(1.);
        let step = pitch.max(1.);
        let used = (rows as f32 * pitch).min(height);
        let mut y = 0.;
        while y < used {
            let row = row_at(y, pitch, rows);
            for found in (self.bars)(self.rows.line(row)) {
                quad(
                    window,
                    left + found.col as f32,
                    top + y,
                    found.len as f32,
                    bar,
                    found.color,
                );
            }
            y += step;
        }
        for mark in &self.marks {
            across(window, mark.row.min(rows.saturating_sub(1)), mark.color);
        }
        // the part of the file the editor shows
        let seen = f32::from(self.editor.get().size.height) / self.row_height;
        let first = -f32::from(self.scroll.0.borrow().base_handle.offset().y) / self.row_height;
        let shown = (seen * pitch).max(8.);
        let at = (first * pitch).clamp(0., (used - shown).max(0.));
        let light: Hsla = rgb(0xffffff).into();
        quad(
            window,
            f32::from(bounds.origin.x),
            top + at,
            WIDTH,
            shown,
            Hsla { a: 0.1, ..light },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(n: u32) -> Hsla {
        rgb(n).into()
    }

    #[::core::prelude::v1::test]
    fn a_short_file_has_tall_rows_and_a_long_one_squeezes_them() {
        assert_eq!(pitch(10, 800.), MAX_PITCH);
        assert_eq!(pitch(4000, 800.), 0.2);
        assert_eq!(pitch(0, 800.), MAX_PITCH);
    }

    #[::core::prelude::v1::test]
    fn a_y_is_the_row_it_falls_on_and_never_past_the_file() {
        assert_eq!(row_at(5., 2., 100), 2);
        assert_eq!(row_at(-3., 2., 100), 0);
        assert_eq!(row_at(900., 2., 100), 99);
    }

    #[::core::prelude::v1::test]
    fn words_are_bars_in_their_color_and_blanks_are_gaps() {
        let (red, other) = (color(0xff0000), color(0x00ff00));
        let found = bars("  fn main", &[(2..4, red)], other);
        assert_eq!(
            found,
            [
                Bar {
                    col: 2,
                    len: 2,
                    color: red
                },
                Bar {
                    col: 5,
                    len: 4,
                    color: other
                },
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn a_word_that_changes_color_is_split() {
        let (a, b) = (color(0xff0000), color(0x0000ff));
        let found = bars("abcd", &[(0..2, a), (2..4, b)], b);
        assert_eq!(
            found,
            [
                Bar {
                    col: 0,
                    len: 2,
                    color: a
                },
                Bar {
                    col: 2,
                    len: 2,
                    color: b
                },
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn long_lines_are_cut_at_the_width() {
        let text = "x".repeat(500);
        let found = bars(&text, &[], color(0));
        assert_eq!(found.iter().map(|b| b.len).sum::<usize>(), 96);
    }
}
