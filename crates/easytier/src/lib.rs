#![allow(dead_code)]

// Re-export `Instant` at the crate root so public APIs that expose it
// (e.g. `Route::get_peer_info_last_update_time`) reference a deliberate
// public type rather than leaking an inaccessible one.
pub use quanta::Instant;

mod arch;
mod gateway;
pub mod instance;
mod peer_center;
mod vpn_portal;

pub mod common;
pub mod connector;
pub mod embedded;
pub mod instance_manager;
pub mod launcher;
pub mod peers;
pub mod proto;
pub mod rpc_service;
pub mod tunnel;
pub mod utils;
pub mod web_client;

#[cfg(test)]
mod tests;

pub const VERSION: &str = common::constants::EASYTIER_VERSION;
rust_i18n::i18n!("locales", fallback = "en");
