//! Port of IntelliJ's `AllOccurrencesMatcherTest`.

use zide_matcher::{AllOccurrencesMatcher, MatchedFragment, Matcher, MatchingMode, NoopLayout};

fn matcher(pattern: &str, mode: MatchingMode) -> AllOccurrencesMatcher {
    AllOccurrencesMatcher::new(pattern, mode, "", &NoopLayout)
}

fn frags(pairs: &[(usize, usize)]) -> Option<Vec<MatchedFragment>> {
    Some(
        pairs
            .iter()
            .map(|&(s, e)| MatchedFragment::new(s, e))
            .collect(),
    )
}

#[test]
fn simple_case() {
    let m = matcher("*fooBar", MatchingMode::IgnoreCase);
    assert_eq!(m.match_fragments("fooBarFooBar"), frags(&[(0, 6), (6, 12)]));
    assert_eq!(
        m.match_fragments("fooBarFooBuzzBar"),
        frags(&[(0, 6), (6, 9), (13, 16)])
    );
    assert_eq!(m.match_fragments("fooBarBuzzFoo"), frags(&[(0, 6)]));
    assert_eq!(
        m.match_fragments("fooBarBuzzFooBuzzBar"),
        frags(&[(0, 6), (10, 13), (17, 20)])
    );
    assert_eq!(
        m.match_fragments("fooBuzzFooBuzzBar"),
        frags(&[(0, 3), (14, 17)])
    );
}

#[test]
fn case_insensitive() {
    let m = matcher("*foo", MatchingMode::IgnoreCase);
    assert_eq!(
        m.match_fragments("fooFooFOO"),
        frags(&[(0, 3), (3, 6), (6, 9)])
    );
    assert_eq!(m.match_fragments("FooBarfooBar"), frags(&[(0, 3), (6, 9)]));
}

#[test]
fn case_sensitive() {
    let m = matcher("*Foo", MatchingMode::MatchCase);
    assert_eq!(m.match_fragments("FooBarFooBar"), frags(&[(0, 3), (6, 9)]));
    assert_eq!(m.match_fragments("FooBarfooBar"), frags(&[(0, 3)]));
    assert_eq!(m.match_fragments("fooBarFOOBar"), None);
}

#[test]
fn first_letter_sensitive() {
    let lower = matcher("*foo", MatchingMode::FirstLetter);
    assert_eq!(lower.match_fragments("fooBarfooBar"), frags(&[(0, 3)]));
    assert_eq!(lower.match_fragments("FooBarFooBar"), None);

    let upper = matcher("*Foo", MatchingMode::FirstLetter);
    assert_eq!(
        upper.match_fragments("FooBarFooBar"),
        frags(&[(0, 3), (6, 9)])
    );
    assert_eq!(upper.match_fragments("fooBarfooBar"), None);
}

#[test]
fn first_letter_sensitive_with_wildcard_leading_space() {
    let lower = matcher(" foo", MatchingMode::FirstLetter);
    assert_eq!(lower.match_fragments("fooBarfoo"), frags(&[(0, 3)]));
    assert_eq!(lower.match_fragments("FooBarFoo"), None);

    let upper = matcher(" Foo", MatchingMode::FirstLetter);
    assert_eq!(upper.match_fragments("FooBarFoo"), frags(&[(0, 3), (6, 9)]));
    assert_eq!(upper.match_fragments("fooBarfoo"), None);
}
