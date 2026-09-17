//! Owns the engine on its own thread so the render loop never blocks.

use crate::protocol::{EngineOps, Event, Request, ResolvedExpansion};
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::{Expansion, Node, NodeId, NodeState};
use lspgraph_core::symbols::SymbolMatch;
use lspgraph_core::Result;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

/// The real engine, wearing the trait the worker speaks to.
pub struct CoreEngine {
    pub engine: Engine,
    pub can_search: bool,
}

impl EngineOps for CoreEngine {
    fn can_search(&self) -> bool {
        self.can_search
    }
    fn search(&mut self, query: &str) -> Result<Vec<SymbolMatch>> {
        self.engine.server().workspace_symbols(query)
    }
    fn seed(&mut self, m: &SymbolMatch) -> Result<Option<NodeId>> {
        self.engine.seed(&m.into())
    }
    fn expand(&mut self, id: &NodeId) -> Result<Expansion> {
        self.engine.expand(id)
    }
    fn node(&self, id: &NodeId) -> Option<Node> {
        self.engine.graph().get(id).cloned()
    }
    fn shutdown(self: Box<Self>) -> Result<()> {
        self.engine.shutdown()
    }
}

/// Resolves an endpoint id to the graph's full `Node` so the interface can
/// show its signature. The graph is expected to know every id it just
/// handed back from `seed`/`expand`, but if it somehow does not, this still
/// hands back a usable (if detail-less) `Node` rather than dropping the
/// endpoint.
fn resolve(engine: &dyn EngineOps, id: NodeId) -> Node {
    engine.node(&id).unwrap_or_else(|| Node {
        id,
        kind_name: "Function".into(),
        detail: None,
        state: NodeState::Unexpanded,
    })
}

pub fn spawn_worker(
    mut engine: Box<dyn EngineOps>,
    rx: Receiver<Request>,
    tx: Sender<Event>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Ready {
            can_search: engine.can_search(),
        });

        // `recv` ending means the UI thread is gone; fall through and shut the
        // engine down rather than leaking a language server.
        while let Ok(req) = rx.recv() {
            match req {
                Request::Shutdown => break,
                Request::Restart => {
                    // The worker owns an engine but not the recipe for building
                    // one; main.rs holds the config and root. Ending the loop
                    // shuts this engine down cleanly and lets main decide.
                    let _ = tx.send(Event::Fatal(
                        "language server restart requires relaunching lspgraph".into(),
                    ));
                    break;
                }
                Request::Search(q) => match engine.search(&q) {
                    Ok(ms) => {
                        let _ = tx.send(Event::Matches(ms));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Warning(e.to_string()));
                    }
                },
                Request::Seed(m) => match engine.seed(&m) {
                    Ok(id) => {
                        let node = id.map(|nid| resolve(engine.as_ref(), nid));
                        let _ = tx.send(Event::Seeded(node));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Warning(e.to_string()));
                    }
                },
                Request::Expand(id) => match engine.expand(&id) {
                    Ok(exp) => {
                        let resolved = ResolvedExpansion {
                            callers: exp
                                .callers
                                .into_iter()
                                .map(|nid| resolve(engine.as_ref(), nid))
                                .collect(),
                            callees: exp
                                .callees
                                .into_iter()
                                .map(|nid| resolve(engine.as_ref(), nid))
                                .collect(),
                        };
                        let _ = tx.send(Event::Expanded(id, resolved));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Failed(id, e.to_string()));
                    }
                },
            }
        }

        let _ = engine.shutdown();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lspgraph_core::graph::NodeState;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[derive(Default)]
    struct Stub {
        can_search: bool,
        fail_expand: bool,
        fail_search: bool,
        shutdown_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    }

    fn id(name: &str) -> NodeId {
        NodeId {
            uri: "file:///a.rs".into(),
            line: 1,
            character: 3,
            name: name.into(),
        }
    }

    impl EngineOps for Stub {
        fn can_search(&self) -> bool {
            self.can_search
        }
        fn search(&mut self, query: &str) -> Result<Vec<SymbolMatch>> {
            let _ = query;
            if self.fail_search {
                return Err(lspgraph_core::Error::Protocol("nope".into()));
            }
            Ok(Vec::new())
        }
        fn seed(&mut self, m: &SymbolMatch) -> Result<Option<NodeId>> {
            let _ = m;
            Ok(Some(id("f")))
        }
        fn expand(&mut self, node: &NodeId) -> Result<Expansion> {
            if self.fail_expand {
                return Err(lspgraph_core::Error::Protocol("nope".into()));
            }
            let _ = node;
            Ok(Expansion {
                callers: vec![id("caller")],
                callees: vec![id("callee")],
            })
        }
        fn node(&self, node: &NodeId) -> Option<Node> {
            Some(Node {
                id: node.clone(),
                kind_name: "Function".into(),
                detail: None,
                state: NodeState::Unexpanded,
            })
        }
        fn shutdown(self: Box<Self>) -> Result<()> {
            if let Some(f) = &self.shutdown_flag {
                f.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        }
    }

    fn recv(rx: &Receiver<Event>) -> Event {
        rx.recv_timeout(Duration::from_secs(2)).expect("event")
    }

    #[test]
    fn announces_ready_with_search_availability() {
        let (_qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                can_search: true,
                ..Default::default()
            }),
            qrx,
            etx,
        );
        assert_eq!(recv(&erx), Event::Ready { can_search: true });
        drop(_qtx);
        h.join().unwrap();
    }

    #[test]
    fn expands_and_reports_the_expansion() {
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(Box::new(Stub::default()), qrx, etx);
        let _ = recv(&erx); // Ready
        qtx.send(Request::Expand(id("f"))).unwrap();
        match recv(&erx) {
            Event::Expanded(n, exp) => {
                assert_eq!(n, id("f"));
                assert_eq!(exp.callers.len(), 1);
                assert_eq!(exp.callees.len(), 1);
            }
            other => panic!("expected Expanded, got {other:?}"),
        }
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn a_failed_expansion_does_not_end_the_session() {
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                fail_expand: true,
                ..Default::default()
            }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        qtx.send(Request::Expand(id("f"))).unwrap();
        assert!(matches!(recv(&erx), Event::Failed(_, _)));
        // Still alive: a second request is still served.
        qtx.send(Request::Seed(SymbolMatch {
            name: "f".into(),
            container: None,
            uri: lspgraph_core::server::path_to_uri(&std::env::temp_dir().join("a.rs")),
            position: Default::default(),
            kind: lspgraph_core::symbols::function_kind(),
        }))
        .unwrap();
        assert!(matches!(recv(&erx), Event::Seeded(Some(_))));
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn a_failed_search_warns_rather_than_killing_the_session() {
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                fail_search: true,
                ..Default::default()
            }),
            qrx,
            etx,
        );
        let _ = recv(&erx); // Ready
        qtx.send(Request::Search("f".into())).unwrap();
        match recv(&erx) {
            Event::Warning(_) => {}
            other => panic!("expected Warning, got {other:?}"),
        }
        // Still alive: a second request is still served.
        qtx.send(Request::Expand(id("f"))).unwrap();
        assert!(matches!(recv(&erx), Event::Expanded(_, _)));
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn shutdown_reaches_the_engine() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                shutdown_flag: Some(flag.clone()),
                ..Default::default()
            }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
        assert!(
            flag.load(std::sync::atomic::Ordering::SeqCst),
            "engine was not shut down"
        );
    }

    #[test]
    fn restart_reports_clearly_and_shuts_down_rather_than_hanging() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                shutdown_flag: Some(flag.clone()),
                ..Default::default()
            }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        qtx.send(Request::Restart).unwrap();
        assert!(matches!(recv(&erx), Event::Fatal(_)));
        h.join().unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn dropping_the_request_channel_also_shuts_the_engine_down() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub {
                shutdown_flag: Some(flag.clone()),
                ..Default::default()
            }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        drop(qtx); // UI thread died without saying goodbye
        h.join().unwrap();
        assert!(
            flag.load(std::sync::atomic::Ordering::SeqCst),
            "engine leaked on channel drop"
        );
    }
}
