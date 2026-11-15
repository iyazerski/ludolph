pub const VERSION: &str = match option_env!("LUDOLPH_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub mod engine;
pub mod feed;
pub mod index;
mod jsonl;
pub mod live;
pub mod login;
pub mod models;
pub mod pi;
pub mod power;
pub mod relay;
pub mod repo;
pub mod runtime;
pub mod seal;
pub mod server;
pub mod session;
pub mod settings;
pub mod skills;
mod text;
pub mod title;
pub mod tools;
pub mod transcript;
pub mod turns;
pub mod watch;
