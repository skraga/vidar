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

/// Is the PC itself ARM64, whatever this build is? An x64 build running emulated on
/// Windows on ARM answers true, so it can move to the native ARM64 build.
#[cfg(windows)]
pub fn native_arm64() -> bool {
    use windows::Win32::System::SystemInformation::{IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_ARM64};
    use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
    if cfg!(target_arch = "aarch64") {
        return true;
    }
    let (mut process, mut native) = (IMAGE_FILE_MACHINE::default(), IMAGE_FILE_MACHINE::default());
    // SAFETY: the pseudo handle of the current process is always valid; both pointers are
    // to locals.
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) };
    ok.is_ok() && native == IMAGE_FILE_MACHINE_ARM64
}

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
