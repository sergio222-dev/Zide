//! Find (and replace) inside one text: plain or regular expression, case and word options.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// More matches than this are not collected: the panel could not show them anyway.
const MAX_MATCHES: usize = 100_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindQuery {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Debug, Clone)]
pub struct Finder {
    regex: Regex,
    is_regex: bool,
}

impl FindQuery {
    /// `Ok(None)` for an empty pattern, `Err` with a readable message for an invalid one.
    pub fn compile(&self) -> Result<Option<Finder>, String> {
        if self.pattern.is_empty() {
            return Ok(None);
        }
        let mut source = if self.regex {
            self.pattern.clone()
        } else {
            regex::escape(&self.pattern)
        };
        if self.whole_word {
            source = format!(r"\b(?:{source})\b");
        }
        RegexBuilder::new(&source)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map(|regex| {
                Some(Finder {
                    regex,
                    is_regex: self.regex,
                })
            })
            .map_err(|e| {
                // the first line of the error is the one that says what is wrong
                e.to_string()
                    .lines()
                    .last()
                    .unwrap_or("invalid pattern")
                    .trim_start_matches("error: ")
                    .to_string()
            })
    }
}

impl Finder {
    /// Byte ranges of the non-empty matches in `text`, in order.
    pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
        self.regex
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .take(MAX_MATCHES)
            .map(|m| m.range())
            .collect()
    }

    /// What `template` stands for when it replaces the match at `range` of `text`: in regular
    /// expression mode `$1` and `${name}` refer to groups; otherwise it is taken literally.
    pub fn replacement(&self, text: &str, range: Range<usize>, template: &str) -> String {
        if !self.is_regex {
            return template.to_string();
        }
        let mut out = String::new();
        match self.regex.captures_at(text, range.start) {
            Some(captures) if captures.get(0).is_some_and(|m| m.range() == range) => {
                captures.expand(template, &mut out)
            }
            _ => out.push_str(template),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, text: &str, case: bool, word: bool, regex: bool) -> Vec<Range<usize>> {
        FindQuery {
            pattern: pattern.into(),
            case_sensitive: case,
            whole_word: word,
            regex,
        }
        .compile()
        .unwrap()
        .unwrap()
        .find_all(text)
    }

    #[test]
    fn plain_search_ignores_case_by_default_and_escapes_the_pattern() {
        assert_eq!(
            find("a.c", "A.C abc a.c", false, false, false),
            [0..3, 8..11]
        );
        assert_eq!(find("a.c", "A.C abc a.c", true, false, false), [8..11]);
    }

    #[test]
    fn whole_word_needs_word_boundaries() {
        assert_eq!(
            find("cat", "cat concat cat_x cat", false, true, false),
            [0..3, 17..20]
        );
    }

    #[test]
    fn regex_mode_and_line_anchors() {
        assert_eq!(find(r"\d+", "a1 b22", false, false, true), [1..2, 4..6]);
        assert_eq!(find("^b", "a\nb\nbb", false, false, true), [2..3, 4..5]);
    }

    #[test]
    fn empty_matches_are_dropped() {
        assert!(find("x*", "abc", false, false, true).is_empty());
    }

    #[test]
    fn empty_and_invalid_patterns() {
        assert!(FindQuery::default().compile().unwrap().is_none());
        let invalid = FindQuery {
            pattern: "(".into(),
            regex: true,
            ..Default::default()
        };
        assert!(invalid.compile().is_err());
        // the same text is fine as a plain search
        let plain = FindQuery {
            pattern: "(".into(),
            ..Default::default()
        };
        assert!(plain.compile().is_ok());
    }

    #[test]
    fn replacement_expands_groups_only_in_regex_mode() {
        let regex = FindQuery {
            pattern: r"(\w+)=(\w+)".into(),
            regex: true,
            ..Default::default()
        }
        .compile()
        .unwrap()
        .unwrap();
        let text = "a=b";
        assert_eq!(regex.replacement(text, 0..3, "$2=$1"), "b=a");
        let plain = FindQuery {
            pattern: "a=b".into(),
            ..Default::default()
        }
        .compile()
        .unwrap()
        .unwrap();
        assert_eq!(plain.replacement(text, 0..3, "$2=$1"), "$2=$1");
    }
}
