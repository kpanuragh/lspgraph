# lspgraph Core Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `lspgraph-core`, a Rust library that lazily explores a codebase's call graph by driving any LSP server's call hierarchy API.

**Architecture:** A synchronous LSP client. A reader thread decodes framed JSON-RPC and dispatches responses to waiting callers through a pending-request map; notifications go to a channel. On top sit server lifecycle, symbol enumeration, a readiness prober, a graph model, and a memoizing expansion engine. No async runtime: the consumer (a ratatui TUI) is synchronous, and §5.6 of the spec defaults concurrency to serial.

**Tech Stack:** Rust 2021, `serde`/`serde_json`, `toml`, `thiserror`, `lsp-types` (upstream LSP structs, including the call hierarchy types), `dirs`, `sha2`. No tokio.

**Spec:** `docs/superpowers/specs/2026-09-16-lspgraph-design.md`

## Global Constraints

- Rust edition 2021. Minimum supported Rust version 1.75.
- No async runtime. Threads and channels only.
- `lspgraph-core` has no terminal or UI dependency of any kind.
- Symbol identity is exactly `(uri, selectionRange.start, name)` — spec §5.4.
- Named-callable filter is exactly: `SymbolKind::FUNCTION` or `SymbolKind::METHOD`, **and** name matches `^[A-Za-z_$][A-Za-z0-9_$]*$` — spec §5.3.
- Readiness defaults: 3 symbols from each of up to 40 files, even stride, 300s timeout — spec §5.2.
- Cache schema mismatch discards the cache; it is never migrated — spec §5.4.
- Symbols that do not resolve are represented explicitly, never dropped — spec §5.5.
- Every task ends with a commit.

## File Structure

```
Cargo.toml                          workspace root
crates/lspgraph-core/
  Cargo.toml
  src/
    lib.rs                          re-exports, crate docs
    error.rs                        Error, Result
    transport/
      mod.rs                        Connection: JSON-RPC correlation
      codec.rs                      Content-Length framing
    config.rs                       servers.toml
    server.rs                       LanguageServer: process + LSP methods
    symbols.rs                      enumeration + named-callable filter
    readiness.rs                    multi-candidate readiness probe
    graph.rs                        NodeId, Node, CallGraph
    engine.rs                       lazy expansion + memoization
    cache.rs                        persistence + invalidation
  examples/
    crawl.rs                        end-to-end driver
  tests/
    integration_servers.rs          gated on installed servers
  fixtures/
    rust-fixture/                   awkward Rust cases
    ts-fixture/                     anonymous callbacks, overloads
    py-fixture/                     awkward Python cases
```

Each module owns one responsibility and is testable without the ones above it. `transport` and `graph` are pure enough for ordinary unit tests; `server`, `symbols` and `readiness` need a real language server and are covered by Task 10.

---

### Task 1: Workspace, error type, and framing codec

**Files:**
- Create: `Cargo.toml`
- Create: `crates/lspgraph-core/Cargo.toml`
- Create: `crates/lspgraph-core/src/lib.rs`
- Create: `crates/lspgraph-core/src/error.rs`
- Create: `crates/lspgraph-core/src/transport/mod.rs`
- Create: `crates/lspgraph-core/src/transport/codec.rs`
- Create: `.gitignore`

**Interfaces:**
- Consumes: nothing.
- Produces: `Error`, `Result<T>`; `codec::encode(&[u8]) -> Vec<u8>`; `codec::Decoder` with `new()`, `push(&[u8])`, `next_message() -> Option<Vec<u8>>`.

The codec is the whole reason this task exists on its own: LSP framing splits messages arbitrarily across reads, and getting that wrong produces corruption that is painful to diagnose four layers up.

- [ ] **Step 1: Create the workspace files**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/lspgraph-core"]

[workspace.package]
edition = "2021"
rust-version = "1.75"
license = "MIT OR Apache-2.0"
```

`crates/lspgraph-core/Cargo.toml`:

```toml
[package]
name = "lspgraph-core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "Lazily explore a codebase's call graph through any LSP server"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
thiserror = "1"
lsp-types = "0.95"
dirs = "5"
sha2 = "0.10"
```

`.gitignore`:

```
/target
```

- [ ] **Step 2: Write the failing codec tests**

Create `crates/lspgraph-core/src/transport/codec.rs`:

```rust
//! Content-Length framing for JSON-RPC over stdio.

/// Frame a payload with an LSP `Content-Length` header.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
    out.extend_from_slice(payload);
    out
}

/// Incremental decoder. Bytes arrive in arbitrary chunks; messages come out whole.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        todo!("implemented in step 4")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_with_content_length() {
        assert_eq!(encode(b"{}"), b"Content-Length: 2\r\n\r\n{}".to_vec());
    }

    #[test]
    fn decodes_a_single_message() {
        let mut d = Decoder::new();
        d.push(&encode(b"{\"a\":1}"));
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
        assert!(d.next_message().is_none());
    }

    #[test]
    fn decodes_two_messages_from_one_chunk() {
        let mut d = Decoder::new();
        let mut chunk = encode(b"{\"a\":1}");
        chunk.extend_from_slice(&encode(b"{\"b\":2}"));
        d.push(&chunk);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
        assert_eq!(d.next_message().unwrap(), b"{\"b\":2}".to_vec());
        assert!(d.next_message().is_none());
    }

    #[test]
    fn waits_for_a_message_split_mid_header() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        d.push(&full[..8]);
        assert!(d.next_message().is_none());
        d.push(&full[8..]);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn waits_for_a_message_split_mid_body() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        let split = full.len() - 3;
        d.push(&full[..split]);
        assert!(d.next_message().is_none());
        d.push(&full[split..]);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn ignores_unknown_headers_and_is_case_insensitive() {
        let mut d = Decoder::new();
        d.push(b"content-type: application/json\r\nCONTENT-LENGTH: 2\r\n\r\n{}");
        assert_eq!(d.next_message().unwrap(), b"{}".to_vec());
    }
}
```

Create `crates/lspgraph-core/src/transport/mod.rs`:

```rust
pub mod codec;
```

Create `crates/lspgraph-core/src/error.rs`:

```rust
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("config: {0}")]
    Config(String),

    #[error("language server {server} does not advertise callHierarchyProvider")]
    NoCallHierarchy { server: String },

    #[error("request {method} timed out after {}ms", timeout.as_millis())]
    Timeout { method: String, timeout: Duration },

    #[error("language server exited")]
    ServerExited,

    #[error("server not ready after {}s; tried {tried} candidate symbols", timeout.as_secs())]
    NotReady { timeout: Duration, tried: usize },
}

pub type Result<T> = std::result::Result<T, Error>;
```

Create `crates/lspgraph-core/src/lib.rs`:

```rust
//! Lazily explore a codebase's call graph through any LSP server.

pub mod error;
pub mod transport;

pub use error::{Error, Result};
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core codec`
Expected: the `encodes_with_content_length` test passes; every decode test fails by panicking on `not yet implemented`.

- [ ] **Step 4: Implement the decoder**

Replace the `next_message` body in `codec.rs`:

```rust
    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        let header_end = find_subslice(&self.buf, b"\r\n\r\n")?;
        let header = std::str::from_utf8(&self.buf[..header_end]).ok()?;

        let mut len: Option<usize> = None;
        for line in header.split("\r\n") {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().ok();
            }
        }
        let len = len?;

        let body_start = header_end + 4;
        if self.buf.len() < body_start + len {
            return None;
        }
        let body = self.buf[body_start..body_start + len].to_vec();
        self.buf.drain(..body_start + len);
        Some(body)
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
```

Note the closing brace placement: `find_subslice` is a free function after the `impl` block, before `mod tests`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core codec`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml .gitignore crates/
git commit -m "Add workspace, error type, and LSP framing codec"
```

---

### Task 2: JSON-RPC connection with request correlation

**Files:**
- Modify: `crates/lspgraph-core/src/transport/mod.rs`
- Test: inline `#[cfg(test)]` in `crates/lspgraph-core/src/transport/mod.rs`

**Interfaces:**
- Consumes: `codec::encode`, `codec::Decoder`, `Error`, `Result`.
- Produces: `Connection::new(reader, writer) -> Connection`; `Connection::request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value>`; `Connection::notify(&self, method: &str, params: Value) -> Result<()>`; `Connection::notifications(&self) -> &Receiver<Notification>`; `struct Notification { method: String, params: Value }`.

`Connection` is generic over `Read`/`Write` rather than owning a child process, purely so this task can be tested in-process with pipes. Task 4 wires a real process in.

Critically: the reader must **answer** server→client requests. The feasibility probe saw `workspace/configuration` and `client/registerCapability`; replying with a null result kept every tested server healthy. A server that never gets an answer can block.

- [ ] **Step 1: Write the failing tests**

Replace `crates/lspgraph-core/src/transport/mod.rs`:

```rust
pub mod codec;

use crate::error::{Error, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A server-initiated notification (`$/progress`, `window/logMessage`, ...).
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<i64, Sender<Value>>>>;

pub struct Connection {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pending: Pending,
    next_id: AtomicI64,
    notifications: Receiver<Notification>,
}

impl Connection {
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> Connection {
        let _ = (reader, writer);
        todo!("implemented in step 3")
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let _ = (method, params, timeout);
        todo!("implemented in step 3")
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        let _ = (method, params);
        todo!("implemented in step 3")
    }

    pub fn notifications(&self) -> &Receiver<Notification> {
        &self.notifications
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    /// A fake server: reads framed requests, replies per a canned table.
    struct Duplex {
        to_client: Cursor<Vec<u8>>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.to_client.read(buf)
        }
    }

    fn canned(messages: &[Value]) -> Duplex {
        let mut bytes = Vec::new();
        for m in messages {
            bytes.extend_from_slice(&codec::encode(serde_json::to_string(m).unwrap().as_bytes()));
        }
        Duplex { to_client: Cursor::new(bytes) }
    }

    #[test]
    fn resolves_a_response_by_id() {
        let server = canned(&[json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})]);
        let conn = Connection::new(server, Vec::new());
        let got = conn.request("x", json!({}), Duration::from_secs(2)).unwrap();
        assert_eq!(got, json!({"ok": true}));
    }

    #[test]
    fn surfaces_an_error_response() {
        let server = canned(&[
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no such method"}}),
        ]);
        let conn = Connection::new(server, Vec::new());
        let err = conn.request("x", json!({}), Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, Error::Protocol(m) if m.contains("no such method")));
    }

    #[test]
    fn routes_notifications_to_the_channel() {
        let server = canned(&[
            json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"hi"}}),
        ]);
        let conn = Connection::new(server, Vec::new());
        let n = conn.notifications().recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(n.method, "window/logMessage");
    }

    #[test]
    fn times_out_when_no_response_arrives() {
        let server = canned(&[]);
        let conn = Connection::new(server, Vec::new());
        let err = conn.request("x", json!({}), Duration::from_millis(80)).unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }));
    }

    #[test]
    fn answers_server_initiated_requests() {
        // A server->client request must be answered or the server can block.
        let server = canned(&[
            json!({"jsonrpc":"2.0","id":99,"method":"workspace/configuration","params":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":"after"}),
        ]);
        let written = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct Tee(Arc<Mutex<Vec<u8>>>);
        impl Write for Tee {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let conn = Connection::new(server, Tee(written.clone()));
        let got = conn.request("x", json!({}), Duration::from_secs(2)).unwrap();
        assert_eq!(got, json!("after"));

        let sent = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert!(sent.contains("\"id\":99"), "must reply to server request 99: {sent}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core transport`
Expected: all five fail by panicking on `not yet implemented`.

- [ ] **Step 3: Implement `Connection`**

Replace the three `todo!` method bodies:

```rust
    pub fn new(
        mut reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> Connection {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(writer)));
        let (tx, notifications) = channel();

        let pending_r = pending.clone();
        let writer_r = writer.clone();
        std::thread::spawn(move || {
            let mut dec = codec::Decoder::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.push(&chunk[..n]);
                while let Some(body) = dec.next_message() {
                    let msg: Value = match serde_json::from_slice(&body) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let id = msg.get("id").and_then(|v| v.as_i64());
                    let is_response = msg.get("result").is_some() || msg.get("error").is_some();

                    if let (Some(id), true) = (id, is_response) {
                        if let Some(sender) = pending_r.lock().unwrap().remove(&id) {
                            let _ = sender.send(msg);
                        }
                    } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                        match id {
                            // Server->client request: must be answered.
                            Some(id) => {
                                let reply = json!({"jsonrpc":"2.0","id":id,"result":Value::Null});
                                let bytes = serde_json::to_vec(&reply).unwrap_or_default();
                                let mut w = writer_r.lock().unwrap();
                                let _ = w.write_all(&codec::encode(&bytes));
                                let _ = w.flush();
                            }
                            None => {
                                let _ = tx.send(Notification {
                                    method: method.to_string(),
                                    params: msg.get("params").cloned().unwrap_or(Value::Null),
                                });
                            }
                        }
                    }
                }
            }
        });

        Connection { writer, pending, next_id: AtomicI64::new(0), notifications }
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);

        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write(&msg)?;

        match rx.recv_timeout(timeout) {
            Ok(m) => {
                if let Some(e) = m.get("error") {
                    let text = e.get("message").and_then(|v| v.as_str()).unwrap_or("unknown");
                    return Err(Error::Protocol(format!("{method}: {text}")));
                }
                Ok(m.get("result").cloned().unwrap_or(Value::Null))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::Timeout { method: method.to_string(), timeout })
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Error::ServerExited),
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({"jsonrpc":"2.0","method":method,"params":params}))
    }

    fn write(&self, msg: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(msg)?;
        let mut w = self.writer.lock().unwrap();
        w.write_all(&codec::encode(&bytes))?;
        w.flush()?;
        Ok(())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core transport`
Expected: 5 passed (plus the 6 codec tests still passing).

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/transport/mod.rs
git commit -m "Add JSON-RPC connection with request correlation

Answers server-initiated requests rather than ignoring them; a server
that never receives a reply can block."
```

---

### Task 3: Server configuration

**Files:**
- Create: `crates/lspgraph-core/src/config.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `Error`, `Result`.
- Produces: `struct Config { servers: BTreeMap<String, ServerConfig> }`; `struct ServerConfig { cmd: String, extensions: Vec<String>, ready_timeout_secs: u64, concurrency: usize }`; `Config::from_toml(&str) -> Result<Config>`; `Config::for_extension(&self, &str) -> Option<(&str, &ServerConfig)>`; `ServerConfig::command(&self) -> (String, Vec<String>)`.

Per spec §6, adding a language is adding a table entry and requires no code. Defaults come from spec §5.2 and §5.6: 300s readiness timeout, concurrency 1 (serial).

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/config.rs`:

```rust
//! Server configuration. Adding a language means adding a table, not code.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

fn default_ready_timeout() -> u64 {
    300
}

fn default_concurrency() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Full command line, e.g. `"vtsls --stdio"`.
    pub cmd: String,
    /// File extensions without a leading dot.
    pub extensions: Vec<String>,
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

impl ServerConfig {
    /// Split `cmd` into program and arguments.
    pub fn command(&self) -> (String, Vec<String>) {
        let mut parts = self.cmd.split_whitespace().map(str::to_string);
        let prog = parts.next().unwrap_or_default();
        (prog, parts.collect())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct Config {
    pub servers: BTreeMap<String, ServerConfig>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config> {
        let _ = text;
        todo!("implemented in step 3")
    }

    /// Resolve an extension (no leading dot) to a language id and its server.
    pub fn for_extension(&self, ext: &str) -> Option<(&str, &ServerConfig)> {
        let _ = ext;
        todo!("implemented in step 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[rust]
cmd = "rust-analyzer"
extensions = ["rs"]

[typescript]
cmd = "vtsls --stdio"
extensions = ["ts", "tsx"]
concurrency = 4
"#;

    #[test]
    fn parses_servers() {
        let c = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(c.servers.len(), 2);
        assert_eq!(c.servers["rust"].cmd, "rust-analyzer");
    }

    #[test]
    fn applies_documented_defaults() {
        let c = Config::from_toml(SAMPLE).unwrap();
        // Spec 5.2: 300s readiness. Spec 5.6: serial by default.
        assert_eq!(c.servers["rust"].ready_timeout_secs, 300);
        assert_eq!(c.servers["rust"].concurrency, 1);
        assert_eq!(c.servers["typescript"].concurrency, 4);
    }

    #[test]
    fn splits_command_into_program_and_args() {
        let c = Config::from_toml(SAMPLE).unwrap();
        let (prog, args) = c.servers["typescript"].command();
        assert_eq!(prog, "vtsls");
        assert_eq!(args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn resolves_extension_to_language() {
        let c = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(c.for_extension("tsx").unwrap().0, "typescript");
        assert_eq!(c.for_extension("rs").unwrap().0, "rust");
        assert!(c.for_extension("go").is_none());
    }

    #[test]
    fn reports_malformed_toml() {
        assert!(matches!(Config::from_toml("[rust"), Err(Error::Config(_))));
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`, after `pub mod error;`:

```rust
pub mod config;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core config`
Expected: all five fail by panicking on `not yet implemented`.

- [ ] **Step 3: Implement parsing and lookup**

Replace the two `todo!` bodies:

```rust
    pub fn from_toml(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| Error::Config(e.to_string()))
    }

    pub fn for_extension(&self, ext: &str) -> Option<(&str, &ServerConfig)> {
        self.servers
            .iter()
            .find(|(_, s)| s.extensions.iter().any(|e| e == ext))
            .map(|(lang, s)| (lang.as_str(), s))
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core config`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/config.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add servers.toml configuration"
```

---

### Task 4: Language server lifecycle

**Files:**
- Create: `crates/lspgraph-core/src/server.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `Connection`, `ServerConfig`, `Error`, `Result`.
- Produces: `LanguageServer::start(name: &str, cfg: &ServerConfig, root: &Path) -> Result<LanguageServer>`; `.open(path: &Path, language_id: &str) -> Result<()>`; `.document_symbols(path: &Path) -> Result<Vec<DocumentSymbol>>`; `.prepare_call_hierarchy(path: &Path, pos: Position) -> Result<Vec<CallHierarchyItem>>`; `.incoming_calls(&CallHierarchyItem) -> Result<Vec<CallHierarchyItem>>`; `.outgoing_calls(&CallHierarchyItem) -> Result<Vec<CallHierarchyItem>>`; `.shutdown(self) -> Result<()>`; `path_to_uri(&Path) -> Url`.

`incoming_calls` and `outgoing_calls` deliberately return `Vec<CallHierarchyItem>` — the `from`/`to` field respectively — because the engine only needs the other endpoint, and unwrapping here keeps two LSP-shaped wrapper types out of the graph layer.

The capability gate is a hard failure per spec §7: a server without `callHierarchyProvider` is refused at startup, naming the server.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/server.rs`:

```rust
//! Language server process lifecycle and the LSP calls the engine needs.

use crate::config::ServerConfig;
use crate::error::{Error, Result};
use crate::transport::Connection;
use lsp_types::{
    CallHierarchyItem, DocumentSymbol, Position, Url,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub fn path_to_uri(path: &Path) -> Url {
    Url::from_file_path(path).expect("absolute path")
}

pub struct LanguageServer {
    pub name: String,
    pub root: PathBuf,
    conn: Connection,
    child: Child,
}

impl LanguageServer {
    pub fn start(name: &str, cfg: &ServerConfig, root: &Path) -> Result<LanguageServer> {
        let _ = (name, cfg, root);
        todo!("implemented in step 3")
    }

    pub fn open(&self, path: &Path, language_id: &str) -> Result<()> {
        let text = std::fs::read_to_string(path)?;
        self.conn.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": path_to_uri(path),
                "languageId": language_id,
                "version": 1,
                "text": text,
            }}),
        )
    }

    pub fn document_symbols(&self, path: &Path) -> Result<Vec<DocumentSymbol>> {
        let v = self.conn.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": path_to_uri(path)}}),
            REQUEST_TIMEOUT,
        )?;
        if v.is_null() {
            return Ok(Vec::new());
        }
        // Servers may return the flat SymbolInformation form; we only support
        // the hierarchical DocumentSymbol form, which all three validated
        // servers provide. Anything else is treated as "no symbols".
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        pos: Position,
    ) -> Result<Vec<CallHierarchyItem>> {
        let v = self.conn.request(
            "textDocument/prepareCallHierarchy",
            json!({"textDocument": {"uri": path_to_uri(path)}, "position": pos}),
            REQUEST_TIMEOUT,
        )?;
        if v.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(v)?)
    }

    pub fn incoming_calls(&self, item: &CallHierarchyItem) -> Result<Vec<CallHierarchyItem>> {
        let v = self.conn.request(
            "callHierarchy/incomingCalls",
            json!({"item": item}),
            REQUEST_TIMEOUT,
        )?;
        Ok(unwrap_calls(v, "from"))
    }

    pub fn outgoing_calls(&self, item: &CallHierarchyItem) -> Result<Vec<CallHierarchyItem>> {
        let v = self.conn.request(
            "callHierarchy/outgoingCalls",
            json!({"item": item}),
            REQUEST_TIMEOUT,
        )?;
        Ok(unwrap_calls(v, "to"))
    }

    pub fn shutdown(mut self) -> Result<()> {
        let _ = self.conn.request("shutdown", Value::Null, Duration::from_secs(5));
        let _ = self.conn.notify("exit", Value::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

/// Pull the `from`/`to` endpoint out of incoming/outgoing call wrappers.
pub(crate) fn unwrap_calls(v: Value, field: &str) -> Vec<CallHierarchyItem> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|c| c.get(field).cloned())
        .filter_map(|i| serde_json::from_value(i).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_incoming_call_endpoints() {
        let v = json!([
            {"from": {"name":"caller","kind":12,
                      "uri":"file:///a.rs",
                      "range":{"start":{"line":1,"character":0},"end":{"line":9,"character":0}},
                      "selectionRange":{"start":{"line":1,"character":3},"end":{"line":1,"character":9}}},
             "fromRanges": []}
        ]);
        let items = unwrap_calls(v, "from");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "caller");
    }

    #[test]
    fn unwrapping_a_non_array_yields_nothing() {
        assert!(unwrap_calls(Value::Null, "from").is_empty());
    }

    #[test]
    fn builds_a_file_uri() {
        let u = path_to_uri(Path::new("/tmp/x.rs"));
        assert_eq!(u.as_str(), "file:///tmp/x.rs");
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod server;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core server`
Expected: compile succeeds and the three tests pass — but `start` is still `todo!`. This task's unit tests cover only the pure helpers; `start` is exercised in Task 10 against real servers.

- [ ] **Step 3: Implement `start`**

Replace the `start` body:

```rust
    pub fn start(name: &str, cfg: &ServerConfig, root: &Path) -> Result<LanguageServer> {
        let (prog, args) = cfg.command();
        let mut child = Command::new(&prog)
            .args(&args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().ok_or(Error::ServerExited)?;
        let stdin = child.stdin.take().ok_or(Error::ServerExited)?;
        let conn = Connection::new(stdout, stdin);

        let root_uri = path_to_uri(root);
        let caps = conn.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "root"}],
                "capabilities": {
                    "window": {"workDoneProgress": true},
                    "textDocument": {
                        "callHierarchy": {"dynamicRegistration": false},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": true}
                    },
                    "workspace": {"workspaceFolders": true}
                }
            }),
            Duration::from_secs(180),
        )?;

        let supported = caps
            .get("capabilities")
            .and_then(|c| c.get("callHierarchyProvider"))
            .map(|v| v != &Value::Bool(false) && !v.is_null())
            .unwrap_or(false);
        if !supported {
            let _ = child.kill();
            return Err(Error::NoCallHierarchy { server: name.to_string() });
        }

        conn.notify("initialized", json!({}))?;

        Ok(LanguageServer {
            name: name.to_string(),
            root: root.to_path_buf(),
            conn,
            child,
        })
    }
```

- [ ] **Step 4: Run the tests to verify they still pass**

Run: `cargo test -p lspgraph-core server`
Expected: 3 passed, no `todo!` remaining in the file.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/server.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add language server lifecycle with capability gate

Refuses a server that does not advertise callHierarchyProvider, naming
it, rather than failing later with empty results."
```

---

### Task 5: Symbol enumeration and the named-callable filter

**Files:**
- Create: `crates/lspgraph-core/src/symbols.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `lsp_types::{DocumentSymbol, SymbolKind, Position, Url}`.
- Produces: `struct NamedCallable { name: String, uri: Url, position: Position }`; `is_named_callable(name: &str, kind: SymbolKind) -> bool`; `collect_named_callables(&[DocumentSymbol], &Url, &mut Vec<NamedCallable>)`.

This is spec §5.3 and it is the single highest-leverage filter in the project: in zod it removed 74% of enumerated "functions" and took the resolve rate from 14% to 70%. The tests below use the **actual** synthesized names observed from tsserver during the feasibility probe.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/symbols.rs`:

```rust
//! Enumerate named callable declarations.
//!
//! `documentSymbol` is not a list of callables. tsserver synthesizes entries
//! for anonymous callbacks (`expect() callback`, `test("x") callback`), which
//! `prepareCallHierarchy` correctly refuses. Crawling them is pure waste.

use lsp_types::{DocumentSymbol, Position, SymbolKind, Url};

#[derive(Debug, Clone, PartialEq)]
pub struct NamedCallable {
    pub name: String,
    pub uri: Url,
    pub position: Position,
}

/// Spec 5.3: kind is Function or Method, and the name is a plain identifier.
pub fn is_named_callable(name: &str, kind: SymbolKind) -> bool {
    let _ = (name, kind);
    todo!("implemented in step 3")
}

pub fn collect_named_callables(
    symbols: &[DocumentSymbol],
    uri: &Url,
    out: &mut Vec<NamedCallable>,
) {
    for s in symbols {
        if is_named_callable(&s.name, s.kind) {
            out.push(NamedCallable {
                name: s.name.clone(),
                uri: uri.clone(),
                position: s.selection_range.start,
            });
        }
        if let Some(children) = &s.children {
            collect_named_callables(children, uri, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_identifiers() {
        for n in ["types", "handle_request", "_custom", "$ref", "catchall", "uuid"] {
            assert!(is_named_callable(n, SymbolKind::FUNCTION), "should accept {n}");
        }
    }

    #[test]
    fn rejects_synthesized_callback_names() {
        // Observed verbatim from tsserver during the feasibility probe.
        for n in [
            "expect() callback",
            "test(\"check any inference\") callback",
            "on(\"cycle\") callback",
            "_def.checks.find() callback",
            "patternKeys.map() callback",
            "z.custom() callback",
        ] {
            assert!(!is_named_callable(n, SymbolKind::FUNCTION), "should reject {n}");
        }
    }

    #[test]
    fn rejects_non_callable_kinds() {
        assert!(!is_named_callable("Config", SymbolKind::STRUCT));
        assert!(!is_named_callable("Color", SymbolKind::ENUM));
        assert!(!is_named_callable("MAX", SymbolKind::CONSTANT));
    }

    #[test]
    fn accepts_methods() {
        assert!(is_named_callable("pipe", SymbolKind::METHOD));
    }

    #[test]
    fn rejects_empty_and_leading_digit() {
        assert!(!is_named_callable("", SymbolKind::FUNCTION));
        assert!(!is_named_callable("2fast", SymbolKind::FUNCTION));
    }

    #[allow(deprecated)]
    fn sym(name: &str, kind: SymbolKind, line: u32, children: Option<Vec<DocumentSymbol>>) -> DocumentSymbol {
        let r = lsp_types::Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 10 },
        };
        DocumentSymbol {
            name: name.to_string(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range: r,
            selection_range: r,
            children,
        }
    }

    #[test]
    fn collects_recursively_and_filters() {
        let uri = Url::parse("file:///a.ts").unwrap();
        let tree = vec![sym(
            "Outer",
            SymbolKind::CLASS,
            1,
            Some(vec![
                sym("method", SymbolKind::METHOD, 2, None),
                sym("expect() callback", SymbolKind::FUNCTION, 3, None),
            ]),
        )];
        let mut out = Vec::new();
        collect_named_callables(&tree, &uri, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "method");
        assert_eq!(out[0].position.line, 2);
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod symbols;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core symbols`
Expected: all six fail by panicking on `not yet implemented`.

- [ ] **Step 3: Implement the filter**

Replace the `is_named_callable` body:

```rust
pub fn is_named_callable(name: &str, kind: SymbolKind) -> bool {
    if kind != SymbolKind::FUNCTION && kind != SymbolKind::METHOD {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core symbols`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/symbols.rs crates/lspgraph-core/src/lib.rs
git commit -m "Filter enumeration to named callable declarations

documentSymbol synthesizes entries for anonymous callbacks that call
hierarchy refuses. In zod this noise was 74% of enumerated functions."
```

---

### Task 6: Readiness probe

**Files:**
- Create: `crates/lspgraph-core/src/readiness.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `LanguageServer`, `NamedCallable`, `Error`, `Result`.
- Produces: `struct ReadinessConfig { max_files: usize, per_file: usize, timeout: Duration }` with `Default`; `even_stride<T>(&[T], usize) -> Vec<&T>`; `wait_until_ready(&LanguageServer, &[PathBuf], &str, &ReadinessConfig) -> Result<()>`.

Spec §5.2. Two failure modes must be designed against, both observed directly:

- A quiet-period heuristic fires during rust-analyzer's silent 17-second dependency fetch and every later request returns empty.
- Polling a single target cannot be distinguished from a dead server; against a healthy vtsls this produced a 600-second false failure.

`even_stride` is separated out and unit-tested because alphabetical selection is exactly what caused that false failure — the probe's candidates all landed inside a benchmark directory whose imports do not resolve.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/readiness.rs`:

```rust
//! Decide when a language server is genuinely ready.
//!
//! There is no portable readiness signal. rust-analyzer's cachePriming token
//! is proprietary; other servers differ. Quiet-period heuristics are wrong:
//! rust-analyzer goes silent for ~17s during dependency fetch. The only
//! portable answer is to poll a real semantic request across MANY candidates
//! and accept the first that answers.

use crate::error::{Error, Result};
use crate::server::LanguageServer;
use crate::symbols::{collect_named_callables, NamedCallable};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ReadinessConfig {
    pub max_files: usize,
    pub per_file: usize,
    pub timeout: Duration,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        // Spec 5.2.
        Self { max_files: 40, per_file: 3, timeout: Duration::from_secs(300) }
    }
}

/// Sample up to `max` items spread evenly across the slice.
///
/// Taking the first N alphabetically is a trap: it can land entirely inside
/// one directory (a benchmark tree with unresolvable imports, say), which
/// makes a healthy server look dead.
pub fn even_stride<T>(items: &[T], max: usize) -> Vec<&T> {
    let _ = (items, max);
    todo!("implemented in step 3")
}

pub fn wait_until_ready(
    server: &LanguageServer,
    files: &[PathBuf],
    language_id: &str,
    cfg: &ReadinessConfig,
) -> Result<()> {
    let mut candidates: Vec<NamedCallable> = Vec::new();
    for path in even_stride(files, cfg.max_files) {
        if server.open(path, language_id).is_err() {
            continue;
        }
        let Ok(syms) = server.document_symbols(path) else {
            continue;
        };
        let mut found = Vec::new();
        collect_named_callables(&syms, &crate::server::path_to_uri(path), &mut found);
        found.truncate(cfg.per_file);
        candidates.extend(found);
    }

    if candidates.is_empty() {
        return Err(Error::NotReady { timeout: Duration::ZERO, tried: 0 });
    }

    let start = Instant::now();
    while start.elapsed() < cfg.timeout {
        for c in &candidates {
            let path = c.uri.to_file_path().unwrap_or_default();
            if let Ok(items) = server.prepare_call_hierarchy(&path, c.position) {
                if !items.is_empty() {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    Err(Error::NotReady { timeout: cfg.timeout, tried: candidates.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_returns_everything_when_under_the_cap() {
        let v = vec![1, 2, 3];
        assert_eq!(even_stride(&v, 10), vec![&1, &2, &3]);
    }

    #[test]
    fn stride_spreads_across_the_whole_slice() {
        let v: Vec<i32> = (0..100).collect();
        let picked = even_stride(&v, 5);
        assert_eq!(picked.len(), 5);
        // Must reach the far end, not cluster at the start.
        assert!(**picked.last().unwrap() >= 80, "clustered: {picked:?}");
        assert_eq!(**picked.first().unwrap(), 0);
    }

    #[test]
    fn stride_handles_empty_and_zero_max() {
        let empty: Vec<i32> = Vec::new();
        assert!(even_stride(&empty, 5).is_empty());
        let v = vec![1, 2, 3];
        assert!(even_stride(&v, 0).is_empty());
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = ReadinessConfig::default();
        assert_eq!(c.max_files, 40);
        assert_eq!(c.per_file, 3);
        assert_eq!(c.timeout, Duration::from_secs(300));
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod readiness;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core readiness`
Expected: the three `stride_*` tests fail by panicking on `not yet implemented`; `defaults_match_the_spec` passes.

- [ ] **Step 3: Implement `even_stride`**

Replace its body:

```rust
pub fn even_stride<T>(items: &[T], max: usize) -> Vec<&T> {
    if items.is_empty() || max == 0 {
        return Vec::new();
    }
    if items.len() <= max {
        return items.iter().collect();
    }
    let stride = items.len() / max;
    items.iter().step_by(stride.max(1)).take(max).collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core readiness`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/readiness.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add multi-candidate readiness probe

Single-target polling is indistinguishable from a dead server, and
quiet-period heuristics fire during silent dependency fetches. Poll
many evenly-strided candidates; ready when any one answers."
```

---

### Task 7: Graph model and symbol identity

**Files:**
- Create: `crates/lspgraph-core/src/graph.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `lsp_types::CallHierarchyItem`.
- Produces: `struct NodeId { uri: String, line: u32, character: u32, name: String }` (`Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`); `NodeId::from_item(&CallHierarchyItem) -> NodeId`; `enum NodeState { Unexpanded, Expanded, Unresolved(UnresolvedReason) }`; `enum UnresolvedReason { NoCallHierarchyItem }` with `Display`; `struct Node { id, kind_name, detail, state }`; `struct Expansion { callers: Vec<NodeId>, callees: Vec<NodeId> }`; `struct CallGraph` with `upsert`, `get`, `set_state`, `record_expansion`, `callers_of`, `callees_of`, `nodes_in_file`, `remove_file`, `len`.

Identity is spec §5.4 exactly: `(uri, selectionRange.start, name)`. `remove_file` implements the coarse per-file invalidation that spec §5.4 requires, and Task 9 calls it.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/graph.rs`:

```rust
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
}

impl fmt::Display for UnresolvedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnresolvedReason::NoCallHierarchyItem => write!(
                f,
                "no call hierarchy: likely an overload signature or an anonymous function"
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

#[derive(Debug, Default, Serialize, Deserialize)]
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
        let _ = node;
        todo!("implemented in step 3")
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
        let _ = uri;
        todo!("implemented in step 3")
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
    fn unresolved_reason_is_human_readable() {
        let text = UnresolvedReason::NoCallHierarchyItem.to_string();
        assert!(text.contains("overload signature"));
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod graph;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core graph`
Expected: `upsert_preserves_existing_state`, `records_edges_in_both_directions`, both `remove_file_*` tests and `unexpanded_node_has_no_edge_entries` fail by panicking on `not yet implemented`.

- [ ] **Step 3: Implement `upsert` and `remove_file`**

Replace both bodies:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core graph`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/graph.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add call graph model with per-file invalidation"
```

---

### Task 8: Lazy expansion engine

**Files:**
- Create: `crates/lspgraph-core/src/engine.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `LanguageServer`, `CallGraph`, `Node`, `NodeId`, `NodeState`, `UnresolvedReason`, `Expansion`, `NamedCallable`, `Result`.
- Produces: `struct Engine`; `Engine::new(LanguageServer) -> Engine`; `.seed(&NamedCallable) -> Result<Option<NodeId>>`; `.expand(&NodeId) -> Result<Expansion>`; `.graph(&self) -> &CallGraph`; `.invalidate_file(&str)`; `.expansions_performed(&self) -> usize`.

Spec §5.1: the public API is `expand`, memoized, and there is deliberately no "index the repository" call. Spec §5.5: a symbol that does not resolve becomes an `Unresolved` node, never a dropped one.

`expansions_performed` is a counter the memoization test asserts on — without it, "did the second call hit the cache?" is unobservable from outside.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/engine.rs`:

```rust
//! Lazy, memoized call graph expansion.
//!
//! Spec 5.1: there is deliberately no "index the repository" entry point.
//! Warm p50 is 6-31ms, so expanding from the cursor is imperceptible, while a
//! full crawl costs minutes. Sourcetrail had to precompute because it owned
//! the index; an LSP client does not.

use crate::error::Result;
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
        let _ = cand;
        todo!("implemented in step 3")
    }

    /// Expand a node into its callers and callees. Memoized.
    pub fn expand(&mut self, id: &NodeId) -> Result<Expansion> {
        let _ = id;
        todo!("implemented in step 3")
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
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod engine;
```

- [ ] **Step 2: Run the tests to verify they compile and the placeholder passes**

Run: `cargo test -p lspgraph-core engine`
Expected: 1 passed. `seed` and `expand` remain `todo!` until step 3; their real coverage is Task 10.

- [ ] **Step 3: Implement `seed` and `expand`**

Replace both bodies:

```rust
    pub fn seed(&mut self, cand: &NamedCallable) -> Result<Option<NodeId>> {
        let path = match cand.uri.to_file_path() {
            Ok(p) => p,
            Err(()) => return Ok(None),
        };
        let items = self.server.prepare_call_hierarchy(&path, cand.position)?;

        let Some(item) = items.into_iter().next() else {
            // Spec 5.5: record it, do not drop it.
            let id = NodeId {
                uri: cand.uri.to_string(),
                line: cand.position.line,
                character: cand.position.character,
                name: cand.name.clone(),
            };
            self.graph.upsert(Node {
                id: id.clone(),
                kind_name: "Unknown".into(),
                detail: None,
                state: NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem),
            });
            self.graph
                .set_state(&id, NodeState::Unresolved(UnresolvedReason::NoCallHierarchyItem));
            return Ok(None);
        };

        Ok(Some(self.intern(&item)))
    }

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core engine`
Expected: 1 passed, no `todo!` remaining.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/engine.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add lazy memoized expansion engine

No repository-wide index entry point: warm expansion is 6-31ms, so the
cursor-driven path stays interactive and the minutes-long full crawl is
never on it."
```

---

### Task 9: Cache persistence

**Files:**
- Create: `crates/lspgraph-core/src/cache.rs`
- Modify: `crates/lspgraph-core/src/lib.rs`

**Interfaces:**
- Consumes: `CallGraph`, `Result`.
- Produces: `const SCHEMA_VERSION: u32`; `struct CacheFile { schema: u32, file_hashes: BTreeMap<String, String>, graph: CallGraph }`; `cache_dir_for(&Path) -> PathBuf`; `hash_file(&Path) -> Result<String>`; `load(&Path) -> Option<CacheFile>`; `save(&Path, &CacheFile) -> Result<()>`; `stale_files(&CacheFile, &Path) -> Vec<String>`.

Spec §5.4: cache lives under the platform cache directory keyed by a hash of the repository path, is keyed by content hash, and a schema mismatch **discards** rather than migrates. Nothing here is authoritative — a cold cache costs time, never correctness, which is why `load` returns `Option` and never an error.

- [ ] **Step 1: Write the failing tests**

Create `crates/lspgraph-core/src/cache.rs`:

```rust
//! Cross-session cache. Never authoritative: a cold cache costs time only.

use crate::error::Result;
use crate::graph::CallGraph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheFile {
    pub schema: u32,
    /// Absolute file path -> content hash at the time of caching.
    pub file_hashes: BTreeMap<String, String>,
    pub graph: CallGraph,
}

impl CacheFile {
    pub fn new(graph: CallGraph) -> CacheFile {
        CacheFile { schema: SCHEMA_VERSION, file_hashes: BTreeMap::new(), graph }
    }
}

fn hash_str(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Spec 5.4: platform cache dir, keyed by a hash of the repository path.
pub fn cache_dir_for(repo: &Path) -> PathBuf {
    let key = hash_str(&repo.to_string_lossy());
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    base.join("lspgraph").join(&key[..16])
}

pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

/// Load a cache, or nothing. A schema mismatch discards rather than migrates.
pub fn load(repo: &Path) -> Option<CacheFile> {
    let _ = repo;
    todo!("implemented in step 3")
}

pub fn save(repo: &Path, cache: &CacheFile) -> Result<()> {
    let dir = cache_dir_for(repo);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec(cache)?;
    std::fs::write(dir.join("graph.json"), json)?;
    Ok(())
}

/// Cached files whose content hash no longer matches what is on disk.
pub fn stale_files(cache: &CacheFile, _repo: &Path) -> Vec<String> {
    let _ = cache;
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_is_stable_and_repo_specific() {
        let a = cache_dir_for(Path::new("/home/u/proj-a"));
        let b = cache_dir_for(Path::new("/home/u/proj-b"));
        assert_ne!(a, b);
        assert_eq!(a, cache_dir_for(Path::new("/home/u/proj-a")));
        assert!(a.to_string_lossy().contains("lspgraph"));
    }

    #[test]
    fn round_trips_through_disk() {
        let repo = std::env::temp_dir().join(format!("lspgraph-t-{}", std::process::id()));
        let cache = CacheFile::new(CallGraph::new());
        save(&repo, &cache).unwrap();
        let back = load(&repo).expect("should load");
        assert_eq!(back.schema, SCHEMA_VERSION);
        std::fs::remove_dir_all(cache_dir_for(&repo)).ok();
    }

    #[test]
    fn discards_a_mismatched_schema() {
        let repo = std::env::temp_dir().join(format!("lspgraph-s-{}", std::process::id()));
        let dir = cache_dir_for(&repo);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("graph.json"),
            br#"{"schema":9999,"file_hashes":{},"graph":{"nodes":{},"callers":{},"callees":{}}}"#,
        )
        .unwrap();
        assert!(load(&repo).is_none(), "mismatched schema must be discarded");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_cache_is_not_an_error() {
        assert!(load(Path::new("/nonexistent/repo/path/xyz")).is_none());
    }

    #[test]
    fn detects_a_changed_file() {
        let f = std::env::temp_dir().join(format!("lspgraph-f-{}.txt", std::process::id()));
        std::fs::write(&f, "one").unwrap();
        let mut cache = CacheFile::new(CallGraph::new());
        cache
            .file_hashes
            .insert(f.to_string_lossy().to_string(), hash_file(&f).unwrap());
        assert!(stale_files(&cache, Path::new("/")).is_empty());

        std::fs::write(&f, "two").unwrap();
        assert_eq!(stale_files(&cache, Path::new("/")).len(), 1);
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn a_deleted_file_counts_as_stale() {
        let mut cache = CacheFile::new(CallGraph::new());
        cache
            .file_hashes
            .insert("/definitely/not/here.rs".to_string(), "abc".to_string());
        assert_eq!(stale_files(&cache, Path::new("/")).len(), 1);
    }
}
```

Add to `crates/lspgraph-core/src/lib.rs`:

```rust
pub mod cache;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lspgraph-core cache`
Expected: `cache_dir_is_stable_and_repo_specific` passes; the other five fail by panicking on `not yet implemented`.

- [ ] **Step 3: Implement `load` and `stale_files`**

Replace both bodies:

```rust
pub fn load(repo: &Path) -> Option<CacheFile> {
    let path = cache_dir_for(repo).join("graph.json");
    let bytes = std::fs::read(path).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    // Spec 5.4: discard, never migrate.
    if cache.schema != SCHEMA_VERSION {
        return None;
    }
    Some(cache)
}

pub fn stale_files(cache: &CacheFile, _repo: &Path) -> Vec<String> {
    cache
        .file_hashes
        .iter()
        .filter(|(path, cached)| match hash_file(Path::new(path)) {
            Ok(current) => &current != *cached,
            Err(_) => true, // unreadable or deleted counts as stale
        })
        .map(|(path, _)| path.clone())
        .collect()
}
```

`CallGraph` must derive `Serialize`/`Deserialize` for this to compile; it already does from Task 7. Its private fields serialize by field name, which is what the `discards_a_mismatched_schema` fixture assumes.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lspgraph-core cache`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lspgraph-core/src/cache.rs crates/lspgraph-core/src/lib.rs
git commit -m "Add cache persistence with content-hash invalidation

Schema mismatch discards rather than migrates; the cache is never
authoritative, so a cold cache costs time and not correctness."
```

---

### Task 10: End-to-end example and integration tests

**Files:**
- Create: `crates/lspgraph-core/examples/crawl.rs`
- Create: `crates/lspgraph-core/tests/integration_servers.rs`
- Create: `crates/lspgraph-core/fixtures/rust-fixture/Cargo.toml`
- Create: `crates/lspgraph-core/fixtures/rust-fixture/src/lib.rs`
- Create: `crates/lspgraph-core/fixtures/ts-fixture/tsconfig.json`
- Create: `crates/lspgraph-core/fixtures/ts-fixture/src/index.ts`
- Create: `crates/lspgraph-core/fixtures/py-fixture/app.py`
- Create: `servers.toml`
- Create: `README.md`

**Interfaces:**
- Consumes: everything from Tasks 1–9.
- Produces: a runnable `cargo run --example crawl -- <lang> <root>`; integration tests that skip when a server is absent.

Fixtures must contain the awkward cases the probe surfaced — anonymous callbacks, overload signatures, arrow-function assignments — because those are what regress silently.

Integration tests **skip** rather than fail when a server is missing, so `cargo test` stays green on a machine with no language servers. CI installs all three.

- [ ] **Step 1: Write the fixtures**

`crates/lspgraph-core/fixtures/rust-fixture/Cargo.toml`:

```toml
[package]
name = "rust-fixture"
version = "0.1.0"
edition = "2021"

[dependencies]
```

`crates/lspgraph-core/fixtures/rust-fixture/src/lib.rs`:

```rust
pub fn leaf(x: i32) -> i32 {
    x + 1
}

pub fn middle(x: i32) -> i32 {
    leaf(x) + leaf(x + 1)
}

pub fn root(x: i32) -> i32 {
    middle(x)
}

// An anonymous closure: must not appear as a named callable.
pub fn with_closure(v: &[i32]) -> Vec<i32> {
    v.iter().map(|n| n * 2).collect()
}
```

`crates/lspgraph-core/fixtures/ts-fixture/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true
  },
  "include": ["src"]
}
```

`crates/lspgraph-core/fixtures/ts-fixture/src/index.ts`:

```typescript
export function leaf(x: number): number {
  return x + 1;
}

export function middle(x: number): number {
  return leaf(x) + leaf(x + 1);
}

export function root(x: number): number {
  return middle(x);
}

// Overload signatures: resolve to nothing, and that is correct.
export function over(x: number): number;
export function over(x: string): string;
export function over(x: any): any {
  return x;
}

// Arrow-function assignment: also legitimately unresolvable.
export const arrow = (x: number): number => x * 2;

// Anonymous callbacks: documentSymbol synthesizes names for these.
export function withCallbacks(v: number[]): number[] {
  return v.map((n) => n * 2).filter((n) => n > 2);
}
```

`crates/lspgraph-core/fixtures/py-fixture/app.py`:

```python
def leaf(x):
    return x + 1


def middle(x):
    return leaf(x) + leaf(x + 1)


def root(x):
    return middle(x)


def with_lambda(values):
    return list(map(lambda n: n * 2, values))
```

`servers.toml` at the repository root:

```toml
[rust]
cmd = "rust-analyzer"
extensions = ["rs"]

[typescript]
cmd = "vtsls --stdio"
extensions = ["ts", "tsx"]

[python]
cmd = "basedpyright-langserver --stdio"
extensions = ["py"]
```

- [ ] **Step 2: Write the integration tests**

Create `crates/lspgraph-core/tests/integration_servers.rs`:

```rust
//! Integration tests against real language servers.
//!
//! Each test skips when its server is absent, so `cargo test` stays green on
//! a machine with no language servers installed. CI installs all three.

use lspgraph_core::config::Config;
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::NodeState;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::LanguageServer;
use lspgraph_core::symbols::{collect_named_callables, NamedCallable};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn have(prog: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {prog}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn servers_toml() -> Config {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../servers.toml"),
    )
    .expect("servers.toml");
    Config::from_toml(&text).expect("valid servers.toml")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn source_files(root: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("node_modules") && !p.ends_with("target") {
                    stack.push(p);
                }
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Bring a server up, enumerate, and expand `root` -> `middle` -> `leaf`.
fn assert_three_level_chain(lang: &str, ext: &str, fixture_dir: &str, prog: &str) {
    if !have(prog) {
        eprintln!("skipping {lang}: {prog} not installed");
        return;
    }
    let cfg = servers_toml();
    let server_cfg = &cfg.servers[lang];
    let root_dir = fixture(fixture_dir);
    let files = source_files(&root_dir, ext);
    assert!(!files.is_empty(), "no {ext} fixture files");

    let server = LanguageServer::start(lang, server_cfg, &root_dir).expect("server starts");
    wait_until_ready(
        &server,
        &files,
        lang,
        &ReadinessConfig { timeout: Duration::from_secs(180), ..Default::default() },
    )
    .expect("server becomes ready");

    let mut candidates: Vec<NamedCallable> = Vec::new();
    for f in &files {
        server.open(f, lang).ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(
                &syms,
                &lspgraph_core::server::path_to_uri(f),
                &mut candidates,
            );
        }
    }

    // Anonymous callbacks must never survive enumeration.
    for c in &candidates {
        assert!(
            !c.name.contains("callback") && !c.name.contains('('),
            "anonymous callback leaked into enumeration: {}",
            c.name
        );
    }

    let middle = candidates
        .iter()
        .find(|c| c.name == "middle")
        .expect("fixture defines middle")
        .clone();

    let mut engine = Engine::new(server);
    let id = engine.seed(&middle).expect("seed ok").expect("middle resolves");
    let exp = engine.expand(&id).expect("expand ok");

    assert!(
        exp.callers.iter().any(|c| c.name == "root"),
        "middle should be called by root, got {:?}",
        exp.callers
    );
    assert!(
        exp.callees.iter().any(|c| c.name == "leaf"),
        "middle should call leaf, got {:?}",
        exp.callees
    );

    // Memoization: a second expand must not hit the server again.
    let before = engine.expansions_performed();
    let again = engine.expand(&id).expect("second expand ok");
    assert_eq!(engine.expansions_performed(), before, "second expand must be cached");
    assert_eq!(again, exp);

    assert_eq!(engine.graph().get(&id).unwrap().state, NodeState::Expanded);
}

#[test]
fn rust_call_chain() {
    assert_three_level_chain("rust", "rs", "rust-fixture", "rust-analyzer");
}

#[test]
fn typescript_call_chain() {
    assert_three_level_chain("typescript", "ts", "ts-fixture", "vtsls");
}

#[test]
fn python_call_chain() {
    assert_three_level_chain("python", "py", "py-fixture", "basedpyright-langserver");
}

#[test]
fn typescript_unresolvable_symbols_are_recorded_not_dropped() {
    if !have("vtsls") {
        eprintln!("skipping: vtsls not installed");
        return;
    }
    let cfg = servers_toml();
    let root_dir = fixture("ts-fixture");
    let files = source_files(&root_dir, "ts");
    let server =
        LanguageServer::start("typescript", &cfg.servers["typescript"], &root_dir).unwrap();
    wait_until_ready(
        &server,
        &files,
        "typescript",
        &ReadinessConfig { timeout: Duration::from_secs(180), ..Default::default() },
    )
    .unwrap();

    let mut candidates = Vec::new();
    for f in &files {
        server.open(f, "typescript").ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(
                &syms,
                &lspgraph_core::server::path_to_uri(f),
                &mut candidates,
            );
        }
    }

    let mut engine = Engine::new(server);
    let mut unresolved = 0;
    for c in &candidates {
        if engine.seed(c).unwrap().is_none() {
            unresolved += 1;
        }
    }

    // Spec 5.5: whatever failed to resolve is still present in the graph.
    assert!(
        engine.graph().len() >= candidates.len() - unresolved,
        "unresolved symbols must be recorded, not dropped"
    );
}
```

- [ ] **Step 3: Run the integration tests**

Run: `cargo test -p lspgraph-core --test integration_servers -- --nocapture`
Expected: on this machine rust-analyzer, vtsls and basedpyright are all present, so all four tests pass. On a machine missing a server, that test prints `skipping ...` and passes.

- [ ] **Step 4: Write the example driver**

Create `crates/lspgraph-core/examples/crawl.rs`:

```rust
//! End-to-end driver: crawl a repository's call graph and print a summary.
//!
//! Usage: cargo run --example crawl -- <language> <root>

use lspgraph_core::config::Config;
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::NodeState;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::{path_to_uri, LanguageServer};
use lspgraph_core::symbols::{collect_named_callables, NamedCallable};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
                .map(|e| exts.iter().any(|x| x == e))
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
    let lang = args.next().ok_or("usage: crawl <language> <root>")?;
    let root = PathBuf::from(args.next().ok_or("usage: crawl <language> <root>")?).canonicalize()?;

    let cfg = Config::from_toml(&std::fs::read_to_string("servers.toml")?)?;
    let server_cfg = cfg.servers.get(&lang).ok_or("unknown language")?;
    let files = source_files(&root, &server_cfg.extensions);
    println!("{} files", files.len());

    let t0 = Instant::now();
    let server = LanguageServer::start(&lang, server_cfg, &root)?;
    wait_until_ready(&server, &files, &lang, &ReadinessConfig::default())?;
    println!("ready in {:.1}s", t0.elapsed().as_secs_f64());

    let t1 = Instant::now();
    let mut candidates: Vec<NamedCallable> = Vec::new();
    for f in &files {
        server.open(f, &lang).ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(&syms, &path_to_uri(f), &mut candidates);
        }
    }
    println!(
        "{} named callables in {:.1}s",
        candidates.len(),
        t1.elapsed().as_secs_f64()
    );

    let t2 = Instant::now();
    let mut engine = Engine::new(server);
    let (mut resolved, mut unresolved) = (0usize, 0usize);
    for c in candidates.iter().take(120) {
        match engine.seed(c)? {
            Some(id) => {
                engine.expand(&id)?;
                resolved += 1;
            }
            None => unresolved += 1,
        }
    }
    let elapsed = t2.elapsed().as_secs_f64();
    println!(
        "expanded {resolved}, unresolved {unresolved} in {elapsed:.1}s ({:.1} sym/s)",
        resolved as f64 / elapsed.max(0.001)
    );

    let leaves = engine
        .graph()
        .nodes_in_file(&path_to_uri(&files[0]).to_string())
        .into_iter()
        .filter(|id| {
            matches!(
                engine.graph().get(id).map(|n| &n.state),
                Some(NodeState::Unresolved(_))
            )
        })
        .count();
    println!("unresolved nodes recorded in first file: {leaves}");
    Ok(())
}
```

- [ ] **Step 5: Run the example against a fixture and a real repository**

Run: `cargo run --example crawl -- rust crates/lspgraph-core/fixtures/rust-fixture`
Expected: prints a file count, a ready time, a named-callable count of 4 (`leaf`, `middle`, `root`, `with_closure` — the closure itself must not appear), and an expansion summary.

Then run it against any larger checkout available locally to sanity-check throughput against the spec's §3 numbers. Do not treat a mismatch as a failure; record it.

- [ ] **Step 6: Write the README**

Create `README.md`:

```markdown
# lspgraph

Explore a codebase's call graph by driving any LSP server's call hierarchy API.

Sourcetrail did this well and was archived in 2021. It maintained a
hand-written indexer per language, which capped it at four languages and made
maintenance unsustainable. lspgraph writes no indexers: it is an LSP client, so
every language with a language server is reachable.

## Status

Early. `lspgraph-core` is the engine; the TUI is not written yet.

## Server support

Adding a language means adding a table to `servers.toml`. No code.

| Server | Status |
|--------|--------|
| rust-analyzer | validated in CI |
| vtsls | validated in CI |
| basedpyright | validated in CI |
| anything else implementing `callHierarchyProvider` | expected to work, untested |

Servers listed as untested are untested. They are not claimed to be supported.

## Try it

```sh
cargo run --example crawl -- rust /path/to/a/cargo/project
```

## License

MIT OR Apache-2.0
```

- [ ] **Step 7: Run the whole suite**

Run: `cargo test --workspace`
Expected: all unit tests plus the four integration tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/lspgraph-core/examples crates/lspgraph-core/tests \
        crates/lspgraph-core/fixtures servers.toml README.md
git commit -m "Add fixtures, integration tests, and end-to-end crawl example

Fixtures cover the cases that regress silently: anonymous callbacks,
overload signatures, and arrow-function assignments. Integration tests
skip when a server is absent so the suite stays green without one."
```

---

## Self-Review

**Spec coverage.** Every section maps to a task: §5.1 lazy expansion → Task 8; §5.2 readiness → Task 6; §5.3 enumeration filter → Task 5; §5.4 identity, invalidation and cache location → Tasks 7 and 9; §5.5 unresolved nodes → Tasks 7, 8, 10; §5.6 concurrency default → Task 3 (`concurrency` defaults to 1, asserted); §6 configuration → Task 3; §7 error handling → Task 1 (`Error` variants), Task 4 (capability gate), Task 2 (timeouts); §8 testing → Task 10.

**Two spec items are deliberately not implemented here**, and both are recorded rather than silently skipped:

- §5.6's *benchmark* to establish a real concurrency number. The configurable limit exists and defaults to serial as specified, but the benchmark that would justify changing it needs a large repository, which §9 already flags as the project's main unverified risk. It belongs with that work, not here.
- The background cache warmer implied by §5.1. `Engine::expand` is the primitive it would call; the warmer itself is a scheduling concern that belongs with the TUI, since only a UI can usefully cancel it.

**Placeholder scan.** No "TBD", no "add error handling", no "similar to Task N". Every code step carries real code. The `todo!()` markers are deliberate TDD scaffolding, each removed within its own task.

**Type consistency.** `NodeId` fields are `uri: String, line, character, name` everywhere. `incoming_calls`/`outgoing_calls` return `Vec<CallHierarchyItem>` in Task 4 and are consumed as such in Task 8. `NamedCallable { name, uri, position }` is produced in Task 5 and consumed unchanged in Tasks 6, 8 and 10. `Expansion { callers, callees }` is defined in Task 7 and returned by Task 8. `ReadinessConfig` defaults asserted in Task 6 match the values used in Task 10. `CallGraph` derives `Serialize`/`Deserialize` in Task 7, which Task 9 requires.

**One known ordering hazard.** Task 4's unit tests pass while `start` is still `todo!()`, because they cover only pure helpers. An executor who stops at a green test run would ship an unimplemented `start`. Step 4 of that task explicitly says to confirm no `todo!` remains in the file.
