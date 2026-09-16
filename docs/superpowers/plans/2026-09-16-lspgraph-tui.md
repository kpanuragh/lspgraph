# lspgraph-tui Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `lspgraph-tui`, a terminal interface that lets a reader navigate an unfamiliar codebase's call graph one hop at a time, driven by `lspgraph-core`.

**Architecture:** The engine runs on a worker thread behind two `std::sync::mpsc` channels; the UI thread renders state and never blocks on a language server. Readiness takes 3–15s (up to ~28s on a first-ever Rust open), so a synchronous call from the render loop would freeze the interface at exactly the moments the user is waiting. One capability — `workspace/symbol` — is added to core first, because the engine currently has no way to answer "where is this function?".

**Tech Stack:** Rust 2021, `ratatui` 0.30 (which re-exports `crossterm` 0.29), `lspgraph-core`. No async runtime.

**Spec:** `docs/superpowers/specs/2026-09-16-lspgraph-tui-design.md` (and its parent, `docs/superpowers/specs/2026-09-16-lspgraph-design.md`)

## Global Constraints

- Rust edition 2021.
- **MSRV is per-crate.** `lspgraph-core` stays at `rust-version = "1.75"`. `lspgraph-tui` declares `rust-version = "1.88"`, because ratatui 0.30.2 requires 1.88. Do NOT raise the workspace-wide or core MSRV — the crate split exists so the engine can be depended on independently.
- No async runtime. Threads and `std::sync::mpsc` only.
- `lspgraph-core` must remain free of any terminal or UI dependency.
- Use `ratatui::crossterm`, never a separately-declared `crossterm` dependency — ratatui pins it non-optionally and re-exports it.
- Unresolved symbols are marked and shown, never hidden (engine spec §5.5).
- A pending pane shows a spinner, never an empty list.
- No Claude/AI attribution in commit messages. Commit messages end at the last line of their body.
- Every task ends with a commit.

## File Structure

```
crates/lspgraph-core/src/
  symbols.rs          + SymbolMatch, From<&SymbolMatch> for NamedCallable
  server.rs           + workspace_symbols(), supports_workspace_symbol()

crates/lspgraph-tui/
  Cargo.toml
  src/
    main.rs           CLI args, terminal lifecycle, event loop wiring
    protocol.rs       Request/Event enums, EngineOps trait
    worker.rs         worker thread: owns Engine, serves requests
    app.rs            App state machine, history, pending tracking
    ui/
      mod.rs          draw() dispatch by screen
      graph.rs        three-pane graph view
      search.rs       query box + result list
      overlay.rs      error overlay + restart banner
```

`protocol.rs` and `app.rs` are pure — no ratatui, no language server — so they carry the bulk of the tests. `worker.rs` is tested against a stub `EngineOps`. The `ui/` modules are tested with `ratatui::backend::TestBackend`.

---

### Task 1: `workspace/symbol` in the engine

**Files:**
- Modify: `crates/lspgraph-core/src/symbols.rs`
- Modify: `crates/lspgraph-core/src/server.rs`

**Interfaces:**
- Consumes: `symbols::is_named_callable`, `LanguageServer`'s private `conn`.
- Produces: `SymbolMatch { name, container, uri, position, kind }`; `impl From<&SymbolMatch> for NamedCallable`; `LanguageServer::workspace_symbols(&self, query: &str) -> Result<Vec<SymbolMatch>>`; `LanguageServer::supports_workspace_symbol(&self) -> bool`.

The engine can only enumerate one file's symbols today, so a reader has no way to ask where a function lives. `workspace/symbol` is optional in LSP, so the capability flag is part of the deliverable, not an afterthought: search must be able to say "this server cannot search" rather than render an empty list.

- [ ] **Step 1: Write the failing tests**

Append to `crates/lspgraph-core/src/symbols.rs`:

```rust
/// A symbol found by a repository-wide `workspace/symbol` search.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolMatch {
    pub name: String,
    /// The enclosing class/module the server reported, if any.
    pub container: Option<String>,
    pub uri: Url,
    pub position: Position,
    pub kind: SymbolKind,
}

impl From<&SymbolMatch> for NamedCallable {
    fn from(m: &SymbolMatch) -> NamedCallable {
        NamedCallable {
            name: m.name.clone(),
            uri: m.uri.clone(),
            position: m.position,
        }
    }
}

/// Parse a `workspace/symbol` result, keeping only named callables.
///
/// Servers return `SymbolInformation[]` (with `location.range`) or
/// `WorkspaceSymbol[]` (with `location` possibly being `{uri}` only). Both
/// carry `name`, `kind` and a uri, which is all we need.
pub fn parse_workspace_symbols(v: &serde_json::Value) -> Vec<SymbolMatch> {
    let _ = v;
    todo!("implemented in step 3")
}

#[cfg(test)]
mod workspace_symbol_tests {
    use super::*;
    use serde_json::json;

    fn sample() -> serde_json::Value {
        json!([
          {"name":"send_request","kind":12,
           "containerName":"transport",
           "location":{"uri":"file:///r/src/transport.rs",
                       "range":{"start":{"line":87,"character":3},
                                "end":{"line":87,"character":15}}}},
          {"name":"expect() callback","kind":12,
           "location":{"uri":"file:///r/src/t.ts",
                       "range":{"start":{"line":4,"character":2},
                                "end":{"line":4,"character":9}}}},
          {"name":"Config","kind":23,
           "location":{"uri":"file:///r/src/cfg.rs",
                       "range":{"start":{"line":1,"character":0},
                                "end":{"line":1,"character":6}}}}
        ])
    }

    #[test]
    fn keeps_named_callables() {
        let out = parse_workspace_symbols(&sample());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "send_request");
        assert_eq!(out[0].container.as_deref(), Some("transport"));
        assert_eq!(out[0].position.line, 87);
        assert_eq!(out[0].position.character, 3);
    }

    #[test]
    fn rejects_synthesized_callbacks_and_non_callables() {
        let names: Vec<String> =
            parse_workspace_symbols(&sample()).into_iter().map(|m| m.name).collect();
        assert!(!names.iter().any(|n| n.contains("callback")));
        assert!(!names.contains(&"Config".to_string()));
    }

    #[test]
    fn a_null_or_non_array_result_is_empty_not_a_panic() {
        assert!(parse_workspace_symbols(&serde_json::Value::Null).is_empty());
        assert!(parse_workspace_symbols(&json!({"unexpected": true})).is_empty());
    }

    #[test]
    fn converts_to_a_named_callable() {
        let m = &parse_workspace_symbols(&sample())[0];
        let c: NamedCallable = m.into();
        assert_eq!(c.name, "send_request");
        assert_eq!(c.position, m.position);
        assert_eq!(c.uri, m.uri);
    }
}
```

Ensure `symbols.rs`'s imports include what these need:

```rust
use lsp_types::{DocumentSymbol, Position, SymbolKind, Url};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core workspace_symbol`
Expected: the first, second and fourth fail by panicking on `not yet implemented`; the third also fails the same way.

- [ ] **Step 3: Implement the parser**

Replace `parse_workspace_symbols`'s body:

```rust
pub fn parse_workspace_symbols(v: &serde_json::Value) -> Vec<SymbolMatch> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let name = e.get("name")?.as_str()?.to_string();
            let kind_n = e.get("kind")?.as_u64()?;
            let kind = match kind_n {
                6 => SymbolKind::METHOD,
                12 => SymbolKind::FUNCTION,
                _ => return None,
            };
            if !is_named_callable(&name, kind) {
                return None;
            }
            let loc = e.get("location")?;
            let uri = Url::parse(loc.get("uri")?.as_str()?).ok()?;
            let start = loc.get("range").and_then(|r| r.get("start"));
            let position = Position {
                line: start.and_then(|s| s.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32,
                character: start
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0) as u32,
            };
            Some(SymbolMatch {
                name,
                container: e
                    .get("containerName")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                uri,
                position,
                kind,
            })
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core workspace_symbol`
Expected: 4 passed.

- [ ] **Step 5: Add the server method and capability flag**

In `crates/lspgraph-core/src/server.rs`, add a field to `LanguageServer`:

```rust
    /// Whether `initialize` advertised `workspaceSymbolProvider`. Search must
    /// be able to say "this server cannot search" rather than show an empty
    /// list, which would be indistinguishable from "nothing matched".
    supports_workspace_symbol: bool,
```

In `start`, after the existing `callHierarchyProvider` gate and before constructing `LanguageServer`, compute it with the same truthiness rule (present, not `false`, not `null`):

```rust
        let supports_workspace_symbol = caps
            .get("capabilities")
            .and_then(|c| c.get("workspaceSymbolProvider"))
            .map(|v| v != &Value::Bool(false) && !v.is_null())
            .unwrap_or(false);
```

Add it to the struct literal that `start` returns, then add these methods to `impl LanguageServer`:

```rust
    pub fn supports_workspace_symbol(&self) -> bool {
        self.supports_workspace_symbol
    }

    /// Repository-wide symbol search, filtered to named callables.
    ///
    /// Returns an empty vec when the server does not advertise the capability;
    /// callers must check `supports_workspace_symbol()` to tell that apart
    /// from a query that genuinely matched nothing.
    pub fn workspace_symbols(&self, query: &str) -> Result<Vec<SymbolMatch>> {
        if !self.supports_workspace_symbol {
            return Ok(Vec::new());
        }
        let v = self.conn.request(
            "workspace/symbol",
            json!({ "query": query }),
            REQUEST_TIMEOUT,
        )?;
        Ok(crate::symbols::parse_workspace_symbols(&v))
    }
```

Add `use crate::symbols::SymbolMatch;` to `server.rs`'s imports. Also add `workspace: { "symbol": { "dynamicRegistration": false } }` alongside the existing `workspace` capabilities in the `initialize` params if not already present — check the existing JSON and extend it rather than replacing it.

- [ ] **Step 6: Run the whole crate's tests**

Run: `cargo test -p lspgraph-core`
Expected: all previously-passing tests still pass, plus the 4 new ones.

- [ ] **Step 7: Verify against a real server**

Run:
```bash
LSPGRAPH_SERVERS_TOML=/home/c729/Projects/opensource/lspgraph/servers.local.toml \
  cargo test -p lspgraph-core --test integration_servers -- --test-threads=1
```
Expected: 4 passed. This confirms the `initialize` change did not break the handshake for any of the three servers.

- [ ] **Step 8: Commit**

```bash
git add crates/lspgraph-core/src/symbols.rs crates/lspgraph-core/src/server.rs
git commit -m "Add repository-wide symbol search to the engine

workspace/symbol is optional in LSP, so the capability flag ships with it:
a server that cannot search must be distinguishable from a query that
matched nothing."
```

---

### Task 2: TUI crate, worker protocol, and the engine trait

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/lspgraph-tui/Cargo.toml`
- Create: `crates/lspgraph-tui/src/main.rs`
- Create: `crates/lspgraph-tui/src/protocol.rs`

**Interfaces:**
- Consumes: `lspgraph_core::{symbols::SymbolMatch, graph::{NodeId, Node, Expansion}, Result}`.
- Produces: `enum Request { Search(String), Seed(SymbolMatch), Expand(NodeId), Restart, Shutdown }`; `enum Event { Progress(String), Ready { can_search: bool }, Matches(Vec<SymbolMatch>), Seeded(Option<NodeId>), Expanded(NodeId, Expansion), Failed(NodeId, String), Fatal(String) }`; `trait EngineOps`.

`EngineOps` exists so the worker's protocol can be tested without a language server. Without it, every worker test needs a live rust-analyzer and the state machine becomes untestable in CI.

- [ ] **Step 1: Add the crate to the workspace**

Edit `Cargo.toml` at the repo root, changing only the `members` line:

```toml
members = ["crates/lspgraph-core", "crates/lspgraph-tui"]
```

Create `crates/lspgraph-tui/Cargo.toml`:

```toml
[package]
name = "lspgraph-tui"
version = "0.1.0"
edition.workspace = true
# ratatui 0.30 requires 1.88. Deliberately NOT raising the workspace or core
# MSRV: the engine stays usable by consumers on older toolchains.
rust-version = "1.88"
license.workspace = true
description = "Terminal interface for exploring a codebase's call graph"

[[bin]]
name = "lspgraph"
path = "src/main.rs"

[dependencies]
lspgraph-core = { path = "../lspgraph-core" }
ratatui = "0.30"
```

Note there is no `crossterm` entry: ratatui pins it non-optionally and re-exports it as `ratatui::crossterm`. Declaring a second copy risks two incompatible versions in one binary.

- [ ] **Step 2: Confirm the ratatui API surface before writing more**

Create `crates/lspgraph-tui/src/main.rs`:

```rust
fn main() {
    // Replaced in Task 9. This exists so the crate builds from Task 2 onward.
    println!("lspgraph-tui");
}
```

Run: `cargo build -p lspgraph-tui`
Expected: builds, downloading ratatui 0.30.x and crossterm 0.29.x.

Then confirm the exact items later tasks rely on actually exist in the resolved version:

```bash
cargo doc -p ratatui --no-deps 2>/dev/null >/dev/null
grep -rn "pub use" ~/.cargo/registry/src/*/ratatui-0.30*/src/lib.rs | grep -i crossterm | head -3
```
Expected: a line re-exporting crossterm. If it is absent, STOP and report — the plan's `ratatui::crossterm` assumption would be wrong and every later task depends on it.

- [ ] **Step 3: Write the failing protocol tests**

Create `crates/lspgraph-tui/src/protocol.rs`:

```rust
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
    /// Human-readable startup progress, forwarded so a 28-second wait is
    /// legible rather than looking like a hang.
    Progress(String),
    /// The server is ready. `can_search` is false when it does not advertise
    /// `workspaceSymbolProvider`.
    Ready { can_search: bool },
    Matches(Vec<SymbolMatch>),
    Seeded(Option<NodeId>),
    Expanded(NodeId, Expansion),
    /// A per-node failure. The session continues.
    Failed(NodeId, String),
    /// The session cannot continue.
    Fatal(String),
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
    use lsp_types_reexport::*;

    // `lsp_types` is not a direct dependency of this crate; build the few
    // values tests need through lspgraph-core's public types instead.
    mod lsp_types_reexport {}

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
```

Add the module to `main.rs`:

```rust
mod protocol;

fn main() {
    println!("lspgraph-tui");
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p lspgraph-tui`
Expected: 3 passed. If `NodeId`'s fields are not public, STOP and report — Task 4 onward depends on constructing them.

Remove the empty `lsp_types_reexport` module and its `use` line if the compiler warns about them; they are scaffolding and should not survive.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/lspgraph-tui
git commit -m "Add lspgraph-tui crate and the UI/worker protocol

The protocol is plain data and the engine sits behind a trait, so the
worker's behaviour is testable without a terminal or a live server."
```

---

### Task 3: The worker thread

**Files:**
- Create: `crates/lspgraph-tui/src/worker.rs`
- Modify: `crates/lspgraph-tui/src/main.rs`

**Interfaces:**
- Consumes: `Request`, `Event`, `EngineOps`.
- Produces: `spawn_worker(engine: Box<dyn EngineOps>, rx: Receiver<Request>, tx: Sender<Event>) -> JoinHandle<()>`; `impl EngineOps for CoreEngine`; `struct CoreEngine`.

The worker owns the engine outright. It must call `shutdown` when it exits: the engine's final review found that a `LanguageServer` moved into an `Engine` was otherwise unreachable, and `Child::drop` does not kill a process — a long-lived TUI is exactly the consumer that turns that into accumulating multi-gigabyte servers.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-tui/src/worker.rs`:

```rust
//! Owns the engine on its own thread so the render loop never blocks.

use crate::protocol::{EngineOps, Event, Request};
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::{Expansion, Node, NodeId};
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

pub fn spawn_worker(
    engine: Box<dyn EngineOps>,
    rx: Receiver<Request>,
    tx: Sender<Event>,
) -> JoinHandle<()> {
    let _ = (engine, rx, tx);
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lspgraph_core::graph::{NodeState, UnresolvedReason};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[derive(Default)]
    struct Stub {
        can_search: bool,
        fail_expand: bool,
        shutdown_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    }

    fn id(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
    }

    impl EngineOps for Stub {
        fn can_search(&self) -> bool {
            self.can_search
        }
        fn search(&mut self, query: &str) -> Result<Vec<SymbolMatch>> {
            let _ = query;
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
            Ok(Expansion { callers: vec![id("caller")], callees: vec![id("callee")] })
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
        let h = spawn_worker(Box::new(Stub { can_search: true, ..Default::default() }), qrx, etx);
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
            Box::new(Stub { fail_expand: true, ..Default::default() }),
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
            uri: lspgraph_core::server::path_to_uri(std::path::Path::new("/a.rs")),
            position: Default::default(),
            kind: lspgraph_core::symbols::function_kind(),
        }))
        .unwrap();
        assert!(matches!(recv(&erx), Event::Seeded(Some(_))));
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn shutdown_reaches_the_engine() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub { shutdown_flag: Some(flag.clone()), ..Default::default() }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        qtx.send(Request::Shutdown).unwrap();
        h.join().unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst), "engine was not shut down");
    }

    #[test]
    fn dropping_the_request_channel_also_shuts_the_engine_down() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub { shutdown_flag: Some(flag.clone()), ..Default::default() }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        drop(qtx); // UI thread died without saying goodbye
        h.join().unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst), "engine leaked on channel drop");
    }

    let _ = UnresolvedReason::NoCallHierarchyItem; // keep the import honest
}
```

Two helpers this test file needs. Add to `crates/lspgraph-core/src/symbols.rs`:

```rust
/// The `SymbolKind` this crate treats as a plain function. Exposed so
/// downstream crates can build `SymbolMatch` values in tests without
/// depending on `lsp-types` directly.
pub fn function_kind() -> SymbolKind {
    SymbolKind::FUNCTION
}
```

And add to `crates/lspgraph-core/src/engine.rs`:

```rust
    /// Borrow the underlying server, for capabilities the engine does not wrap.
    pub fn server(&self) -> &LanguageServer {
        &self.server
    }
```

Add `mod worker;` to `main.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui worker`
Expected: all five fail by panicking on `not yet implemented`.

Note: the stray `let _ = UnresolvedReason::...;` line at the end of the test module is invalid Rust at module scope — delete it and the `UnresolvedReason` import if the compiler objects. It is scaffolding, not a requirement.

- [ ] **Step 3: Implement the worker**

Replace `spawn_worker`'s body:

```rust
pub fn spawn_worker(
    mut engine: Box<dyn EngineOps>,
    rx: Receiver<Request>,
    tx: Sender<Event>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Ready { can_search: engine.can_search() });

        // `recv` ending means the UI thread is gone; fall through and shut the
        // engine down rather than leaking a language server.
        while let Ok(req) = rx.recv() {
            match req {
                Request::Shutdown => break,
                Request::Restart => {
                    let _ = tx.send(Event::Fatal("restart is not implemented".into()));
                }
                Request::Search(q) => match engine.search(&q) {
                    Ok(ms) => {
                        let _ = tx.send(Event::Matches(ms));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Fatal(e.to_string()));
                    }
                },
                Request::Seed(m) => match engine.seed(&m) {
                    Ok(id) => {
                        let _ = tx.send(Event::Seeded(id));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Fatal(e.to_string()));
                    }
                },
                Request::Expand(id) => match engine.expand(&id) {
                    Ok(exp) => {
                        let _ = tx.send(Event::Expanded(id, exp));
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
```

Note `Request::Restart` is answered but not yet implemented — Task 8 replaces that arm. Leaving it as an explicit `Fatal` rather than silence means a stray restart cannot look like a hang.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-tui worker`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-tui/src/worker.rs crates/lspgraph-tui/src/main.rs \
        crates/lspgraph-core/src/symbols.rs crates/lspgraph-core/src/engine.rs
git commit -m "Run the engine on a worker thread

Shuts the engine down on both exit paths, including a dropped request
channel, so a dying UI cannot leak a language server."
```

---

### Task 4: App state machine

**Files:**
- Create: `crates/lspgraph-tui/src/app.rs`
- Modify: `crates/lspgraph-tui/src/main.rs`

**Interfaces:**
- Consumes: `Event`, `Request`, `SymbolMatch`, `NodeId`, `Node`, `Expansion`.
- Produces: `enum Screen { Starting, Search, Graph }`; `enum Pane { Callers, Focus, Callees }`; `struct App` with `new()`, `on_event(Event) -> Vec<Request>`, `screen()`, `focus()`, `callers()`, `callees()`, `is_pending(Pane)`, `breadcrumb()`, `back()`, `select_next()`, `select_prev()`, `move_pane()`, `recentre() -> Option<Request>`, `selected_in(Pane)`, `status()`, `error()`.

This module is pure: no ratatui, no engine. Everything about how the interface behaves is decided here and tested without a terminal.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-tui/src/app.rs`:

```rust
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
        let _ = ev;
        todo!("implemented in step 3")
    }

    pub fn recentre(&mut self) -> Option<Request> {
        todo!("implemented in step 3")
    }

    pub fn back(&mut self) -> Option<Request> {
        todo!("implemented in step 3")
    }

    /// The reason text shown when an unresolved node is selected.
    pub fn status(&self) -> String {
        todo!("implemented in step 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
    }
    fn node(name: &str, state: NodeState) -> Node {
        Node { id: nid(name), kind_name: "Function".into(), detail: None, state }
    }

    #[test]
    fn starts_on_the_starting_screen() {
        let a = App::new();
        assert_eq!(a.screen(), Screen::Starting);
        assert!(!a.progress().is_empty(), "a 28s wait must show what it is waiting for");
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
            Expansion { callers: vec![nid("up")], callees: vec![nid("down")] },
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
            Expansion { callers: vec![nid("up")], callees: vec![] },
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
            Expansion { callers: vec![nid("up")], callees: vec![] },
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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui app`
Expected: compilation fails on the missing `set_focus_for_test`, then the rest fail on `not yet implemented`. Add this to `impl App`, gated to tests, before re-running:

```rust
    #[cfg(test)]
    pub fn set_focus_for_test(&mut self, n: Node) {
        self.focus = Some(n);
    }
```

- [ ] **Step 3: Implement the state machine**

Replace the four `todo!` bodies:

```rust
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
                self.focus = Some(Node {
                    id: id.clone(),
                    kind_name: "Function".into(),
                    detail: None,
                    state: NodeState::Unexpanded,
                });
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
                if self.pending.as_ref() == Some(&id) {
                    self.pending = None;
                }
                let to_node = |n: &NodeId| Node {
                    id: n.clone(),
                    kind_name: "Function".into(),
                    detail: None,
                    state: NodeState::Unexpanded,
                };
                self.callers = exp.callers.iter().map(to_node).collect();
                self.callees = exp.callees.iter().map(to_node).collect();
                self.caller_sel = 0;
                self.callee_sel = 0;
                Vec::new()
            }
            Event::Failed(id, why) => {
                if self.pending.as_ref() == Some(&id) {
                    self.pending = None;
                }
                self.error = Some(format!("{}: {}", id.name, why));
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

    pub fn status(&self) -> String {
        match self.focus.as_ref().map(|n| &n.state) {
            Some(NodeState::Unresolved(r)) => r.to_string(),
            _ => String::new(),
        }
    }
```

`UnresolvedReason`'s `Display` already produces "no call hierarchy: likely an overload signature or an anonymous function" and the transient variant's text, so `status()` inherits the wording the engine spec fixed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-tui app`
Expected: 13 passed.

If `a_transient_unresolved_focus_says_it_may_resolve_later` fails, read `UnresolvedReason`'s `Display` in `crates/lspgraph-core/src/graph.rs` and adjust the assertion to match the real wording — do NOT change the engine's message to satisfy the test.

- [ ] **Step 5: Add selection and pane movement**

Add to `impl App`:

```rust
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
```

Add these tests to the test module:

```rust
    #[test]
    fn selection_is_clamped_at_both_ends() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion { callers: vec![nid("a"), nid("b")], callees: vec![] },
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
```

Run: `cargo test -p lspgraph-tui app`
Expected: 15 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/lspgraph-tui/src/app.rs crates/lspgraph-tui/src/main.rs
git commit -m "Add the interface state machine

Pure state with no rendering and no engine, so every behavioural rule --
including that an empty expansion is resolved rather than pending -- is
tested without a terminal."
```

---

### Task 5: Graph view rendering

**Files:**
- Create: `crates/lspgraph-tui/src/ui/mod.rs`
- Create: `crates/lspgraph-tui/src/ui/graph.rs`
- Modify: `crates/lspgraph-tui/src/main.rs`

**Interfaces:**
- Consumes: `App`, `Pane`, `Screen`.
- Produces: `ui::draw(f: &mut Frame, app: &App)`; `ui::graph::draw_graph(f: &mut Frame, area: Rect, app: &App)`.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-tui/src/ui/mod.rs`:

```rust
pub mod graph;

use crate::app::{App, Screen};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Graph => graph::draw_graph(f, f.area(), app),
        _ => {}
    }
}
```

Create `crates/lspgraph-tui/src/ui/graph.rs`:

```rust
//! The three-pane call graph view.

use crate::app::{App, Pane};
use lspgraph_core::graph::{Node, NodeState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

/// Marker for a symbol the language server could not resolve. Engine spec
/// §5.5: these are shown, never hidden.
pub const UNRESOLVED_MARK: &str = "⊘";
const SPINNER: &str = "…";

pub fn label(n: &Node) -> String {
    match n.state {
        NodeState::Unresolved(_) => format!("{UNRESOLVED_MARK} {}", n.id.name),
        _ => n.id.name.clone(),
    }
}

pub fn draw_graph(f: &mut Frame, area: Rect, app: &App) {
    let _ = (f, area, app);
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Event;
    use lspgraph_core::graph::{Expansion, NodeId, UnresolvedReason};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn nid(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
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
        a.on_event(Event::Seeded(Some(nid("send_request"))));
        a.on_event(Event::Expanded(
            nid("send_request"),
            Expansion { callers: vec![nid("handle_request")], callees: vec![nid("validate")] },
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
        a.on_event(Event::Seeded(Some(nid("f")))); // expansion in flight
        assert!(a.is_pending());
        let out = rendered(&a);
        assert!(out.contains(SPINNER), "pending must be visible:\n{out}");
    }

    #[test]
    fn an_empty_expansion_renders_no_spinner() {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(nid("f"), Expansion::default()));
        let out = rendered(&a);
        assert!(!out.contains(SPINNER), "resolved-and-empty must not look pending:\n{out}");
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
```

Add to `main.rs`:

```rust
mod app;
mod ui;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui graph`
Expected: the four rendering tests fail on `not yet implemented`; the two `label` tests pass already.

- [ ] **Step 3: Implement the view**

Replace `draw_graph`'s body:

```rust
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

    let side = |title: &str, nodes: &[Node], active: bool, sel: usize| {
        let border = if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let items: Vec<ListItem> = if pending {
            vec![ListItem::new(SPINNER)]
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
            format!("{}\n{}:{}", detail, n.id.uri, n.id.line + 1)
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-tui graph`
Expected: 6 passed.

If a ratatui API differs in the resolved 0.30.x (for example `f.area()` versus `f.size()`, or `buf[(x,y)]` indexing), consult `cargo doc -p ratatui --open` and adapt — but change only the call, never the assertion.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-tui/src/ui crates/lspgraph-tui/src/main.rs
git commit -m "Render the three-pane graph view

Pending panes show a spinner and unresolved nodes carry a marker, so
'still loading', 'genuinely empty' and 'cannot resolve' are three visibly
different things rather than one blank list."
```

---

### Task 6: Search view

**Files:**
- Create: `crates/lspgraph-tui/src/ui/search.rs`
- Modify: `crates/lspgraph-tui/src/ui/mod.rs`
- Modify: `crates/lspgraph-tui/src/app.rs`

**Interfaces:**
- Consumes: `App`.
- Produces: `ui::search::draw_search(f: &mut Frame, area: Rect, app: &App)`; `App::push_query_char(char)`, `App::pop_query_char()`, `App::submit_query() -> Option<Request>`, `App::select_match_next()`, `App::select_match_prev()`, `App::chosen_match() -> Option<Request>`.

- [ ] **Step 1: Add the query methods to `App` with failing tests**

Add to `impl App` in `app.rs`:

```rust
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
```

Add these tests to `app.rs`'s test module:

```rust
    fn a_ready(can_search: bool) -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search });
        a
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
```

Run: `cargo test -p lspgraph-tui app`
Expected: 18 passed.

- [ ] **Step 2: Write the failing search-view tests**

Create `crates/lspgraph-tui/src/ui/search.rs`:

```rust
//! Repository-wide symbol search.

use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

/// Shown when the language server does not advertise `workspaceSymbolProvider`.
/// An empty list here would be indistinguishable from "nothing matched".
pub const NO_SEARCH_MSG: &str =
    "this language server does not support workspace symbol search";

pub fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    let _ = (f, area, app);
    todo!("implemented in step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Event;
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
}
```

Extend `ui/mod.rs`:

```rust
pub mod graph;
pub mod search;

use crate::app::{App, Screen};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Graph => graph::draw_graph(f, f.area(), app),
        Screen::Search => search::draw_search(f, f.area(), app),
        Screen::Starting => {
            use ratatui::widgets::Paragraph;
            f.render_widget(
                Paragraph::new(format!("starting… {}", app.progress())),
                f.area(),
            );
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui search`
Expected: both fail on `not yet implemented`.

- [ ] **Step 4: Implement the view**

Replace `draw_search`'s body:

```rust
pub fn draw_search(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    f.render_widget(
        Paragraph::new(format!("> {}", app.query()))
            .block(Block::default().borders(Borders::ALL).title("search")),
        rows[0],
    );

    if !app.can_search() {
        f.render_widget(
            Paragraph::new(NO_SEARCH_MSG)
                .block(Block::default().borders(Borders::ALL)),
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-tui`
Expected: all tests pass (app, worker, protocol, graph, search).

- [ ] **Step 6: Commit**

```bash
git add crates/lspgraph-tui/src/ui crates/lspgraph-tui/src/app.rs
git commit -m "Add the symbol search view

A server without workspace/symbol says so on screen rather than rendering
an empty list, which would be indistinguishable from a query that matched
nothing."
```

---

### Task 7: Key handling

**Files:**
- Create: `crates/lspgraph-tui/src/keys.rs`
- Modify: `crates/lspgraph-tui/src/main.rs`

**Interfaces:**
- Consumes: `App`, `Request`, `Screen`.
- Produces: `handle_key(app: &mut App, key: KeyEvent) -> Vec<Request>`.

Key handling lives apart from rendering so every binding is testable by constructing a `KeyEvent`, with no terminal involved.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-tui/src/keys.rs`:

```rust
//! Keyboard handling, separated from rendering so bindings are testable.

use crate::app::{App, Screen};
use crate::protocol::Request;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<Request> {
    let _ = (app, key);
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Event;
    use lspgraph_core::graph::{Expansion, NodeId};
    use ratatui::crossterm::event::KeyCode;

    fn nid(name: &str) -> NodeId {
        NodeId { uri: "file:///a.rs".into(), line: 1, character: 3, name: name.into() }
    }
    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn graph_app() -> App {
        let mut a = App::new();
        a.on_event(Event::Ready { can_search: true });
        a.on_event(Event::Seeded(Some(nid("f"))));
        a.on_event(Event::Expanded(
            nid("f"),
            Expansion { callers: vec![nid("up")], callees: vec![nid("down")] },
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
```

Add `mod keys;` to `main.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui keys`
Expected: all six fail on `not yet implemented`.

- [ ] **Step 3: Implement key handling**

Replace `handle_key`'s body:

```rust
pub fn handle_key(app: &mut App, key: KeyEvent) -> Vec<Request> {
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
```

Add the two screen-switching methods to `impl App` in `app.rs`:

```rust
    pub fn enter_search(&mut self) {
        self.screen = Screen::Search;
        self.query.clear();
        self.matches.clear();
        self.match_sel = 0;
    }

    /// Leaving search returns to the graph if one is open, otherwise stays put.
    pub fn leave_search(&mut self) {
        if self.focus.is_some() {
            self.screen = Screen::Graph;
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-tui`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-tui/src/keys.rs crates/lspgraph-tui/src/app.rs crates/lspgraph-tui/src/main.rs
git commit -m "Add key handling

Bindings are screen-scoped, so 'q' quits the graph but types a character
in search."
```

---

### Task 8: Error overlay and server restart

**Files:**
- Create: `crates/lspgraph-tui/src/ui/overlay.rs`
- Modify: `crates/lspgraph-tui/src/ui/mod.rs`
- Modify: `crates/lspgraph-tui/src/worker.rs`
- Modify: `crates/lspgraph-tui/src/keys.rs`

**Interfaces:**
- Consumes: `App`.
- Produces: `ui::overlay::draw_error(f: &mut Frame, area: Rect, msg: &str)`; `App::dismiss_error()`; a working `Request::Restart` arm.

Spec §6 requires that a `ServerExited` leaves the already-built graph on screen — it is still true, just no longer growable — and offers `r` to restart.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-tui/src/ui/overlay.rs`:

```rust
//! Errors render over whatever is behind them. Never as an empty state.

use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw_error(f: &mut Frame, area: Rect, msg: &str) {
    let _ = (f, area, msg);
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(msg: &str) -> String {
        let mut t = Terminal::new(TestBackend::new(60, 10)).unwrap();
        t.draw(|f| draw_error(f, f.area(), msg)).unwrap();
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

    #[test]
    fn shows_the_message() {
        assert!(rendered("server not ready after 300s").contains("not ready"));
    }

    #[test]
    fn mentions_how_to_dismiss() {
        let out = rendered("boom");
        assert!(out.to_lowercase().contains("esc"), "must say how to dismiss:\n{out}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-tui overlay`
Expected: both fail on `not yet implemented`.

- [ ] **Step 3: Implement the overlay and wire it in**

Replace `draw_error`'s body:

```rust
pub fn draw_error(f: &mut Frame, area: Rect, msg: &str) {
    let w = area.width.saturating_sub(8).min(70).max(20);
    let h = 7u16.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(format!("{msg}\n\nesc dismiss · r restart server · q quit"))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("error")),
        rect,
    );
}
```

Extend `ui/mod.rs`'s `draw` so the overlay renders last, over whatever is behind it:

```rust
pub mod graph;
pub mod overlay;
pub mod search;

use crate::app::{App, Screen};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Graph => graph::draw_graph(f, f.area(), app),
        Screen::Search => search::draw_search(f, f.area(), app),
        Screen::Starting => {
            use ratatui::widgets::Paragraph;
            f.render_widget(
                Paragraph::new(format!("starting… {}", app.progress())),
                f.area(),
            );
        }
    }
    if let Some(e) = app.error() {
        overlay::draw_error(f, f.area(), e);
    }
}
```

Add to `impl App`:

```rust
    pub fn dismiss_error(&mut self) {
        self.error = None;
    }
```

In `keys.rs`, handle dismissal and restart before the per-screen match, so they work from anywhere:

```rust
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
```

- [ ] **Step 4: Make `Request::Restart` real in the worker**

In `worker.rs`, the `Request::Restart` arm currently answers `Fatal`. Replace it so the worker reports honestly that it cannot rebuild an engine it does not own the constructor for:

```rust
                Request::Restart => {
                    // The worker owns an engine but not the recipe for building
                    // one; main.rs holds the config and root. Ending the loop
                    // shuts this engine down cleanly and lets main decide.
                    let _ = tx.send(Event::Fatal(
                        "language server restart requires relaunching lspgraph".into(),
                    ));
                    break;
                }
```

Add a worker test:

```rust
    #[test]
    fn restart_reports_clearly_and_shuts_down_rather_than_hanging() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (qtx, qrx) = channel();
        let (etx, erx) = channel();
        let h = spawn_worker(
            Box::new(Stub { shutdown_flag: Some(flag.clone()), ..Default::default() }),
            qrx,
            etx,
        );
        let _ = recv(&erx);
        qtx.send(Request::Restart).unwrap();
        assert!(matches!(recv(&erx), Event::Fatal(_)));
        h.join().unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p lspgraph-tui`
Expected: all pass, including the two overlay tests and the new worker test.

- [ ] **Step 6: Commit**

```bash
git add crates/lspgraph-tui/src
git commit -m "Add the error overlay and an honest restart path

Errors draw over the existing view rather than replacing it, so a graph
already built stays readable. Restart reports that it needs a relaunch
instead of silently doing nothing."
```

---

### Task 9: Wire it up and run it

**Files:**
- Modify: `crates/lspgraph-tui/src/main.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: everything above.
- Produces: a working `lspgraph <language> <root>` binary.

- [ ] **Step 1: Write `main.rs`**

```rust
//! Terminal interface for exploring a codebase's call graph.

mod app;
mod keys;
mod protocol;
mod ui;
mod worker;

use app::App;
use lspgraph_core::config::Config;
use lspgraph_core::engine::Engine;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::LanguageServer;
use protocol::{Event, Request};
use ratatui::crossterm::event::{self, Event as CEvent};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, TryRecvError};
use std::time::Duration;

fn source_files(root: &Path, exts: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.is_dir() {
                if !matches!(name, ".git" | "node_modules" | "target" | "vendor") {
                    stack.push(p);
                }
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .map(|x| exts.iter().any(|e| e == x))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let lang = args.next().ok_or("usage: lspgraph <language> <root>")?;
    let root = PathBuf::from(args.next().ok_or("usage: lspgraph <language> <root>")?)
        .canonicalize()?;

    let cfg_path = std::env::var("LSPGRAPH_SERVERS_TOML")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("servers.toml"));
    let cfg = Config::from_toml(&std::fs::read_to_string(&cfg_path)?)?;
    let server_cfg = cfg.servers.get(&lang).ok_or("unknown language")?;
    let files = source_files(&root, &server_cfg.extensions);

    // Startup happens before the terminal is taken over, so a failure prints
    // plainly instead of being swallowed by an alternate screen.
    eprintln!("starting {lang} server…");
    let server = LanguageServer::start(&lang, server_cfg, &root)?;
    let can_search = server.supports_workspace_symbol();
    eprintln!("waiting for the server to become ready…");
    wait_until_ready(
        &server,
        &files,
        &lang,
        &ReadinessConfig::from_server_config(server_cfg),
    )?;

    let engine = worker::CoreEngine { engine: Engine::new(server), can_search };
    let (qtx, qrx) = channel::<Request>();
    let (etx, erx) = channel::<Event>();
    let handle = worker::spawn_worker(Box::new(engine), qrx, etx);

    let mut terminal = ratatui::init();
    let mut app = App::new();
    let res = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| ui::draw(f, &app))?;

            loop {
                match erx.try_recv() {
                    Ok(ev) => {
                        for r in app.on_event(ev) {
                            let _ = qtx.send(r);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        app.should_quit = true;
                        break;
                    }
                }
            }

            if event::poll(Duration::from_millis(50))? {
                if let CEvent::Key(k) = event::read()? {
                    for r in keys::handle_key(&mut app, k) {
                        let _ = qtx.send(r);
                    }
                }
            }

            if app.should_quit {
                return Ok(());
            }
        }
    })();

    ratatui::restore();
    let _ = qtx.send(Request::Shutdown);
    drop(qtx);
    let _ = handle.join();
    res
}
```

- [ ] **Step 2: Build and check**

Run: `cargo build -p lspgraph-tui && cargo clippy -p lspgraph-tui --all-targets`
Expected: builds, no warnings.

If `ratatui::init()` / `ratatui::restore()` do not exist in the resolved version, replace them with explicit `crossterm` terminal setup via `ratatui::crossterm` — enter the alternate screen and enable raw mode, and reverse both on exit. **The restore must run even on the error path**, which is why the loop body above is wrapped in a closure rather than using `?` directly in `main`.

- [ ] **Step 3: Run it against the Rust fixture**

Run:
```bash
LSPGRAPH_SERVERS_TOML=/home/c729/Projects/opensource/lspgraph/servers.local.toml \
  cargo run -p lspgraph-tui -- rust crates/lspgraph-core/fixtures/rust-fixture
```
Expected: startup messages, then the search screen. Type `middle`, press Enter, pick the match, press Enter. The graph view appears with `root` in callers and `leaf` in callees. Press `u` to go back, `q` to quit.

Confirm all of these, and report any that fail rather than adjusting expectations:
- the terminal is restored on quit (your shell prompt behaves normally)
- no language server survives: `pgrep -a rust-analyzer` prints nothing afterwards
- pressing `q` in search types a `q` rather than quitting

- [ ] **Step 4: Run it against a real repository**

Run the same command against a larger checkout (for example the `zod` or `ripgrep` clones under the scratchpad, if present, or this repository itself with `rust .`). Note the startup time and whether the Starting screen reports progress rather than appearing frozen. Record the numbers in your report.

- [ ] **Step 5: Update the README**

In `README.md`, replace the "Status" section's claim that only the engine exists, and add usage:

```markdown
## Try it

```sh
cargo run -p lspgraph-tui -- rust /path/to/a/cargo/project
```

`←/→` move between panes, `↑/↓` select, `Enter` re-centres on the selected
symbol, `u` goes back, `/` searches, `q` quits.

Symbols the language server cannot resolve are shown with a `⊘` marker and an
explanation, rather than hidden — between 2.5% (Rust) and 30% (TypeScript) of
named callables legitimately have no call hierarchy.
```

Keep the existing honesty notes: caching is still not wired up, and there is still no CI pipeline. Do not claim either has changed.

- [ ] **Step 6: Full verification**

Run:
```bash
cargo test --workspace
LSPGRAPH_SERVERS_TOML=/home/c729/Projects/opensource/lspgraph/servers.local.toml \
  cargo test -p lspgraph-core --test integration_servers -- --test-threads=1
cargo clippy --workspace --all-targets
```
Expected: all green, no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/lspgraph-tui/src/main.rs README.md
git commit -m "Wire up the terminal interface

Startup runs before the terminal is taken over so failures print plainly,
and restore runs on every exit path including errors."
```

---

## Self-Review

**Spec coverage.** §2 entry-point gap → Task 1. §3 threading → Tasks 2 and 3. §4 screens → Task 4 (state) plus Tasks 5, 6 and 9 (rendering). §5 graph view, keybindings, pending-is-not-empty, unresolved markers → Tasks 5 and 7. §6 error handling → Task 8, with `NoCallHierarchy`/`NotReady`/`NoCandidates` surfacing through `main.rs`'s pre-terminal startup in Task 9. §7 testing → every task; `TestBackend` in 5, 6, 8; stub `EngineOps` in 3. §8 dependencies → Task 2. §9 non-goals → nothing in this plan implements them.

**One spec item is deliberately narrowed, and recorded here rather than silently dropped:** §6 offers `r` to restart the language server. The worker owns an engine but not the means to build one — `main.rs` holds the config and root. Task 8 therefore makes `r` report that a relaunch is needed and shut down cleanly, instead of pretending to restart. Full in-session restart needs the worker to own a constructor closure, which is a design change worth making only if the restart path proves common.

**Placeholder scan.** No "TBD", no "handle errors appropriately", no "similar to Task N". Every code step carries real code. The `todo!()` markers are TDD scaffolding, each removed inside its own task. Two steps tell the implementer to STOP and report rather than guess (the `ratatui::crossterm` re-export check in Task 2, and public `NodeId` fields in Task 2 step 4) — those are escalation instructions, not placeholders.

**Type consistency.** `SymbolMatch { name, container, uri, position, kind }` is produced in Task 1 and consumed unchanged in 2, 3, 6. `Request`/`Event` are defined in Task 2 and used identically in 3, 4, 7, 8, 9. `App`'s accessors (`screen`, `progress`, `can_search`, `query`, `matches`, `focus`, `callers`, `callees`, `error`, `pane`, `is_pending`, `breadcrumb`, `selected_index`, `selected_match`) are defined in Task 4 and called by that exact spelling in 5, 6, 7, 8. `EngineOps` is declared in Task 2 and implemented twice in Task 3 (real and stub). `Engine::server()` and `symbols::function_kind()` are added in Task 3 because Task 3's own code needs them.

**Known risk carried from the engine.** The integration suite has one failure observed once and never reproduced. It is unrelated to this crate but shares `cargo test --workspace`, so an executor may hit it. If Task 1 step 7 or Task 9 step 6 fails there, re-run once and report both outcomes rather than assuming a regression in this work.
