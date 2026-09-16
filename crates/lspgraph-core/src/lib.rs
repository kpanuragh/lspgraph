//! Lazily explore a codebase's call graph through any LSP server.

pub mod error;
pub mod config;
pub mod server;
pub mod transport;

pub use error::{Error, Result};
