//! Typed domain model and the (pure) parsers that build it from wuzapi's JSON.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Error, Result};

/// A WhatsApp address such as `15550100@s.whatsapp.net` or `123@g.us`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Jid(String);

impl Jid {
    /// The part before `@`.
    pub fn user(&self) -> &str {
        self.0.split_once('@').map_or(&self.0, |(u, _)| u)
    }
    /// The part after `@`.
    pub fn server(&self) -> &str {
        self.0.split_once('@').map_or("", |(_, s)| s)
    }
    /// True for group chats (`@g.us`).
    pub fn is_group(&self) -> bool {
        self.server() == "g.us"
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Jid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.split_once('@') {
            Some((user, server))
                if !user.is_empty()
                    && !server.is_empty()
                    && !s.chars().any(char::is_whitespace) =>
            {
                Ok(Jid(s.to_string()))
            }
            _ => Err(Error::InvalidInput(
                "not a JID (expected user@server)".into(),
            )),
        }
    }
}

impl fmt::Display for Jid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// WhatsApp's identifier for one message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub String);

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who a message is addressed to: a phone number (digits, country code
/// included) or a full JID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    Phone(String),
    Jid(Jid),
}

impl Recipient {
    /// The string wuzapi expects in its `Phone` parameter.
    pub fn as_wire(&self) -> &str {
        match self {
            Recipient::Phone(p) => p,
            Recipient::Jid(j) => j.as_str(),
        }
    }
}

impl FromStr for Recipient {
    type Err = Error;
    /// Accepts `user@server` JIDs, or phone numbers such as `+1 555 0100`
    /// (separators are stripped; 6 to 15 digits are required).
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.contains('@') {
            return s.parse().map(Recipient::Jid);
        }
        let digits: String = s
            .strip_prefix('+')
            .unwrap_or(s)
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '.'))
            .collect();
        if (6..=15).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
            Ok(Recipient::Phone(digits))
        } else {
            Err(Error::InvalidInput(
                "expected a JID or a phone number with country code (6-15 digits)".into(),
            ))
        }
    }
}

impl From<Jid> for Recipient {
    fn from(j: Jid) -> Self {
        Recipient::Jid(j)
    }
}

/// Non-text content carried by a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
    Sticker,
    Location,
    Contact,
    Poll,
}

/// An emoji reaction to a message. An empty `emoji` means the reaction was
/// removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub target: MessageId,
    pub chat: Jid,
    pub sender: Option<Jid>,
    pub sender_name: Option<String>,
    pub from_me: bool,
    pub emoji: String,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

/// A chat message (reactions are separate: see [`Reaction`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub chat: Jid,
    /// `None` only for locally stored outgoing messages that carry no sender.
    pub sender: Option<Jid>,
    pub sender_name: Option<String>,
    pub from_me: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub text: Option<String>,
    pub media: Vec<MediaKind>,
    pub quoted: Option<MessageId>,
    /// Reactions attached by [`History`] assembly; empty on live events.
    pub reactions: Vec<Reaction>,
}

/// Either kind of chat item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Message(Message),
    Reaction(Reaction),
}

/// Result of a history request: messages oldest first, each carrying the
/// reactions that target it; `reactions` is the flat list of all reactions
/// seen (including ones whose target is outside the window).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub chat: Jid,
    pub messages: Vec<Message>,
    pub reactions: Vec<Reaction>,
}

/// One conversation known to the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    pub jid: Jid,
    pub name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_updated: Option<OffsetDateTime>,
}

/// Where the session is in its link lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// No account has ever been paired: link with a QR code or pair code.
    NeedsPairing,
    /// Connected to WhatsApp but not logged in: a pairing is in progress.
    Pairing,
    /// An account is linked but the session is offline.
    Disconnected,
    /// Linked and online.
    Connected,
}

/// Snapshot of the wuzapi user's session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// wuzapi's internal user id.
    pub id: String,
    pub connected: bool,
    pub logged_in: bool,
    pub jid: Option<Jid>,
    /// Messages kept per chat (0 means history is disabled).
    pub history: u32,
}

impl Status {
    pub fn state(&self) -> LinkState {
        match (self.connected, self.logged_in, &self.jid) {
            (true, true, _) => LinkState::Connected,
            (true, false, _) => LinkState::Pairing,
            (false, _, Some(_)) => LinkState::Disconnected,
            (false, _, None) => LinkState::NeedsPairing,
        }
    }
}

/// What wuzapi reports after sending a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentMessage {
    pub id: MessageId,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

// ---------------------------------------------------------------- parsers --

fn str_at<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(str::to_string)
}

fn jid_at(v: &Value, path: &str) -> Option<Jid> {
    str_at(v, path).and_then(|s| s.parse().ok())
}

/// Parse an RFC 3339 timestamp (Go's `time` JSON form).
pub(crate) fn parse_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// Content extracted from a protobuf-JSON `Message` object.
struct Body {
    text: Option<String>,
    media: Vec<MediaKind>,
    quoted: Option<MessageId>,
}

fn parse_body(msg: &Value) -> Body {
    let caption = |key: &str| non_empty(str_at(msg, &format!("/{key}/caption")));
    let text = non_empty(str_at(msg, "/conversation"))
        .or_else(|| non_empty(str_at(msg, "/extendedTextMessage/text")))
        .or_else(|| caption("imageMessage"))
        .or_else(|| caption("videoMessage"))
        .or_else(|| caption("documentMessage"));
    let media = [
        ("imageMessage", MediaKind::Image),
        ("videoMessage", MediaKind::Video),
        ("audioMessage", MediaKind::Audio),
        ("documentMessage", MediaKind::Document),
        ("stickerMessage", MediaKind::Sticker),
        ("locationMessage", MediaKind::Location),
        ("liveLocationMessage", MediaKind::Location),
        ("contactMessage", MediaKind::Contact),
        ("contactsArrayMessage", MediaKind::Contact),
        ("pollCreationMessage", MediaKind::Poll),
        ("pollCreationMessageV2", MediaKind::Poll),
        ("pollCreationMessageV3", MediaKind::Poll),
    ]
    .into_iter()
    .filter(|(key, _)| msg.get(key).is_some_and(|v| !v.is_null()))
    .map(|(_, kind)| kind)
    .fold(Vec::new(), |mut acc, kind| {
        if !acc.contains(&kind) {
            acc.push(kind);
        }
        acc
    });
    let quoted = [
        "/extendedTextMessage/contextInfo/stanzaID",
        "/imageMessage/contextInfo/stanzaID",
        "/videoMessage/contextInfo/stanzaID",
    ]
    .into_iter()
    .find_map(|p| non_empty(str_at(msg, p)))
    .map(MessageId);
    Body {
        text,
        media,
        quoted,
    }
}

/// Parse a whatsmeow `events.Message` JSON object (`{"Info":…,"Message":…}`),
/// as found in `Message` notifications and in history rows' `data_json`.
/// Returns `None` for protocol chatter with no user-visible content.
pub(crate) fn parse_event_message(event: &Value) -> Option<Item> {
    let info = event.get("Info")?;
    let chat = jid_at(info, "/Chat")?;
    let timestamp = str_at(info, "/Timestamp").and_then(parse_time)?;
    let sender = jid_at(info, "/Sender");
    let sender_name = non_empty(str_at(info, "/PushName"));
    let from_me = info
        .get("IsFromMe")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let msg = event.get("Message")?;

    if let Some(reaction) = msg.get("reactionMessage").filter(|v| !v.is_null()) {
        let target =
            non_empty(str_at(reaction, "/key/ID").or_else(|| str_at(reaction, "/key/id")))?;
        return Some(Item::Reaction(Reaction {
            target: MessageId(target),
            chat,
            sender,
            sender_name,
            from_me,
            emoji: str_at(reaction, "/text").unwrap_or_default().to_string(),
            timestamp,
        }));
    }

    let Body {
        text,
        media,
        quoted,
    } = parse_body(msg);
    if text.is_none() && media.is_empty() {
        return None;
    }
    Some(Item::Message(Message {
        id: MessageId(non_empty(str_at(info, "/ID"))?),
        chat,
        sender,
        sender_name,
        from_me,
        timestamp,
        text,
        media,
        quoted,
        reactions: Vec::new(),
    }))
}

/// A row of wuzapi's `chat.history` result.
#[derive(Debug, Deserialize)]
pub(crate) struct HistoryRow {
    #[serde(default)]
    pub chat_jid: String,
    #[serde(default)]
    pub sender_jid: String,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub message_type: String,
    #[serde(default)]
    pub text_content: String,
    #[serde(default)]
    pub quoted_message_id: String,
    #[serde(default)]
    pub data_json: String,
}

/// Rows with a full `data_json` payload parse exactly like live events; rows
/// without one (locally recorded outgoing messages) fall back to the plain
/// columns.
pub(crate) fn parse_history_row(row: &HistoryRow) -> Option<Item> {
    if !row.data_json.is_empty() {
        if let Some(item) = serde_json::from_str::<Value>(&row.data_json)
            .ok()
            .and_then(|v| parse_event_message(&v))
        {
            return Some(item);
        }
    }
    let chat: Jid = row.chat_jid.parse().ok()?;
    let timestamp = parse_time(&row.timestamp)?;
    let from_me = row.sender_jid == "me";
    let text = non_empty(Some(&row.text_content));
    let media = match row.message_type.as_str() {
        "image" => vec![MediaKind::Image],
        "video" => vec![MediaKind::Video],
        "audio" => vec![MediaKind::Audio],
        "document" => vec![MediaKind::Document],
        "sticker" => vec![MediaKind::Sticker],
        _ => Vec::new(),
    };
    if text.is_none() && media.is_empty() {
        return None;
    }
    Some(Item::Message(Message {
        id: MessageId(non_empty(Some(&row.message_id))?),
        chat,
        sender: row.sender_jid.parse().ok(),
        sender_name: None,
        from_me,
        timestamp,
        text,
        media,
        quoted: non_empty(Some(&row.quoted_message_id)).map(MessageId),
        reactions: Vec::new(),
    }))
}

/// Assemble a [`History`] from parsed items: sort oldest first, fold
/// reactions onto their target messages (the latest reaction per sender wins;
/// an empty emoji removes it), and keep the flat reaction list.
pub(crate) fn assemble_history(chat: Jid, items: Vec<Item>) -> History {
    let (mut messages, mut reactions): (Vec<Message>, Vec<Reaction>) =
        items
            .into_iter()
            .fold((vec![], vec![]), |(mut ms, mut rs), item| {
                match item {
                    Item::Message(m) => ms.push(m),
                    Item::Reaction(r) => rs.push(r),
                }
                (ms, rs)
            });
    messages.sort_by_key(|m| m.timestamp);
    reactions.sort_by_key(|r| r.timestamp);
    for reaction in &reactions {
        if let Some(target) = messages.iter_mut().find(|m| m.id == reaction.target) {
            target
                .reactions
                .retain(|r| r.sender != reaction.sender || r.from_me != reaction.from_me);
            if !reaction.emoji.is_empty() {
                target.reactions.push(reaction.clone());
            }
        }
    }
    History {
        chat,
        messages,
        reactions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(message: Value) -> Value {
        json!({
            "Info": {
                "ID": "MSG1", "Chat": "15550100@s.whatsapp.net",
                "Sender": "15550100@s.whatsapp.net", "IsFromMe": false,
                "PushName": "Sam", "Timestamp": "2025-01-02T03:04:05Z"
            },
            "Message": message
        })
    }

    #[test]
    fn jid_parts() {
        let j: Jid = "123-456@g.us".parse().unwrap();
        assert_eq!(
            (j.user(), j.server(), j.is_group()),
            ("123-456", "g.us", true)
        );
        assert!("nope".parse::<Jid>().is_err());
        assert!("@g.us".parse::<Jid>().is_err());
        assert!("a b@c".parse::<Jid>().is_err());
    }

    #[test]
    fn recipient_parsing() {
        assert_eq!(
            "+1 (555) 010-0100".parse::<Recipient>().unwrap(),
            Recipient::Phone("15550100100".into())
        );
        assert!(matches!(
            "1@s.whatsapp.net".parse::<Recipient>(),
            Ok(Recipient::Jid(_))
        ));
        assert!("12".parse::<Recipient>().is_err());
        assert!("abc12345".parse::<Recipient>().is_err());
        assert!("+1234567890123456".parse::<Recipient>().is_err());
    }

    #[test]
    fn plain_text_message() {
        let Some(Item::Message(m)) = parse_event_message(&event(json!({"conversation": "hi"})))
        else {
            panic!("expected a message")
        };
        assert_eq!(m.id, MessageId("MSG1".into()));
        assert_eq!(m.text.as_deref(), Some("hi"));
        assert_eq!(m.sender_name.as_deref(), Some("Sam"));
        assert!(!m.from_me && m.media.is_empty());
    }

    #[test]
    fn extended_text_with_quote() {
        let Some(Item::Message(m)) = parse_event_message(&event(json!({
            "extendedTextMessage": {"text": "re", "contextInfo": {"stanzaID": "OLD"}}
        }))) else {
            panic!()
        };
        assert_eq!(m.text.as_deref(), Some("re"));
        assert_eq!(m.quoted, Some(MessageId("OLD".into())));
    }

    #[test]
    fn media_tags_and_caption() {
        let Some(Item::Message(m)) = parse_event_message(&event(json!({
            "imageMessage": {"caption": "look", "mimetype": "image/jpeg"}
        }))) else {
            panic!()
        };
        assert_eq!(m.media, vec![MediaKind::Image]);
        assert_eq!(m.text.as_deref(), Some("look"));
        let Some(Item::Message(m)) =
            parse_event_message(&event(json!({"audioMessage": {}, "stickerMessage": {}})))
        else {
            panic!()
        };
        assert_eq!(m.media, vec![MediaKind::Audio, MediaKind::Sticker]);
    }

    #[test]
    fn reactions_are_not_messages() {
        let Some(Item::Reaction(r)) = parse_event_message(&event(json!({
            "reactionMessage": {"key": {"ID": "TARGET"}, "text": "👍"}
        }))) else {
            panic!("expected a reaction")
        };
        assert_eq!(r.target, MessageId("TARGET".into()));
        assert_eq!(r.emoji, "👍");
    }

    #[test]
    fn protocol_chatter_is_ignored() {
        assert_eq!(
            parse_event_message(&event(json!({"senderKeyDistributionMessage": {}}))),
            None
        );
        assert_eq!(parse_event_message(&json!({"Info": {}})), None);
        assert_eq!(parse_event_message(&json!("x")), None);
    }

    #[test]
    fn status_state_machine() {
        let mk = |connected, logged_in, jid: Option<&str>| Status {
            id: "u".into(),
            connected,
            logged_in,
            jid: jid.map(|j| j.parse().unwrap()),
            history: 10,
        };
        assert_eq!(mk(false, false, None).state(), LinkState::NeedsPairing);
        assert_eq!(mk(true, false, None).state(), LinkState::Pairing);
        assert_eq!(
            mk(false, false, Some("1@s.whatsapp.net")).state(),
            LinkState::Disconnected
        );
        assert_eq!(
            mk(true, true, Some("1@s.whatsapp.net")).state(),
            LinkState::Connected
        );
    }

    #[test]
    fn history_rows_fall_back_to_columns() {
        let row = HistoryRow {
            chat_jid: "15550100@s.whatsapp.net".into(),
            sender_jid: "me".into(),
            message_id: "OUT1".into(),
            timestamp: "2025-01-02T03:04:05.5Z".into(),
            message_type: "text".into(),
            text_content: "sent by me".into(),
            quoted_message_id: String::new(),
            data_json: String::new(),
        };
        let Some(Item::Message(m)) = parse_history_row(&row) else {
            panic!()
        };
        assert!(m.from_me);
        assert_eq!(m.text.as_deref(), Some("sent by me"));
    }

    #[test]
    fn history_assembly_sorts_and_folds_reactions() {
        let at = |s: &str| parse_time(s).unwrap();
        let chat: Jid = "15550100@s.whatsapp.net".parse().unwrap();
        let msg = |id: &str, t: &str| {
            Item::Message(Message {
                id: MessageId(id.into()),
                chat: chat.clone(),
                sender: None,
                sender_name: None,
                from_me: false,
                timestamp: at(t),
                text: Some(id.into()),
                media: vec![],
                quoted: None,
                reactions: vec![],
            })
        };
        let react = |target: &str, emoji: &str, t: &str| {
            Item::Reaction(Reaction {
                target: MessageId(target.into()),
                chat: chat.clone(),
                sender: Some("9@s.whatsapp.net".parse().unwrap()),
                sender_name: None,
                from_me: false,
                emoji: emoji.into(),
                timestamp: at(t),
            })
        };
        let h = assemble_history(
            chat.clone(),
            vec![
                msg("B", "2025-01-01T00:00:02Z"),
                react("A", "x", "2025-01-01T00:00:03Z"),
                react("A", "y", "2025-01-01T00:00:04Z"),
                msg("A", "2025-01-01T00:00:01Z"),
                react("B", "z", "2025-01-01T00:00:05Z"),
                react("B", "", "2025-01-01T00:00:06Z"),
            ],
        );
        let ids: Vec<_> = h.messages.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert_eq!(h.messages[0].reactions.len(), 1);
        assert_eq!(h.messages[0].reactions[0].emoji, "y");
        assert!(h.messages[1].reactions.is_empty());
        assert_eq!(h.reactions.len(), 4);
    }
}
