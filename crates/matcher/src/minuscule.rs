use crate::fragment::MatchedFragment;
use crate::name_util::{
    eq_ignore_case, is_digit, is_letter_or_digit, is_word_start, next_word, to_lower, to_upper,
};

/// Camel-hump matching is worse than O(n), so longer patterns fall back to a prefix match.
const MAX_CAMEL_HUMP_MATCHING_LENGTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchingMode {
    /// Pattern chars match regardless of case.
    IgnoreCase,
    /// The first letter of the pattern must match the case of the first letter of the name.
    FirstLetter,
    /// Every pattern char must match case exactly.
    MatchCase,
}

type Fragments = Vec<MatchedFragment>;

/// Tells whether a name matches a pattern. Allows lowercase camel-hump matching.
///
/// `*` and space in the pattern are wildcards. Fragments returned by
/// [`match_fragments`](Self::match_fragments) are sorted by start offset.
#[derive(Debug, Clone)]
pub struct MinusculeMatcher {
    pattern: Vec<char>,
    mode: MatchingMode,
    hard_separators: Vec<char>,
    is_lower: Vec<bool>,
    is_upper: Vec<bool>,
    is_word_sep: Vec<bool>,
    upper: Vec<char>,
    lower: Vec<char>,
    mixed_case: bool,
    has_separators: bool,
    has_dots: bool,
    /// Pairs `[lower, upper]` for every non-wildcard pattern char.
    meaningful: Vec<char>,
}

fn is_wildcard_char(c: char) -> bool {
    c == ' ' || c == '*'
}

fn is_word_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '_' | '-' | ':' | '+' | '.')
}

impl MinusculeMatcher {
    /// `hard_separators`: lowercase humps don't match across these chars;
    /// the pattern needs an explicit uppercase letter or the same separator.
    pub fn new(pattern: &str, mode: MatchingMode, hard_separators: &str) -> Self {
        let pattern: Vec<char> = pattern
            .strip_suffix("* ")
            .unwrap_or(pattern)
            .chars()
            .collect();
        let n = pattern.len();
        let mut m = Self {
            mode,
            hard_separators: hard_separators.chars().collect(),
            is_lower: vec![false; n],
            is_upper: vec![false; n],
            is_word_sep: vec![false; n],
            upper: vec!['\0'; n],
            lower: vec!['\0'; n],
            mixed_case: false,
            has_separators: false,
            has_dots: false,
            meaningful: Vec::new(),
            pattern: Vec::new(),
        };

        let mut seen_non_wildcard = false;
        let mut seen_lower = false;
        let mut seen_upper_not_after_wildcard = false;
        for (k, &c) in pattern.iter().enumerate() {
            let sep = is_word_separator(c);
            let up = c.is_uppercase();
            if c.is_lowercase() {
                seen_lower = true;
            }
            if c == '.' {
                m.has_dots = true;
            }
            if seen_non_wildcard && up {
                seen_upper_not_after_wildcard = true;
            }
            if !is_wildcard_char(c) {
                seen_non_wildcard = true;
                m.meaningful.push(to_lower(c));
                m.meaningful.push(to_upper(c));
            }
            if seen_non_wildcard && sep {
                m.has_separators = true;
            }
            m.is_word_sep[k] = sep;
            m.is_upper[k] = up;
            m.is_lower[k] = c.is_lowercase();
            m.upper[k] = to_upper(c);
            m.lower[k] = to_lower(c);
        }
        m.mixed_case = seen_lower && seen_upper_not_after_wildcard;
        m.pattern = pattern;
        m
    }

    pub fn pattern(&self) -> String {
        self.pattern.iter().collect()
    }

    pub fn matches(&self, name: &str) -> bool {
        self.match_fragments(name).is_some()
    }

    /// Fragments of `name` matched by the pattern, or `None` when it doesn't match.
    pub fn match_fragments(&self, name: &str) -> Option<Fragments> {
        let name: Vec<char> = name.chars().collect();
        self.match_chars(&name)
    }

    pub fn match_chars(&self, name: &[char]) -> Option<Fragments> {
        if name.len() < self.meaningful.len() / 2 {
            return None;
        }
        if self.pattern.len() > MAX_CAMEL_HUMP_MATCHING_LENGTH {
            return self.match_by_substring(name);
        }
        if !name_contains_all_meaningful_chars_in_order(name, &self.meaningful) {
            return None;
        }
        // fragments are collected back to front
        self.match_wildcards(name, 0, Some(0)).map(|mut r| {
            r.reverse();
            r
        })
    }

    pub fn is_start_match(&self, name: &str) -> bool {
        self.match_fragments(name)
            .is_some_and(|f| is_start_match(&f))
    }

    /// Higher is a better match. `i32::MIN` when `name` doesn't match.
    pub fn matching_degree(&self, name: &str) -> i32 {
        self.matching_degree_with_case(name, false)
    }

    pub fn matching_degree_with_case(&self, name: &str, value_start_case_match: bool) -> i32 {
        let name: Vec<char> = name.chars().collect();
        let fragments = self.match_chars(&name);
        self.calculate_score(&name, value_start_case_match, fragments.as_deref())
    }

    fn calculate_score(
        &self,
        name: &[char],
        value_start_case_match: bool,
        fragments: Option<&[MatchedFragment]>,
    ) -> i32 {
        let Some(fragments) = fragments else {
            return i32::MIN;
        };
        let Some(first) = fragments.first() else {
            return 0;
        };

        let start_match = first.start == 0;
        let valued_start_match = start_match && value_start_case_match;

        let mut matching_case = 0;
        let mut p: Option<usize> = None; // last matched pattern index
        let mut skipped_humps = 0;
        let mut next_hump_start = 0;
        let mut hump_start_matched_upper = false;

        for (fi, range) in fragments.iter().enumerate() {
            for i in range.start..range.end {
                let after_gap = i == range.start && fi > 0;
                let mut is_hump_start = false;
                while next_hump_start <= i {
                    if next_hump_start == i {
                        is_hump_start = true;
                    } else if after_gap {
                        skipped_humps += 1;
                    }
                    next_hump_start =
                        if next_hump_start < name.len() && is_digit(name[next_hump_start]) {
                            next_hump_start + 1 // each digit is a separate hump
                        } else {
                            next_word(name, next_hump_start)
                        };
                }

                let c = name[i];
                let from = p.map_or(0, |p| p + 1);
                let Some(found) =
                    (from..self.pattern.len()).find(|&k| eq_ignore_case(self.pattern[k], c))
                else {
                    break;
                };
                p = Some(found);

                if is_hump_start {
                    hump_start_matched_upper = c == self.pattern[found] && self.is_upper[found];
                }
                matching_case += self.evaluate_case_matching(
                    valued_start_match,
                    found,
                    hump_start_matched_upper,
                    i,
                    after_gap,
                    is_hump_start,
                    c,
                );
            }
        }

        let start_index = first.start;
        let after_separator = name[..start_index.min(name.len())]
            .iter()
            .any(|c| self.hard_separators.contains(c));
        let word_start = start_index == 0
            || (is_word_start(name, start_index) && !is_word_start(name, start_index - 1));
        let final_match = fragments.last().is_some_and(|f| f.end == name.len());

        (if word_start { 1000 } else { 0 }) + matching_case
            - fragments.len() as i32
            - skipped_humps * 10
            + if after_separator { 0 } else { 2 }
            + i32::from(start_match)
            + i32::from(final_match)
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_case_matching(
        &self,
        valued_start_match: bool,
        pi: usize,
        hump_start_matched_upper: bool,
        name_index: usize,
        after_gap: bool,
        is_hump_start: bool,
        name_char: char,
    ) -> i32 {
        if after_gap && is_hump_start && self.is_lower[pi] {
            -10 // a hump nothing in the pattern asked for
        } else if name_char == self.pattern[pi] {
            if self.is_upper[pi] {
                50 // the user pressed Shift on purpose
            } else if name_index == 0 && valued_start_match {
                150
            } else if is_hump_start {
                1
            } else {
                0
            }
        } else if is_hump_start || (self.is_lower[pi] && hump_start_matched_upper) {
            -1 // case mismatch on a hump start, or lowercase non-hump on an uppercase hump
        } else {
            0
        }
    }

    fn match_by_substring(&self, name: &[char]) -> Option<Fragments> {
        let meaningful_count = self.meaningful.len() / 2;
        if name.len() < meaningful_count {
            return None;
        }
        if self.pattern.first() == Some(&'*') {
            for i in 0..=name.len() - meaningful_count {
                let len = self.meaningful_chars_match_at(name, i);
                if len != 0 {
                    return Some(vec![MatchedFragment::new(i, i + len)]);
                }
            }
            return None;
        }
        match self.meaningful_chars_match_at(name, 0) {
            0 => None,
            len => Some(vec![MatchedFragment::new(0, len)]),
        }
    }

    fn meaningful_chars_match_at(&self, name: &[char], name_index: usize) -> usize {
        let mut mi = 0;
        let mut pos = name_index;
        while pos < name.len() && mi + 1 < self.meaningful.len() {
            let c = name[pos];
            if c == self.meaningful[mi] || c == self.meaningful[mi + 1] {
                mi += 2;
                pos += 1;
            } else if is_wildcard_char(c) {
                pos += 1;
            } else {
                return 0;
            }
        }
        pos - name_index
    }

    /// After a wildcard (`*` or space), finds the first non-wildcard pattern char
    /// in the name from `name_index` and tries to match a fragment from it.
    fn match_wildcards(
        &self,
        name: &[char],
        mut pi: usize,
        name_index: Option<usize>,
    ) -> Option<Fragments> {
        let ni = name_index?;
        if !self.is_wildcard(pi) {
            return if pi == self.pattern.len() {
                Some(Vec::new())
            } else {
                self.match_fragment(name, pi, ni)
            };
        }

        loop {
            pi += 1;
            if !self.is_wildcard(pi) {
                break;
            }
        }

        if pi == self.pattern.len() {
            // the trailing space matches if the pattern ends with the last word part,
            // or only its first hump char
            return if self.is_trailing_space_pattern()
                && ni != name.len()
                && (pi < 2 || !self.is_upper_or_digit(pi - 2))
            {
                let space = name.get(ni..)?.iter().position(|&c| c == ' ')? + ni;
                Some(vec![MatchedFragment::new(space, space + 1)])
            } else {
                Some(Vec::new())
            };
        }

        let next = self.find_next_pattern_char_occurrence(name, ni, pi);
        self.match_skipping_words(name, pi, next, true)
    }

    fn is_trailing_space_pattern(&self) -> bool {
        self.pattern.last() == Some(&' ')
    }

    fn is_upper_or_digit(&self, pi: usize) -> bool {
        self.is_upper[pi] || is_digit(self.pattern[pi])
    }

    /// Enumerates the name places that could match the pattern at `pi`
    /// and tries to match a fragment at each of them.
    fn match_skipping_words(
        &self,
        name: &[char],
        pi: usize,
        name_index: Option<usize>,
        allow_special_chars: bool,
    ) -> Option<Fragments> {
        let mut max_found = 0;
        let mut cur = name_index;
        while let Some(ni) = cur {
            let fragment_len = if self.seems_like_fragment_start(name, pi, ni) {
                self.max_matching_fragment(name, pi, ni)
            } else {
                0
            };

            // Match the rest only if no fragment of the same or bigger length was seen:
            // that attempt would have failed the same way with even less name left.
            if fragment_len > max_found
                || (ni + fragment_len == name.len() && self.is_trailing_space_pattern())
            {
                if !self.is_middle_match(name, pi, ni) {
                    max_found = fragment_len;
                }
                if let Some(r) = self.match_inside_fragment(name, pi, ni, fragment_len) {
                    return Some(r);
                }
            }
            let next = self.find_next_pattern_char_occurrence(name, ni + 1, pi);
            cur = if allow_special_chars {
                next
            } else {
                self.check_for_special_chars(name, ni + 1, next, pi)
            };
        }
        None
    }

    fn find_next_pattern_char_occurrence(
        &self,
        name: &[char],
        start_at: usize,
        pi: usize,
    ) -> Option<usize> {
        if !self.pattern_char_before(pi, '*') && !self.is_word_sep[pi] {
            self.index_of_word_start(name, pi, start_at)
        } else {
            self.index_of_ignore_case(name, start_at, pi)
        }
    }

    fn check_for_special_chars(
        &self,
        name: &[char],
        start: usize,
        end: Option<usize>,
        pi: usize,
    ) -> Option<usize> {
        let end = end?;
        let range = start..end.min(name.len());
        let slice = |r: std::ops::Range<usize>| name.get(r).unwrap_or(&[]);
        // humps may match across " ()", lowercase chars may not
        if !self.has_separators
            && !self.mixed_case
            && slice(range.clone())
                .iter()
                .any(|c| self.hard_separators.contains(c))
        {
            return None;
        }
        // if the user typed a dot, don't skip other dots between humps;
        // one pattern dot may match several name dots
        if self.has_dots && !self.pattern_char_before(pi, '.') && slice(range).contains(&'.') {
            return None;
        }
        Some(end)
    }

    fn seems_like_fragment_start(&self, name: &[char], pi: usize, next: usize) -> bool {
        // uppercase matches either uppercase or a word start
        !self.is_upper[pi]
            || name[next].is_uppercase()
            || is_word_start(name, next)
            // uppercase may match lowercase if the whole prefix is uppercase and the mode allows it
            || (!self.mixed_case && self.mode != MatchingMode::MatchCase)
    }

    fn char_equals(&self, pattern_char: char, pi: usize, c: char, ignore_case: bool) -> bool {
        pattern_char == c || (ignore_case && (self.lower[pi] == c || self.upper[pi] == c))
    }

    fn match_fragment(&self, name: &[char], pi: usize, ni: usize) -> Option<Fragments> {
        match self.max_matching_fragment(name, pi, ni) {
            0 => None,
            len => self.match_inside_fragment(name, pi, ni, len),
        }
    }

    fn max_matching_fragment(&self, name: &[char], pi: usize, ni: usize) -> usize {
        if !self.is_first_char_matching(name, ni, pi) {
            return 0;
        }
        let ignore_case = self.mode != MatchingMode::MatchCase;
        let mut i = 1;
        while ni + i < name.len() && pi + i < self.pattern.len() {
            let name_char = name[ni + i];
            if !self.char_equals(self.pattern[pi + i], pi + i, name_char, ignore_case) {
                if self.is_skipping_digit_between_pattern_digits(pi + i, name_char) {
                    return 0;
                }
                break;
            }
            i += 1;
        }
        i
    }

    fn is_skipping_digit_between_pattern_digits(&self, pi: usize, name_char: char) -> bool {
        is_digit(self.pattern[pi]) && is_digit(self.pattern[pi - 1]) && is_digit(name_char)
    }

    /// `fragment_len` is the longest fragment matching pattern and name here.
    fn match_inside_fragment(
        &self,
        name: &[char],
        pi: usize,
        ni: usize,
        fragment_len: usize,
    ) -> Option<Fragments> {
        // exact middle matches need at least 3 chars, to prevent too many irrelevant matches
        let min_fragment = if self.is_middle_match(name, pi, ni) {
            3
        } else {
            1
        };
        self.improve_camel_humps(name, pi, ni, fragment_len, min_fragment)
            .or_else(|| self.find_longest_matching_prefix(name, pi, ni, fragment_len, min_fragment))
    }

    fn is_middle_match(&self, name: &[char], pi: usize, ni: usize) -> bool {
        self.pattern_char_before(pi, '*')
            && !self.is_wildcard(pi + 1)
            && is_letter_or_digit(name[ni])
            && !is_word_start(name, ni)
    }

    fn find_longest_matching_prefix(
        &self,
        name: &[char],
        pi: usize,
        ni: usize,
        fragment_len: usize,
        min_fragment: usize,
    ) -> Option<Fragments> {
        if pi + fragment_len >= self.pattern.len() {
            return Some(vec![MatchedFragment::new(ni, ni + fragment_len)]);
        }

        // The longest fragment may not let the rest of the pattern match: try shorter ones.
        let mut i = fragment_len;
        while i >= min_fragment || (i > 0 && self.is_wildcard(pi + i)) {
            let ranges = if self.is_wildcard(pi + i) {
                self.match_wildcards(name, pi + i, Some(ni + i))
            } else {
                let next = self.find_next_pattern_char_occurrence(name, ni + i + 1, pi + i);
                let next = self.check_for_special_chars(name, ni + i, next, pi + i);
                next.and_then(|n| self.match_skipping_words(name, pi + i, Some(n), false))
            };
            if let Some(r) = ranges {
                return Some(append_range(r, ni, i));
            }
            i -= 1;
        }
        None
    }

    /// With pattern "CU" and name "CurrentUser" the prefix "Cu" already matches,
    /// but a later uppercase "U" gives a better match.
    fn improve_camel_humps(
        &self,
        name: &[char],
        pi: usize,
        ni: usize,
        max_fragment: usize,
        min_fragment: usize,
    ) -> Option<Fragments> {
        for i in min_fragment..max_fragment {
            if self.is_upper[pi + i] && self.pattern[pi + i] != name[ni + i] {
                let next_word_start = self.index_of_word_start(name, pi + i, ni + i);
                if let Some(r) = self.match_wildcards(name, pi + i, next_word_start) {
                    return Some(append_range(r, ni, i));
                }
            }
        }
        None
    }

    fn is_first_char_matching(&self, name: &[char], ni: usize, pi: usize) -> bool {
        let Some(&name_char) = name.get(ni) else {
            return false;
        };
        let ignore_case = self.mode != MatchingMode::MatchCase;
        if !self.char_equals(self.pattern[pi], pi, name_char, ignore_case) {
            return false;
        }
        !(self.mode == MatchingMode::FirstLetter
            && (pi == 0 || (pi == 1 && self.is_wildcard(0)))
            && self.has_case(pi)
            && self.is_upper[pi] != name[0].is_uppercase())
    }

    fn has_case(&self, pi: usize) -> bool {
        self.is_upper[pi] || self.is_lower[pi]
    }

    fn is_wildcard(&self, pi: usize) -> bool {
        self.pattern.get(pi).is_some_and(|&c| is_wildcard_char(c))
    }

    /// Whether the pattern char just before `pi` is `c`.
    fn pattern_char_before(&self, pi: usize, c: char) -> bool {
        pi > 0 && self.pattern.get(pi - 1) == Some(&c)
    }

    fn index_of_word_start(&self, name: &[char], pi: usize, start_from: usize) -> Option<usize> {
        let p = self.pattern[pi];
        if start_from >= name.len()
            || (self.mixed_case && self.is_lower[pi] && !(pi > 0 && self.is_word_sep[pi - 1]))
        {
            return None;
        }
        let is_special_symbol = !is_letter_or_digit(p);
        let mut i = start_from;
        loop {
            i = self.index_of_ignore_case(name, i, pi)?;
            if is_special_symbol || is_word_start(name, i) {
                return Some(i);
            }
            i += 1;
        }
    }

    fn index_of_ignore_case(&self, name: &[char], from: usize, pi: usize) -> Option<usize> {
        let p = self.pattern[pi];
        let tail = name.get(from..)?;
        let found = if p.is_ascii() {
            let (pu, pl) = (self.upper[pi], self.lower[pi]);
            tail.iter().position(|&c| c == pu || c == pl)
        } else {
            tail.iter().position(|&c| eq_ignore_case(c, p))
        };
        found.map(|k| k + from)
    }
}

/// Whether the first fragment starts at the beginning of the name (or there is none).
pub fn is_start_match(sorted_fragments: &[MatchedFragment]) -> bool {
    sorted_fragments.first().is_none_or(|f| f.start == 0)
}

/// Prepends the fragment `[from, from + length)` to `ranges` (collected back to front),
/// merging it with the last one when they touch.
fn append_range(mut ranges: Fragments, from: usize, length: usize) -> Fragments {
    match ranges.last_mut() {
        Some(last) if last.start == from + length => last.start = from,
        _ => ranges.push(MatchedFragment::new(from, from + length)),
    }
    ranges
}

fn name_contains_all_meaningful_chars_in_order(name: &[char], meaningful: &[char]) -> bool {
    let find = |c: char, from: usize| {
        name.get(from..)?
            .iter()
            .position(|&x| x == c)
            .map(|k| k + from)
    };
    let mut mi = 0;
    let mut ni = 0;
    while mi + 1 < meaningful.len() {
        if ni >= name.len() {
            return false;
        }
        let c1 = meaningful[mi];
        let i1 = find(c1, ni);
        if i1 == Some(ni) {
            // found on the very first index: there cannot be a better result
            ni += 1;
        } else {
            let c2 = meaningful[mi + 1];
            if c1 == c2 {
                // digits and symbols: lower == upper
                match i1 {
                    Some(i) => ni = i + 1,
                    None => return false,
                }
            } else {
                ni = match (i1, find(c2, ni)) {
                    (Some(a), Some(b)) => a.min(b) + 1,
                    (Some(a), None) => a + 1,
                    (None, Some(b)) => b + 1,
                    (None, None) => return false,
                };
            }
        }
        mi += 2;
    }
    true
}
