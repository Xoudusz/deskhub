//! Desk Hub service core: board protocol, WebSocket session, settings and HTTP helpers.
//! No Tauri or GUI dependency.

pub mod claude_install;
pub mod collectors;
pub mod firmware;
pub mod ghauth;
pub mod http;
pub mod ics;
pub mod msgraph;
pub mod pages;
pub mod plex;
pub mod protocol;
pub mod relay;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod text;
pub mod theme;
pub mod usb;
