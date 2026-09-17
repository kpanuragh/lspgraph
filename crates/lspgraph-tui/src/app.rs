//! All interface behaviour, with no rendering and no engine.

use crate::protocol::{Event, Request};
use lspgraph_core::graph::{Expansion, Node, NodeId, NodeState, UnresolvedReason};
use lspgraph_core::symbols::SymbolMatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Starting,
    Search,
    Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Callers,
    Focus,
    Callees,
}

pub struct App {
    screen: Screen,
    progress: String,
    can_search: bool,
    query: String,
    matches: Vec<SymbolMatch>,
    match_sel: usize,
    focus: Option<Node>,
    callers: Vec<Node>,
    callees: Vec<Node>,
    pending: Option<NodeId>,
    history: Vec<Node>,
    pane: Pane,
    caller_sel: usize,
    callee_sel: usize,
    error: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> App {
        App {
            screen: Screen::Starting,
            progress: "starting language server".into(),
            can_search: false,
            query: String::new(),
            matches: Vec::new(),
            match_sel: 0,
            focus: None,
            callers: Vec::new(),
            callees: Vec::new(),
            pending: None,
            history: Vec::new(),
            pane: Pane::Callers,
            caller_sel: 0,
            callee_sel: 0,
            error: None,
            should_quit: false,
        }
    }

    /// A freshly-seen node id, not yet expanded. Shared by `Seeded` (the
    /// initial focus) and `Expanded` (the neighbours it reveals) so the two
    /// arms cannot drift apart on how a `Node` is built.
    fn unexpanded_node(id: NodeId) -> Node {
        Node {
            id,
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unexpanded,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }
    pub fn progress(&self) -> &str {
        &self.progress
    }
    pub fn can_search(&self) -> bool {
        self.can_search
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn matches(&self) -> &[SymbolMatch] {
        &self.matches
    }
    pub fn focus(&self) -> Option<&Node> {
        self.focus.as_ref()
    }
    pub fn callers(&self) -> &[Node] {
        &self.callers
    }
    pub fn callees(&self) -> &[Node] {
        &self.callees
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn pane(&self) -> Pane {
        self.pane
    }

    pub fn push_query_char(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop_query_char(&mut self) {
        self.query.pop();
    }

    /// Ask the worker to search. Returns None when the server cannot search,
    /// so the caller can leave the explanatory message on screen.
    pub fn submit_query(&mut self) -> Option<Request> {
        if !self.can_search || self.query.is_empty() {
            return None;
        }
        Some(Request::Search(self.query.clone()))
    }

    pub fn select_match_next(&mut self) {
        if !self.matches.is_empty() {
            self.match_sel = (self.match_sel + 1).min(self.matches.len() - 1);
        }
    }

    pub fn select_match_prev(&mut self) {
        self.match_sel = self.match_sel.saturating_sub(1);
    }

    pub fn selected_match(&self) -> usize {
        self.match_sel
    }

    pub fn chosen_match(&mut self) -> Option<Request> {
        self.matches.get(self.match_sel).cloned().map(Request::Seed)
    }

    /// True while the focused node's expansion is still in flight. The views
    /// use this to draw a spinner instead of an empty list.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn breadcrumb(&self) -> Vec<&str> {
        self.history
            .iter()
            .map(|n| n.id.name.as_str())
            .chain(self.focus.iter().map(|n| n.id.name.as_str()))
            .collect()
    }

    pub fn on_event(&mut self, ev: Event) -> Vec<Request> {
        match ev {
            Event::Progress(p) => {
                self.progress = p;
                Vec::new()
            }
            Event::Ready { can_search } => {
                self.can_search = can_search;
                self.screen = Screen::Search;
                Vec::new()
            }
            Event::Matches(ms) => {
                self.matches = ms;
                self.match_sel = 0;
                Vec::new()
            }
            Event::Seeded(Some(id)) => {
                self.screen = Screen::Graph;
                self.focus = Some(Self::unexpanded_node(id.clone()));
                self.callers.clear();
                self.callees.clear();
                self.caller_sel = 0;
                self.callee_sel = 0;
                self.pending = Some(id.clone());
                vec![Request::Expand(id)]
            }
            Event::Seeded(None) => {
                self.error = Some("that symbol has no call hierarchy".into());
                Vec::new()
            }
            Event::Expanded(id, exp) => {
                // A stale response for a node that is no longer the one we
                // asked about (the user re-centred or went back before it
                // arrived) must be discarded whole: no list replacement, no
                // selection reset. `pending` is set by every path that
                // issues an `Expand`, so it is the current request's id.
                if self.pending.as_ref() == Some(&id) {
                    self.pending = None;
                    self.callers = exp.callers.iter().cloned().map(Self::unexpanded_node).collect();
                    self.callees = exp.callees.iter().cloned().map(Self::unexpanded_node).collect();
                    self.caller_sel = 0;
                    self.callee_sel = 0;
                }
                Vec::new()
            }
            Event::Failed(id, why) => {
                if self.pending.as_ref() == Some(&id) {
                    self.pending = None;
                }
                self.error = Some(format!("{}: {}", id.name, why));
                Vec::new()
            }
            Event::Warning(why) => {
                self.error = Some(why);
                Vec::new()
            }
            Event::Fatal(why) => {
                self.error = Some(why);
                self.should_quit = true;
                Vec::new()
            }
        }
    }

    pub fn recentre(&mut self) -> Option<Request> {
        let next = match self.pane {
            Pane::Callers => self.callers.get(self.caller_sel).cloned(),
            Pane::Callees => self.callees.get(self.callee_sel).cloned(),
            Pane::Focus => None,
        }?;
        if let Some(cur) = self.focus.take() {
            self.history.push(cur);
        }
        let id = next.id.clone();
        self.focus = Some(next);
        self.callers.clear();
        self.callees.clear();
        self.caller_sel = 0;
        self.callee_sel = 0;
        self.pending = Some(id.clone());
        Some(Request::Expand(id))
    }

    pub fn back(&mut self) -> Option<Request> {
        let prev = self.history.pop()?;
        let id = prev.id.clone();
        self.focus = Some(prev);
        self.callers.clear();
        self.callees.clear();
        self.caller_sel = 0;
        self.callee_sel = 0;
        self.pending = Some(id.clone());
        Some(Request::Expand(id))
    }

    /// The reason text shown when an unresolved node is selected.
    pub fn status(&self) -> String {
        match self.focus.as_ref().map(|n| &n.state) {
            Some(NodeState::Unresolved(r)) => r.to_string(),
            _ => String::new(),
        }
    }

    pub fn move_pane(&mut self, right: bool) {
        self.pane = match (self.pane, right) {
            (Pane::Callers, true) => Pane::Focus,
            (Pane::Focus, true) => Pane::Callees,
            (Pane::Callees, true) => Pane::Callees,
            (Pane::Callees, false) => Pane::Focus,
            (Pane::Focus, false) => Pane::Callers,
            (Pane::Callers, false) => Pane::Callers,
        };
    }

    pub fn select_next(&mut self) {
        match self.pane {
            Pane::Callers => {
                if !self.callers.is_empty() {
                    self.caller_sel = (self.caller_sel + 1).min(self.callers.len() - 1);
                }
            }
            Pane::Callees => {
                if !self.callees.is_empty() {
                    self.callee_sel = (self.callee_sel + 1).min(self.callees.len() - 1);
                }
            }
            Pane::Focus => {}
        }
    }

    pub fn select_prev(&mut self) {
        match self.pane {
            Pane::Callers => self.caller_sel = self.caller_sel.saturating_sub(1),
            Pane::Callees => self.callee_sel = self.callee_sel.saturating_sub(1),
            Pane::Focus => {}
        }
    }

    pub fn selected_index(&self, pane: Pane) -> usize {
        match pane {
            Pane::Callers => self.caller_sel,
            Pane::Callees => self.callee_sel,
            Pane::Focus => 0,
        }
    }

    #[cfg(test)]
    pub fn set_focus_for_test(&mut self, n: Node) {
        self.focus = Some(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(name: &str) -> NodeId {
        NodeId {
            uri: "file:///a.rs".into(),
            line: 1,
            character: 3,
            name: name.into(),
        }
    }
    fn node(name: &str, state: NodeState) -> Node {
        Node {
            id: nid(name),
            kind_name: "Function".into(),
            detail: None,
            state,
        }
    }

    fn a_ready(can_search: bool) -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search });
        a
    }

    #[test]
    fn starts_on_the_starting_screen() {
        let a = App::new();
        assert_eq!(a.screen(), Screen::Starting);
        assert!(
            !a.progress().is_empty(),
            "a 28s wait must show what it is waiting for"
        );
    }

    #[test]
    fn progress_events_update_the_starting_screen() {
        let mut a = App::new();
        a.on_event(Event::Progress("indexing".into()));
        assert_eq!(a.progress(), "indexing");
        assert_eq!(a.screen(), Screen::Starting);
    }

    #[test]
    fn ready_moves_to_search_and_records_search_availability() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: false });
        assert_eq!(a.screen(), Screen::Search);
        assert!(!a.can_search(), "must remember the server cannot search");
    }

    #[test]
    fn seeding_moves_to_the_graph_and_requests_an_expansion() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        let reqs = a.on_event(Event::Seeded(Some(nid("f"))));
        assert_eq!(a.screen(), Screen::Graph);
        assert_eq!(reqs, vec![Request::Expand(nid("f"))]);
        assert!(a.is_pending(), "expansion in flight must read as pending");
    }

    #[test]
    fn an_expansion_clears_pending_and_fills_both_panes() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion {
                callers: vec![nid("up")],
                callees: vec![nid("down")],
            },
        ));
        assert!(!a.is_pending());
        assert_eq!(a.callers().len(), 1);
        assert_eq!(a.callees().len(), 1);
    }

    #[test]
    fn an_empty_expansion_is_not_pending() {
        // "no callers" is a real answer and must not look like "still loading".
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(nid("f"), Expansion::default()));
        assert!(!a.is_pending(), "an empty result is resolved, not pending");
        assert!(a.callers().is_empty());
    }

    #[test]
    fn recentring_pushes_history_and_asks_for_the_new_expansion() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion {
                callers: vec![nid("up")],
                callees: vec![],
            },
        ));
        let req = a.recentre().expect("a caller is selected");
        assert_eq!(req, Request::Expand(nid("up")));
        assert_eq!(a.breadcrumb(), vec!["f", "up"]);
    }

    #[test]
    fn back_pops_history() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion {
                callers: vec![nid("up")],
                callees: vec![],
            },
        ));
        a.recentre();
        a.back();
        assert_eq!(a.breadcrumb(), vec!["f"]);
    }

    #[test]
    fn back_at_the_root_does_nothing() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        assert!(a.back().is_none());
        assert_eq!(a.breadcrumb(), vec!["f"]);
    }

    #[test]
    fn a_failed_node_shows_an_error_without_quitting() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Failed(nid("f"), "boom".into()));
        assert!(a.error().unwrap().contains("boom"));
        assert!(!a.should_quit);
    }

    #[test]
    fn a_warning_shows_an_error_without_quitting() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Warning("search failed".into()));
        assert!(a.error().unwrap().contains("search failed"));
        assert!(
            !a.should_quit,
            "a recoverable warning must not end the session"
        );
    }

    #[test]
    fn fatal_sets_error_and_quits() {
        let mut a = App::new();
        a.on_event(Event::Fatal("server died".into()));
        assert!(a.error().unwrap().contains("server died"));
        assert!(a.should_quit);
    }

    #[test]
    fn an_unresolved_focus_explains_itself_in_the_status_line() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("over"))));
        a.set_focus_for_test(node(
            "over",
            NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem),
        ));
        let s = a.status();
        assert!(
            s.contains("overload signature") || s.contains("anonymous"),
            "unresolved reason must be explained, got {s:?}"
        );
    }

    #[test]
    fn a_transient_unresolved_focus_says_it_may_resolve_later() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("x"))));
        a.set_focus_for_test(node(
            "x",
            NodeState::Unresolved(UnresolvedReason::TransientContentModified),
        ));
        assert!(a.status().to_lowercase().contains("later"));
    }

    #[test]
    fn selection_is_clamped_at_both_ends() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion {
                callers: vec![nid("a"), nid("b")],
                callees: vec![],
            },
        ));
        a.select_prev();
        assert_eq!(a.selected_index(Pane::Callers), 0);
        a.select_next();
        a.select_next();
        a.select_next();
        assert_eq!(a.selected_index(Pane::Callers), 1, "must not run past the end");
    }

    #[test]
    fn pane_movement_stops_at_the_edges() {
        let mut a = App::new();
        a.move_pane(false);
        assert_eq!(a.pane(), Pane::Callers);
        a.move_pane(true);
        a.move_pane(true);
        a.move_pane(true);
        assert_eq!(a.pane(), Pane::Callees);
    }

    #[test]
    fn a_stale_expansion_does_not_overwrite_the_current_panes() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion {
                callers: vec![nid("up")],
                callees: vec![],
            },
        ));
        // Re-centre onto "up" (Expand(up) in flight), then go back to "f"
        // (Expand(f) in flight) before "up"'s answer arrives.
        a.recentre();
        a.back();
        // The late response for "up" arrives after we've already moved on.
        a.on_event(Event::Expanded(
            nid("up"),
            Expansion {
                callers: vec![nid("ghost")],
                callees: vec![nid("ghost2")],
            },
        ));
        assert_eq!(a.focus().unwrap().id.name, "f", "focus must still be f");
        assert!(!a.callers().iter().any(|n| n.id.name == "ghost"));
        assert!(!a.callees().iter().any(|n| n.id.name == "ghost2"));
        assert!(
            a.is_pending(),
            "f's own expansion has not arrived; the stale reply for up must not be mistaken for it"
        );
    }

    #[test]
    fn typing_builds_a_query_and_submits_it() {
        let mut a = a_ready(true);
        a.push_query_char('s');
        a.push_query_char('e');
        a.pop_query_char();
        assert_eq!(a.query(), "s");
        assert_eq!(a.submit_query(), Some(Request::Search("s".into())));
    }

    #[test]
    fn a_server_that_cannot_search_never_issues_a_search() {
        let mut a = a_ready(false);
        a.push_query_char('s');
        assert_eq!(a.submit_query(), None, "must not send a doomed request");
    }

    #[test]
    fn an_empty_query_is_not_submitted() {
        let mut a = a_ready(true);
        assert_eq!(a.submit_query(), None);
    }
}
