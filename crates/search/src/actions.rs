//! Word-based matching of command names (ADR-011).
//!
//! Unlike file and symbol search, actions are matched by whole words, not by camel-hump
//! initials, as IntelliJ does: `tog reg` finds `Toggle Regex`.

use std::ops::Range;

/// What a command is searched by: its visible name and its synonyms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionText {
    pub name: String,
    pub synonyms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionHit {
    /// Index into the searched entries.
    pub index: usize,
    pub score: i32,
    /// Matched parts of the name, as `char` ranges. Empty when only a synonym matched.
    pub fragments: Vec<Range<usize>>,
}

const NAME_SCORE: i32 = 1000;
const SYNONYM_SCORE: i32 = 500;
const EXACT_WORD_BONUS: i32 = 5;

/// Entries matching `query`, best first. Every word of the query must start a different word of
/// the name (in any order) or of one synonym. An empty query lists every entry in order.
pub fn search_actions(entries: &[ActionText], query: &str, limit: usize) -> Vec<ActionHit> {
    let query_words: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|w| w.chars().flat_map(char::to_lowercase).collect())
        .collect();

    let mut hits: Vec<ActionHit> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if query_words.is_empty() {
                return Some(ActionHit {
                    index,
                    score: 0,
                    fragments: Vec::new(),
                });
            }
            if let Some((score, fragments)) = match_words(&entry.name, &query_words) {
                return Some(ActionHit {
                    index,
                    score: NAME_SCORE + score,
                    fragments,
                });
            }
            entry
                .synonyms
                .iter()
                .filter_map(|s| match_words(s, &query_words))
                .map(|(score, _)| score)
                .max()
                .map(|score| ActionHit {
                    index,
                    score: SYNONYM_SCORE + score,
                    fragments: Vec::new(),
                })
        })
        .collect();
    if query_words.is_empty() {
        hits.truncate(limit);
        return hits; // registry order
    }
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                entries[a.index]
                    .name
                    .len()
                    .cmp(&entries[b.index].name.len())
            })
            .then_with(|| a.index.cmp(&b.index))
    });
    hits.truncate(limit);
    hits
}

/// `(score, matched char ranges)` when each query word starts a different word of `text`.
fn match_words(text: &str, query_words: &[Vec<char>]) -> Option<(i32, Vec<Range<usize>>)> {
    let words = words_of(text);
    let mut used = vec![false; words.len()];
    let mut score = 0;
    let mut fragments = Vec::new();
    for query_word in query_words {
        let (position, (start, word)) = words
            .iter()
            .enumerate()
            .find(|(i, (_, word))| !used[*i] && word.starts_with(query_word))?;
        used[position] = true;
        // earlier words are better; so are words typed in full
        score -= position as i32;
        if word.len() == query_word.len() {
            score += EXACT_WORD_BONUS;
        }
        fragments.push(*start..*start + query_word.len());
    }
    fragments.sort_by_key(|r| r.start);
    Some((score, fragments))
}

/// Alphanumeric runs of `text`, lowercased, with the `char` index where each starts.
fn words_of(text: &str) -> Vec<(usize, Vec<char>)> {
    let mut words = Vec::new();
    let mut current: Option<(usize, Vec<char>)> = None;
    for (i, c) in text.chars().enumerate() {
        if c.is_alphanumeric() {
            let word = current.get_or_insert_with(|| (i, Vec::new()));
            word.1.extend(c.to_lowercase());
        } else if let Some(word) = current.take() {
            words.push(word);
        }
    }
    words.extend(current);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<ActionText> {
        let entry = |name: &str, synonyms: &[&str]| ActionText {
            name: name.into(),
            synonyms: synonyms.iter().map(|s| s.to_string()).collect(),
        };
        vec![
            entry("Toggle Regex", &["regular expression"]),
            entry("Toggle Match Case", &["case sensitive"]),
            entry("Quit", &["exit", "close ide"]),
            entry("Find in Files", &["grep", "search text"]),
        ]
    }

    fn names(query: &str) -> Vec<String> {
        let entries = entries();
        search_actions(&entries, query, 10)
            .iter()
            .map(|h| entries[h.index].name.clone())
            .collect()
    }

    #[test]
    fn words_match_by_prefix_in_any_order() {
        assert_eq!(names("tog reg"), ["Toggle Regex"]);
        assert_eq!(names("case match"), ["Toggle Match Case"]);
    }

    #[test]
    fn camel_hump_initials_do_not_match() {
        assert_eq!(names("tr"), Vec::<String>::new());
    }

    #[test]
    fn a_word_cannot_be_used_twice() {
        assert_eq!(names("t t"), Vec::<String>::new());
        let tab = [ActionText {
            name: "Toggle Tab".into(),
            synonyms: vec![],
        }];
        assert_eq!(search_actions(&tab, "t t", 10).len(), 1);
    }

    #[test]
    fn synonyms_match_without_highlights() {
        let entries = entries();
        let hits = search_actions(&entries, "grep", 10);
        assert_eq!(entries[hits[0].index].name, "Find in Files");
        assert!(hits[0].fragments.is_empty());
    }

    #[test]
    fn name_matches_rank_before_synonym_matches() {
        // "case" is a word of one name and a synonym word of another entry
        let mut entries = entries();
        entries.push(ActionText {
            name: "Fold".into(),
            synonyms: vec!["case".into()],
        });
        let hits = search_actions(&entries, "case", 10);
        assert_eq!(entries[hits[0].index].name, "Toggle Match Case");
        assert_eq!(entries[hits[1].index].name, "Fold");
    }

    #[test]
    fn fragments_are_char_ranges_of_the_name() {
        let entries = entries();
        let hits = search_actions(&entries, "mat cas", 10);
        assert_eq!(hits[0].fragments, [7..10, 13..16]);
    }

    #[test]
    fn empty_query_lists_everything_in_order_up_to_the_limit() {
        let entries = entries();
        let hits = search_actions(&entries, "  ", 3);
        assert_eq!(hits.iter().map(|h| h.index).collect::<Vec<_>>(), [0, 1, 2]);
    }
}
