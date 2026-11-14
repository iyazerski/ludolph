//! Messages between the Ludolph engine (on each Mac) and the relay.
//!
//! The Mac opens one WebSocket to the relay and keeps it open. Both sides
//! send JSON text frames: the relay sends [`Down`] messages, calls and chats
//! to follow for the phone, and the Mac answers each call with an
//! [`Up::Reply`], streams followed chats as [`Up::Frame`]s, and reports chat
//! changes with [`Up::Notify`].
//!
//! What the phone and the Mac say to each other is [`Sealed`]: the relay
//! routes it but can't read or change it.
//!
//! Linking a phone takes a second, short WebSocket on [`PAIR_PATH`]: the
//! Mac opens it with a [`PairStart`], shows the code it gets as a QR code,
//! and learns from a [`Paired::Linked`] when a phone has claimed it.

use serde::{Deserialize, Serialize};

/// Bumped when a message or a tool's answer changes shape, since the relay
/// serves the phone app the answers are for; it refuses other versions.
pub const PROTOCOL: u32 = 7;

/// The path the Mac connects to, with `Authorization: Bearer <token>`.
pub const CONNECT_PATH: &str = "/connect";

/// The path the Mac waits on while a phone links, with its token if it has
/// one, so a phone can link through a Mac the relay already knows.
pub const PAIR_PATH: &str = "/pair";

/// The phone app's page that claims pairing code `code` and keeps `secret`,
/// which the phone and this Mac seal their messages with. Both are in the
/// fragment, which the browser never sends, so neither the relay nor a log
/// sees them.
pub fn pair_url(relay: &str, code: &str, secret: &str) -> String {
    format!("{}/#/pair/{code}/{secret}", relay.trim_end_matches('/'))
}

/// Mac → relay: the first message on [`PAIR_PATH`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub protocol: u32,
    /// The Mac's name, which the phone shows.
    pub name: String,
}

/// Relay → Mac on [`PAIR_PATH`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Paired {
    /// The code to show, valid for `expires_in` seconds; the relay closes
    /// the socket when it expires.
    #[serde(rename_all = "camelCase")]
    Code { code: String, expires_in: u64 },
    /// A phone claimed the code. A Mac new to the relay gets its token,
    /// which [`CONNECT_PATH`] takes.
    Linked { token: Option<String> },
}

/// A message only one phone and the Mac can read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// Which of the Mac's phones sealed it, or may open it.
    pub phone: String,
    /// The sealed bytes, in base64url.
    pub data: String,
}

/// Relay → Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Down {
    /// Run one of the phone's tools (`list_chats`, `send_message`, …), and
    /// answer with an [`Up::Reply`] with the same `id`.
    Call { id: u64, sealed: Sealed },
    /// Follow a chat for the phone: push its feed's frames as [`Up::Frame`]s
    /// with this `id` until [`Down::Unwatch`] or the connection ends.
    Watch { id: u64, sealed: Sealed },
    /// Stop following the chat of [`Down::Watch`] `id`.
    Unwatch { id: u64 },
}

/// Mac → relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Up {
    /// The first message after connecting.
    Hello {
        protocol: u32,
        /// The engine's version.
        version: String,
    },
    /// The answer to the [`Down::Call`] with this `id`: sealed for the phone
    /// that called, or why the Mac couldn't open the call.
    Reply {
        id: u64,
        result: Result<String, String>,
    },
    /// A frame of the chat followed for [`Down::Watch`] `id`, sealed for the
    /// phone, or why the Mac can't follow it. An error is the last frame.
    Frame {
        id: u64,
        frame: Result<String, String>,
    },
    /// A chat finished, needs an answer, failed, or was cut off: the notice,
    /// sealed for each linked phone.
    Notify { notices: Vec<Sealed> },
    /// The Mac's battery, `None` on a Mac without one: sent after
    /// [`Up::Hello`] and on each change.
    Power { battery: Option<Battery> },
    /// The Mac unlinked: the relay forgets it and its token, and closes.
    Forget,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sealed(phone: &str) -> Sealed {
        Sealed {
            phone: phone.into(),
            data: "AQID".into(),
        }
    }

    #[test]
    fn messages_keep_their_wire_shape() {
        let call = Down::Call {
            id: 7,
            sealed: sealed("p"),
        };
        let value = serde_json::to_value(&call).unwrap();
        assert_eq!(
            value,
            json!({ "type": "call", "id": 7, "sealed": { "phone": "p", "data": "AQID" } })
        );
        assert_eq!(serde_json::from_value::<Down>(value).unwrap(), call);
        let frame = serde_json::to_value(Up::Frame {
            id: 8,
            frame: Ok("AQID".into()),
        })
        .unwrap();
        assert_eq!(
            frame,
            json!({ "type": "frame", "id": 8, "frame": { "Ok": "AQID" } })
        );
        let reply = Up::Reply {
            id: 7,
            result: Err("this phone isn't linked".into()),
        };
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({ "type": "reply", "id": 7, "result": { "Err": "this phone isn't linked" } })
        );
        let notify = Up::Notify {
            notices: vec![sealed("a"), sealed("b")],
        };
        let value = serde_json::to_value(&notify).unwrap();
        assert_eq!(value["type"], "notify");
        assert_eq!(value["notices"][1]["phone"], "b");
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
    fn pairing_messages_keep_their_wire_shape() {
        let code = Paired::Code {
            code: "c0de".into(),
            expires_in: 600,
        };
        let value = serde_json::to_value(&code).unwrap();
        assert_eq!(
            value,
            json!({ "type": "code", "code": "c0de", "expiresIn": 600 })
        );
        assert_eq!(serde_json::from_value::<Paired>(value).unwrap(), code);
        let linked = serde_json::to_value(Paired::Linked { token: None }).unwrap();
        assert_eq!(linked, json!({ "type": "linked", "token": null }));
        let forget = serde_json::to_value(Up::Forget).unwrap();
        assert_eq!(forget, json!({ "type": "forget" }));
        assert_eq!(
            pair_url("https://relay.example.com/", "c0de", "s3cret"),
            "https://relay.example.com/#/pair/c0de/s3cret"
        );
    }
}
