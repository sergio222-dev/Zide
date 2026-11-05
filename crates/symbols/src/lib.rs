//! Symbols (functions, types, ...) extracted with tree-sitter, and fuzzy search over them.
//!
//! A language is a tree-sitter grammar plus its tags query; supporting another one means adding
//! an entry to [`languages`].

mod extract;
mod index;

pub use extract::{Symbol, SymbolKind, extract_symbols, language_for_path};
pub use index::{SymbolEntry, SymbolHit, SymbolIndex, search_symbols};
