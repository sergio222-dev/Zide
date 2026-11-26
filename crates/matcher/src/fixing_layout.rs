use crate::fragment::MatchedFragment;
use crate::matcher::Matcher;
use crate::minuscule::{MatchingMode, MinusculeMatcher};
use crate::name_util::is_letter;

/// Converts a char typed in a wrong keyboard layout to the same physical key in another layout.
pub trait KeyboardLayoutConverter {
    /// `None` when the char has no equivalent.
    fn convert(&self, c: char) -> Option<char>;
}

/// Disables layout fixing.
pub struct NoopLayout;

impl KeyboardLayoutConverter for NoopLayout {
    fn convert(&self, _: char) -> Option<char> {
        None
    }
}

/// Russian (ЙЦУКЕН) to English (QWERTY).
pub struct RussianToEnglish;

impl KeyboardLayoutConverter for RussianToEnglish {
    fn convert(&self, c: char) -> Option<char> {
        const LL: [(char, char); 33] = [
            ('й', 'q'),
            ('ц', 'w'),
            ('у', 'e'),
            ('к', 'r'),
            ('е', 't'),
            ('н', 'y'),
            ('г', 'u'),
            ('ш', 'i'),
            ('щ', 'o'),
            ('з', 'p'),
            ('х', '['),
            ('ъ', ']'),
            ('ф', 'a'),
            ('ы', 's'),
            ('в', 'd'),
            ('а', 'f'),
            ('п', 'g'),
            ('р', 'h'),
            ('о', 'j'),
            ('л', 'k'),
            ('д', 'l'),
            ('ж', ';'),
            ('э', '\''),
            ('я', 'z'),
            ('ч', 'x'),
            ('с', 'c'),
            ('м', 'v'),
            ('и', 'b'),
            ('т', 'n'),
            ('ь', 'm'),
            ('б', ','),
            ('ю', '.'),
            ('.', '/'),
        ];
        LL.iter().find(|&&(from, _)| from == c).map(|&(_, to)| to)
    }
}

/// Pattern with every char converted, when the pattern has letters and all of them are non-ASCII.
pub fn fix_layout(pattern: &str, converter: &impl KeyboardLayoutConverter) -> Option<String> {
    let mut has_letters = false;
    for c in pattern.chars().filter(|&c| is_letter(c)) {
        has_letters = true;
        if c.is_ascii() {
            return None;
        }
    }
    has_letters.then(|| {
        pattern
            .chars()
            .map(|c| converter.convert(c).unwrap_or(c))
            .collect()
    })
}

/// Matches the pattern as typed; when that finds nothing, matches it again as if typed
/// in the other keyboard layout.
#[derive(Debug, Clone)]
pub struct FixingLayoutMatcher {
    main: MinusculeMatcher,
    fallback: Option<MinusculeMatcher>,
}

impl FixingLayoutMatcher {
    pub fn new(
        pattern: &str,
        mode: MatchingMode,
        hard_separators: &str,
        converter: &impl KeyboardLayoutConverter,
    ) -> Self {
        let fallback = fix_layout(pattern, converter)
            .filter(|fixed| fixed != pattern)
            .map(|fixed| MinusculeMatcher::new(&fixed, mode, hard_separators));
        Self {
            main: MinusculeMatcher::new(pattern, mode, hard_separators),
            fallback,
        }
    }

    pub fn pattern(&self) -> String {
        self.main.pattern()
    }

    /// The main matcher's fragments when it matched something, or there is no fallback.
    fn main_fragments(&self, name: &[char]) -> (Option<Vec<MatchedFragment>>, bool) {
        let main = self.main.match_chars(name);
        let use_main = main.as_ref().is_some_and(|f| !f.is_empty()) || self.fallback.is_none();
        (main, use_main)
    }
}

impl Matcher for FixingLayoutMatcher {
    fn match_chars(&self, name: &[char]) -> Option<Vec<MatchedFragment>> {
        let (main, use_main) = self.main_fragments(name);
        match &self.fallback {
            Some(fallback) if !use_main => fallback.match_chars(name),
            _ => main,
        }
    }

    fn degree_for_fragments(
        &self,
        name: &[char],
        value_start_case_match: bool,
        fragments: Option<&[MatchedFragment]>,
    ) -> i32 {
        let (_, use_main) = self.main_fragments(name);
        match &self.fallback {
            Some(fallback) if !use_main => {
                fallback.degree_for_fragments(name, value_start_case_match, fragments)
            }
            _ => self
                .main
                .degree_for_fragments(name, value_start_case_match, fragments),
        }
    }

    fn matches(&self, name: &str) -> bool {
        let chars: Vec<char> = name.chars().collect();
        self.main.match_chars(&chars).is_some()
            || self
                .fallback
                .as_ref()
                .is_some_and(|f| f.match_chars(&chars).is_some())
    }
}
