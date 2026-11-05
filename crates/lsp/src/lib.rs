//! Client of the Language Server Protocol: the semantic layer (ADR layer 2).

mod connection;
mod framing;

pub use connection::{Connection, Error};
