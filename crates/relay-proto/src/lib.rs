//! Messages between the Ludolph engine (on each Mac) and the relay.
//!
//! It's still the protocol of Zeron's relay, which serves the phone today, so
//! the engine works with it as deployed. Version 2 comes with moving the
//! relay into this repo (`docs/design.md`, "Phone and relay").
//!
//! The Mac opens one WebSocket to the relay and keeps it open. Both sides
//! send JSON text frames: the relay sends [`Call`]s, the Mac answers each with
//! an [`Up::Reply`] and reports chat changes with [`Up::Notify`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped when a message changes shape; the relay refuses other versions.
pub const PROTOCOL: u32 = 1;

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
    /// A chat finished, needs an answer, or failed.
    Notify(Notice),
    /// Whether the Mac takes calls: sent after [`Up::Hello`] and on each
    /// change.
    Engine(Engine),
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

/// Whether the Mac can take calls. Zeron's relay tells the phone which;
/// the engine, which serves calls itself, is always [`Engine::Running`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Engine {
    /// Taking calls. `running` is the version serving them, if known.
    Running {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        running: Option<String>,
    },
    /// Zeron isn't open; its `open_zeron` call starts it.
    Closed,
    /// Zeron is open, but its IPC changed in a way zeron-tools can't read.
    Outdated { running: String },
}

impl Engine {
    /// Whether calls can go through.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
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
    /// Its run failed or was cut off.
    Failed,
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
        let engine = Up::Engine(Engine::Outdated {
            running: "0.3.0".into(),
        });
        let value = serde_json::to_value(&engine).unwrap();
        assert_eq!(
            value,
            json!({ "type": "engine", "state": "outdated", "running": "0.3.0" })
        );
        assert_eq!(serde_json::from_value::<Up>(value).unwrap(), engine);
        let running = Up::Engine(Engine::Running {
            running: Some("0.2.114".into()),
        });
        let value = serde_json::to_value(&running).unwrap();
        assert_eq!(
            value,
            json!({ "type": "engine", "state": "running", "running": "0.2.114" })
        );
        assert_eq!(serde_json::from_value::<Up>(value).unwrap(), running);
        assert_eq!(
            serde_json::from_value::<Engine>(json!({ "state": "running" })).unwrap(),
            Engine::Running { running: None }
        );
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
}
