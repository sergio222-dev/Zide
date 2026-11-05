//! Where a motion goes. Motions are pure functions of the text and the cursor.

use zide_text::Buffer;

pub(crate) struct Motion {
    pub target: usize,
    /// Operators act on whole lines (`j`, `k`, `G`, `gg`).
    pub linewise: bool,
    /// The character at `target` belongs to the operated text (`e`, `$`).
    pub inclusive: bool,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// The last line that has text: a trailing line break does not start another line.
pub(crate) fn last_line(buffer: &Buffer) -> usize {
    let lines = buffer.line_count();
    if lines > 1 && buffer.line_len(lines - 1) == 0 {
        lines - 2
    } else {
        lines - 1
    }
}

pub(crate) fn line_end(buffer: &Buffer, line: usize) -> usize {
    buffer.line_start(line) + buffer.line_len(line)
}

pub(crate) fn first_non_blank(buffer: &Buffer, line: usize) -> usize {
    let start = buffer.line_start(line);
    let blanks = buffer
        .line(line)
        .chars()
        .take_while(|c| c.is_whitespace())
        .count();
    start + blanks.min(buffer.line_len(line).saturating_sub(1))
}

/// `key` as a motion from `from`, `count` times (`None` when it was not typed).
/// `operator` says an operator waits for it, which changes how a few motions end.
pub(crate) fn motion(
    buffer: &Buffer,
    key: char,
    from: usize,
    want_column: usize,
    count: Option<usize>,
    operator: bool,
) -> Option<Motion> {
    let n = count.unwrap_or(1);
    let position = buffer.position_of(from);
    let last = last_line(buffer);
    let charwise = |target| Motion {
        target,
        linewise: false,
        inclusive: false,
    };
    let at_column = |line: usize| {
        let length = buffer.line_len(line);
        let max = if operator {
            length
        } else {
            length.saturating_sub(1)
        };
        buffer.line_start(line) + want_column.min(max)
    };
    Some(match key {
        'h' => charwise(from - n.min(position.column)),
        'l' => {
            let length = buffer.line_len(position.line);
            let max = if operator {
                length
            } else {
                length.saturating_sub(1)
            };
            charwise(from + n.min(max.saturating_sub(position.column)))
        }
        '0' => charwise(buffer.line_start(position.line)),
        '^' => charwise(first_non_blank(buffer, position.line)),
        '$' => {
            let line = (position.line + n - 1).min(last);
            let end = line_end(buffer, line);
            Motion {
                target: if operator {
                    end
                } else {
                    end.saturating_sub(1).max(buffer.line_start(line))
                },
                linewise: false,
                inclusive: false,
            }
        }
        'j' | 'k' => {
            let line = if key == 'j' {
                position.line + n
            } else {
                position.line.checked_sub(n)?
            };
            if line > last {
                return None;
            }
            Motion {
                target: at_column(line),
                linewise: true,
                inclusive: false,
            }
        }
        'G' => {
            let line = count.map_or(last, |c| (c - 1).min(last));
            Motion {
                target: first_non_blank(buffer, line),
                linewise: true,
                inclusive: false,
            }
        }
        // `gg`
        'g' => {
            let line = count.map_or(0, |c| (c - 1).min(last));
            Motion {
                target: first_non_blank(buffer, line),
                linewise: true,
                inclusive: false,
            }
        }
        'w' => charwise(word_forward(buffer, from, n, operator)),
        'b' => charwise(word_backward(buffer, from, n)),
        'e' => Motion {
            target: word_end(buffer, from, n),
            linewise: false,
            inclusive: true,
        },
        _ => return None,
    })
}

fn class_at(buffer: &Buffer, offset: usize) -> Option<Class> {
    buffer.char_at(offset).map(class)
}

pub(crate) fn word_forward(buffer: &Buffer, from: usize, count: usize, operator: bool) -> usize {
    let mut p = from;
    for _ in 0..count {
        if let Some(run) = class_at(buffer, p).filter(|c| *c != Class::Space) {
            while class_at(buffer, p) == Some(run) {
                p += 1;
            }
        }
        // spaces and line breaks; an empty line is a word of its own
        while let Some(c) = buffer.char_at(p).filter(|c| c.is_whitespace()) {
            p += 1;
            if c == '\n' && buffer.char_at(p) == Some('\n') {
                break;
            }
        }
    }
    if operator && buffer.position_of(p).line > buffer.position_of(from).line {
        // the last word of a line ends there; the operator must not eat the line break
        while p > from && buffer.char_at(p - 1).is_some_and(char::is_whitespace) {
            p -= 1;
        }
    }
    p.min(buffer.len())
}

pub(crate) fn word_backward(buffer: &Buffer, from: usize, count: usize) -> usize {
    let mut p = from;
    for _ in 0..count {
        if p == 0 {
            break;
        }
        p -= 1;
        while p > 0
            && buffer.char_at(p).is_some_and(char::is_whitespace)
            && !(buffer.char_at(p) == Some('\n') && buffer.char_at(p - 1) == Some('\n'))
        {
            p -= 1;
        }
        let run = class_at(buffer, p);
        while p > 0 && class_at(buffer, p - 1) == run && run != Some(Class::Space) {
            p -= 1;
        }
    }
    p
}

pub(crate) fn word_end(buffer: &Buffer, from: usize, count: usize) -> usize {
    let mut p = from;
    for _ in 0..count {
        p += 1;
        while buffer.char_at(p).is_some_and(char::is_whitespace) {
            p += 1;
        }
        let run = class_at(buffer, p);
        while run.is_some() && class_at(buffer, p + 1) == run {
            p += 1;
        }
    }
    p.min(buffer.len().saturating_sub(1))
}
