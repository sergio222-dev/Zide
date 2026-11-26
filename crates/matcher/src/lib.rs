//! Fuzzy name matcher, a port of IntelliJ's `MinusculeMatcher`.
//!
//! Offsets in [`MatchedFragment`] are indices into the name's `char`s
//! (Unicode scalar values), not byte offsets.

mod all_occurrences;
mod fixing_layout;
mod fragment;
mod matcher;
mod minuscule;
pub mod name_util;

pub use all_occurrences::AllOccurrencesMatcher;
pub use fixing_layout::{
    FixingLayoutMatcher, KeyboardLayoutConverter, NoopLayout, RussianToEnglish, fix_layout,
};
pub use fragment::MatchedFragment;
pub use matcher::Matcher;
pub use minuscule::{MatchingMode, MinusculeMatcher, is_start_match};
