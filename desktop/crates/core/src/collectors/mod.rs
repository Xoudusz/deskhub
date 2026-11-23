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
pub mod media;
pub mod power;
pub mod shortcuts;
pub mod stats;
pub mod steam;

/// What a collector task gets: where to publish, the live settings, board commands.
pub struct Ctx {
    pub out: Publisher,
    pub settings: watch::Receiver<Settings>,
    pub incoming: broadcast::Receiver<Incoming>,
}

impl Ctx {
    pub fn new(handle: &ServiceHandle) -> Self {
        Self {
            out: handle.publisher(),
            settings: handle.settings(),
            incoming: handle.incoming(),
        }
    }
}

/// Start every collector on the current tokio runtime.
pub fn start_all(handle: &ServiceHandle) {
    tokio::spawn(stats::run(Ctx::new(handle)));
    tokio::spawn(media::run(Ctx::new(handle)));
    tokio::spawn(power::run(Ctx::new(handle)));
    tokio::spawn(ai::run(Ctx::new(handle)));
    tokio::spawn(alerts::run(Ctx::new(handle)));
    tokio::spawn(shortcuts::run(Ctx::new(handle)));
    tokio::spawn(steam::run(Ctx::new(handle)));
}
