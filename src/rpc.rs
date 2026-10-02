//! Newline-delimited JSON-RPC 2.0 transport: id correlation and notification
//! fan-out over any `Read`/`Write` pair.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};

/// A JSON-RPC error object as sent by wuzapi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteError {
    pub code: i64,
    pub message: String,
}

/// A server-initiated, id-less message (wuzapi's "webhook" in stdio mode).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notification {
    pub method: String,
    pub params: Value,
}

/// One parsed line of wuzapi's stdout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Frame {
    Response {
        id: u64,
        outcome: Result<Value, RemoteError>,
    },
    Notification(Notification),
    /// Not JSON, not an object, or neither a response nor a notification
    /// (for example a bare log line, or an error reply with a null id).
    Unroutable(String),
}

/// Classify one line. Pure, so every malformed shape is unit-testable.
pub(crate) fn classify(line: &str) -> Frame {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
        return Frame::Unroutable("not a JSON object".into());
    };
    let id = obj.get("id").filter(|v| !v.is_null());
    match (obj.get("method").and_then(Value::as_str), id) {
        (Some(method), None) => Frame::Notification(Notification {
            method: method.to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) => {
            let Some(id) = parse_id(id) else {
                return Frame::Unroutable("response id is not an integer".into());
            };
            match obj.get("error").filter(|e| !e.is_null()) {
                Some(err) => Frame::Response {
                    id,
                    outcome: Err(RemoteError {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
                        message: err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string(),
                    }),
                },
                None if obj.contains_key("result") => Frame::Response {
                    id,
                    outcome: Ok(obj.get("result").cloned().unwrap_or(Value::Null)),
                },
                None => Frame::Unroutable("response without result or error".into()),
            }
        }
        _ => Frame::Unroutable("neither a response nor a notification".into()),
    }
}

fn parse_id(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

type Outcome = Result<Value, RemoteError>;

#[derive(Default)]
struct Pending {
    closed: bool,
    waiting: HashMap<u64, Sender<Outcome>>,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    subscribers: Mutex<Vec<Sender<Notification>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Request/response correlation plus notification fan-out.
pub(crate) struct Rpc {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    next_id: AtomicU64,
    shared: Arc<Shared>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Rpc {
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        let handle = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || read_loop(BufReader::new(reader), &shared))
        };
        Rpc {
            writer: Mutex::new(Some(Box::new(writer))),
            next_id: AtomicU64::new(1),
            shared,
            reader: Mutex::new(Some(handle)),
        }
    }

    /// Receive every notification that arrives from now on.
    pub fn subscribe(&self) -> Receiver<Notification> {
        let (tx, rx) = channel();
        lock(&self.shared.subscribers).push(tx);
        rx
    }

    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        {
            let mut pending = lock(&self.shared.pending);
            if pending.closed {
                return Err(Error::Disconnected);
            }
            pending.waiting.insert(id, tx);
        }
        let line = json!({ "id": id, "method": method, "params": params }).to_string();
        if let Err(e) = self.write_line(&line) {
            lock(&self.shared.pending).waiting.remove(&id);
            return Err(e);
        }
        let outcome = rx.recv_timeout(timeout);
        lock(&self.shared.pending).waiting.remove(&id);
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(remote)) => Err(Error::Remote {
                method: method.to_string(),
                code: remote.code,
                message: remote.message,
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout {
                method: method.to_string(),
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Error::Disconnected),
        }
    }

    fn write_line(&self, line: &str) -> Result<()> {
        let mut guard = lock(&self.writer);
        let writer = guard.as_mut().ok_or(Error::Disconnected)?;
        writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush())
            .map_err(|_| Error::Disconnected)
    }

    /// Close our end of the child's stdin; wuzapi exits on EOF.
    pub fn close_writer(&self) {
        lock(&self.writer).take();
    }

    /// Wait for the reader thread to see EOF (call after the peer is gone).
    pub fn join_reader(&self) {
        if let Some(handle) = lock(&self.reader).take() {
            let _ = handle.join();
        }
    }
}

fn read_loop<R: BufRead>(mut reader: R, shared: &Shared) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim();
        if line.is_empty() {
            continue;
        }
        match classify(line) {
            Frame::Response { id, outcome } => {
                let waiter = lock(&shared.pending).waiting.remove(&id);
                match waiter {
                    Some(tx) => {
                        let _ = tx.send(outcome);
                    }
                    None => log::debug!("response for unknown request id {id}"),
                }
            }
            Frame::Notification(n) => {
                log::debug!("notification {}", n.method);
                lock(&shared.subscribers).retain(|tx| tx.send(n.clone()).is_ok());
            }
            Frame::Unroutable(reason) => log::debug!("skipping frame: {reason}"),
        }
    }
    // EOF: fail every waiter and end every subscription.
    let mut pending = lock(&shared.pending);
    pending.closed = true;
    pending.waiting.clear();
    drop(pending);
    lock(&shared.subscribers).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_success_error_and_notification() {
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","id":3,"result":{"a":1}}"#),
            Frame::Response {
                id: 3,
                outcome: Ok(json!({"a": 1}))
            }
        );
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","id":"7","result":null}"#),
            Frame::Response {
                id: 7,
                outcome: Ok(Value::Null)
            }
        );
        assert_eq!(
            classify(r#"{"id":4,"error":{"code":501,"message":"nope"}}"#),
            Frame::Response {
                id: 4,
                outcome: Err(RemoteError {
                    code: 501,
                    message: "nope".into()
                })
            }
        );
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","method":"Message","params":{"x":1}}"#),
            Frame::Notification(Notification {
                method: "Message".into(),
                params: json!({"x": 1})
            })
        );
    }

    #[test]
    fn rejects_malformed_frames() {
        for line in [
            "",
            "garbage",
            "[1,2]",
            r#"{"id":null,"error":{"code":400,"message":"x"}}"#,
            r#"{"id":1}"#,
            r#"{"id":"abc","result":1}"#,
            r#"{"id":1,"method":"x"}"#,
        ] {
            assert!(matches!(classify(line), Frame::Unroutable(_)), "{line}");
        }
    }
}
