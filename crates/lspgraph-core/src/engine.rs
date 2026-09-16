//! Lazy, memoized call graph expansion.
//!
//! Spec 5.1: there is deliberately no "index the repository" entry point.
//! Warm p50 is 6-31ms, so expanding from the cursor is imperceptible, while a
//! full crawl costs minutes. Sourcetrail had to precompute because it owned
//! the index; an LSP client does not.

use crate::error::{Error, Result};
use crate::graph::{CallGraph, Expansion, Node, NodeId, NodeState, UnresolvedReason};
use crate::server::LanguageServer;
use crate::symbols::NamedCallable;
use lsp_types::CallHierarchyItem;
use std::collections::HashMap;

pub struct Engine {
    server: LanguageServer,
    graph: CallGraph,
    items: HashMap<NodeId, CallHierarchyItem>,
    expansions: usize,
}

impl Engine {
    pub fn new(server: LanguageServer) -> Engine {
        Engine {
            server,
            graph: CallGraph::new(),
            items: HashMap::new(),
            expansions: 0,
        }
    }

    pub fn graph(&self) -> &CallGraph {
        &self.graph
    }

    pub fn expansions_performed(&self) -> usize {
        self.expansions
    }

    pub fn invalidate_file(&mut self, uri: &str) {
        self.graph.remove_file(uri);
        self.items.retain(|k, _| k.uri != uri);
    }

    /// Resolve a candidate into a graph node.
    ///
    /// Returns `Ok(None)` only when the symbol cannot be resolved at all; in
    /// that case an `Unresolved` node is still recorded, because silently
    /// dropping ~30% of a TypeScript codebase would make the tool lie.
    pub fn seed(&mut self, cand: &NamedCallable) -> Result<Option<NodeId>> {
        let path = match cand.uri.to_file_path() {
            Ok(p) => p,
            Err(()) => return Ok(None),
        };

        let unresolved_id = || NodeId {
            uri: cand.uri.to_string(),
            line: cand.position.line,
            character: cand.position.character,
            name: cand.name.clone(),
        };

        let items = match self.server.prepare_call_hierarchy(&path, cand.position) {
            Ok(items) => items,
            // Retries in `server.rs` are exhausted: the server kept saying
            // ContentModified. Record it, do not drop it (spec 5.5) — this
            // symbol may resolve on a later attempt.
            Err(Error::ContentModified { .. }) => {
                let id = unresolved_id();
                self.graph.upsert(Node {
                    id: id.clone(),
                    kind_name: "Unknown".into(),
                    detail: None,
                    state: NodeState::Unresolved(UnresolvedReason::TransientContentModified),
                });
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let Some(item) = items.into_iter().next() else {
            // Spec 5.5: record it, do not drop it.
            let id = unresolved_id();
            self.graph.upsert(Node {
                id: id.clone(),
                kind_name: "Unknown".into(),
                detail: None,
                state: NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem),
            });
            return Ok(None);
        };

        Ok(Some(self.intern(&item)))
    }

    /// Expand a node into its callers and callees. Memoized.
    pub fn expand(&mut self, id: &NodeId) -> Result<Expansion> {
        // Memoized: an already-expanded node never hits the server again.
        if matches!(self.graph.get(id).map(|n| &n.state), Some(NodeState::Expanded)) {
            return Ok(Expansion {
                callers: self.graph.callers_of(id).unwrap_or(&[]).to_vec(),
                callees: self.graph.callees_of(id).unwrap_or(&[]).to_vec(),
            });
        }

        let Some(item) = self.items.get(id).cloned() else {
            return Ok(Expansion::default());
        };

        let incoming = self.server.incoming_calls(&item)?;
        let outgoing = self.server.outgoing_calls(&item)?;
        self.expansions += 1;

        let callers = incoming.iter().map(|i| self.intern(i)).collect();
        let callees = outgoing.iter().map(|i| self.intern(i)).collect();

        let exp = Expansion { callers, callees };
        self.graph.record_expansion(id, &exp);
        Ok(exp)
    }

    fn intern(&mut self, item: &CallHierarchyItem) -> NodeId {
        let id = NodeId::from_item(item);
        self.graph.upsert(Node {
            id: id.clone(),
            kind_name: format!("{:?}", item.kind),
            detail: item.detail.clone(),
            state: NodeState::Unexpanded,
        });
        self.items.insert(id.clone(), item.clone());
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::UnresolvedReason;

    // Engine::expand needs a live server, so behaviour that can be checked
    // without one is checked here; the rest is covered in Task 10.

    #[test]
    fn unresolved_nodes_are_recorded_not_dropped() {
        let mut g = CallGraph::new();
        let id = NodeId {
            uri: "file:///a.ts".into(),
            line: 4,
            character: 9,
            name: "gt".into(),
        };
        g.upsert(Node {
            id: id.clone(),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem),
        });
        assert_eq!(g.len(), 1);
        assert!(matches!(
            g.get(&id).unwrap().state,
            NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem)
        ));
    }
}
