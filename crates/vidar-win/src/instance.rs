//! One running copy per user session. A second launch asks the first to show its window.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

const MUTEX: PCWSTR = w!("Local\\Vidar_SingleInstance");
const SHOW_EVENT: PCWSTR = w!("Local\\Vidar_ShowWindow");

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
}

// Kernel handles are usable from any thread.
unsafe impl Send for InstanceGuard {}
unsafe impl Sync for InstanceGuard {}

impl InstanceGuard {
    /// Block until a second launch asks this copy to show itself, or `timeout_ms` passes.
    /// Returns true when asked. Call from a background thread in a loop.
    pub fn wait_show_request(&self, timeout_ms: u32) -> bool {
        // SAFETY: valid auto-reset event handle owned by self.
        unsafe { WaitForSingleObject(self.show_event, timeout_ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: handles created in `acquire`, closed once.
        unsafe {
            let _ = CloseHandle(self.show_event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

pub fn acquire() -> Acquire {
    // SAFETY: plain kernel object calls; handles are closed on every path.
    unsafe {
        let Ok(mutex) = CreateMutexW(None, false, MUTEX) else {
            // Cannot create a mutex at all: run anyway rather than refuse to start.
            return Acquire::First(InstanceGuard {
                mutex: HANDLE::default(),
                show_event: HANDLE::default(),
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
        Acquire::First(InstanceGuard { mutex, show_event })
    }
}
