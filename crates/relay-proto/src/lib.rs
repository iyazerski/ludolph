use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Bump when messages or tool answers change shape; the relay rejects mismatched clients.
pub const PROTOCOL: u32 = 7;

pub const CONNECT_PATH: &str = "/connect";

/// How often a connected Mac pings the relay. Both sides drop a link that
/// stays quiet for a few pings, since a network change can leave it open but dead.
pub const PING_EVERY: Duration = Duration::from_secs(10);

pub const PAIR_PATH: &str = "/pair";

/// Put the pairing code and secret in the fragment so neither reaches the relay or its logs.
pub fn pair_url(relay: &str, code: &str, secret: &str) -> String {
    format!("{}/#/pair/{code}/{secret}", relay.trim_end_matches('/'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub protocol: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Paired {
    #[serde(rename_all = "camelCase")]
    Code { code: String, expires_in: u64 },
    /// A phone claimed the code. A Mac new to the relay gets its token,
    /// which [`CONNECT_PATH`] takes.
    Linked { token: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub phone: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Down {
    Call {
        id: u64,
        sealed: Sealed,
    },
    /// Follow a chat for the phone: push its feed's frames as [`Up::Frame`]s
    /// with this `id` until [`Down::Unwatch`] or the connection ends.
    Watch {
        id: u64,
        sealed: Sealed,
    },
    Unwatch {
        id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Up {
    Hello {
        protocol: u32,
        version: String,
    },
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
    Notify {
        notices: Vec<Sealed>,
    },
    Power {
        battery: Option<Battery>,
    },
    Forget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatteryState {
    Charging,
    /// On power, but not charging (full, or holding its charge).
    Plugged,
    Unplugged,
}
