//! "Start with Windows": a value under HKCU\...\Run that launches Vidar minimised to the tray.

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Vidar");

fn open(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: opening a well-known per-user key; closed by the caller.
    unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key)
            .is_ok()
            .then_some(key)
    }
}

pub fn is_enabled() -> bool {
    let Some(key) = open(KEY_READ) else {
        return false;
    };
    // SAFETY: size-only query, then the key is closed.
    unsafe {
        let found = RegQueryValueExW(key, VALUE, None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}

pub fn set(enabled: bool) -> Result<(), String> {
    let key = open(KEY_SET_VALUE).ok_or("cannot open the Run key")?;
    // SAFETY: writes or deletes one string value in the user's Run key.
    let r = unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = format!("\"{}\" --minimized", exe.display());
            let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(key, VALUE, None, REG_SZ, Some(bytes))
                .ok()
                .map_err(|e| e.to_string())
        } else {
            let r = RegDeleteValueW(key, VALUE);
            if r.is_ok() || r.0 == 2 {
                Ok(())
            } else {
                Err(format!("error {}", r.0))
            }
        }
    };
    // SAFETY: key opened above.
    unsafe {
        let _ = RegCloseKey(key);
    }
    let _ = HSTRING::new();
    r
}
