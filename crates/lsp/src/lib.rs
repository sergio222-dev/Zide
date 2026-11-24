//! Client of the Language Server Protocol: the semantic layer (ADR layer 2).

mod client;
mod completion;
mod connection;
mod framing;
mod servers;
mod settings;
mod signature;
mod text;

pub use client::{Client, ParameterHint, ServerSpec, server_for};
pub use completion::{
    Completion, CompletionItem, CompletionTrigger, parse_completion, rank, word_start,
};
pub use connection::{Connection, Error};
pub use lsp_types::{
    CompletionItemKind, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location,
    NumberOrString, Position, Range, TextEdit,
};
pub use servers::servers_dir;
pub use signature::{SignatureHint, parse_signature_help};
pub use text::{char_column, path_to_uri, uri_to_path, utf16_column};
