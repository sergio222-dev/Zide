use crate::fixing_layout::{FixingLayoutMatcher, KeyboardLayoutConverter};
use crate::fragment::MatchedFragment;
use crate::matcher::Matcher;
use crate::minuscule::MatchingMode;

/// A [`FixingLayoutMatcher`] that returns every match in the name, not just the first one.
#[derive(Debug, Clone)]
pub struct AllOccurrencesMatcher {
    delegate: FixingLayoutMatcher,
}

impl AllOccurrencesMatcher {
    pub fn new(
        pattern: &str,
        mode: MatchingMode,
        hard_separators: &str,
        converter: &impl KeyboardLayoutConverter,
    ) -> Self {
        Self {
            delegate: FixingLayoutMatcher::new(pattern, mode, hard_separators, converter),
        }
    }

    pub fn pattern(&self) -> String {
        self.delegate.pattern()
    }
}

impl Matcher for AllOccurrencesMatcher {
    fn match_chars(&self, name: &[char]) -> Option<Vec<MatchedFragment>> {
        let first = self.delegate.match_chars(name);
        if first.as_ref().is_none_or(|f| f.is_empty()) {
            return first;
        }
        let mut all = Vec::new();
        let mut last_offset = 0;
        let mut current = first;
        while let Some(found) = current.filter(|f| !f.is_empty()) {
            let shifted = found
                .iter()
                .map(|f| MatchedFragment::new(f.start + last_offset, f.end + last_offset));
            all.extend(shifted);
            last_offset = all.last().map_or(last_offset, |f| f.end);
            current = self.delegate.match_chars(&name[last_offset..]);
        }
        Some(all)
    }

    fn degree_for_fragments(
        &self,
        name: &[char],
        value_start_case_match: bool,
        fragments: Option<&[MatchedFragment]>,
    ) -> i32 {
        self.delegate
            .degree_for_fragments(name, value_start_case_match, fragments)
    }
}
