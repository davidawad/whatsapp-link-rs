//! Typed notifications from wuzapi.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use serde::Serialize;

use crate::model::{parse_event_message, Item, Jid, Message, Reaction};
use crate::qr::QrCode;
use crate::rpc::Notification;

/// Event names this crate subscribes to when connecting (wuzapi only emits
/// notifications for subscribed event types).
pub(crate) const SUBSCRIBED: [&str; 8] = [
    "Message",
    "Connected",
    "Disconnected",
    "LoggedOut",
    "PairSuccess",
    "PairError",
    "QR",
    "QRTimeout",
];

/// Something that happened on the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A pairing QR code (a new one arrives every time WhatsApp rotates it).
    Qr {
        qr: QrCode,
    },
    /// The QR codes ran out; wuzapi dropped the pairing attempt.
    QrTimeout,
    PairSuccess {
        jid: Option<Jid>,
    },
    PairError {
        message: String,
    },
    Connected,
    Disconnected,
    LoggedOut,
    /// An incoming (or own-device) chat message.
    Message {
        message: Message,
    },
    /// An incoming (or own-device) emoji reaction.
    Reaction {
        reaction: Reaction,
    },
    /// A subscribed event this crate has no typed form for.
    Other {
        method: String,
    },
    /// A `Message` notification without any user-visible content or whose
    /// payload could not be understood.
    Undecodable {
        method: String,
    },
    /// The wuzapi process went away. Always the last event of a stream.
    Exited,
}

impl Event {
    pub(crate) fn from_notification(n: &Notification) -> Event {
        let params = &n.params;
        match n.method.as_str() {
            "QR" => match params.get("qrCodeBase64").and_then(|v| v.as_str()) {
                Some(url) => Event::Qr {
                    qr: QrCode::from_data_url(url),
                },
                None => Event::Undecodable {
                    method: n.method.clone(),
                },
            },
            "QRTimeout" => Event::QrTimeout,
            "PairSuccess" => Event::PairSuccess {
                jid: params
                    .pointer("/event/ID")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
            },
            "PairError" => Event::PairError {
                message: params
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pairing failed")
                    .to_string(),
            },
            "Connected" => Event::Connected,
            "Disconnected" => Event::Disconnected,
            "LoggedOut" => Event::LoggedOut,
            "Message" => match params.get("event").and_then(parse_event_message) {
                Some(Item::Message(message)) => Event::Message { message },
                Some(Item::Reaction(reaction)) => Event::Reaction { reaction },
                None => Event::Undecodable {
                    method: n.method.clone(),
                },
            },
            other => Event::Other {
                method: other.to_string(),
            },
        }
    }
}

/// A blocking stream of [`Event`]s. Ends with [`Event::Exited`] when wuzapi
/// goes away.
#[derive(Debug)]
pub struct Events {
    rx: Receiver<Notification>,
    done: bool,
}

impl Events {
    pub(crate) fn new(rx: Receiver<Notification>) -> Self {
        Events { rx, done: false }
    }

    /// Wait up to `timeout` for the next event; `None` on timeout or after the
    /// stream has ended.
    pub fn next_timeout(&mut self, timeout: Duration) -> Option<Event> {
        if self.done {
            return None;
        }
        match self.rx.recv_timeout(timeout) {
            Ok(n) => Some(Event::from_notification(&n)),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                self.done = true;
                Some(Event::Exited)
            }
        }
    }
}

impl Iterator for Events {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        if self.done {
            return None;
        }
        match self.rx.recv() {
            Ok(n) => Some(Event::from_notification(&n)),
            Err(_) => {
                self.done = true;
                Some(Event::Exited)
            }
        }
    }
}
