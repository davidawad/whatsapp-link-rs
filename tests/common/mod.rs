//! An in-process fake of wuzapi's stdio JSON-RPC peer, built on OS pipes.
//! All data is synthetic.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};
use whatsapp_link::Wuzapi;

pub const USER_TOKEN: &str = "test-user-token";
pub const ADMIN_TOKEN: &str = "test-admin-token";
pub const CHAT: &str = "15550100@s.whatsapp.net";

pub type Log = Arc<Mutex<Vec<(String, Value)>>>;

/// Shared handle for pushing unsolicited lines from a test.
#[derive(Clone)]
pub struct Wire(Arc<Mutex<Option<std::io::PipeWriter>>>);

impl Wire {
    pub fn send(&self, line: &str) {
        if let Some(w) = self.0.lock().unwrap().as_mut() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }

    /// Simulate the peer dying: its stdout closes.
    pub fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

pub fn ok(id: u64, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

pub fn err(id: u64, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

pub fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// A `Message` notification carrying a synthetic whatsmeow event.
pub fn message_notification(id: &str, text: &str, ts: &str) -> String {
    notification(
        "Message",
        json!({"type": "Message", "event": message_event(id, text, ts)}),
    )
}

pub fn message_event(id: &str, text: &str, ts: &str) -> Value {
    json!({
        "Info": {"ID": id, "Chat": CHAT, "Sender": CHAT, "IsFromMe": false,
                 "IsGroup": false, "PushName": "Sam", "Timestamp": ts},
        "Message": {"conversation": text}
    })
}

pub fn reaction_event(id: &str, target: &str, emoji: &str, ts: &str) -> Value {
    json!({
        "Info": {"ID": id, "Chat": CHAT, "Sender": CHAT, "IsFromMe": false,
                 "PushName": "Sam", "Timestamp": ts},
        "Message": {"reactionMessage": {"key": {"ID": target}, "text": emoji}}
    })
}

/// A `chat.history` row as wuzapi stores it.
pub fn history_row(message_id: &str, ts: &str, event: &Value) -> Value {
    json!({
        "id": 1, "user_id": "u1", "chat_jid": CHAT, "sender_jid": CHAT,
        "message_id": message_id, "timestamp": ts, "message_type": "text",
        "text_content": "", "media_link": "", "data_json": event.to_string()
    })
}

/// Handles the provisioning calls every client makes first. Returns `None`
/// for anything else.
pub fn provisioning(id: u64, method: &str, params: &Value, existing: bool) -> Option<Vec<String>> {
    match method {
        "admin.users.list" => {
            assert_eq!(params["adminToken"], ADMIN_TOKEN);
            let users = if existing {
                json!([{ "id": "u1", "token": USER_TOKEN }])
            } else {
                json!([])
            };
            Some(vec![ok(id, users)])
        }
        "admin.users.add" => {
            assert_eq!(params["token"], USER_TOKEN);
            assert_eq!(params["adminToken"], ADMIN_TOKEN);
            Some(vec![ok(id, json!({ "id": "u1" }))])
        }
        _ => None,
    }
}

/// Start a fake peer. `handler(id, method, params)` returns the lines to emit
/// (responses and/or notifications, garbage included) in order.
pub fn start<F>(handler: F) -> (Wuzapi, Wire, Log)
where
    F: FnMut(u64, &str, &Value) -> Vec<String> + Send + 'static,
{
    start_with(true, handler)
}

pub fn start_with<F>(existing_user: bool, mut handler: F) -> (Wuzapi, Wire, Log)
where
    F: FnMut(u64, &str, &Value) -> Vec<String> + Send + 'static,
{
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    let wire = Wire(Arc::new(Mutex::new(Some(resp_w))));
    let log: Log = Arc::default();
    let _: JoinHandle<()> = {
        let (wire, log) = (wire.clone(), Arc::clone(&log));
        thread::spawn(move || {
            for line in BufReader::new(req_r).lines().map_while(Result::ok) {
                let v: Value = serde_json::from_str(&line).expect("client sent invalid JSON");
                let id = v["id"].as_u64().expect("numeric id");
                let method = v["method"].as_str().expect("method").to_string();
                let params = v["params"].clone();
                log.lock().unwrap().push((method.clone(), params.clone()));
                let out = provisioning(id, &method, &params, existing_user)
                    .unwrap_or_else(|| handler(id, &method, &params));
                for l in out {
                    wire.send(&l);
                }
            }
            // Client closed stdin: the peer exits, which closes its stdout.
            wire.close();
        })
    };
    let client = Wuzapi::from_streams(resp_r, req_w, USER_TOKEN, ADMIN_TOKEN)
        .expect("provisioning against the fake");
    (client, wire, log)
}

pub fn calls(log: &Log, method: &str) -> Vec<Value> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| p.clone())
        .collect()
}
