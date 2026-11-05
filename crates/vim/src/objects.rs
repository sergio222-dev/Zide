//! Text objects (`iw`, `a"`, `i(`...): the range an `i`/`a` command selects around the cursor.

use std::ops::Range;

use zide_text::Buffer;

use crate::motion::{Class, class};

/// `kind` is the key typed after `i` or `a`. `None` when there is no such object here.
pub(crate) fn text_object(
    buffer: &Buffer,
    cursor: usize,
    inner: bool,
    kind: char,
) -> Option<Range<usize>> {
    match kind {
        'w' => word(buffer, cursor, inner),
        '"' | '\'' | '`' => quoted(buffer, cursor, inner, kind),
        '(' | ')' | 'b' => enclosed(buffer, cursor, inner, '(', ')'),
        '{' | '}' | 'B' => enclosed(buffer, cursor, inner, '{', '}'),
        '[' | ']' => enclosed(buffer, cursor, inner, '[', ']'),
        '<' | '>' => enclosed(buffer, cursor, inner, '<', '>'),
        _ => None,
    }
}

/// The class of the character at `offset`; a line break belongs to no run.
fn run_class(buffer: &Buffer, offset: usize) -> Option<Class> {
    buffer.char_at(offset).filter(|&c| c != '\n').map(class)
}

fn run_around(buffer: &Buffer, at: usize) -> Option<Range<usize>> {
    let kind = run_class(buffer, at)?;
    let mut start = at;
    while start > 0 && run_class(buffer, start - 1) == Some(kind) {
        start -= 1;
    }
    let mut end = at;
    while run_class(buffer, end) == Some(kind) {
        end += 1;
    }
    Some(start..end)
}

fn is_blank(buffer: &Buffer, offset: usize) -> bool {
    matches!(buffer.char_at(offset), Some(' ' | '\t'))
}

fn word(buffer: &Buffer, cursor: usize, inner: bool) -> Option<Range<usize>> {
    let run = run_around(buffer, cursor)?;
    if inner {
        return Some(run);
    }
    let (mut start, mut end) = (run.start, run.end);
    if is_blank(buffer, cursor) {
        // `aw` on blanks: the blanks and the word after them
        if let Some(next) = run_around(buffer, end) {
            end = next.end;
        }
    } else if is_blank(buffer, end) {
        while is_blank(buffer, end) {
            end += 1;
        }
    } else {
        while start > 0 && is_blank(buffer, start - 1) {
            start -= 1;
        }
    }
    Some(start..end)
}

fn quoted(buffer: &Buffer, cursor: usize, inner: bool, quote: char) -> Option<Range<usize>> {
    let line = buffer.position_of(cursor).line;
    let start = buffer.line_start(line);
    let text: Vec<char> = buffer.line(line).chars().collect();
    let mut marks = Vec::new();
    for (i, &c) in text.iter().enumerate() {
        if c == quote && (i == 0 || text[i - 1] != '\\') {
            marks.push(start + i);
        }
    }
    let (open, close) = marks
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[open, close]| (open, close))
        .find(|&(_, close)| cursor <= close)
        .or_else(|| {
            // an odd quote (`it's 'ok'`) broke the pairing: use the nearest ones around the cursor
            let before = marks.iter().rev().find(|&&m| m < cursor)?;
            let after = marks.iter().find(|&&m| m > cursor)?;
            Some((*before, *after))
        })?;
    Some(if inner {
        open + 1..close
    } else {
        open..close + 1
    })
}

fn enclosed(
    buffer: &Buffer,
    cursor: usize,
    inner: bool,
    open: char,
    close: char,
) -> Option<Range<usize>> {
    let opening = if buffer.char_at(cursor) == Some(open) {
        cursor
    } else {
        let mut depth = 0;
        let mut p = cursor;
        loop {
            if p == 0 {
                return None;
            }
            p -= 1;
            match buffer.char_at(p)? {
                c if c == close => depth += 1,
                c if c == open && depth == 0 => break p,
                c if c == open => depth -= 1,
                _ => {}
            }
        }
    };
    let mut depth = 0;
    let mut closing = opening + 1;
    loop {
        match buffer.char_at(closing)? {
            c if c == open => depth += 1,
            c if c == close && depth == 0 => break,
            c if c == close => depth -= 1,
            _ => {}
        }
        closing += 1;
    }
    if !inner {
        return Some(opening..closing + 1);
    }
    let (mut from, mut to) = (opening + 1, closing);
    // a block opened at the end of a line and closed on a line of its own: the lines between
    if buffer.char_at(from) == Some('\n') {
        let close_line = buffer.position_of(closing).line;
        let before_close = buffer.line_start(close_line);
        if (before_close..closing).all(|p| is_blank(buffer, p)) && before_close > from {
            from += 1;
            to = before_close;
        }
    }
    Some(from..to)
}
