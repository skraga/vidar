//! One running copy per user session. A second launch asks the first to show its window;
//! `vidar.exe --quit` (used by the installer) asks it to exit.

use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, OpenMutexW, SetEvent, WaitForMultipleObjects,
    EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
};

const MUTEX: PCWSTR = w!("Local\\Vidar_SingleInstance");
const SHOW_EVENT: PCWSTR = w!("Local\\Vidar_ShowWindow");
const QUIT_EVENT: PCWSTR = w!("Local\\Vidar_Quit");

/// What another process asked the running copy to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Show,
    Quit,
}

/// Result of [`request_quit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// No copy was running.
    NotRunning,
    /// The running copy exited (its microphone routing is restored).
    Exited,
    /// It is still running after the timeout, or it could not be signalled (it belongs to
    /// another user, e.g. setup elevated with another account's credentials).
    StillRunning,
}

#[derive(Debug)]
pub enum Acquire {
    /// This is the only copy; keep the guard alive for the process lifetime.
    First(InstanceGuard),
    /// Another Vidar is running; it has been asked to show its window.
    AlreadyRunning,
}

#[derive(Debug)]
pub struct InstanceGuard {
    mutex: HANDLE,
    show_event: HANDLE,
    quit_event: HANDLE,
}

// Kernel handles are usable from any thread.
unsafe impl Send for InstanceGuard {}
unsafe impl Sync for InstanceGuard {}

impl InstanceGuard {
    /// Block until another process asks this copy to show itself or to quit, or `timeout_ms`
    /// passes. Call from a background thread in a loop.
    pub fn wait_request(&self, timeout_ms: u32) -> Option<Request> {
        if self.show_event.is_invalid() || self.quit_event.is_invalid() {
            std::thread::sleep(Duration::from_millis(u64::from(timeout_ms)));
            return None;
        }
        // SAFETY: valid auto-reset event handles owned by self.
        let r = unsafe {
            WaitForMultipleObjects(&[self.show_event, self.quit_event], false, timeout_ms)
        };
        match r.0.wrapping_sub(WAIT_OBJECT_0.0) {
            0 => Some(Request::Show),
            1 => Some(Request::Quit),
            _ => None,
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: handles created in `acquire`, closed once.
        unsafe {
            let _ = CloseHandle(self.quit_event);
            let _ = CloseHandle(self.show_event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

/// Is a copy running in this session? The single-instance mutex exists exactly as long as
/// a running copy holds it. Access denied (another user's copy) also counts as running.
fn running() -> bool {
    // SAFETY: open-and-close of a named object.
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, MUTEX) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(e) => e.code() != ERROR_FILE_NOT_FOUND.to_hresult(),
        }
    }
}

/// Ask the running copy to quit (it stops, restores the user's microphone and exits) and
/// wait up to `timeout` for it to be gone.
pub fn request_quit(timeout: Duration) -> QuitOutcome {
    if !running() {
        return QuitOutcome::NotRunning;
    }
    // SAFETY: open, signal and close a named event.
    let signalled = unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, QUIT_EVENT) {
            Ok(ev) => {
                let ok = SetEvent(ev).is_ok();
                let _ = CloseHandle(ev);
                ok
            }
            Err(_) => false,
        }
    };
    if !signalled {
        return QuitOutcome::StillRunning;
    }
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !running() {
            return QuitOutcome::Exited;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    QuitOutcome::StillRunning
}

pub fn acquire() -> Acquire {
    // SAFETY: plain kernel object calls; handles are closed on every path.
    unsafe {
        let Ok(mutex) = CreateMutexW(None, false, MUTEX) else {
            // Cannot create a mutex at all: run anyway rather than refuse to start.
            return Acquire::First(InstanceGuard {
                mutex: HANDLE::default(),
                show_event: HANDLE::default(),
                quit_event: HANDLE::default(),
            });
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
            return Acquire::AlreadyRunning;
        }

        let show_event = CreateEventW(None, false, false, SHOW_EVENT).unwrap_or_default();
        let quit_event = CreateEventW(None, false, false, QUIT_EVENT).unwrap_or_default();
        Acquire::First(InstanceGuard {
            mutex,
            show_event,
            quit_event,
        })
    }
}
