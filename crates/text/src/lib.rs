//! Text buffer: a rope with revisions, undo/redo and stable anchors.
//!
//! Offsets are `char` indices unless a name says otherwise. Every mutation bumps the
//! [`Revision`]; consumers that computed something at an older revision must drop it.

mod buffer;
mod find;
mod history;

pub use buffer::{Anchor, Buffer, ByteEdit, Edit, Position, Revision};
pub use find::{FindQuery, Finder};
