//! The UI thread and the worker thread speak only these two types.
//!
//! Keeping them plain data — no ratatui, no language server — is what makes
//! the worker's behaviour testable without a terminal or a real server.

use lspgraph_core::graph::{Expansion, Node, NodeId};
use lspgraph_core::symbols::SymbolMatch;
use lspgraph_core::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Search(String),
    Seed(SymbolMatch),
    Expand(NodeId),
    Restart,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The server is ready. `can_search` is false when it does not advertise
    /// `workspaceSymbolProvider`.
    Ready {
        can_search: bool,
    },
    Matches(Vec<SymbolMatch>),
    /// The seeded symbol, resolved to its full `Node` (signature and all) so
    /// the interface never has to fall back to a bare name.
    Seeded(Option<Node>),
    Expanded(NodeId, ResolvedExpansion),
    /// A per-node failure. The session continues.
    Failed(NodeId, String),
    /// A recoverable failure with no particular node to blame — a failed
    /// search, or a failed seed. The session continues.
    Warning(String),
    /// The session cannot continue.
    Fatal(String),
}

/// An expansion whose endpoints have been resolved to full `Node`s, so the
/// interface can show each symbol's signature rather than just its name.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedExpansion {
    pub callers: Vec<Node>,
    pub callees: Vec<Node>,
}

/// What the worker needs from an engine. Implemented for the real
/// `lspgraph_core::engine::Engine` in Task 3, and by a stub in tests.
pub trait EngineOps: Send {
    fn can_search(&self) -> bool;
    fn search(&mut self, query: &str) -> Result<Vec<SymbolMatch>>;
    fn seed(&mut self, m: &SymbolMatch) -> Result<Option<NodeId>>;
    fn expand(&mut self, id: &NodeId) -> Result<Expansion>;
    fn node(&self, id: &NodeId) -> Option<Node>;
    fn shutdown(self: Box<Self>) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_id(name: &str) -> NodeId {
        NodeId {
            uri: "file:///a.rs".into(),
            line: 1,
            character: 3,
            name: name.into(),
        }
    }

    #[test]
    fn requests_compare_by_value() {
        assert_eq!(Request::Expand(node_id("f")), Request::Expand(node_id("f")));
        assert_ne!(Request::Expand(node_id("f")), Request::Expand(node_id("g")));
    }

    #[test]
    fn ready_carries_whether_search_is_available() {
        assert_ne!(
            Event::Ready { can_search: true },
            Event::Ready { can_search: false }
        );
    }

    #[test]
    fn a_failed_node_is_not_fatal() {
        let f = Event::Failed(node_id("f"), "boom".into());
        assert!(!matches!(f, Event::Fatal(_)));
    }
}
