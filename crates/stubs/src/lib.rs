//! Stubs: the declarations of every project file (functions, types, imports, ...), extracted
//! with tree-sitter and searched by name.
//!
//! This is IntelliJ's stub layer: a sparse, purely syntactic projection of each file, kept
//! apart from the parser the editor uses. It never resolves types or references.
//! A language is a tree-sitter grammar plus a rules table; see `extract`.

mod cache;
mod extract;
mod go;
mod js;
mod model;
mod python;
mod rust;
mod store;

pub use cache::StubCache;
pub use extract::{RULES_VERSION, extract_nodes, extract_nodes_in, language_for_path};
pub use model::{FileStubs, ImportTarget, Modifiers, Stamp, StubKind, StubNode};
pub use store::{StubStore, Symbol, SymbolHit, SymbolRef, search_symbols};
