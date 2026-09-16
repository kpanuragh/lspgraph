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

    /// Borrow the underlying server, for capabilities the engine does not wrap.
    pub fn server(&self) -> &LanguageServer {
        &self.server
    }

    /// Shut the language server down, consuming the engine.
    ///
    /// Without this, `LanguageServer::shutdown` is unreachable for anything
    /// held by an `Engine` — which is every real caller.
    pub fn shutdown(self) -> Result<()> {
        self.server.shutdown()
    }

    /// Hand the language server back, discarding the graph.
    ///
    /// For callers that want to keep the (expensive) server alive across
    /// engines — switching repositories reuses the process, for instance.
    pub fn into_server(self) -> LanguageServer {
        self.server
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
    use lsp_types::{Position, Url};

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

    /// A script that speaks just enough LSP to reach the branch under test:
    /// it completes `initialize` advertising `callHierarchyProvider`, then
    /// answers every `prepareCallHierarchy` with the transient
    /// ContentModified error (-32801), forever.
    ///
    /// It reads stdin frame by frame rather than replying blindly, because
    /// a response that arrives before its request was registered is dropped.
    #[cfg(unix)]
    const ALWAYS_CONTENT_MODIFIED: &str = r##"send() {
  printf 'Content-Length: %d\r\n\r\n%s' "${#1}" "$1"
}
while IFS= read -r header; do
  case "$header" in
    Content-Length:*) ;;
    *) continue ;;
  esac
  len=$(printf '%s' "$header" | tr -d '\r' | sed 's/^Content-Length:[ ]*//')
  IFS= read -r _blank
  body=$(dd bs=1 count="$len" 2>/dev/null)
  id=$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$body" in
    *'"method":"initialize"'*)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"capabilities\":{\"callHierarchyProvider\":true}}}"
      ;;
    *prepareCallHierarchy*)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32801,\"message\":\"content modified\"}}"
      ;;
  esac
done
"##;

    /// Spec 5.5: a symbol that only ever fails transiently must still appear
    /// in the graph. The retry in `server.rs` absorbs the transient case
    /// against a live server, so this branch is otherwise never reached —
    /// a server that never stops saying ContentModified is the only way in.
    #[cfg(unix)]
    #[test]
    fn a_persistently_content_modified_symbol_is_recorded_as_unresolved() {
        use crate::testutil::FakeServerScript;

        let script = FakeServerScript::new("content-modified", ALWAYS_CONTENT_MODIFIED);
        let server = script.start().expect("fake server initializes");
        let mut engine = Engine::new(server);

        // The file need not exist: `prepareCallHierarchy` sends a URI, and
        // this server errors on it without ever reading it.
        let cand = NamedCallable {
            name: "middle".to_string(),
            uri: Url::parse("file:///tmp/lspgraph-content-modified.rs").unwrap(),
            position: Position { line: 7, character: 3 },
        };

        let seeded = engine.seed(&cand).expect("seed must not surface the transient error");
        assert!(seeded.is_none(), "an unresolvable symbol yields no node id");

        let id = NodeId {
            uri: cand.uri.to_string(),
            line: 7,
            character: 3,
            name: "middle".to_string(),
        };
        let node = engine
            .graph()
            .get(&id)
            .expect("the symbol must be recorded, not dropped");
        assert_eq!(
            node.state,
            NodeState::Unresolved(UnresolvedReason::TransientContentModified)
        );

        engine.shutdown().expect("shutdown");
        assert!(
            script.no_process_survives(),
            "fake server {} was leaked",
            script.name()
        );
    }
}
