//! Ludolph's engine: it owns pi processes and keeps the chat index that the
//! app and the phone read.

pub mod engine;
pub mod index;
mod jsonl;
pub mod live;
pub mod models;
pub mod pi;
pub mod repo;
pub mod runtime;
pub mod server;
pub mod session;
pub mod skills;
mod text;
pub mod title;
pub mod tools;
pub mod transcript;
pub mod turns;
pub mod watch;
