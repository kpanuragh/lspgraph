//! Lazily explore a codebase's call graph through any LSP server.

pub mod cache;
pub mod error;
pub mod config;
pub mod engine;
pub mod graph;
pub mod server;
pub mod transport;
pub mod symbols;
pub mod readiness;

#[cfg(all(test, unix))]
mod testutil;

pub use error::{Error, Result};
