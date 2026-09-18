//! Repository-wide symbol search.

use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

/// Shown when the language server does not advertise `workspaceSymbolProvider`.
/// An empty list here would be indistinguishable from "nothing matched".
pub const NO_SEARCH_MSG: &str = "this language server does not support workspace symbol search";

/// Shown when a query returns nothing. Deliberately not phrased as "no
/// matches in this repository": some servers (vtsls observed) only return
/// `workspace/symbol` results for files already opened, so an empty list is
/// never authoritative about the whole repository.
pub const NO_MATCHES_MSG: &str = "no matches among the files opened so far";

/// Shown before anything has been searched for. Without it the screen is two
/// empty boxes and a prompt: nothing states that Enter runs the search, and an
/// empty pane titled "matches" reads as "zero matches" rather than "nothing
/// asked yet". The graph view has carried a key line from the start; this is
/// the same affordance for the screen the user actually lands on first.
pub const IDLE_HINT: &str = "type a symbol name, then press Enter to search";

/// The keys this screen answers to. `q` is absent on purpose: it is a character
/// and goes into the query box, so Esc and ctrl-c are the ways out.
pub const KEY_HINT: &str = "Enter search · ↑/↓ select · Esc back · ctrl-c quit";

pub fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    f.render_widget(Paragraph::new(KEY_HINT), rows[2]);

    f.render_widget(
        Paragraph::new(format!("> {}", app.query()))
            .block(Block::default().borders(Borders::ALL).title("search")),
        rows[0],
    );

    if !app.can_search() {
        f.render_widget(
            Paragraph::new(NO_SEARCH_MSG).block(Block::default().borders(Borders::ALL)),
            rows[1],
        );
        return;
    }

    // Only after a search has actually been answered: keying off the query
    // alone tells the user "no matches" on their first keystroke, before
    // anything has been asked.
    if app.searched() && app.matches().is_empty() {
        f.render_widget(
            Paragraph::new(NO_MATCHES_MSG).block(Block::default().borders(Borders::ALL)),
            rows[1],
        );
        return;
    }

    // Nothing has been asked yet. Say what to do rather than rendering an empty
    // bordered list, which looks identical to a search that found nothing.
    if app.matches().is_empty() {
        let msg = if app.query().is_empty() {
            IDLE_HINT.to_string()
        } else {
            format!("press Enter to search for \"{}\"", app.query())
        };
        f.render_widget(
            Paragraph::new(msg).block(Block::default().borders(Borders::ALL).title("matches")),
            rows[1],
        );
        return;
    }

    let items: Vec<ListItem> = app
        .matches()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let where_ = m.container.clone().unwrap_or_default();
            let text = if where_.is_empty() {
                m.name.clone()
            } else {
                format!("{}  ({})", m.name, where_)
            };
            let style = if i == app.selected_match() {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(text).style(style)
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("matches")),
        rows[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Event;
    use lspgraph_core::symbols::SymbolMatch;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut t = Terminal::new(TestBackend::new(80, 12)).unwrap();
        t.draw(|f| draw_search(f, f.area(), app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn a_match(name: &str) -> SymbolMatch {
        SymbolMatch {
            name: name.into(),
            container: None,
            uri: lspgraph_core::server::path_to_uri(&std::env::temp_dir().join("a.rs")),
            position: Default::default(),
            kind: lspgraph_core::symbols::function_kind(),
        }
    }

    #[test]
    fn the_idle_screen_says_what_to_do_instead_of_showing_an_empty_list() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        let out = rendered(&a);
        assert!(
            out.contains("type a symbol name"),
            "the first screen the user sees must not be two empty boxes:\n{out}"
        );
    }

    #[test]
    fn every_screen_state_shows_the_keys() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        assert!(
            rendered(&a).contains("ctrl-c quit"),
            "idle screen has no keys"
        );
        a.push_query_char('z');
        assert!(rendered(&a).contains("ctrl-c quit"), "typing hid the keys");
        a.on_event(Event::Matches(vec![]));
        assert!(
            rendered(&a).contains("ctrl-c quit"),
            "no-matches hid the keys"
        );
        a.on_event(Event::Matches(vec![a_match("zed")]));
        assert!(rendered(&a).contains("ctrl-c quit"), "results hid the keys");
    }

    #[test]
    fn a_typed_but_unsubmitted_query_says_enter_runs_it() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('z');
        a.push_query_char('e');
        let out = rendered(&a);
        assert!(
            out.contains("press Enter to search for"),
            "a typed query with no results must explain itself:\n{out}"
        );
    }

    #[test]
    fn shows_the_query_being_typed() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('s');
        a.push_query_char('e');
        assert!(rendered(&a).contains("se"));
    }

    #[test]
    fn a_server_without_search_says_so_explicitly() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: false });
        let out = rendered(&a);
        assert!(
            out.contains("does not support"),
            "an empty list would look like 'no matches':\n{out}"
        );
    }

    #[test]
    fn a_non_empty_query_with_no_matches_says_so_without_claiming_the_repository_is_covered() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('z');
        a.on_event(Event::Matches(vec![]));
        let out = rendered(&a);
        assert!(
            out.contains(NO_MATCHES_MSG),
            "an empty list must not look authoritative:\n{out}"
        );
    }

    #[test]
    fn typing_without_submitting_does_not_claim_there_are_no_matches() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('z');
        a.push_query_char('e');
        let out = rendered(&a);
        assert!(
            !out.contains(NO_MATCHES_MSG),
            "nothing has been searched for yet:\n{out}"
        );
    }

    #[test]
    fn editing_the_query_after_a_search_stops_claiming_there_are_no_matches() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('z');
        a.on_event(Event::Matches(vec![]));
        assert!(rendered(&a).contains(NO_MATCHES_MSG));
        a.push_query_char('e');
        let out = rendered(&a);
        assert!(
            !out.contains(NO_MATCHES_MSG),
            "the new query has not been answered:\n{out}"
        );
    }

    #[test]
    fn matches_present_do_not_show_the_no_matches_message() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.push_query_char('z');
        a.on_event(Event::Matches(vec![a_match("zed")]));
        let out = rendered(&a);
        assert!(!out.contains(NO_MATCHES_MSG), "matches were found:\n{out}");
    }
}
