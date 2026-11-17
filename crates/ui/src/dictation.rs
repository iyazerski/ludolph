#[cfg(target_os = "macos")]
mod speech;

#[cfg(target_os = "macos")]
pub use speech::{Listening, authorize};

/// Whether this platform can dictate; outside macOS the composer shows no microphone.
pub const AVAILABLE: bool = cfg!(target_os = "macos");

#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "only macOS's recognizer hears anything")
)]
pub enum Heard {
    Words(String),
    Level(f32),
    Done,
    Failed(String),
}

type Allowed = Result<(), String>;

pub fn join(before: &str, after: &str) -> String {
    match (before.trim_end(), after.trim_start()) {
        ("", after) => after.to_owned(),
        (before, "") => before.to_owned(),
        (before, after) => format!("{before} {after}"),
    }
}

/// Never made: dictation needs macOS's speech recognizer.
#[cfg(not(target_os = "macos"))]
pub enum Listening {}

#[cfg(not(target_os = "macos"))]
impl Listening {
    pub fn start(_heard: impl Fn(Heard) + Send + Sync + 'static) -> Result<Self, String> {
        Err("Dictation needs macOS".to_owned())
    }

    pub fn stop(&self) {
        match *self {}
    }
}

#[cfg(not(target_os = "macos"))]
pub fn authorize(done: impl FnOnce(Allowed) + Send + 'static) {
    done(Err("Dictation needs macOS".to_owned()));
}
