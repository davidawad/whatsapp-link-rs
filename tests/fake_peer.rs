//! Behaviour tests against an in-process fake of wuzapi's JSON-RPC peer.
mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use serde_json::{json, Value};
use whatsapp_link::{Error, Event, Jid, LinkEvent, LinkState, MediaKind, MessageId, Recipient};

fn chat() -> Jid {
    CHAT.parse().unwrap()
}

fn status_json(connected: bool, logged_in: bool, jid: &str) -> Value {
    json!({"id": "u1", "name": "n", "connected": connected, "loggedIn": logged_in,
           "token": USER_TOKEN, "jid": jid, "history": "100", "qrcode": ""})
}

#[test]
fn provisioning_creates_the_user_only_when_missing() {
    let (_c, _w, log) = start_with(false, |id, _, _| vec![ok(id, json!({}))]);
    assert_eq!(calls(&log, "admin.users.list").len(), 1);
    assert_eq!(calls(&log, "admin.users.add").len(), 1);

    let (_c, _w, log) = start_with(true, |id, _, _| vec![ok(id, json!({}))]);
    assert!(calls(&log, "admin.users.add").is_empty());
}

#[test]
fn status_transitions_are_typed() {
    let seq = Arc::new(Mutex::new(vec![
        status_json(false, false, ""),
        status_json(true, false, ""),
        status_json(true, true, "15550100:3@s.whatsapp.net"),
        status_json(false, true, "15550100:3@s.whatsapp.net"),
    ]));
    let (c, _w, log) = start(move |id, m, _| {
        assert_eq!(m, "session.status");
        vec![ok(id, seq.lock().unwrap().remove(0))]
    });
    let states: Vec<_> = (0..4).map(|_| c.status().unwrap().state()).collect();
    assert_eq!(
        states,
        [
            LinkState::NeedsPairing,
            LinkState::Pairing,
            LinkState::Connected,
            LinkState::Disconnected
        ]
    );
    assert_eq!(calls(&log, "session.status")[0]["token"], USER_TOKEN);
}

#[test]
fn status_reads_history_size_and_jid() {
    let (c, _w, _) =
        start(|id, _, _| vec![ok(id, status_json(true, true, "15550100:3@s.whatsapp.net"))]);
    let s = c.status().unwrap();
    assert_eq!(s.history, 100);
    assert_eq!(s.jid.unwrap().user(), "15550100:3");
}

#[test]
fn send_text_returns_the_wire_message_id() {
    let (c, _w, log) = start(|id, m, p| {
        assert_eq!(m, "chat.send.text");
        assert_eq!(p["Phone"], "15550100");
        vec![ok(
            id,
            json!({"Details": "Sent", "Timestamp": 1_700_000_000, "Id": "3EB0ABCDEF"}),
        )]
    });
    let sent = c
        .send_text(&"+1 555 0100".parse::<Recipient>().unwrap(), "hello")
        .unwrap();
    assert_eq!(sent.id, MessageId("3EB0ABCDEF".into()));
    assert_eq!(sent.timestamp.unix_timestamp(), 1_700_000_000);
    let req = &calls(&log, "chat.send.text")[0];
    assert_eq!(
        (req["Body"].as_str(), req["token"].as_str()),
        (Some("hello"), Some(USER_TOKEN))
    );
}

#[test]
fn send_text_to_a_jid_passes_it_through() {
    let (c, _w, log) = start(|id, _, _| vec![ok(id, json!({"Timestamp": 1, "Id": "X"}))]);
    c.send_text(&Recipient::Jid(chat()), "hi").unwrap();
    assert_eq!(calls(&log, "chat.send.text")[0]["Phone"], CHAT);
}

#[test]
fn send_text_errors_are_typed() {
    let (c, _w, _) = start(|id, _, _| vec![err(id, 500, "error sending message: boom")]);
    let to = Recipient::Jid(chat());
    match c.send_text(&to, "x") {
        Err(Error::Remote {
            method,
            code,
            message,
        }) => {
            assert_eq!((method.as_str(), code), ("chat.send.text", 500));
            assert!(message.contains("boom"));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(c.send_text(&to, ""), Err(Error::InvalidInput(_))));

    let (c, _w, _) = start(|id, _, _| vec![ok(id, json!({"Details": "Sent"}))]);
    assert!(matches!(c.send_text(&to, "x"), Err(Error::Protocol { .. })));
}

#[test]
fn history_completes_sorts_and_separates_reactions() {
    let m1 = message_event("M1", "first", "2025-03-01T10:00:00Z");
    let m2 = message_event("M2", "second", "2025-03-01T10:05:00Z");
    let r1 = reaction_event("R1", "M1", "👍", "2025-03-01T10:06:00Z");
    // wuzapi returns newest first.
    let rows = json!([
        history_row("R1", "2025-03-01T10:06:00Z", &r1),
        history_row("M2", "2025-03-01T10:05:00Z", &m2),
        history_row("M1", "2025-03-01T10:00:00Z", &m1),
        {"id": 9, "chat_jid": CHAT, "sender_jid": "me", "message_id": "OUT1",
         "timestamp": "2025-03-01T09:00:00Z", "message_type": "text",
         "text_content": "from me", "media_link": "", "data_json": ""}
    ]);
    let (c, _w, log) = start(move |id, m, p| {
        assert_eq!(m, "chat.history");
        assert_eq!(p["chat_jid"], CHAT);
        vec![ok(id, rows.clone())]
    });
    let h = c.history(&chat(), 25).unwrap();
    assert_eq!(calls(&log, "chat.history")[0]["limit"], 25);
    let texts: Vec<_> = h
        .messages
        .iter()
        .map(|m| m.text.as_deref().unwrap())
        .collect();
    assert_eq!(texts, ["from me", "first", "second"]);
    assert!(h.messages[0].from_me);
    assert_eq!(h.reactions.len(), 1, "reactions are not messages");
    assert_eq!(h.messages[1].reactions[0].emoji, "👍");
    assert!(h.messages[2].reactions.is_empty());
}

#[test]
fn history_handles_empty_null_and_disabled() {
    let (c, _w, _) = start(|id, _, _| vec![ok(id, Value::Null)]);
    assert!(c.history(&chat(), 10).unwrap().messages.is_empty());
    let (c, _w, _) = start(|id, _, _| vec![ok(id, json!([]))]);
    assert!(c.history(&chat(), 10).unwrap().messages.is_empty());
    let (c, _w, _) =
        start(|id, _, _| vec![err(id, 501, "message history is disabled for this user")]);
    assert!(matches!(
        c.history(&chat(), 10),
        Err(Error::HistoryDisabled)
    ));
}

#[test]
fn chats_join_names_and_filter_by_user() {
    let (c, _w, _) = start(|id, m, _| {
        vec![match m {
            "session.status" => ok(id, status_json(true, true, "1@s.whatsapp.net")),
            "chat.history" => ok(
                id,
                json!({
                    "u1": [{"chat_jid": CHAT, "last_updated": "2025-03-02T00:00:00Z"},
                           {"chat_jid": "999@g.us", "last_updated": "2025-03-03T00:00:00Z"},
                           {"chat_jid": "777@s.whatsapp.net", "last_updated": "2025-03-01T00:00:00Z"}],
                    "other": [{"chat_jid": "555@s.whatsapp.net", "last_updated": "2025-03-04T00:00:00Z"}]
                }),
            ),
            "user.contacts" => ok(
                id,
                json!({CHAT: {"Found": true, "FullName": "Sam Example", "PushName": "sam"},
                       "777@s.whatsapp.net": {"Found": true, "FullName": "", "PushName": "Pat"}}),
            ),
            "group.list" => ok(
                id,
                json!({"Groups": [{"JID": "999@g.us", "Name": "Book club"}]}),
            ),
            other => panic!("unexpected {other}"),
        }]
    });
    let chats = c.chats().unwrap();
    let view: Vec<_> = chats
        .iter()
        .map(|c| (c.jid.as_str(), c.name.as_deref()))
        .collect();
    assert_eq!(
        view,
        [
            ("999@g.us", Some("Book club")),
            (CHAT, Some("Sam Example")),
            ("777@s.whatsapp.net", Some("Pat")),
        ]
    );
}

#[test]
fn chats_work_offline_without_names() {
    let (c, _w, _) = start(|id, m, _| {
        vec![match m {
            "session.status" => ok(id, status_json(false, true, "1@s.whatsapp.net")),
            "chat.history" => ok(id, json!({"u1": [{"chat_jid": CHAT, "last_updated": "x"}]})),
            _ => err(id, 500, "no session"),
        }]
    });
    let chats = c.chats().unwrap();
    assert_eq!(chats.len(), 1);
    assert_eq!((chats[0].name.clone(), chats[0].last_updated), (None, None));
}

#[test]
fn events_arrive_in_order_and_typed() {
    let (c, w, _) = start(|id, _, _| vec![ok(id, json!({}))]);
    let mut events = c.events();
    w.send(&message_notification("A", "one", "2025-03-01T10:00:00Z"));
    w.send(&notification(
        "Message",
        json!({"type": "Message", "event": reaction_event("RX", "A", "❤", "2025-03-01T10:00:01Z")}),
    ));
    w.send(&message_notification("B", "two", "2025-03-01T10:00:02Z"));
    w.send(&notification("Connected", json!({"type": "Connected"})));
    w.send(&notification("Presence", json!({"type": "Presence"})));

    let got: Vec<Event> = (0..5).map(|_| events.next().unwrap()).collect();
    let summary: Vec<String> = got
        .iter()
        .map(|e| match e {
            Event::Message { message } => format!("msg:{}", message.text.clone().unwrap()),
            Event::Reaction { reaction } => format!("react:{}:{}", reaction.target, reaction.emoji),
            Event::Connected => "connected".into(),
            Event::Other { method } => format!("other:{method}"),
            e => format!("{e:?}"),
        })
        .collect();
    assert_eq!(
        summary,
        [
            "msg:one",
            "react:A:❤",
            "msg:two",
            "connected",
            "other:Presence"
        ]
    );
}

#[test]
fn media_and_unreadable_message_events() {
    let (c, w, _) = start(|id, _, _| vec![ok(id, json!({}))]);
    let mut events = c.events();
    let mut ev = message_event("P1", "", "2025-03-01T10:00:00Z");
    ev["Message"] = json!({"imageMessage": {"caption": "pic"}, "contextInfo": {}});
    w.send(&notification("Message", json!({"event": ev})));
    w.send(&notification(
        "Message",
        json!({"event": {"Info": {}, "Message": {}}}),
    ));
    w.send(&notification("Message", json!({"no": "event"})));
    match events.next().unwrap() {
        Event::Message { message } => {
            assert_eq!(message.media, vec![MediaKind::Image]);
            assert_eq!(message.text.as_deref(), Some("pic"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(events.next().unwrap(), Event::Undecodable { .. }));
    assert!(matches!(events.next().unwrap(), Event::Undecodable { .. }));
}

#[test]
fn qr_codes_refresh_then_pair() {
    let (c, w, log) = start(move |id, m, _| match m {
        "session.status" => vec![ok(id, status_json(false, false, ""))],
        "session.connect" => vec![ok(id, json!({"details": "Connected!"}))],
        "session.qr" => vec![err(id, 500, "not connected")],
        other => panic!("unexpected {other}"),
    });
    let mut stream = c.link_qr().unwrap();
    let qr = |n: u8| {
        notification(
            "QR",
            json!({"type": "QR", "qrCodeBase64": format!("data:image/png;base64,QR{n}")}),
        )
    };
    w.send(&qr(1));
    w.send(&qr(1)); // duplicate refresh is not re-shown
    w.send(&qr(2));
    w.send(&notification(
        "PairSuccess",
        json!({"type": "PairSuccess", "event": {"ID": "15550100:9@s.whatsapp.net"}}),
    ));
    let steps: Vec<_> = stream.by_ref().map(Result::unwrap).collect();
    assert_eq!(steps.len(), 3);
    assert!(matches!(&steps[0], LinkEvent::Code(q) if q.data_url.ends_with("QR1")));
    assert!(matches!(&steps[1], LinkEvent::Code(q) if q.data_url.ends_with("QR2")));
    assert!(matches!(&steps[2], LinkEvent::Paired { jid: Some(j) } if j.user() == "15550100:9"));
    let connect = &calls(&log, "session.connect")[0];
    assert_eq!(connect["immediate"], true);
    assert!(connect["subscribe"]
        .as_array()
        .unwrap()
        .contains(&json!("QR")));
}

#[test]
fn qr_stream_picks_up_a_code_already_on_screen_and_ends_on_timeout() {
    let (c, w, _) = start(|id, m, _| {
        vec![match m {
            "session.status" => ok(id, status_json(true, false, "")),
            "session.connect" => err(id, 409, "already connected"),
            "session.qr" => ok(
                id,
                json!({"QRCode": "data:image/png;base64,CURRENT", "passkeyPending": false}),
            ),
            other => panic!("unexpected {other}"),
        }]
    });
    let mut stream = c.link_qr().unwrap();
    w.send(&notification("QRTimeout", json!({"type": "QRTimeout"})));
    assert!(
        matches!(stream.next(), Some(Ok(LinkEvent::Code(q))) if q.data_url.ends_with("CURRENT"))
    );
    assert!(matches!(stream.next(), Some(Ok(LinkEvent::Timeout))));
    assert!(stream.next().is_none());
}

#[test]
fn linking_an_already_linked_account_is_refused() {
    let (c, _w, _) = start(|id, _, _| vec![ok(id, status_json(true, true, "1@s.whatsapp.net"))]);
    assert!(matches!(c.link_qr(), Err(Error::AlreadyLinked)));
    assert!(matches!(
        c.link_pair_code("15550100"),
        Err(Error::AlreadyLinked)
    ));
    assert!(matches!(
        c.link_pair_code("not a number"),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        c.link_pair_code("1@s.whatsapp.net"),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn pair_code_flow() {
    let (c, w, log) = start(|id, m, _| {
        vec![match m {
            "session.status" => ok(id, status_json(true, false, "")),
            "session.connect" => ok(id, json!({})),
            "session.pairphone" => ok(id, json!({"LinkingCode": "ABCD-EFGH"})),
            other => panic!("unexpected {other}"),
        }]
    });
    let pair = c.link_pair_code("+1 555 0100").unwrap();
    assert_eq!(pair.code, "ABCD-EFGH");
    assert_eq!(calls(&log, "session.pairphone")[0]["Phone"], "15550100");
    w.send(&notification(
        "PairSuccess",
        json!({"event": {"ID": "15550100:1@s.whatsapp.net"}}),
    ));
    let jid = pair.wait_paired(Duration::from_secs(5)).unwrap();
    assert!(jid.is_some());
}

#[test]
fn pair_code_wait_times_out_quietly() {
    let (c, _w, _) = start(|id, m, _| {
        vec![match m {
            "session.status" => ok(id, status_json(true, false, "")),
            "session.pairphone" => ok(id, json!({"LinkingCode": "AAAA-BBBB"})),
            _ => ok(id, json!({})),
        }]
    });
    let pair = c.link_pair_code("15550100").unwrap();
    assert_eq!(pair.wait_paired(Duration::from_millis(100)).unwrap(), None);
}

#[test]
fn connect_waits_for_the_session_and_tolerates_already_connected() {
    let polls = Arc::new(Mutex::new(0));
    let (c, _w, log) = start(move |id, m, _| {
        vec![match m {
            "session.connect" => err(id, 409, "already connected"),
            "session.status" => {
                let mut n = polls.lock().unwrap();
                *n += 1;
                ok(id, status_json(*n >= 2, true, "1@s.whatsapp.net"))
            }
            other => panic!("unexpected {other}"),
        }]
    });
    assert!(c.connect().unwrap().connected);
    assert_eq!(calls(&log, "session.status").len(), 2);
}

#[test]
fn logout_and_remote_errors() {
    let (c, _w, _) = start(|id, m, _| {
        assert_eq!(m, "session.logout");
        vec![err(id, 500, "could not logout as it was not logged in")]
    });
    assert!(matches!(c.logout(), Err(Error::Remote { code: 500, .. })));
}

#[test]
fn responses_may_arrive_out_of_order_and_interleaved_with_notifications() {
    let held: Arc<Mutex<Option<u64>>> = Arc::default();
    let (c, _w, _) = start(move |id, m, _| {
        let mut held = held.lock().unwrap();
        match (m, held.take()) {
            ("session.status", None) => {
                *held = Some(id); // hold the first request until the second arrives
                vec![]
            }
            ("chat.send.text", Some(first)) => vec![
                notification("Connected", json!({})),
                ok(id, json!({"Id": "SECOND", "Timestamp": 5})),
                "this is not json".into(),
                ok(first, status_json(true, true, "1@s.whatsapp.net")),
            ],
            other => panic!("unexpected {other:?}"),
        }
    });
    let to = Recipient::Jid(chat());
    std::thread::scope(|s| {
        let status = s.spawn(|| c.status().unwrap());
        // Make sure the status request is the first in flight.
        std::thread::sleep(Duration::from_millis(150));
        let sent = c.send_text(&to, "x").unwrap();
        assert_eq!(sent.id.0, "SECOND");
        assert!(status.join().unwrap().connected);
    });
}

#[test]
fn garbage_stray_and_malformed_frames_are_skipped() {
    let (c, w, _) = start(|id, _, _| {
        vec![
            "".into(),
            "{not json".into(),
            "[1,2,3]".into(),
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":400,"message":"invalid JSON request"}}"#
                .into(),
            ok(424242, json!({"stray": true})), // nobody asked for this id
            r#"{"id":"abc","result":1}"#.into(),
            "2026-01-01T00:00:00Z INF plain log line on stdout".into(),
            ok(id, status_json(true, true, "1@s.whatsapp.net")),
        ]
    });
    w.send("   ");
    assert!(c.status().unwrap().logged_in);
    assert!(
        c.status().unwrap().connected,
        "client still healthy afterwards"
    );
}

#[test]
fn string_ids_in_responses_are_matched() {
    let (c, _w, _) = start(|id, _, _| {
        vec![
            json!({"jsonrpc": "2.0", "id": id.to_string(), "result": status_json(true, true, "")})
                .to_string(),
        ]
    });
    assert!(c.status().unwrap().connected);
}

#[test]
fn request_timeout_is_reported() {
    let (c, _w, _) = start(|_, _, _| vec![]);
    let c = c.with_request_timeout(Duration::from_millis(150));
    match c.status() {
        Err(Error::Timeout { method }) => assert_eq!(method, "session.status"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn peer_exit_fails_requests_and_ends_the_event_stream() {
    let (c, w, _) = start(|_, _, _| vec![]); // never answers
    let mut events = c.events();
    let pending = std::thread::scope(|s| {
        let call = s.spawn(|| c.status());
        std::thread::sleep(Duration::from_millis(100));
        w.close();
        call.join().unwrap()
    });
    assert!(matches!(pending, Err(Error::Disconnected)), "{pending:?}");
    assert!(matches!(c.status(), Err(Error::Disconnected)));
    assert!(matches!(events.next(), Some(Event::Exited)));
    assert!(events.next().is_none());
}

#[test]
fn every_request_carries_the_user_token_but_admin_calls_do_not_leak_it() {
    let (c, _w, log) = start(|id, _, _| vec![ok(id, status_json(true, true, ""))]);
    c.status().unwrap();
    for (method, params) in log.lock().unwrap().iter() {
        if method.starts_with("session.") {
            assert_eq!(params["token"], USER_TOKEN);
        }
        if method.starts_with("admin.") {
            assert_eq!(params["adminToken"], ADMIN_TOKEN);
        }
    }
}
