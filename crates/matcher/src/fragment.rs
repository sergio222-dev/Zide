/// A matched range of a name, as a half-open interval `[start, end)` of `char` indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchedFragment {
    pub start: usize,
    pub end: usize,
}

impl MatchedFragment {
    pub fn new(start: usize, end: usize) -> Self {
        assert!(start <= end, "start={start} > end={end}");
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}
