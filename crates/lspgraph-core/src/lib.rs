//! Lazily explore a codebase's call graph through any LSP server.

pub mod error;
pub mod config;
pub mod server;
pub mod transport;
pub mod symbols;
pub mod readiness;

pub use error::{Error, Result};
