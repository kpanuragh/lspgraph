//! Keyboard handling, separated from rendering so bindings are testable.

use crate::app::{App, Screen};
use crate::protocol::Request;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<Request> {
    // Raw mode is on, so the terminal does not turn ctrl-c into a signal: it
    // arrives here as a plain key and would otherwise be typed into the query
    // box. It is handled before everything else so that every screen, and the
    // error overlay, has the exit the user already expects. Quitting this way
    // runs the shutdown path, so the language server is not orphaned.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Vec::new();
    }
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
        NodeId {
            uri: "file:///a.rs".into(),
            line: 1,
            character: 3,
            name: name.into(),
        }
    }
    fn node(name: &str) -> Node {
        Node {
            id: nid(name),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unexpanded,
        }
    }
    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn graph_app() -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            ResolvedExpansion {
                callers: vec![node("up")],
                callees: vec![node("down")],
            },
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
        assert!(
            !a.should_quit,
            "'q' is a query character in search, not a quit"
        );
        assert_eq!(a.query(), "q");
    }

    #[test]
    fn esc_leaves_search_without_quitting_when_a_graph_is_behind_it() {
        let mut a = graph_app();
        handle_key(&mut a, k(KeyCode::Char('/')));
        assert_eq!(a.screen(), Screen::Search);
        handle_key(&mut a, k(KeyCode::Esc));
        assert!(!a.should_quit, "there is a graph to go back to");
        assert_eq!(a.screen(), Screen::Graph);
    }

    #[test]
    fn esc_on_the_first_search_screen_quits_rather_than_stranding_the_user() {
        // The first search screen has nothing behind it and `q` is a query
        // character, so if esc did nothing there would be no way out at all.
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        assert_eq!(a.screen(), Screen::Search);
        handle_key(&mut a, k(KeyCode::Esc));
        assert!(
            a.should_quit,
            "esc must be an exit when nothing is behind it"
        );
    }

    #[test]
    fn ctrl_c_quits_from_every_screen() {
        let mut starting = App::new();
        assert_eq!(starting.screen(), Screen::Starting);
        handle_key(&mut starting, ctrl('c'));
        assert!(starting.should_quit, "ctrl-c must work while starting up");

        let mut search = App::new();
        search.on_event(Event::Ready { can_search: true });
        assert_eq!(search.screen(), Screen::Search);
        handle_key(&mut search, ctrl('c'));
        assert!(
            search.should_quit,
            "ctrl-c must not be typed into the query"
        );
        assert_eq!(search.query(), "", "ctrl-c is not a query character");

        let mut graph = graph_app();
        handle_key(&mut graph, ctrl('c'));
        assert!(graph.should_quit, "ctrl-c must work on the graph");
    }

    #[test]
    fn ctrl_c_quits_through_the_error_overlay() {
        // The overlay swallows every key it does not know; ctrl-c must still
        // get out, because a fatal error is exactly when the user reaches for it.
        let mut a = App::new();
        a.on_event(Event::Warning("boom".into()));
        assert!(a.error().is_some());
        handle_key(&mut a, ctrl('c'));
        assert!(a.should_quit);
    }

    #[test]
    fn editing_the_query_after_a_search_clears_the_stale_matches() {
        use lspgraph_core::symbols::SymbolMatch;
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        for c in "mid".chars() {
            handle_key(&mut a, k(KeyCode::Char(c)));
        }
        a.on_event(Event::Matches(vec![SymbolMatch {
            name: "middle".into(),
            container: None,
            uri: lspgraph_core::server::path_to_uri(&std::env::temp_dir().join("a.rs")),
            position: Default::default(),
            kind: lspgraph_core::symbols::function_kind(),
        }]));
        assert_eq!(a.matches().len(), 1);
        for c in "dle".chars() {
            handle_key(&mut a, k(KeyCode::Char(c)));
        }
        assert_eq!(a.query(), "middle");
        assert!(
            a.matches().is_empty(),
            "the box and the list must not disagree"
        );
        // With the stale match gone, Enter runs the new search rather than
        // seeding the symbol found for the old query.
        let reqs = handle_key(&mut a, k(KeyCode::Enter));
        assert_eq!(reqs, vec![Request::Search("middle".into())]);
    }
}
