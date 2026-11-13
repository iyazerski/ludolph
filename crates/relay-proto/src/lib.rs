//! Messages between the Ludolph engine (on each Mac) and the relay.
//!
//! The Mac opens one WebSocket to the relay and keeps it open. Both sides
//! send JSON text frames: the relay sends [`Call`]s, the Mac answers each with
//! an [`Up::Reply`] and reports chat changes with [`Up::Notify`].

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped when a message or a tool's answer changes shape, since the relay
/// serves the phone app the answers are for; it refuses other versions.
pub const PROTOCOL: u32 = 3;

/// The path the Mac connects to, with `Authorization: Bearer <token>`.
pub const CONNECT_PATH: &str = "/connect";

/// Relay → Mac: run one of the phone's tools (`list_chats`, `send_message`, …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub id: u64,
    pub tool: String,
    pub args: Value,
}

/// Mac → relay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Up {
    /// The first message after connecting.
    Hello {
        protocol: u32,
        /// The engine's version.
        version: String,
    },
    /// The answer to the [`Call`] with this `id`: the tool's result or its error.
    Reply {
        id: u64,
        result: Result<Value, String>,
    },
    /// A chat finished, needs an answer, failed, or was cut off.
    Notify(Notice),
    /// The Mac's battery, `None` on a Mac without one: sent after
    /// [`Up::Hello`] and on each change.
    Power { battery: Option<Battery> },
}

/// The Mac's battery, which the phone shows beside the Mac's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}

/// Whether the battery is charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatteryState {
    /// On power and charging.
    Charging,
    /// On power, but not charging (full, or holding its charge).
    Plugged,
    Unplugged,
}

/// A notification for the phone about one chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    /// The chat's id, which the notification opens.
    pub chat: String,
    pub title: String,
    pub body: String,
}

/// What happened to the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    /// Its run ended.
    Finished,
    /// The agent waits on an answer.
    Input,
    /// Its run failed.
    Failed,
    /// Its run was cut off: pi exited in the middle of it.
    Interrupted,
}

/// What the phone's **Add a Mac** gives: the relay's address and the Mac's
/// token, as one string to paste on the Mac.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCode {
    pub relay: String,
    pub token: String,
}

impl LinkCode {
    /// The code as text: its JSON, in URL-safe base64.
    pub fn encode(&self) -> String {
        B64.encode(serde_json::to_vec(self).expect("two strings serialize"))
    }

    /// Read a code, or `None` if `code` isn't one. Spaces around it, from
    /// copying, don't matter.
    pub fn decode(code: &str) -> Option<Self> {
        serde_json::from_slice(&B64.decode(code.trim()).ok()?).ok()
    }
}

/// Leaves the token out, so logs can't leak it.
impl std::fmt::Debug for LinkCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkCode")
            .field("relay", &self.relay)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_keep_their_wire_shape() {
        let reply = Up::Reply {
            id: 7,
            result: Err("no such chat".into()),
        };
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({ "type": "reply", "id": 7, "result": { "Err": "no such chat" } })
        );
        let notify = Up::Notify(Notice {
            kind: NoticeKind::Input,
            chat: "c".into(),
            title: "t".into(),
            body: "b".into(),
        });
        let value = serde_json::to_value(&notify).unwrap();
        assert_eq!(
            value,
            json!({ "type": "notify", "kind": "input", "chat": "c", "title": "t", "body": "b" })
        );
        assert_eq!(serde_json::from_value::<Up>(value).unwrap(), notify);
        let power = Up::Power {
            battery: Some(Battery {
                percent: 85,
                state: BatteryState::Charging,
            }),
        };
        let value = serde_json::to_value(&power).unwrap();
        assert_eq!(
            value,
            json!({ "type": "power", "battery": { "percent": 85, "state": "charging" } })
        );
        assert_eq!(serde_json::from_value::<Up>(value).unwrap(), power);
        let none = serde_json::to_value(Up::Power { battery: None }).unwrap();
        assert_eq!(none, json!({ "type": "power", "battery": null }));
    }

    #[test]
    fn a_link_code_reads_back() {
        let link = LinkCode {
            relay: "https://relay.example.com".into(),
            token: "t0k-en_".into(),
        };
        let code = link.encode();
        assert_eq!(LinkCode::decode(&format!(" {code}\n")), Some(link));
        assert_eq!(LinkCode::decode("not a code"), None);
        assert_eq!(LinkCode::decode(&B64.encode("{}")), None);
    }
}
