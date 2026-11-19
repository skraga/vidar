//! Installing and removing the bundled VB-Audio Virtual Cable driver, keeping the user's
//! default devices.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::branding::{self, Brand};
use crate::model::Flow;
use crate::router::{cable_capture, ensure_physical_default, restore_default, AudioSystem};
use crate::sys::WinAudio;
use crate::Error;

pub const SETUP_X64: &str = "VBCABLE_Setup_x64.exe";

/// Is the cable present and active?
pub fn cable_installed() -> bool {
    WinAudio::new()
        .map(|a| cable_capture(&a, None).is_some())
        .unwrap_or(false)
}

pub fn is_elevated() -> bool {
    // SAFETY: simple query of the current token.
    unsafe { IsUserAnAdmin().as_bool() }
}

/// Run `exe args` (elevated with a UAC prompt unless already elevated) and wait.
/// Returns the exit code.
pub fn run_maybe_elevated(
    exe: &Path,
    args: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<u32, Error> {
    let verb = if is_elevated() {
        HSTRING::from("open")
    } else {
        HSTRING::from("runas")
    };
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let directory = HSTRING::from(dir.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: all strings outlive the call; the process handle is closed below.
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| {
            Error::Other(format!(
                "could not start {}: {e} (UAC declined?)",
                exe.display()
            ))
        })?;
        if info.hProcess.is_invalid() {
            return Ok(0);
        }
        let waited = WaitForSingleObject(
            info.hProcess,
            timeout.as_millis().min(u32::MAX as u128) as u32,
        );
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err(Error::Other(format!(
                "{} did not finish in time",
                exe.display()
            )));
        }
        Ok(code)
    }
}

/// Start `exe args` elevated (one UAC prompt) and return once it is running, without waiting
/// for it. Fails if the user declines the prompt.
pub fn launch_elevated(exe: &Path, args: &str) -> Result<(), Error> {
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let directory = HSTRING::from(exe.parent().unwrap_or(Path::new(".")).as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: all strings outlive the call; the process handle is closed below.
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| {
            Error::Other(format!(
                "could not start {}: {e} (UAC declined?)",
                exe.display()
            ))
        })?;
        if !info.hProcess.is_invalid() {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

/// What `install` did.
#[derive(Debug, Default)]
pub struct InstallReport {
    pub restored_playback: Option<String>,
    pub restored_recording: Option<String>,
    pub branded: Option<(String, String)>,
    pub brand_error: Option<String>,
}

/// Install the cable from `driver_dir`, keep the user's default devices, then brand it.
pub fn install(driver_dir: &Path, brand: Option<(&Brand, &Path)>) -> Result<InstallReport, Error> {
    let setup = driver_dir.join(SETUP_X64);
    if !setup.is_file() {
        return Err(Error::Other(format!(
            "driver setup not found at {}",
            setup.display()
        )));
    }

    let before = WinAudio::new()?;
    let prev_render = before
        .default_endpoint(Flow::Render)
        .filter(|e| !e.is_virtual())
        .map(|e| e.id);
    let prev_capture = before
        .default_endpoint(Flow::Capture)
        .filter(|e| !e.is_virtual())
        .map(|e| e.id);
    drop(before);

    run_maybe_elevated(&setup, "-i -h", driver_dir, Duration::from_secs(120))?;

    // Give the endpoint builder time to register the new devices.
    let mut audio = WinAudio::new()?;
    for _ in 0..20 {
        if cable_capture(&audio, None).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        audio = WinAudio::new()?;
    }

    let mut report = InstallReport::default();
    for (flow, prev) in [(Flow::Render, prev_render), (Flow::Capture, prev_capture)] {
        let restored = match prev {
            Some(id) if restore_default(&audio, &id) => Some(id),
            _ => ensure_physical_default(&audio, flow)
                .ok()
                .flatten()
                .map(|e| e.id),
        };
        match flow {
            Flow::Render => report.restored_playback = restored,
            Flow::Capture => report.restored_recording = restored,
        }
    }

    if cable_capture(&audio, None).is_none() {
        return Err(Error::Other(
            "the driver installer finished but the cable did not appear (a reboot may be needed)"
                .into(),
        ));
    }

    if let Some((brand, backup)) = brand {
        match branding::brand(&audio, brand, backup) {
            Ok((cap, ren)) => report.branded = Some((cap.name, ren.name)),
            Err(e) => report.brand_error = Some(e.to_string()),
        }
    }
    Ok(report)
}

/// Remove the cable (used by the uninstaller when the user agrees). Unbrands first.
pub fn uninstall(driver_dir: &Path, backup: &Path) -> Result<(), Error> {
    if let Ok(audio) = WinAudio::new() {
        let _ = branding::unbrand(&audio, backup);
        let _ = ensure_physical_default(&audio, Flow::Capture);
        let _ = ensure_physical_default(&audio, Flow::Render);
    }
    let setup = driver_dir.join(SETUP_X64);
    run_maybe_elevated(&setup, "-u -h", driver_dir, Duration::from_secs(120))?;
    Ok(())
}

/// `%ProgramData%\Vidar\endpoint-branding.tsv`: shared by the installer (elevated) and the app.
pub fn default_branding_backup() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    base.join("Vidar").join("endpoint-branding.tsv")
}
