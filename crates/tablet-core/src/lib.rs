//! Everything Tablet knows that is not D-Bus or drawing: where files live,
//! the config, openvpn3's statuses and sessions, the checks, the doctor,
//! the daemon's state, the bar line, the socket protocol and the waybar
//! reader.

pub mod bar;
pub mod checks;
pub mod config;
pub mod doctor;
pub mod model;
pub mod openvpn;
pub mod paths;
pub mod protocol;
pub mod route;
pub mod state;
pub mod text;
pub mod transport;
pub mod waybar;

pub use bar_anchor::{Anchor, Edge};

/// Unix time in seconds.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
