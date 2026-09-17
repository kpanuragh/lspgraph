//! Lazily explore a codebase's call graph through any LSP server.

pub mod cache;
pub mod config;
pub mod engine;
pub mod error;
pub mod graph;
pub mod readiness;
pub mod server;
pub mod symbols;
pub mod transport;

#[cfg(all(test, unix))]
mod testutil;

pub use error::{Error, Result};
