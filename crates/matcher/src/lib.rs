//! Fuzzy name matcher, a port of IntelliJ's `MinusculeMatcher`.
//!
//! Offsets in [`MatchedFragment`] are indices into the name's `char`s
//! (Unicode scalar values), not byte offsets.

mod fragment;
mod minuscule;
pub mod name_util;

pub use fragment::MatchedFragment;
pub use minuscule::{MatchingMode, MinusculeMatcher};
