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

/// Owns a spawned child for the duration of `start`. If `start` returns
/// early for any reason (an early `?`, the capability gate, or anything
/// added later), `Drop` kills and reaps the process so a rejected or
/// failed-to-initialize server is never leaked. The success path calls
/// `disarm` to take the `Child` back out without killing it.
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
            return Err(Error::NoCallHierarchy { server: name.to_string() });
        }

        conn.notify("initialized", json!({}))?;

        let child = guard.disarm();
        Ok(LanguageServer {
            name: name.to_string(),
            root: root.to_path_buf(),
            conn,
            child,
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

    #[cfg(unix)]
    #[test]
    fn failed_start_does_not_leak_the_child_process() {
        use std::os::unix::fs::PermissionsExt;

        let pid = std::process::id();
        let script_name = format!("lspgraph-fake-server-{pid}.sh");
        let script_path = std::env::temp_dir().join(&script_name);

        // `cat` echoes our framed `initialize` request back to us. The
        // reader loop treats the echo (an id + method, no result/error) as a
        // server-initiated request and auto-replies with a null result,
        // which `cat` echoes again — this time it *is* a response, so it
        // resolves our `initialize` call with `null` capabilities, failing
        // the call-hierarchy gate quickly instead of timing out. If `start`
        // did not kill the child, it would sit in `cat` (no EOF ever
        // arrives) and then `sleep 300`.
        std::fs::write(&script_path, "#!/bin/sh\ncat\nsleep 300\n").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cfg = ServerConfig {
            cmd: script_path.display().to_string(),
            extensions: vec![],
            ready_timeout_secs: 300,
            concurrency: 1,
        };
        let root = std::env::temp_dir();

        let result = LanguageServer::start("fake", &cfg, &root);
        assert!(result.is_err(), "expected start to fail against the fake server");

        // Poll briefly rather than asserting immediately, so the test isn't
        // racing the kill/wait done by ChildGuard's Drop.
        let mut still_running = true;
        for _ in 0..20 {
            let found = Command::new("pgrep")
                .arg("-f")
                .arg(&script_name)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !found {
                still_running = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = std::fs::remove_file(&script_path);

        assert!(!still_running, "fake server process {script_name} was leaked");
    }
}
