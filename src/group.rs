//! WhatsApp groups the linked account belongs to (metadata only, never
//! messages): the typed model, its pure parser, and `Wuzapi::groups`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;

use crate::client::Wuzapi;
use crate::error::{Error, Result};
use crate::model::{parse_time, Jid};

/// Large accounts take well over the default 30 seconds to list every group.
const LIST_TIMEOUT: Duration = Duration::from_secs(240);

/// A WhatsApp group the account belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub jid: Jid,
    pub name: Option<String>,
    /// The group description, when one is set.
    pub topic: Option<String>,
    /// Number of participants as reported by WhatsApp.
    pub members: usize,
    /// True when the linked account is a (super) admin.
    pub self_admin: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub created: Option<OffsetDateTime>,
    /// True for announce-only groups.
    pub announce_only: bool,
}

/// Build a [`Group`] from one `group.list` entry. `own_user` is the user part
/// of the linked account's JID, used to find the account's own participant
/// record (phone or LID form).
pub(crate) fn parse_group(value: &Value, own_user: &str) -> Option<Group> {
    let jid: Jid = value.get("JID")?.as_str()?.parse().ok()?;
    let text = |k: &str| {
        value
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let participants = value
        .get("Participants")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let is_self = |p: &Value| {
        !own_user.is_empty()
            && ["JID", "PhoneNumber", "LID"].iter().any(|k| {
                p.get(*k)
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<Jid>().ok())
                    .is_some_and(|j| j.user().split(':').next() == Some(own_user))
            })
    };
    let flag = |p: &Value, k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(Group {
        jid,
        name: text("Name"),
        topic: text("Topic"),
        members: participants.len(),
        self_admin: participants
            .iter()
            .any(|p| is_self(p) && (flag(p, "IsAdmin") || flag(p, "IsSuperAdmin"))),
        created: text("GroupCreated").and_then(|t| parse_time(&t)),
        announce_only: value
            .get("IsAnnounce")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl Wuzapi {
    /// Every group the account belongs to: names, sizes and the account's own
    /// admin flag, never messages. Needs an online session.
    pub fn groups(&self) -> Result<Vec<Group>> {
        let own = self
            .status()?
            .jid
            .map(|j| j.user().split(':').next().unwrap_or("").to_string())
            .unwrap_or_default();
        let listed = self.rpc.call(
            "group.list",
            json!({ "token": self.user_token }),
            self.timeout.max(LIST_TIMEOUT),
        )?;
        // A session that is connected but not yet synced answers without a
        // `Groups` key (seen as `{"code":500}`); surface it instead of
        // reporting "no groups".
        let Some(entries) = listed.get("Groups").and_then(Value::as_array) else {
            return Err(Error::Protocol {
                method: "group.list".into(),
                detail: format!(
                    "no Groups in reply: {}",
                    listed.to_string().chars().take(200).collect::<String>()
                ),
            });
        };
        let mut groups: Vec<Group> = entries
            .iter()
            .filter_map(|g| parse_group(g, &own))
            .collect();
        groups.sort_by(|a, b| a.name.cmp(&b.name).then(a.jid.cmp(&b.jid)));
        Ok(groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_group_counts_members_and_finds_own_admin_flag() {
        let v = json!({
            "JID": "120363@g.us", "Name": "Team", "Topic": "", "IsAnnounce": true,
            "GroupCreated": "2024-01-02T03:04:05Z",
            "Participants": [
                {"JID": "111@s.whatsapp.net", "IsAdmin": false},
                {"JID": "222:7@s.whatsapp.net", "IsAdmin": true}
            ]
        });
        let g = parse_group(&v, "222").unwrap();
        assert_eq!((g.members, g.self_admin, g.announce_only), (2, true, true));
        assert_eq!(g.name.as_deref(), Some("Team"));
        assert_eq!(g.topic, None);
        assert!(g.created.is_some());
        assert!(!parse_group(&v, "111").unwrap().self_admin);
        assert!(!parse_group(&v, "").unwrap().self_admin);
    }
}
