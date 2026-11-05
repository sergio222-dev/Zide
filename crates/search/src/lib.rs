mod files;
mod text;

pub use files::{FileHit, FileQuery, search_files, search_files_cancellable};
pub use text::{TextHit, TextQuery, TextSearchError, search_text};
