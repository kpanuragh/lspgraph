//! Language server process lifecycle and the LSP calls the engine needs.

use crate::config::ServerConfig;
use crate::error::{Error, Result};
use crate::symbols::SymbolMatch;
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
    /// `None` once the child has been killed and reaped, which is what makes
    /// an explicit `shutdown()` and the `Drop` below compose: whichever runs
    /// first takes the `Child`, and the other becomes a no-op.
    child: Option<Child>,
    /// Whether `initialize` advertised `workspaceSymbolProvider`. Search must
    /// be able to say "this server cannot search" rather than show an empty
    /// list, which would be indistinguishable from "nothing matched".
    supports_workspace_symbol: bool,
}

/// A server that is merely dropped must still die.
///
/// Dropping the `Connection` is not enough on its own: the language server
/// only sees EOF on its stdin once every writer handle is gone, and even then
/// a server that ignores EOF would linger. Without this, a caller that drops
/// a `LanguageServer` (or an `Engine` wrapping one) leaks the process — for
/// rust-analyzer, multiple gigabytes per leak — with nothing but process exit
/// to clean it up.
impl Drop for LanguageServer {
    fn drop(&mut self) {
        Self::kill_and_reap(&mut self.child);
    }
}

/// Owns a spawned child for the duration of `start`. If `start` returns
/// early for any reason (an early `?`, the capability gate, or anything
/// added later), `Drop` kills and reaps the process so a rejected or
/// failed-to-initialize server is never leaked. The success path calls
/// `disarm` to hand the `Child` over to the `LanguageServer`, whose own
/// `Drop` takes over the same responsibility from there.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> ChildGuard {
        ChildGuard(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child present while guard is armed")
    }

    /// Take the child out without killing it. Used only on the success path.
    fn disarm(mut self) -> Child {
        self.0.take().expect("child present while guard is armed")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl LanguageServer {
    pub fn start(name: &str, cfg: &ServerConfig, root: &Path) -> Result<LanguageServer> {
        let (prog, args) = cfg.command();
        let child = Command::new(&prog)
            .args(&args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut guard = ChildGuard::new(child);

        let stdout = guard.child_mut().stdout.take().ok_or(Error::ServerExited)?;
        let stdin = guard.child_mut().stdin.take().ok_or(Error::ServerExited)?;
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
                    "workspace": {
                        "workspaceFolders": true,
                        "symbol": {"dynamicRegistration": false}
                    }
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
            return Err(Error::NoCallHierarchy { server: name.to_string() });
        }

        let supports_workspace_symbol = caps
            .get("capabilities")
            .and_then(|c| c.get("workspaceSymbolProvider"))
            .map(|v| v != &Value::Bool(false) && !v.is_null())
            .unwrap_or(false);

        conn.notify("initialized", json!({}))?;

        let child = guard.disarm();
        Ok(LanguageServer {
            name: name.to_string(),
            root: root.to_path_buf(),
            conn,
            child: Some(child),
            supports_workspace_symbol,
        })
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

    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        pos: Position,
    ) -> Result<Vec<CallHierarchyItem>> {
        let v = self.request_retrying_content_modified(
            "textDocument/prepareCallHierarchy",
            json!({"textDocument": {"uri": path_to_uri(path)}, "position": pos}),
        )?;
        if v.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(v)?)
    }

    pub fn incoming_calls(&self, item: &CallHierarchyItem) -> Result<Vec<CallHierarchyItem>> {
        let v = self.request_retrying_content_modified(
            "callHierarchy/incomingCalls",
            json!({"item": item}),
        )?;
        Ok(unwrap_calls(v, "from"))
    }

    pub fn outgoing_calls(&self, item: &CallHierarchyItem) -> Result<Vec<CallHierarchyItem>> {
        let v = self.request_retrying_content_modified(
            "callHierarchy/outgoingCalls",
            json!({"item": item}),
        )?;
        Ok(unwrap_calls(v, "to"))
    }

    /// The LSP spec defines error code -32801 (ContentModified) as transient
    /// and explicitly sanctions the client reissuing the request, unlike
    /// every other error code. Used only for the call-hierarchy requests,
    /// which are the ones observed to receive it against a live server.
    fn request_retrying_content_modified(&self, method: &str, params: Value) -> Result<Value> {
        const MAX_ATTEMPTS: u32 = 3; // initial attempt + 2 retries
        const RETRY_DELAY: Duration = Duration::from_millis(150);

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.conn.request(method, params.clone(), REQUEST_TIMEOUT) {
                Err(Error::ContentModified { .. }) if attempt < MAX_ATTEMPTS => {
                    std::thread::sleep(RETRY_DELAY);
                }
                other => return other,
            }
        }
    }

    /// Ask the server to exit politely, then make sure it actually has.
    ///
    /// Taking the `Child` here leaves `Drop` with nothing to do, so calling
    /// `shutdown()` and then dropping neither double-kills nor panics.
    pub fn shutdown(mut self) -> Result<()> {
        let _ = self.conn.request("shutdown", Value::Null, Duration::from_secs(5));
        let _ = self.conn.notify("exit", Value::Null);
        Self::kill_and_reap(&mut self.child);
        Ok(())
    }

    /// Kill and reap the child if it is still ours to kill. Errors are
    /// ignored: a server that already exited is exactly the desired state,
    /// and this runs from `Drop`, where there is nobody to report to.
    fn kill_and_reap(slot: &mut Option<Child>) {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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

    #[cfg(unix)]
    mod process_lifecycle {
        use super::*;
        use crate::testutil::{sh_send_frame, FakeServerScript, INITIALIZE_OK, SH_IDLE};

        #[test]
        fn failed_start_does_not_leak_the_child_process() {
            // `cat` echoes our framed `initialize` request back to us. The
            // reader loop treats the echo (an id + method, no result/error) as
            // a server-initiated request and auto-replies with a null result,
            // which `cat` echoes again — this time it *is* a response, so it
            // resolves our `initialize` call with `null` capabilities, failing
            // the call-hierarchy gate quickly instead of timing out. If `start`
            // did not kill the child, it would sit in `cat` (no EOF ever
            // arrives) and then `sleep 300`.
            let script = FakeServerScript::new("failed-start", "cat\nsleep 300\n");

            // `LanguageServer` is not `Debug`, so unwrap the error side; a
            // successful start would simply be dropped (and killed) here.
            let err = script
                .start()
                .err()
                .expect("start must fail against the fake server");
            assert!(
                matches!(err, Error::NoCallHierarchy { .. }),
                "expected the capability gate to reject the fake server, got {err:?}"
            );

            assert!(
                script.no_process_survives(),
                "fake server process {} was leaked",
                script.name()
            );
        }

        /// A script that initializes successfully and then idles forever
        /// without ever reading its stdin, so the only thing that can end it
        /// is somebody killing it.
        fn long_lived_script(tag: &str) -> FakeServerScript {
            FakeServerScript::new(tag, &format!("{}{SH_IDLE}", sh_send_frame(INITIALIZE_OK)))
        }

        #[test]
        fn dropping_a_started_server_kills_the_child_process() {
            // A `LanguageServer` that is merely dropped — no `shutdown()` —
            // must not leave the server running. A TUI switching repositories
            // would otherwise accumulate one full language server per switch.
            let script = long_lived_script("drop-leak");
            let server = script.start().expect("fake server initializes");
            assert!(script.is_running(), "fake server should be up before the drop");

            drop(server);

            assert!(
                script.no_process_survives(),
                "dropped server leaked process {}",
                script.name()
            );
        }

        #[test]
        fn dropping_an_engine_kills_the_wrapped_server() {
            // The same guarantee has to survive being wrapped in an `Engine`,
            // which is how every real caller holds a server.
            let script = long_lived_script("engine-drop-leak");
            let server = script.start().expect("fake server initializes");
            let engine = crate::engine::Engine::new(server);
            assert!(script.is_running(), "fake server should be up before the drop");

            drop(engine);

            assert!(
                script.no_process_survives(),
                "dropped engine leaked process {}",
                script.name()
            );
        }

        #[test]
        fn explicit_shutdown_kills_the_child_and_the_later_drop_is_a_no_op() {
            // `shutdown()` consumes the server, so its `Drop` runs immediately
            // afterwards. It must not double-kill or panic on an absent child.
            let script = long_lived_script("shutdown");
            let server = script.start().expect("fake server initializes");

            server.shutdown().expect("shutdown reports success");

            assert!(
                script.no_process_survives(),
                "shutdown left process {} running",
                script.name()
            );
        }

        #[test]
        fn engine_shutdown_kills_the_wrapped_server() {
            let script = long_lived_script("engine-shutdown");
            let server = script.start().expect("fake server initializes");

            crate::engine::Engine::new(server)
                .shutdown()
                .expect("engine shutdown reports success");

            assert!(
                script.no_process_survives(),
                "engine shutdown left process {} running",
                script.name()
            );
        }

        #[test]
        fn into_server_hands_the_server_back() {
            let script = long_lived_script("into-server");
            let server = script.start().expect("fake server initializes");

            let server = crate::engine::Engine::new(server).into_server();
            assert_eq!(server.name, "fake");
            server.shutdown().expect("shutdown reports success");

            assert!(
                script.no_process_survives(),
                "process {} survived into_server + shutdown",
                script.name()
            );
        }
    }
}
