//! What the editor draws over the indentation: dots on whitespace and rainbow bands.

/// Columns (chars of the display text) of the spaces to mark with a dot: the leading
/// ones, the trailing ones and the ones in a run of two or more. A single space
/// between two words stays clean.
pub fn dots(text: &str) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    let mut marked = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars[start] != ' ' {
            start += 1;
            continue;
        }
        let end = start + chars[start..].iter().take_while(|&&c| c == ' ').count();
        if end - start >= 2 || start == 0 || end == chars.len() {
            marked.extend(start..end);
        }
        start = end;
    }
    marked
}

/// The indent step of the file and, for each line, how many whole steps it is indented.
/// A blank line takes the smaller depth of its neighbours, so the bands run through it.
pub fn levels(lines: &[&str]) -> (usize, Vec<usize>) {
    let widths: Vec<Option<usize>> = lines
        .iter()
        .map(|line| {
            (!line.trim().is_empty()).then(|| line.chars().take_while(|&c| c == ' ').count())
        })
        .collect();
    let step = step(&widths);
    let own: Vec<Option<usize>> = widths.iter().map(|w| w.map(|w| w / step)).collect();

    // a blank line is as deep as the shallower of the code lines around it
    let mut before = Vec::with_capacity(own.len());
    let mut last = 0;
    for level in &own {
        before.push(last);
        last = level.unwrap_or(last);
    }
    let mut after = vec![0; own.len()];
    let mut next = 0;
    for (i, level) in own.iter().enumerate().rev() {
        after[i] = next;
        next = level.unwrap_or(next);
    }
    let levels = own
        .iter()
        .enumerate()
        .map(|(i, level)| level.unwrap_or_else(|| before[i].min(after[i])))
        .collect();
    (step, levels)
}

/// One indent level as the file already writes it: a tab when most indented lines start with
/// one, else as many spaces as its usual step. `None` when nothing is indented (or only by one
/// space, as in the ` *` of block comments), so the configured default applies.
pub fn unit(lines: &[&str]) -> Option<String> {
    let code = lines
        .iter()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('*'));
    let (mut tabs, mut spaces) = (0, 0);
    for line in code.clone() {
        match line.chars().next() {
            Some('\t') => tabs += 1,
            Some(' ') => spaces += 1,
            _ => {}
        }
    }
    if tabs > spaces {
        return Some("\t".into());
    }
    if spaces == 0 {
        return None;
    }
    let widths: Vec<Option<usize>> = code
        .map(|line| Some(line.chars().take_while(|&c| c == ' ').count()))
        .collect();
    let step = step(&widths);
    (step >= 2).then(|| " ".repeat(step))
}

const DEFAULT_STEP: usize = 4;
/// A bigger jump is a continuation or alignment, not a new level.
const MAX_STEP: usize = 8;

/// The indent increase seen most often between one code line and the next (the smaller
/// on a tie), which is how deep the file nests; a tab when nothing is indented.
fn step(widths: &[Option<usize>]) -> usize {
    let mut seen = [0usize; MAX_STEP + 1];
    let mut previous = 0;
    for width in widths.iter().flatten() {
        if *width > previous && width - previous <= MAX_STEP {
            seen[width - previous] += 1;
        }
        previous = *width;
    }
    let mut best = DEFAULT_STEP;
    let mut best_count = 0;
    for (step, &count) in seen.iter().enumerate().skip(1) {
        if count > best_count {
            best = step;
            best_count = count;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn leading_spaces_get_dots() {
        assert_eq!(dots("    a"), [0, 1, 2, 3]);
    }

    #[::core::prelude::v1::test]
    fn a_single_space_between_words_is_clean() {
        assert_eq!(dots("a b"), Vec::<usize>::new());
    }

    #[::core::prelude::v1::test]
    fn runs_and_trailing_spaces_get_dots() {
        assert_eq!(dots("a  b"), [1, 2]);
        assert_eq!(dots("a "), [1]);
        assert_eq!(dots(" a b "), [0, 4]);
    }

    #[::core::prelude::v1::test]
    fn a_whitespace_only_line_is_all_dots() {
        assert_eq!(dots("  "), [0, 1]);
        assert_eq!(dots(""), Vec::<usize>::new());
    }

    #[::core::prelude::v1::test]
    fn the_step_is_the_most_common_increase() {
        let lines = ["fn a() {", "  b;", "  if x {", "    c;", "  }", "}"];
        assert_eq!(levels(&lines), (2, vec![0, 1, 1, 2, 1, 0]));
    }

    #[::core::prelude::v1::test]
    fn without_indentation_the_step_is_a_tab() {
        assert_eq!(levels(&["a", "b"]), (4, vec![0, 0]));
    }

    #[::core::prelude::v1::test]
    fn an_odd_continuation_does_not_change_the_step() {
        let lines = [
            "a {",
            "    b(",
            "       c,",
            "    );",
            "    d {",
            "        e",
            "    }",
        ];
        assert_eq!(levels(&lines).0, 4);
    }

    #[::core::prelude::v1::test]
    fn the_unit_is_what_the_file_already_uses() {
        assert_eq!(
            unit(&["a {", "  b {", "    c", "  }", "}"]).as_deref(),
            Some("  ")
        );
        assert_eq!(unit(&["a {", "\tb", "\t\tc", "}"]).as_deref(), Some("\t"));
        assert_eq!(unit(&["a", "", "b"]), None);
        // the space before a block comment's stars is not a level
        assert_eq!(unit(&["/**", " * a", " */", "b"]), None);
    }

    #[::core::prelude::v1::test]
    fn blank_lines_take_the_smaller_neighbour() {
        let lines = ["  a", "", "  b", "", "c"];
        assert_eq!(levels(&lines), (2, vec![1, 1, 1, 0, 0]));
    }
}
