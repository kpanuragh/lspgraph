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
