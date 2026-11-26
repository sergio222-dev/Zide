use crate::fragment::MatchedFragment;

/// A name matcher. Implementations work on `char` slices; the `&str` methods collect first.
pub trait Matcher {
    /// Fragments of `name` matched by the pattern, sorted by start; `None` when it doesn't match.
    fn match_chars(&self, name: &[char]) -> Option<Vec<MatchedFragment>>;

    /// Score of `fragments` (as returned by a match of `name`). Higher is better;
    /// `i32::MIN` when `fragments` is `None`.
    fn degree_for_fragments(
        &self,
        name: &[char],
        value_start_case_match: bool,
        fragments: Option<&[MatchedFragment]>,
    ) -> i32;

    fn matches(&self, name: &str) -> bool {
        self.match_fragments(name).is_some()
    }

    fn match_fragments(&self, name: &str) -> Option<Vec<MatchedFragment>> {
        self.match_chars(&name.chars().collect::<Vec<_>>())
    }

    fn matching_degree(&self, name: &str) -> i32 {
        self.matching_degree_with_case(name, false)
    }

    fn matching_degree_with_case(&self, name: &str, value_start_case_match: bool) -> i32 {
        let name: Vec<char> = name.chars().collect();
        let fragments = self.match_chars(&name);
        self.degree_for_fragments(&name, value_start_case_match, fragments.as_deref())
    }

    fn is_start_match(&self, name: &str) -> bool {
        self.match_fragments(name)
            .is_some_and(|f| crate::minuscule::is_start_match(&f))
    }
}
