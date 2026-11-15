//! Windows audio endpoint control for Vidar: listing, default switching, branding, device
//! events, driver install and single instance.
//!
//! `model` and `router` hold the decisions and are platform independent (and unit tested
//! against a fake system); `sys`, `branding`, `events` and `instance` are the Windows side.

pub mod model;
pub mod router;

#[cfg(windows)]
pub mod branding;
#[cfg(windows)]
pub mod driver;
#[cfg(windows)]
pub mod events;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod sys;

pub use model::{is_valid_endpoint_id, Bus, Endpoint, Flow, VB_CABLE_INTERFACE};
pub use router::{AudioSystem, RouteError, RouteState};
#[cfg(windows)]
pub use sys::WinAudio;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Windows audio call failed: {0}")]
    Com(String),
    #[error("{0}")]
    Other(String),
}
