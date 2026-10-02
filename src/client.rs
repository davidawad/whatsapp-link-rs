//! The wuzapi client: spawn, provision, link, read, write.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::event::{Event, Events, SUBSCRIBED};
use crate::guard::DataDirGuard;
use crate::model::{
    assemble_history, parse_history_row, Chat, History, HistoryRow, Jid, MessageId, Recipient,
    SentMessage, Status,
};
use crate::qr::QrCode;
use crate::rpc::Rpc;

/// Messages wuzapi keeps per chat for a user created by this crate.
const HISTORY_PER_CHAT: u32 = 1000;
const USER_NAME: &str = "whatsapp-link";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Handle to a running wuzapi child (or to any peer speaking the same
/// JSON-RPC over a reader/writer pair). Dropping it shuts the child down.
pub struct Wuzapi {
    // Field order is drop order beyond `Drop::drop`: the guard is released last.
    rpc: Rpc,
    user_token: String,
    timeout: Duration,
    child: Option<Child>,
    _guard: Option<DataDirGuard>,
}

impl std::fmt::Debug for Wuzapi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wuzapi").finish_non_exhaustive()
    }
}

impl Wuzapi {
    /// Claim the data directory, start `wuzapi -mode=stdio`, and make sure the
    /// configured user exists.
    ///
    /// Fails with [`Error::DataDirBusy`] if another process already uses the
    /// data directory.
    pub fn spawn(config: Config) -> Result<Self> {
        let guard = DataDirGuard::acquire(&config.data_dir)?;
        let mut child = Command::new(&config.wuzapi_bin)
            .arg("-mode=stdio")
            .arg(format!("-datadir={}", guard.dir().display()))
            // Media payloads are never needed here, only their type tags.
            .arg("-skipmedia")
            .env("WUZAPI_ADMIN_TOKEN", &config.admin_token)
            .env("TZ", &config.timezone)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| Error::Spawn {
                bin: config.wuzapi_bin.clone(),
                source,
            })?;
        let (stdin, stdout, stderr) = (
            child.stdin.take().ok_or(Error::Disconnected)?,
            child.stdout.take().ok_or(Error::Disconnected)?,
            child.stderr.take(),
        );
        // wuzapi logs (which can include tokens) go to stderr: drain and drop.
        if let Some(mut stderr) = stderr {
            thread::spawn(move || {
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            });
        }
        let client = Wuzapi {
            rpc: Rpc::new(stdout, stdin),
            user_token: config.user_token,
            timeout: DEFAULT_TIMEOUT,
            child: Some(child),
            _guard: Some(guard),
        };
        // On error `client` drops here and reaps the child.
        client.ensure_user(&config.admin_token)?;
        Ok(client)
    }

    /// Speak the protocol over an arbitrary reader/writer pair (an already
    /// running peer, or a test double). No process is managed and no data
    /// directory is guarded.
    pub fn from_streams(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        user_token: impl Into<String>,
        admin_token: &str,
    ) -> Result<Self> {
        let client = Wuzapi {
            rpc: Rpc::new(reader, writer),
            user_token: user_token.into(),
            timeout: DEFAULT_TIMEOUT,
            child: None,
            _guard: None,
        };
        client.ensure_user(admin_token)?;
        Ok(client)
    }

    /// Change how long a single request may take (default 30 seconds).
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Receive every event that arrives from now on.
    pub fn events(&self) -> Events {
        Events::new(self.rpc.subscribe())
    }

    // ----------------------------------------------------------- plumbing --

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.rpc.call(method, params, self.timeout)
    }

    fn call_as_user(&self, method: &str, mut params: Value) -> Result<Value> {
        params["token"] = json!(self.user_token);
        self.call(method, params)
    }

    fn call_parsed<T: for<'de> Deserialize<'de>>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.call_as_user(method, params)?;
        serde_json::from_value(value).map_err(|e| Error::Protocol {
            method: method.to_string(),
            detail: e.to_string(),
        })
    }

    /// Create the wuzapi user for our token if it does not exist yet.
    fn ensure_user(&self, admin_token: &str) -> Result<()> {
        let users = self.call("admin.users.list", json!({ "adminToken": admin_token }))?;
        let exists = users
            .as_array()
            .map(|all| {
                all.iter()
                    .any(|u| u.get("token").and_then(Value::as_str) == Some(&self.user_token))
            })
            .ok_or_else(|| Error::Protocol {
                method: "admin.users.list".into(),
                detail: "expected an array of users".into(),
            })?;
        if exists {
            return Ok(());
        }
        let added = self.call(
            "admin.users.add",
            json!({
                "adminToken": admin_token,
                "name": USER_NAME,
                "token": self.user_token,
                "events": SUBSCRIBED.join(","),
                "history": HISTORY_PER_CHAT,
            }),
        );
        match added {
            Ok(_) => Ok(()),
            // Lost a race with a concurrent creation of the same token.
            Err(Error::Remote { code: 409, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------ session --

    /// Connection and login state of the session.
    pub fn status(&self) -> Result<Status> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            id: String,
            #[serde(default)]
            connected: bool,
            #[serde(default, rename = "loggedIn")]
            logged_in: bool,
            #[serde(default)]
            jid: String,
            #[serde(default)]
            history: Value,
        }
        let raw: Raw = self.call_parsed("session.status", json!({}))?;
        let history = match &raw.history {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
        .unwrap_or(0) as u32;
        Ok(Status {
            id: raw.id,
            connected: raw.connected,
            logged_in: raw.logged_in,
            jid: raw.jid.parse().ok(),
            history,
        })
    }

    /// Ask wuzapi to go online without waiting for the connection.
    fn connect_now(&self) -> Result<()> {
        let result = self.call_as_user(
            "session.connect",
            json!({ "subscribe": SUBSCRIBED, "immediate": true }),
        );
        match result {
            Ok(_) | Err(Error::Remote { code: 409, .. }) => Ok(()), // already connected
            Err(e) => Err(e),
        }
    }

    fn wait_connected(&self, within: Duration) -> Result<Status> {
        let deadline = Instant::now() + within;
        loop {
            let status = self.status()?;
            if status.connected {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    method: "session.connect".into(),
                });
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// Go online (idempotent) and wait until the session is connected.
    pub fn connect(&self) -> Result<Status> {
        self.connect_now()?;
        self.wait_connected(CONNECT_TIMEOUT)
    }

    /// Log the account out of WhatsApp (the next link needs a new pairing).
    pub fn logout(&self) -> Result<()> {
        self.call_as_user("session.logout", json!({})).map(drop)
    }

    /// Start QR pairing. The returned stream yields each QR code as it
    /// refreshes and ends once the account is paired or the codes time out.
    pub fn link_qr(&self) -> Result<QrStream> {
        if self.status()?.logged_in {
            return Err(Error::AlreadyLinked);
        }
        let events = self.events();
        self.connect_now()?;
        // A QR may already be on screen from an earlier attempt: fetch it.
        let current = self
            .call_as_user("session.qr", json!({}))
            .ok()
            .and_then(|v| v.get("QRCode").and_then(Value::as_str).map(str::to_string))
            .filter(|url| !url.is_empty())
            .map(|url| QrCode::from_data_url(&url));
        Ok(QrStream {
            queue: current.into_iter().collect(),
            events,
            done: false,
            last: None,
        })
    }

    /// Start pairing with a phone number: wuzapi returns an 8-character code
    /// to type into WhatsApp (Linked devices, Link with phone number).
    pub fn link_pair_code(&self, phone: &str) -> Result<PairCode> {
        let Recipient::Phone(phone) = phone.parse()? else {
            return Err(Error::InvalidInput(
                "expected a phone number, not a JID".into(),
            ));
        };
        if self.status()?.logged_in {
            return Err(Error::AlreadyLinked);
        }
        let events = self.events();
        self.connect_now()?;
        self.wait_connected(CONNECT_TIMEOUT)?;
        let result = self.call_as_user("session.pairphone", json!({ "Phone": phone }))?;
        let code = result
            .get("LinkingCode")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Protocol {
                method: "session.pairphone".into(),
                detail: "missing LinkingCode".into(),
            })?
            .to_string();
        Ok(PairCode { code, events })
    }

    // --------------------------------------------------------------- read --

    /// Every chat with stored history, newest first, with display names where
    /// the session knows them (names need an online session).
    pub fn chats(&self) -> Result<Vec<Chat>> {
        #[derive(Deserialize)]
        struct Entry {
            chat_jid: String,
            #[serde(default)]
            last_updated: String,
        }
        let own_id = self.status().map(|s| s.id).unwrap_or_default();
        // A `null` result means no history has been stored yet.
        let index: Option<HashMap<String, Option<Vec<Entry>>>> =
            self.call_parsed("chat.history", json!({ "chat_jid": "index" }))?;
        let index = index.unwrap_or_default();
        let names = self.display_names();
        let mut chats: Vec<Chat> = index
            .into_iter()
            .filter(|(user, _)| own_id.is_empty() || *user == own_id)
            .flat_map(|(_, entries)| entries.unwrap_or_default())
            .filter_map(|e| {
                let jid: Jid = e.chat_jid.parse().ok()?;
                Some(Chat {
                    name: names.get(&jid).cloned(),
                    last_updated: crate::model::parse_time(&e.last_updated),
                    jid,
                })
            })
            .collect();
        chats.sort_by(|a, b| b.last_updated.cmp(&a.last_updated).then(a.jid.cmp(&b.jid)));
        Ok(chats)
    }

    /// Best-effort JID to display-name map from contacts and groups. Empty
    /// when the session is offline.
    fn display_names(&self) -> HashMap<Jid, String> {
        let from_contacts = self
            .call_as_user("user.contacts", json!({}))
            .ok()
            .and_then(|v| match v {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .into_iter()
            .flatten()
            .filter_map(|(jid, info)| {
                let name = ["FullName", "PushName", "FirstName", "BusinessName"]
                    .iter()
                    .find_map(|k| {
                        info.get(*k)
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                    })?;
                Some((jid.parse::<Jid>().ok()?, name.to_string()))
            });
        let from_groups = self
            .call_as_user("group.list", json!({}))
            .ok()
            .and_then(|v| v.get("Groups").and_then(Value::as_array).cloned())
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let jid = g.get("JID").and_then(Value::as_str)?.parse::<Jid>().ok()?;
                let name = g
                    .get("Name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())?;
                Some((jid, name.to_string()))
            });
        from_contacts.chain(from_groups).collect()
    }

    /// The newest `limit` messages of a chat, oldest first, with reactions
    /// separated from messages. Returns when wuzapi has answered.
    pub fn history(&self, chat: &Jid, limit: u32) -> Result<History> {
        let rows: Option<Vec<HistoryRow>> = match self.call_parsed(
            "chat.history",
            json!({ "chat_jid": chat.as_str(), "limit": limit }),
        ) {
            Err(Error::Remote { code: 501, .. }) => return Err(Error::HistoryDisabled),
            other => other?,
        };
        let items = rows
            .unwrap_or_default()
            .iter()
            .filter_map(parse_history_row)
            .collect();
        Ok(assemble_history(chat.clone(), items))
    }

    // -------------------------------------------------------------- write --

    /// Send a text message. The returned id is wuzapi's `Id` field.
    pub fn send_text(&self, to: &Recipient, body: &str) -> Result<SentMessage> {
        if body.is_empty() {
            return Err(Error::InvalidInput("message body is empty".into()));
        }
        let method = "chat.send.text";
        let result = self.call_as_user(method, json!({ "Phone": to.as_wire(), "Body": body }))?;
        let protocol = |detail: &str| Error::Protocol {
            method: method.into(),
            detail: detail.into(),
        };
        let id = result
            .get("Id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| protocol("missing Id"))?;
        let ts = result
            .get("Timestamp")
            .and_then(Value::as_i64)
            .ok_or_else(|| protocol("missing Timestamp"))?;
        Ok(SentMessage {
            id: MessageId(id.to_string()),
            timestamp: time::OffsetDateTime::from_unix_timestamp(ts)
                .map_err(|_| protocol("Timestamp out of range"))?,
        })
    }
}

impl Drop for Wuzapi {
    fn drop(&mut self) {
        // wuzapi exits when its stdin closes; kill it if it lingers.
        self.rpc.close_writer();
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
            self.rpc.join_reader();
        }
    }
}

/// One step of QR pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// Show this QR code (and replace any earlier one).
    Code(QrCode),
    /// The codes ran out without a scan; start over with `link_qr`.
    Timeout,
    /// The phone accepted the pairing.
    Paired { jid: Option<Jid> },
}

/// Blocking stream of [`LinkEvent`]s from [`Wuzapi::link_qr`].
#[derive(Debug)]
pub struct QrStream {
    queue: VecDeque<QrCode>,
    events: Events,
    done: bool,
    last: Option<String>,
}

impl Iterator for QrStream {
    type Item = Result<LinkEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let fresh = |last: &mut Option<String>, qr: QrCode| {
            (last.as_deref() != Some(qr.data_url.as_str())).then(|| {
                *last = Some(qr.data_url.clone());
                qr
            })
        };
        if let Some(qr) = self
            .queue
            .pop_front()
            .and_then(|qr| fresh(&mut self.last, qr))
        {
            return Some(Ok(LinkEvent::Code(qr)));
        }
        loop {
            let item = match self.events.next()? {
                Event::Qr { qr } => match fresh(&mut self.last, qr) {
                    Some(qr) => Ok(LinkEvent::Code(qr)),
                    None => continue,
                },
                Event::QrTimeout => {
                    self.done = true;
                    Ok(LinkEvent::Timeout)
                }
                Event::PairSuccess { jid } => {
                    self.done = true;
                    Ok(LinkEvent::Paired { jid })
                }
                Event::PairError { message } => {
                    self.done = true;
                    Err(Error::PairingFailed(message))
                }
                Event::Exited => {
                    self.done = true;
                    Err(Error::Disconnected)
                }
                _ => continue,
            };
            return Some(item);
        }
    }
}

/// A pairing code to type into the phone, from [`Wuzapi::link_pair_code`].
#[derive(Debug)]
pub struct PairCode {
    /// The 8-character code.
    pub code: String,
    events: Events,
}

impl PairCode {
    /// Wait for the phone to accept the code. `Ok(None)` means the wait timed
    /// out; `Ok(Some(jid))` is the newly linked account.
    pub fn wait_paired(mut self, timeout: Duration) -> Result<Option<Option<Jid>>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.next_timeout(left) {
                None => return Ok(None),
                Some(Event::PairSuccess { jid }) => return Ok(Some(jid)),
                Some(Event::PairError { message }) => return Err(Error::PairingFailed(message)),
                Some(Event::Exited) => return Err(Error::Disconnected),
                Some(_) => {}
            }
        }
    }
}
