//! Sticky scroll: the declarations (class, function...) that have scrolled out of view above
//! the code on screen stay pinned at the top, outermost first.

/// The lines a declaration spans, 0-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    pub start: usize,
    pub end: usize,
}

/// How many declarations stay pinned at most (the innermost ones when there are more).
pub const MAX_STICKY: usize = 5;

/// The header line of each declaration to pin above the row `top` (the first one on screen),
/// outermost first. `scopes` are in source order, so a nested one comes after its parent;
/// `line_of_row` maps a row to its buffer line (folded regions make them differ).
///
/// A pinned header covers the rows under it, so what counts is the line that ends up right
/// below the pinned ones: the count is refined until it settles.
pub fn sticky_headers(
    scopes: &[Scope],
    line_of_row: impl Fn(usize) -> usize,
    top: usize,
) -> Vec<usize> {
    let mut count = 0;
    let mut headers = Vec::new();
    for _ in 0..=MAX_STICKY {
        let line = line_of_row(top + count);
        headers = scopes
            .iter()
            .filter(|s| s.start < line && line <= s.end)
            .map(|s| s.start)
            .collect();
        if headers.len() > MAX_STICKY {
            headers.drain(..headers.len() - MAX_STICKY);
        }
        if headers.len() == count {
            break;
        }
        count = headers.len();
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASS: Scope = Scope { start: 0, end: 100 };
    const METHOD: Scope = Scope { start: 10, end: 30 };
    const OTHER: Scope = Scope { start: 40, end: 60 };

    fn headers(scopes: &[Scope], top: usize) -> Vec<usize> {
        sticky_headers(scopes, |row| row, top)
    }

    #[test]
    fn nothing_is_pinned_while_the_declaration_is_on_screen() {
        assert!(headers(&[CLASS, METHOD], 0).is_empty());
    }

    #[test]
    fn the_class_stays_once_its_line_scrolls_out() {
        assert_eq!(headers(&[CLASS, METHOD], 1), vec![0]);
        assert_eq!(headers(&[CLASS, METHOD, OTHER], 35), vec![0]);
    }

    #[test]
    fn entering_a_method_adds_it_below_the_class() {
        assert_eq!(headers(&[CLASS, METHOD], 20), vec![0, 10]);
    }

    #[test]
    fn a_method_whose_header_is_the_top_row_is_pinned_in_place() {
        // its row is covered by the pinned copy of itself, so nothing jumps
        assert_eq!(headers(&[CLASS, METHOD], 10), vec![0, 10]);
    }

    #[test]
    fn a_method_stops_being_pinned_after_it_ends() {
        assert_eq!(headers(&[CLASS, METHOD], 31), vec![0]);
        assert_eq!(headers(&[CLASS, METHOD], 101), Vec::<usize>::new());
    }

    #[test]
    fn at_most_the_innermost_few_are_kept() {
        let nested: Vec<Scope> = (0..8).map(|i| Scope { start: i, end: 100 }).collect();
        assert_eq!(headers(&nested, 50), vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn folded_rows_are_mapped_to_their_lines() {
        // rows 0..=2 show lines 0, 1, 50: lines 2..=49 are folded away
        let line_of_row = |row: usize| if row < 2 { row } else { row + 48 };
        assert_eq!(sticky_headers(&[CLASS], line_of_row, 2), vec![0]);
    }
}
