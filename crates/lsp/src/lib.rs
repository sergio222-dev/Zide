//! Client of the Language Server Protocol: the semantic layer (ADR layer 2).

mod client;
mod connection;
mod framing;
mod text;

pub use client::{Client, ServerSpec, server_for};
pub use connection::{Connection, Error};
pub use lsp_types::{Location, Position, Range};
pub use text::{char_column, path_to_uri, uri_to_path, utf16_column};
