//! Keyboard handling, separated from rendering so bindings are testable.

use crate::app::{App, Screen};
use crate::protocol::Request;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<Request> {
    if app.error().is_some() {
        match key.code {
            KeyCode::Esc => {
                app.dismiss_error();
                return Vec::new();
            }
            KeyCode::Char('r') => {
                app.dismiss_error();
                return vec![Request::Restart];
            }
            KeyCode::Char('q') => {
                app.should_quit = true;
                return Vec::new();
            }
            _ => return Vec::new(),
        }
    }
    match app.screen() {
        Screen::Starting => Vec::new(),
        Screen::Search => match key.code {
            KeyCode::Esc => {
                app.leave_search();
                Vec::new()
            }
            KeyCode::Enter => {
                // Enter submits the query; a second Enter on a highlighted
                // match seeds it.
                if app.matches().is_empty() {
                    app.submit_query().into_iter().collect()
                } else {
                    app.chosen_match().into_iter().collect()
                }
            }
            KeyCode::Backspace => {
                app.pop_query_char();
                Vec::new()
            }
            KeyCode::Down => {
                app.select_match_next();
                Vec::new()
            }
            KeyCode::Up => {
                app.select_match_prev();
                Vec::new()
            }
            KeyCode::Char(c) => {
                app.push_query_char(c);
                Vec::new()
            }
            _ => Vec::new(),
        },
        Screen::Graph => match key.code {
            KeyCode::Char('q') => {
                app.should_quit = true;
                Vec::new()
            }
            KeyCode::Char('/') => {
                app.enter_search();
                Vec::new()
            }
            KeyCode::Char('u') => app.back().into_iter().collect(),
            KeyCode::Enter => app.recentre().into_iter().collect(),
            KeyCode::Left => {
                app.move_pane(false);
                Vec::new()
            }
            KeyCode::Right => {
                app.move_pane(true);
                Vec::new()
            }
            KeyCode::Down => {
                app.select_next();
                Vec::new()
            }
            KeyCode::Up => {
                app.select_prev();
                Vec::new()
            }
            _ => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Event, ResolvedExpansion};
    use lspgraph_core::graph::{Node, NodeId, NodeState};
    use ratatui::crossterm::event::KeyCode;

    fn nid(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
    }
    fn node(name: &str) -> Node {
        Node { id: nid(name), kind_name: "Function".into(), detail: None, state: NodeState::Unexpanded }
    }
    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn graph_app() -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            ResolvedExpansion { callers: vec![node("up")], callees: vec![node("down")] },
        ));
        a
    }

    #[test]
    fn q_quits_from_the_graph() {
        let mut a = graph_app();
        handle_key(&mut a, k(KeyCode::Char('q')));
        assert!(a.should_quit);
    }

    #[test]
    fn enter_recentres_and_requests_an_expansion() {
        let mut a = graph_app();
        let reqs = handle_key(&mut a, k(KeyCode::Enter));
        assert_eq!(reqs, vec![Request::Expand(nid("up"))]);
        assert_eq!(a.breadcrumb(), vec!["f", "up"]);
    }

    #[test]
    fn u_goes_back() {
        let mut a = graph_app();
        handle_key(&mut a, k(KeyCode::Enter));
        handle_key(&mut a, k(KeyCode::Char('u')));
        assert_eq!(a.breadcrumb(), vec!["f"]);
    }

    #[test]
    fn slash_opens_search() {
        let mut a = graph_app();
        handle_key(&mut a, k(KeyCode::Char('/')));
        assert_eq!(a.screen(), Screen::Search);
    }

    #[test]
    fn typing_in_search_does_not_quit_on_q() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        handle_key(&mut a, k(KeyCode::Char('q')));
        assert!(!a.should_quit, "'q' is a query character in search, not a quit");
        assert_eq!(a.query(), "q");
    }

    #[test]
    fn esc_leaves_search_without_quitting() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        handle_key(&mut a, k(KeyCode::Esc));
        assert!(!a.should_quit);
    }
}
