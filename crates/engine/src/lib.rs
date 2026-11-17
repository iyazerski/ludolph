pub const VERSION: &str = match option_env!("LUDOLPH_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod agent_tools;
pub mod attachments;
pub mod engine;
pub mod feed;
pub mod index;
mod jsonl;
pub mod live;
pub mod login;
pub mod models;
#[cfg_attr(target_os = "macos", path = "notify/libsystem.rs")]
#[cfg_attr(not(target_os = "macos"), path = "notify/none.rs")]
mod notify;
pub mod pi;
pub mod pi_setup;
#[cfg_attr(target_os = "macos", path = "power/iokit.rs")]
#[cfg_attr(not(target_os = "macos"), path = "power/none.rs")]
pub mod power;
pub mod relay;
pub mod repo;
pub mod runtime;
pub mod seal;
pub mod server;
pub mod session;
pub mod settings;
pub mod sign_in;
pub mod skills;
mod text;
pub mod title;
pub mod tools;
pub mod transcript;
pub mod turns;
pub mod watch;
