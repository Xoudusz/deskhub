//! Data collectors: each one gathers one kind of PC state and publishes it to the
//! board through a `Publisher` (latest state per key, resent after every reconnect).
//! Windows-only parts sit behind `#[cfg(windows)]`; elsewhere the collector idles, so
//! the crate builds and tests on Linux.

use tokio::sync::{broadcast, watch};

use crate::protocol::Incoming;
use crate::session::{Publisher, ServiceHandle};
use crate::settings::Settings;

pub mod ai;
pub mod alerts;
pub mod location;
pub mod media;
pub mod plex;
pub mod power;
pub mod remote;
pub mod shortcuts;
pub mod stats;
pub mod steam;
pub mod theme;

/// What a collector task gets: where to publish, the live settings, board commands.
pub struct Ctx {
    pub out: Publisher,
    pub settings: watch::Receiver<Settings>,
    pub incoming: broadcast::Receiver<Incoming>,
    /// Claude Code on a server (collectors/remote.rs).
    pub remote: remote::RemoteRx,
}

impl Ctx {
    pub fn new(handle: &ServiceHandle, remote: &remote::RemoteRx) -> Self {
        Self {
            out: handle.publisher(),
            settings: handle.settings(),
            incoming: handle.incoming(),
            remote: remote.clone(),
        }
    }
}

/// Start every collector on the current tokio runtime.
/// Starts every collector. Returns the server poller's receiver (its status for the UI).
pub fn start_all(handle: &ServiceHandle) -> remote::RemoteRx {
    let (tx, rx) = remote::channel();
    tokio::spawn(remote::run(tx, handle.settings()));
    tokio::spawn(stats::run(Ctx::new(handle, &rx)));
    tokio::spawn(media::run(Ctx::new(handle, &rx)));
    tokio::spawn(power::run(Ctx::new(handle, &rx)));
    tokio::spawn(ai::run(Ctx::new(handle, &rx)));
    tokio::spawn(alerts::run(Ctx::new(handle, &rx)));
    tokio::spawn(shortcuts::run(Ctx::new(handle, &rx)));
    tokio::spawn(steam::run(Ctx::new(handle, &rx)));
    tokio::spawn(theme::run(Ctx::new(handle, &rx)));
    tokio::spawn(location::run(Ctx::new(handle, &rx)));
    tokio::spawn(plex::run(Ctx::new(handle, &rx)));
    rx
}
