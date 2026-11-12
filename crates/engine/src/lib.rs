//! Ludolph's engine: it owns pi processes and keeps the chat index that the
//! app and the phone read.

pub mod engine;
pub mod index;
mod jsonl;
pub mod live;
pub mod pi;
pub mod runtime;
pub mod server;
pub mod session;
mod text;
pub mod tools;
pub mod transcript;
