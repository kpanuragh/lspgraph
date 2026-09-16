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
                loop {
                    match dec.next_message() {
                        Ok(Some(body)) => {
                            let msg: Value = match serde_json::from_slice(&body) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let id = msg.get("id").and_then(|v| v.as_i64());
                            let is_response =
                                msg.get("result").is_some() || msg.get("error").is_some();

                            if let (Some(id), true) = (id, is_response) {
                                if let Some(sender) = pending_r.lock().unwrap().remove(&id) {
                                    let _ = sender.send(msg);
                                }
                            } else if let Some(method) =
                                msg.get("method").and_then(|m| m.as_str())
                            {
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
                        Ok(None) => break,
                        // A server emitting unparseable frames is not recoverable for
                        // this connection. Breaking the outer read loop drops the
                        // pending senders, so in-flight requests resolve to
                        // Error::ServerExited rather than hanging.
                        Err(_) => return,
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
