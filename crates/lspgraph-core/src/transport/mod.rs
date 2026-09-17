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

/// Drain `reader` and dispatch framed messages until EOF or an unrecoverable
/// framing error. Returns in either case; the caller is responsible for
/// clearing `pending` afterwards so in-flight requests unblock.
fn read_loop(
    reader: &mut (impl Read + ?Sized),
    pending: &Pending,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    tx: &Sender<Notification>,
) {
    let mut dec = codec::Decoder::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
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
                    // JSON-RPC allows an id to be a number OR a string, and
                    // `"id": null` is not an id at all. Keep it as a `&Value`
                    // so a server-initiated request can be answered with its
                    // id echoed back verbatim, whatever its type: a server
                    // that uses string ids would otherwise never see a reply
                    // and could block indefinitely.
                    let id = msg.get("id").filter(|v| !v.is_null());
                    let is_response = msg.get("result").is_some() || msg.get("error").is_some();

                    // Response correlation stays integer-only on purpose: the
                    // only ids we correlate are ones we generated ourselves,
                    // and those are always integers.
                    if let (Some(id), true) = (id.and_then(Value::as_i64), is_response) {
                        if let Some(sender) = pending.lock().unwrap().remove(&id) {
                            let _ = sender.send(msg);
                        }
                    } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                        match id {
                            // Server->client request: must be answered.
                            Some(id) => {
                                let reply = json!({"jsonrpc":"2.0","id":id,"result":Value::Null});
                                let bytes = serde_json::to_vec(&reply).unwrap_or_default();
                                let mut w = writer.lock().unwrap();
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
                // A server emitting an unparseable frame is not recoverable for
                // this connection.
                Err(_) => return,
            }
        }
    }
}

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
            // `read_loop` has multiple early-return paths (EOF, an unparseable
            // frame). Whichever way it exits, the pending map must be cleared
            // afterwards so every blocked `request` call observes a dropped
            // sender (`RecvTimeoutError::Disconnected` -> `Error::ServerExited`)
            // immediately, instead of waiting out its full timeout.
            read_loop(&mut reader, &pending_r, &writer_r, &tx);
            // Release this thread's writer handle explicitly. The reader
            // thread holds a clone of the writer `Arc` only so it can answer
            // server->client requests; holding it past the end of the loop
            // would keep the child's stdin open forever, so dropping the
            // `Connection` would never let the child see EOF (it would never
            // exit, and nothing would ever close this loop).
            drop(writer_r);
            if let Ok(mut p) = pending_r.lock() {
                p.clear();
            }
        });

        Connection {
            writer,
            pending,
            next_id: AtomicI64::new(0),
            notifications,
        }
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
                    let code = e.get("code").and_then(|v| v.as_i64());
                    if code == Some(-32801) {
                        return Err(Error::ContentModified {
                            method: method.to_string(),
                        });
                    }
                    let text = e
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    return Err(Error::Protocol(format!("{method}: {text}")));
                }
                Ok(m.get("result").cloned().unwrap_or(Value::Null))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::Timeout {
                    method: method.to_string(),
                    timeout,
                })
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

    /// Opens once the client has written something. The canned reader waits
    /// on this so it cannot answer a request that has not been made yet —
    /// which is the one thing a real server also cannot do. Deterministic
    /// alternative to sleeping a fixed delay: the reader thread starts as
    /// soon as `Connection::new` returns, and without this gate it can race
    /// `Connection::request` and remove-and-send a response before
    /// `request` has registered the id it is replying to, silently
    /// dropping the response and leaving the caller to time out instead of
    /// seeing the canned answer.
    #[derive(Clone, Default)]
    struct Handshake(std::sync::Arc<(Mutex<bool>, std::sync::Condvar)>);

    impl Handshake {
        fn opened(&self) {
            let (lock, cv) = &*self.0;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        fn wait(&self) {
            let (lock, cv) = &*self.0;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }
    }

    /// A fake server: reads framed requests, replies per a canned table.
    struct Duplex {
        to_client: Cursor<Vec<u8>>,
        handshake: Handshake,
        delivered: bool,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.delivered {
                self.delivered = true;
                self.handshake.wait();
            }
            self.to_client.read(buf)
        }
    }

    /// A writer that records everything the client sends, and opens
    /// `handshake` on write so a paired `Duplex` knows a request has
    /// actually been made before it delivers the response to it.
    #[derive(Default)]
    struct Tee {
        sent: Arc<Mutex<Vec<u8>>>,
        handshake: Handshake,
    }

    impl Write for Tee {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.sent.lock().unwrap().extend_from_slice(b);
            self.handshake.opened();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Builds a fake server plus the writer that gates it: the returned
    /// `Duplex` will not deliver its first byte until the returned `Tee` is
    /// written to.
    fn canned(messages: &[Value]) -> (Duplex, Tee) {
        let mut bytes = Vec::new();
        for m in messages {
            bytes.extend_from_slice(&codec::encode(serde_json::to_string(m).unwrap().as_bytes()));
        }
        let writer = Tee::default();
        let server = Duplex {
            to_client: Cursor::new(bytes),
            handshake: writer.handshake.clone(),
            delivered: false,
        };
        (server, writer)
    }

    /// A reader whose `read` blocks on a channel, so a test controls exactly
    /// when data arrives and when EOF happens: `Some(bytes)` yields data,
    /// `None` (or the sender being dropped) reports EOF via `Ok(0)`.
    struct BlockingReader(Receiver<Option<Vec<u8>>>);

    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.recv() {
                Ok(Some(bytes)) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Ok(None) | Err(_) => Ok(0),
            }
        }
    }

    #[test]
    fn resolves_a_response_by_id() {
        let (server, writer) = canned(&[json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})]);
        let conn = Connection::new(server, writer);
        let got = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(got, json!({"ok": true}));
    }

    #[test]
    fn surfaces_an_error_response() {
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no such method"}}),
        ]);
        let conn = Connection::new(server, writer);
        let err = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert!(matches!(err, Error::Protocol(m) if m.contains("no such method")));
    }

    #[test]
    fn content_modified_error_code_is_distinguished_from_protocol_errors() {
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32801,"message":"content modified"}}),
        ]);
        let conn = Connection::new(server, writer);
        let err = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert!(matches!(err, Error::ContentModified { method } if method == "x"));
    }

    #[test]
    fn other_error_codes_still_surface_as_protocol_errors() {
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no such method"}}),
        ]);
        let conn = Connection::new(server, writer);
        let err = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[test]
    fn routes_notifications_to_the_channel() {
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"hi"}}),
        ]);
        // No client request is made in this test, so nothing will write to
        // `writer` and open the handshake on its own; a notification is not
        // a reply to anything, so open it directly.
        writer.handshake.opened();
        let conn = Connection::new(server, writer);
        let n = conn
            .notifications()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(n.method, "window/logMessage");
    }

    #[test]
    fn times_out_when_no_response_arrives() {
        // The reader stays open (never EOFs) so the timeout path is exercised
        // deterministically rather than racing a from-EOF `ServerExited`.
        let (_data_tx, data_rx) = channel::<Option<Vec<u8>>>();
        let conn = Connection::new(BlockingReader(data_rx), Vec::new());
        let err = conn
            .request("x", json!({}), Duration::from_millis(80))
            .unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }));
    }

    #[test]
    fn reader_death_resolves_inflight_requests_as_server_exited() {
        let (data_tx, data_rx) = channel::<Option<Vec<u8>>>();
        let conn = Connection::new(BlockingReader(data_rx), Vec::new());

        // Trigger EOF (via a helper thread that only owns the data-channel
        // sender, not the Connection) shortly after the request is issued.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = data_tx.send(None);
        });

        let start = std::time::Instant::now();
        let err = conn
            .request("x", json!({}), Duration::from_secs(30))
            .unwrap_err();
        let elapsed = start.elapsed();

        assert!(matches!(err, Error::ServerExited), "got {err:?}");
        assert!(
            elapsed < Duration::from_secs(1),
            "should fail fast on reader death, took {elapsed:?}"
        );
    }

    #[test]
    fn answers_server_initiated_requests() {
        // A server->client request must be answered or the server can block.
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":99,"method":"workspace/configuration","params":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":"after"}),
        ]);
        let written = writer.sent.clone();
        let conn = Connection::new(server, writer);
        let got = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(got, json!("after"));

        let sent = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert!(
            sent.contains("\"id\":99"),
            "must reply to server request 99: {sent}"
        );
    }

    #[test]
    fn answers_a_server_request_carrying_a_string_id() {
        // JSON-RPC permits a string id. Matching ids with `as_i64` alone sent
        // such a request down the notification path, so it was never
        // answered and a server awaiting the reply could block forever.
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":"cfg-7","method":"workspace/configuration","params":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":"after"}),
        ]);
        let written = writer.sent.clone();
        let conn = Connection::new(server, writer);
        let got = conn
            .request("x", json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(got, json!("after"));

        let sent = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert!(
            sent.contains(r#""id":"cfg-7""#),
            "must echo the server's string id back verbatim: {sent}"
        );
        // ...and it must not have been mistaken for a notification.
        assert!(
            conn.notifications()
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "a server request must not be delivered as a notification"
        );
    }

    #[test]
    fn a_null_id_is_treated_as_a_notification() {
        // `"id": null` is not an id; answering it would be wrong.
        let (server, writer) = canned(&[
            json!({"jsonrpc":"2.0","id":Value::Null,"method":"window/logMessage","params":{}}),
        ]);
        // No client request is made in this test either; open the handshake
        // directly for the same reason as `routes_notifications_to_the_channel`.
        writer.handshake.opened();
        let written = writer.sent.clone();
        let conn = Connection::new(server, writer);
        let n = conn
            .notifications()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(n.method, "window/logMessage");
        assert!(
            written.lock().unwrap().is_empty(),
            "nothing should be replied"
        );
    }
}
