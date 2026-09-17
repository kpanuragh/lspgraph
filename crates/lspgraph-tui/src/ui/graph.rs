//! The three-pane call graph view.

use crate::app::{App, Pane};
use lspgraph_core::graph::{Node, NodeId, NodeState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

/// Marker for a symbol the language server could not resolve. Engine spec
/// §5.5: these are shown, never hidden.
pub const UNRESOLVED_MARK: &str = "⊘";
const SPINNER: &str = "…";

/// Shown in both side panes when an expansion came back as a failure. The
/// panes are empty either way, and an empty list is this interface's way of
/// saying "this symbol genuinely has no callers" — so a failure has to say
/// so itself, or a transient server error teaches the user something false.
pub const FAILED_LINE: &str = "expansion failed";
pub const FAILED_HINT: &str = "u to go back";

pub fn label(n: &Node) -> String {
    match n.state {
        NodeState::Unresolved(_) => format!("{UNRESOLVED_MARK} {}", n.id.name),
        _ => n.id.name.clone(),
    }
}

/// Renders a `NodeId`'s location the way the spec's mockup shows it —
/// `transport.rs:88`, not the raw `file://` uri the centre pane would
/// otherwise clip mid-path (spec §5). Total: a uri with no `/` still yields
/// something readable rather than an empty string.
fn short_location(id: &NodeId) -> String {
    let file = id
        .uri
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(id.uri.as_str());
    format!("{file}:{}", id.line + 1)
}

pub fn draw_graph(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(app.breadcrumb().join(" › "))),
        rows[0],
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(rows[1]);

    let pending = app.is_pending();
    let failed = app.expansion_failed();

    let side = |title: &str, nodes: &[Node], active: bool, sel: usize| {
        let border = if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let items: Vec<ListItem> = if pending {
            vec![ListItem::new(SPINNER)]
        } else if failed {
            vec![ListItem::new(FAILED_LINE), ListItem::new(FAILED_HINT)]
        } else {
            nodes
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let s = if active && i == sel {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    ListItem::new(label(n)).style(s)
                })
                .collect()
        };
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title.to_string()).border_style(border))
    };

    f.render_widget(
        side("callers", app.callers(), app.pane() == Pane::Callers, app.selected_index(Pane::Callers)),
        cols[0],
    );

    let focus_title = app.focus().map(|n| n.id.name.clone()).unwrap_or_default();
    let focus_body = match app.focus() {
        Some(n) => {
            let detail = n.detail.clone().unwrap_or_else(|| n.kind_name.clone());
            format!("{}\n{}", detail, short_location(&n.id))
        }
        None => String::new(),
    };
    f.render_widget(
        Paragraph::new(focus_body)
            .block(Block::default().borders(Borders::ALL).title(focus_title)),
        cols[1],
    );

    f.render_widget(
        side("callees", app.callees(), app.pane() == Pane::Callees, app.selected_index(Pane::Callees)),
        cols[2],
    );

    let status = if app.status().is_empty() {
        "←/→ pane · ↑/↓ select · ⏎ re-centre · u back · / search · q quit".to_string()
    } else {
        app.status()
    };
    f.render_widget(Paragraph::new(Line::from(status)), rows[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Event, ResolvedExpansion};
    use lspgraph_core::graph::{NodeId, UnresolvedReason};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn nid(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
    }

    fn node(name: &str) -> Node {
        Node { id: nid(name), kind_name: "Function".into(), detail: None, state: NodeState::Unexpanded }
    }

    fn rendered(app: &App) -> String {
        let mut t = Terminal::new(TestBackend::new(80, 16)).unwrap();
        t.draw(|f| draw_graph(f, f.area(), app)).unwrap();
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

    fn app_with_expansion() -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("send_request"))));
        a.on_event(Event::Expanded(
            nid("send_request"),
            ResolvedExpansion { callers: vec![node("handle_request")], callees: vec![node("validate")] },
        ));
        a
    }

    #[test]
    fn shows_both_directions_and_the_focus() {
        let out = rendered(&app_with_expansion());
        assert!(out.contains("handle_request"), "callers pane missing:\n{out}");
        assert!(out.contains("send_request"), "focus missing:\n{out}");
        assert!(out.contains("validate"), "callees pane missing:\n{out}");
    }

    #[test]
    fn shows_the_breadcrumb() {
        let out = rendered(&app_with_expansion());
        assert!(out.contains("send_request"));
    }

    #[test]
    fn a_pending_pane_shows_a_spinner_not_an_empty_list() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("f")))); // expansion in flight
        assert!(a.is_pending());
        let out = rendered(&a);
        assert!(out.contains(SPINNER), "pending must be visible:\n{out}");
    }

    #[test]
    fn an_empty_expansion_renders_no_spinner() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            ResolvedExpansion { callers: vec![], callees: vec![] },
        ));
        let out = rendered(&a);
        assert!(!out.contains(SPINNER), "resolved-and-empty must not look pending:\n{out}");
    }

    #[test]
    fn the_centre_pane_shows_a_short_location_not_a_raw_uri() {
        // Gap B: the raw `file://` uri gets clipped mid-path by the pane's
        // width, cutting off exactly the filename and line number that make
        // it useful. The spec's mockup shows `transport.rs:88`.
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        let n = Node {
            id: NodeId {
                uri: "file:///home/user/project/src/lib.rs".into(),
                line: 4,
                character: 0,
                name: "middle".into(),
            },
            kind_name: "Function".into(),
            detail: Some("fn middle(x: i32) -> i32".into()),
            state: NodeState::Unexpanded,
        };
        a.on_event(Event::Seeded(Some(n)));
        let out = rendered(&a);
        assert!(out.contains("lib.rs:5"), "must show a short location:\n{out}");
        assert!(!out.contains("file://"), "must not leak the raw uri:\n{out}");
    }

    #[test]
    fn an_unresolved_node_is_marked() {
        let n = Node {
            id: nid("over"),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem),
        };
        assert!(label(&n).starts_with(UNRESOLVED_MARK));
    }

    #[test]
    fn an_unresolved_node_is_marked_in_the_rendered_pane() {
        // Spec §7 names the marker as something that must be tested through
        // the render path, not only on `label()`.
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(node("f"))));
        let mut over = node("over");
        over.state = NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem);
        a.on_event(Event::Expanded(
            nid("f"),
            ResolvedExpansion { callers: vec![over], callees: vec![] },
        ));
        let out = rendered(&a);
        assert!(
            out.contains(UNRESOLVED_MARK),
            "an unresolved node must be marked on screen, never quietly hidden:\n{out}"
        );
    }

    #[test]
    fn a_failed_expansion_looks_like_neither_pending_nor_empty() {
        let mut pend = App::new();
        pend.on_event(Event::Ready { can_search: true });
        pend.on_event(Event::Seeded(Some(node("middle"))));
        let pending_out = rendered(&pend);

        let mut empty = App::new();
        empty.on_event(Event::Ready { can_search: true });
        empty.on_event(Event::Seeded(Some(node("middle"))));
        empty.on_event(Event::Expanded(
            nid("middle"),
            ResolvedExpansion { callers: vec![], callees: vec![] },
        ));
        let empty_out = rendered(&empty);

        let mut fail = App::new();
        fail.on_event(Event::Ready { can_search: true });
        fail.on_event(Event::Seeded(Some(node("middle"))));
        fail.on_event(Event::Failed(nid("middle"), "content modified".into()));
        fail.dismiss_error(); // the overlay is dismissed; the panes remain
        let failed_out = rendered(&fail);

        assert!(
            failed_out.contains(FAILED_LINE),
            "a failed expansion must say so:\n{failed_out}"
        );
        assert!(!failed_out.contains(SPINNER), "a failure is not still loading");
        assert!(!empty_out.contains(FAILED_LINE));
        assert!(!pending_out.contains(FAILED_LINE));
        assert_ne!(failed_out, empty_out, "failed must not look like no-callers");
        assert_ne!(failed_out, pending_out, "failed must not look like loading");
    }

    #[test]
    fn a_resolved_node_is_not_marked() {
        let n = Node {
            id: nid("ok"),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Expanded,
        };
        assert_eq!(label(&n), "ok");
    }
}
