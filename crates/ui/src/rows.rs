//! Which buffer lines the list shows. Without folds every line is a row; a folded region
//! takes its hidden lines out, so rows and lines stop being the same number.

use std::ops::Range;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rows {
    /// The buffer line shown by each row, ascending.
    lines: Vec<usize>,
}

impl Rows {
    /// Every one of `count` lines is a row.
    #[cfg(test)]
    pub fn all(count: usize) -> Self {
        Self {
            lines: (0..count).collect(),
        }
    }

    /// The `count` lines except those in `hidden` (sorted, disjoint ranges).
    pub fn hiding(count: usize, hidden: &[Range<usize>]) -> Self {
        let mut hidden = hidden.iter().peekable();
        let lines = (0..count)
            .filter(|line| {
                while hidden.next_if(|r| r.end <= *line).is_some() {}
                !hidden.peek().is_some_and(|r| r.contains(line))
            })
            .collect();
        Self { lines }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// The line `row` shows. Rows past the end show the last line.
    pub fn line(&self, row: usize) -> usize {
        self.lines
            .get(row)
            .or(self.lines.last())
            .copied()
            .unwrap_or(0)
    }

    /// The row that shows `line`, or the nearest row above it when the line is hidden (the
    /// row of the fold that hides it).
    pub fn row_of(&self, line: usize) -> usize {
        self.lines
            .partition_point(|&shown| shown <= line)
            .saturating_sub(1)
    }

    /// False when `line` is folded away.
    pub fn shows(&self, line: usize) -> bool {
        self.lines.binary_search(&line).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_folds_rows_are_lines() {
        let rows = Rows::all(5);
        assert_eq!(rows.len(), 5);
        assert!((0..5).all(|n| rows.line(n) == n && rows.row_of(n) == n));
    }

    #[test]
    fn hidden_lines_leave_no_row() {
        let rows = Rows::hiding(8, &[2..4, 6..7]);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            (0..5).map(|r| rows.line(r)).collect::<Vec<_>>(),
            [0, 1, 4, 5, 7]
        );
    }

    #[test]
    fn a_hidden_line_belongs_to_the_row_of_its_fold() {
        let rows = Rows::hiding(8, &[2..4, 7..8]);
        assert!(rows.shows(1) && !rows.shows(2) && !rows.shows(3) && rows.shows(4));
        assert_eq!(rows.row_of(1), 1); // the header of the fold
        assert_eq!(rows.row_of(3), 1);
        assert_eq!(rows.row_of(4), 2);
        assert_eq!(rows.row_of(7), 4); // folded into line 6's row
    }

    #[test]
    fn rows_past_the_end_show_the_last_line() {
        assert_eq!(Rows::all(3).line(9), 2);
        assert_eq!(Rows::default().line(0), 0);
        assert_eq!(Rows::default().row_of(4), 0);
    }
}
