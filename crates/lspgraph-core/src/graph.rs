//! The call graph: identity, nodes, edges, and per-file invalidation.

use lsp_types::CallHierarchyItem;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Spec 5.4: identity is (uri, selectionRange.start, name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId {
    pub uri: String,
    pub line: u32,
    pub character: u32,
    pub name: String,
}

impl NodeId {
    pub fn from_item(item: &CallHierarchyItem) -> NodeId {
        NodeId {
            uri: item.uri.to_string(),
            line: item.selection_range.start.line,
            character: item.selection_range.start.character,
            name: item.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnresolvedReason {
    /// prepareCallHierarchy returned nothing. Legitimate for overload
    /// signatures and arrow-function assignments (spec 5.5).
    NoCallHierarchyItem,
    /// The server kept reporting ContentModified; the symbol may resolve later.
    TransientContentModified,
}

impl fmt::Display for UnresolvedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnresolvedReason::NoCallHierarchyItem => write!(
                f,
                "no call hierarchy: likely an overload signature or an anonymous function"
            ),
            UnresolvedReason::TransientContentModified => write!(
                f,
                "server repeatedly reported content modified; the symbol may resolve on a later attempt"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Unexpanded,
    Expanded,
    Unresolved(UnresolvedReason),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind_name: String,
    pub detail: Option<String>,
    pub state: NodeState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    pub callers: Vec<NodeId>,
    pub callees: Vec<NodeId>,
}

/// serde_json cannot serialize a map with a struct key ("key must be a
/// string"), so `CallGraph`'s `HashMap<NodeId, _>` fields cannot derive
/// Serialize/Deserialize directly. This mirror struct uses vector-of-pairs
/// representations instead, and `CallGraph` converts through it via
/// `#[serde(from = ..., into = ...)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallGraphRepr {
    nodes: Vec<Node>,
    callers: Vec<(NodeId, Vec<NodeId>)>,
    callees: Vec<(NodeId, Vec<NodeId>)>,
}

impl From<CallGraph> for CallGraphRepr {
    fn from(g: CallGraph) -> Self {
        CallGraphRepr {
            nodes: g.nodes.into_values().collect(),
            callers: g.callers.into_iter().collect(),
            callees: g.callees.into_iter().collect(),
        }
    }
}

impl From<CallGraphRepr> for CallGraph {
    fn from(repr: CallGraphRepr) -> Self {
        CallGraph {
            nodes: repr.nodes.into_iter().map(|n| (n.id.clone(), n)).collect(),
            callers: repr.callers.into_iter().collect(),
            callees: repr.callees.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "CallGraphRepr", into = "CallGraphRepr")]
pub struct CallGraph {
    nodes: HashMap<NodeId, Node>,
    callers: HashMap<NodeId, Vec<NodeId>>,
    callees: HashMap<NodeId, Vec<NodeId>>,
}

impl CallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Insert a node, preserving the existing state if already present.
    pub fn upsert(&mut self, node: Node) {
        match self.nodes.get_mut(&node.id) {
            Some(existing) => {
                existing.kind_name = node.kind_name;
                existing.detail = node.detail;
                // State is deliberately preserved: re-seeing a node must not
                // discard the fact that it was already expanded.
            }
            None => {
                self.nodes.insert(node.id.clone(), node);
            }
        }
    }

    pub fn get(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn set_state(&mut self, id: &NodeId, state: NodeState) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.state = state;
        }
    }

    pub fn record_expansion(&mut self, id: &NodeId, exp: &Expansion) {
        self.callers.insert(id.clone(), exp.callers.clone());
        self.callees.insert(id.clone(), exp.callees.clone());
        self.set_state(id, NodeState::Expanded);
    }

    pub fn callers_of(&self, id: &NodeId) -> Option<&[NodeId]> {
        self.callers.get(id).map(|v| v.as_slice())
    }

    pub fn callees_of(&self, id: &NodeId) -> Option<&[NodeId]> {
        self.callees.get(id).map(|v| v.as_slice())
    }

    pub fn nodes_in_file(&self, uri: &str) -> Vec<NodeId> {
        self.nodes.keys().filter(|k| k.uri == uri).cloned().collect()
    }

    /// Spec 5.4: an edited file invalidates all of its nodes wholesale.
    pub fn remove_file(&mut self, uri: &str) {
        let doomed = self.nodes_in_file(uri);
        for id in &doomed {
            self.nodes.remove(id);
            self.callers.remove(id);
            self.callees.remove(id);
        }
        // Also drop dangling references from surviving nodes.
        for list in self.callers.values_mut().chain(self.callees.values_mut()) {
            list.retain(|n| n.uri != uri);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str, line: u32) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line, character: 3, name: name.into() }
    }

    fn node(name: &str, line: u32) -> Node {
        Node {
            id: id(name, line),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unexpanded,
        }
    }

    #[test]
    fn identity_distinguishes_same_name_at_different_positions() {
        assert_ne!(id("f", 1), id("f", 2));
        assert_eq!(id("f", 1), id("f", 1));
    }

    #[test]
    fn upsert_preserves_existing_state() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        g.set_state(&id("f", 1), NodeState::Expanded);
        g.upsert(node("f", 1)); // would reset to Unexpanded if naive
        assert_eq!(g.get(&id("f", 1)).unwrap().state, NodeState::Expanded);
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn records_edges_in_both_directions() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        let exp = Expansion { callers: vec![id("a", 5)], callees: vec![id("b", 9)] };
        g.record_expansion(&id("f", 1), &exp);
        assert_eq!(g.callers_of(&id("f", 1)).unwrap(), &[id("a", 5)]);
        assert_eq!(g.callees_of(&id("f", 1)).unwrap(), &[id("b", 9)]);
        assert_eq!(g.get(&id("f", 1)).unwrap().state, NodeState::Expanded);
    }

    #[test]
    fn unexpanded_node_has_no_edge_entries() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        assert!(g.callers_of(&id("f", 1)).is_none());
    }

    #[test]
    fn remove_file_drops_nodes_and_their_edges() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        g.upsert(node("g", 2));
        g.record_expansion(&id("f", 1), &Expansion { callers: vec![], callees: vec![id("g", 2)] });
        g.remove_file("file:///a.rs");
        assert_eq!(g.len(), 0);
        assert!(g.callees_of(&id("f", 1)).is_none());
    }

    #[test]
    fn remove_file_leaves_other_files_alone() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        let mut other = node("h", 1);
        other.id.uri = "file:///b.rs".into();
        g.upsert(other);
        g.remove_file("file:///a.rs");
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn remove_file_strips_dangling_references_from_surviving_nodes() {
        // Spec 5.4's invalidation invariant: after a file is invalidated, no
        // surviving node may still point at anything that used to live in it.
        // The other removal tests only have nodes inside the removed file, so
        // the `retain` loop never sees a survivor — this is the test that
        // actually exercises it.
        let mut g = CallGraph::new();

        let mut survivor = node("h", 1);
        survivor.id.uri = "file:///b.rs".into();
        let survivor_id = survivor.id.clone();
        g.upsert(survivor);

        let mut bystander = node("k", 7);
        bystander.id.uri = "file:///b.rs".into();
        let bystander_id = bystander.id.clone();
        g.upsert(bystander);

        g.upsert(node("f", 1)); // in a.rs
        g.upsert(node("g", 2)); // in a.rs

        // The survivor is both called by and calls into the doomed file, and
        // also has one edge to a node that will still be there afterwards.
        g.record_expansion(
            &survivor_id,
            &Expansion {
                callers: vec![id("f", 1)],
                callees: vec![id("g", 2), bystander_id.clone()],
            },
        );

        g.remove_file("file:///a.rs");

        assert_eq!(g.len(), 2, "only the two b.rs nodes should survive");
        assert_eq!(
            g.callers_of(&survivor_id).unwrap(),
            &[] as &[NodeId],
            "dangling caller reference into the removed file survived"
        );
        assert_eq!(
            g.callees_of(&survivor_id).unwrap(),
            &[bystander_id],
            "removal should strip only the references into the removed file"
        );
    }

    #[test]
    fn unresolved_reason_is_human_readable() {
        let text = UnresolvedReason::NoCallHierarchyItem.to_string();
        assert!(text.contains("overload signature"));
    }

    #[test]
    fn transient_content_modified_has_distinct_display_text() {
        let a = UnresolvedReason::NoCallHierarchyItem.to_string();
        let b = UnresolvedReason::TransientContentModified.to_string();
        assert_ne!(a, b);
        assert!(b.contains("content modified"));
    }

    #[test]
    fn call_graph_round_trips_through_json() {
        let mut g = CallGraph::new();
        g.upsert(node("f", 1));
        let exp = Expansion { callers: vec![id("a", 5)], callees: vec![id("b", 9)] };
        g.record_expansion(&id("f", 1), &exp);

        let json = serde_json::to_string(&g).expect("serialize");
        let restored: CallGraph = serde_json::from_str(&json).expect("deserialize");

        let restored_node = restored.get(&id("f", 1)).expect("node survives round trip");
        assert_eq!(restored_node.state, NodeState::Expanded);
        assert_eq!(restored.callers_of(&id("f", 1)).unwrap(), &[id("a", 5)]);
        assert_eq!(restored.callees_of(&id("f", 1)).unwrap(), &[id("b", 9)]);
    }
}
