//! The stub model: a sparse, syntactic projection of a file's syntax tree (see IntelliJ's
//! `LightStubBuilder`). Only declarations become stubs; bodies are never visited. Nothing in
//! here resolves types or references.

use std::ops::Range;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StubKind {
    Function,
    Method,
    Struct,
    Enum,
    Variant,
    Field,
    Trait,
    Impl,
    Module,
    TypeAlias,
    Const,
    Macro,
    Import,
}

impl StubKind {
    /// Kinds worth finding by name in Symbols search.
    pub fn is_searchable(self) -> bool {
        !matches!(self, Self::Impl | Self::Import)
    }
}

/// Bit set of declaration modifiers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const PUBLIC: Self = Self(1);
    pub const ASYNC: Self = Self(2);
    pub const UNSAFE: Self = Self(4);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One imported path as written in the source, e.g. `std::collections::HashMap`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportTarget {
    pub path: Box<str>,
    pub alias: Option<Box<str>>,
    /// `use path::*`.
    pub glob: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubNode {
    pub kind: StubKind,
    /// Index of the parent in [`FileStubs::nodes`]; `None` for top-level declarations.
    pub parent: Option<u32>,
    pub name: Option<Box<str>>,
    /// Bytes of the whole declaration.
    pub range: Range<u32>,
    /// Bytes of the name, empty when there is none.
    pub name_range: Range<u32>,
    /// 1-based.
    pub line: u32,
    pub modifiers: Modifiers,
    /// Declaration text up to its body (`pub fn go(&self) -> u8`), whitespace collapsed.
    pub signature: Option<Box<str>>,
    pub import: Option<ImportTarget>,
}

/// Content identity of a file when its stubs were built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub mtime_ns: u64,
    pub len: u64,
    /// xxh3 of the content: tells a `touch` from a real change.
    pub hash: u64,
}

/// All stubs of one file, in source order (parents before their children).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStubs {
    pub stamp: Stamp,
    /// [`crate::RULES_VERSION`] at build time; a different one invalidates the stubs.
    pub rules_version: u32,
    pub nodes: Vec<StubNode>,
}

impl FileStubs {
    pub fn container_name(&self, node: &StubNode) -> Option<&str> {
        self.nodes[node.parent? as usize].name.as_deref()
    }
}
