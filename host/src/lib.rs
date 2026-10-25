//! PSP Companion host library. The binary in `main.rs` wires these modules to a tray icon.

pub mod actions;
pub mod app;
pub mod audio;
pub mod config;
pub mod imaging;
pub mod macros;
pub mod net;
pub mod pairing;
pub mod settings;
pub mod state;
pub mod streamdeck;
#[cfg(feature = "tray")]
pub mod tray;
pub mod voice;
pub mod widgets;
