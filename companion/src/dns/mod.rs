//! Opt-in DNS development core; independent of the Native Messaging host lifecycle.

pub mod blocklist;
mod cache;
pub mod config;
pub mod diagnostics;
mod resolver;
pub mod server;

pub use config::DnsConfig;
