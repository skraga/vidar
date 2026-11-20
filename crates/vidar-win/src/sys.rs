#![allow(non_snake_case)] // COM method names follow the Windows SDK
//! Windows Core Audio implementation of [`AudioSystem`], plus branding.

use std::ffi::c_void;

use windows::core::{interface, BOOL, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::{
    PKEY_DeviceClass_IconPath, PKEY_DeviceInterface_FriendlyName, PKEY_Device_DeviceDesc,
    PKEY_Device_EnumeratorName, PKEY_Device_FriendlyName,
};
use windows::Win32::Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateActive, EDataFlow,
    ERole, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    PKEY_AudioEndpoint_FormFactor, Speakers, DEVICE_STATEMASK_ALL, DEVICE_STATE_ACTIVE,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32, PROPVARIANT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ, STGM_READWRITE,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::model::{Bus, Endpoint, Flow};
use crate::router::AudioSystem;
use crate::Error;

/// Undocumented interface the Sound control panel uses (Windows 10 1903+ layout).
/// Method order must match exactly; only `SetPropertyValue`, `SetDefaultEndpoint` and
/// `SetEndpointVisibility` are used.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut WAVEFORMATEX) -> HRESULT;
    fn GetDeviceFormat(
        &self,
        device: PCWSTR,
        default: BOOL,
        format: *mut *mut WAVEFORMATEX,
    ) -> HRESULT;
    fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(
        &self,
        device: PCWSTR,
        endpoint: *mut WAVEFORMATEX,
        mix: *mut WAVEFORMATEX,
    ) -> HRESULT;
    fn GetProcessingPeriod(
        &self,
        device: PCWSTR,
        default: BOOL,
        default_period: *mut i64,
        min_period: *mut i64,
    ) -> HRESULT;
    fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn GetPropertyValue(
        &self,
        device: PCWSTR,
        fx_store: BOOL,
        key: *const PROPERTYKEY,
        value: *mut PROPVARIANT,
    ) -> HRESULT;
    fn SetPropertyValue(
        &self,
        device: PCWSTR,
        fx_store: BOOL,
        key: *const PROPERTYKEY,
        value: *const PROPVARIANT,
    ) -> HRESULT;
    fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
    fn SetEndpointVisibility(&self, device: PCWSTR, visible: BOOL) -> HRESULT;
}

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Joins the calling thread to the COM multithreaded apartment (idempotent).
pub fn com_init() {
    // SAFETY: plain COM initialisation; a thread already in an STA keeps its apartment
    // (RPC_E_CHANGED_MODE), which still allows using these free-threaded objects.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        // Nothing to do: the following COM calls will report the failure.
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read a string property; `None` if missing or not a string.
fn read_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    // SAFETY: GetValue returns an owned PROPVARIANT, cleared below; the allocated string
    // from PropVariantToStringAlloc is freed with CoTaskMemFree.
    unsafe {
        let mut pv = store.GetValue(key).ok()?;
        let out = PropVariantToStringAlloc(&pv).ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s
        });
        let _ = PropVariantClear(&mut pv);
        out.filter(|s| !s.is_empty())
    }
}

/// A VT_LPWSTR PROPVARIANT owning a CoTaskMem copy of `s`. Clear it with PropVariantClear.
fn string_variant(s: &str) -> PROPVARIANT {
    let w = wide(s);
    // SAFETY: allocation sized for the UTF-16 text plus terminator; ownership passes to the
    // PROPVARIANT, released by PropVariantClear.
    unsafe {
        let buf = CoTaskMemAlloc(w.len() * 2) as *mut u16;
        std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
        let mut pv = PROPVARIANT::default();
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(buf);
        pv
    }
}

fn to_flow(f: Flow) -> EDataFlow {
    match f {
        Flow::Capture => eCapture,
        Flow::Render => eRender,
    }
}

/// Core Audio access for one thread.
pub struct WinAudio {
    enumerator: IMMDeviceEnumerator,
}

impl WinAudio {
    pub fn new() -> Result<Self, Error> {
        com_init();
        // SAFETY: standard COM object creation.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| Error::Com(e.to_string()))?;
        Ok(Self { enumerator })
    }

    pub fn enumerator(&self) -> &IMMDeviceEnumerator {
        &self.enumerator
    }

    fn describe(&self, dev: &IMMDevice, flow: Flow) -> Option<Endpoint> {
        // SAFETY: COM calls on a valid device; the ID string is freed after copying.
        unsafe {
            let pid = dev.GetId().ok()?;
            let id = pid.to_string().unwrap_or_default();
            CoTaskMemFree(Some(pid.0 as *const c_void));
            let state = dev.GetState().map(|s| s.0).unwrap_or(0);
            let store = dev.OpenPropertyStore(STGM_READ).ok();
            let get = |k: &PROPERTYKEY| {
                store
                    .as_ref()
                    .and_then(|s| read_string(s, k))
                    .unwrap_or_default()
            };
            let description = get(&PKEY_Device_DeviceDesc);
            let interface_name = get(&PKEY_DeviceInterface_FriendlyName);
            let mut name = get(&PKEY_Device_FriendlyName);
            if name.is_empty() {
                name = if interface_name.is_empty() {
                    description.clone()
                } else {
                    format!("{description} ({interface_name})")
                };
            }
            Some(Endpoint {
                id,
                flow,
                name,
                description,
                interface_name,
                bus: Bus::from_enumerator(&get(&PKEY_Device_EnumeratorName)),
                state,
            })
        }
    }

    fn flow_of(id: &str) -> Flow {
        // Endpoint IDs start with {0.0.0...} for render and {0.0.1...} for capture.
        if id.starts_with("{0.0.1.") {
            Flow::Capture
        } else {
            Flow::Render
        }
    }

    /// All endpoints of a flow, any state (for showing unplugged devices too).
    pub fn all_endpoints(&self, flow: Flow) -> Vec<Endpoint> {
        self.list(flow, DEVICE_STATEMASK_ALL)
    }

    fn list(&self, flow: Flow, mask: u32) -> Vec<Endpoint> {
        let mut out = Vec::new();
        // SAFETY: COM enumeration on a valid enumerator.
        unsafe {
            let Ok(col) = self.enumerator.EnumAudioEndpoints(
                to_flow(flow),
                windows::Win32::Media::Audio::DEVICE_STATE(mask),
            ) else {
                return out;
            };
            let n = col.GetCount().unwrap_or(0);
            for i in 0..n {
                if let Ok(dev) = col.Item(i) {
                    if let Some(e) = self.describe(&dev, flow) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }

    fn policy() -> Result<IPolicyConfig, Error> {
        // SAFETY: COM object creation; the class is part of Windows since Vista.
        unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }
            .map_err(|e| Error::Com(e.to_string()))
    }

    /// Write a string property on an endpoint. Tries the Sound control panel's path
    /// (IPolicyConfig, works without elevation on current Windows), then the endpoint's
    /// own property store (needs administrator rights).
    pub fn set_endpoint_string(
        &self,
        id: &str,
        key: &PROPERTYKEY,
        value: &str,
    ) -> Result<(), Error> {
        let wid = wide(id);
        let mut pv = string_variant(value);
        // SAFETY: valid wide string, key and PROPVARIANT for the duration of the calls.
        let result = unsafe {
            let via_policy = Self::policy()
                .ok()
                .map(|p| p.SetPropertyValue(PCWSTR(wid.as_ptr()), BOOL(0), key, &pv))
                .filter(|hr| hr.is_ok());
            if via_policy.is_some() {
                Ok(())
            } else {
                self.enumerator
                    .GetDevice(PCWSTR(wid.as_ptr()))
                    .and_then(|dev| dev.OpenPropertyStore(STGM_READWRITE))
                    .and_then(|store| {
                        store.SetValue(key, &pv)?;
                        store.Commit()
                    })
                    .map_err(|e| Error::Com(e.to_string()))
            }
        };
        // SAFETY: releases the string allocated in string_variant.
        unsafe {
            let _ = PropVariantClear(&mut pv);
        }
        result
    }

    /// Current icon path of an endpoint (`"path,index"`), if set.
    pub fn endpoint_icon(&self, id: &str) -> Option<String> {
        let wid = wide(id);
        // SAFETY: read-only property access.
        unsafe {
            let dev = self.enumerator.GetDevice(PCWSTR(wid.as_ptr())).ok()?;
            let store = dev.OpenPropertyStore(STGM_READ).ok()?;
            read_string(&store, &PKEY_DeviceClass_IconPath)
        }
    }

    /// Rename an endpoint (the part before the parentheses).
    pub fn set_description(&self, id: &str, description: &str) -> Result<(), Error> {
        self.set_endpoint_string(id, &PKEY_Device_DeviceDesc, description)
    }

    /// Enable or disable an endpoint, like Disable/Enable in the Sound control panel. A disabled
    /// endpoint disappears from every device list until it is enabled again.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), Error> {
        let policy = Self::policy()?;
        let wid = wide(id);
        // SAFETY: valid wide string for the duration of the call.
        unsafe { policy.SetEndpointVisibility(PCWSTR(wid.as_ptr()), BOOL::from(enabled)) }
            .ok()
            .map_err(|e| Error::Com(e.to_string()))?;
        Ok(())
    }

    /// True if Windows presents the endpoint as speakers (`PKEY_AudioEndpoint_FormFactor`
    /// = Speakers), as opposed to a line-level jack or other form factor.
    pub fn is_speaker(&self, id: &str) -> bool {
        let wid = wide(id);
        // SAFETY: read-only property access; the PROPVARIANT is cleared before returning.
        unsafe {
            let Ok(dev) = self.enumerator.GetDevice(PCWSTR(wid.as_ptr())) else {
                return false;
            };
            let Ok(store) = dev.OpenPropertyStore(STGM_READ) else {
                return false;
            };
            let Ok(mut pv) = store.GetValue(&PKEY_AudioEndpoint_FormFactor) else {
                return false;
            };
            let form = PropVariantToUInt32(&pv).ok();
            let _ = PropVariantClear(&mut pv);
            form == Some(Speakers.0 as u32)
        }
    }

    /// Set an endpoint's icon to `"C:\path\file.ico,0"` (or a DLL resource like `"%windir%\...dll,-3010"`).
    pub fn set_icon(&self, id: &str, icon: &str) -> Result<(), Error> {
        self.set_endpoint_string(id, &PKEY_DeviceClass_IconPath, icon)
    }

    fn endpoint_volume(&self, id: &str) -> Result<IAudioEndpointVolume, Error> {
        let wid = wide(id);
        // SAFETY: COM lookup by ID string, then activation of a standard interface.
        unsafe {
            self.enumerator
                .GetDevice(PCWSTR(wid.as_ptr()))
                .and_then(|dev| dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None))
        }
        .map_err(|e| Error::Com(e.to_string()))
    }

    /// The endpoint's Windows volume (the "Input volume" of a microphone in Sound settings):
    /// level 0..1 and whether it is muted. `None` if the device has no volume control.
    pub fn volume(&self, id: &str) -> Option<EndpointVolume> {
        let vol = self.endpoint_volume(id).ok()?;
        // SAFETY: plain getters on a live interface.
        unsafe {
            Some(EndpointVolume {
                level: vol.GetMasterVolumeLevelScalar().ok()?,
                muted: vol.GetMute().ok()?.as_bool(),
            })
        }
    }

    /// Set the endpoint's Windows volume (0..1, clamped). Other apps and Sound settings see
    /// the change at once.
    pub fn set_volume(&self, id: &str, level: f32) -> Result<(), Error> {
        let vol = self.endpoint_volume(id)?;
        // SAFETY: setter on a live interface; no event context.
        unsafe { vol.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) }
            .map_err(|e| Error::Com(e.to_string()))
    }

    /// Is any app streaming on the endpoint right now? For a microphone: is anything
    /// recording from it. `None` when Windows cannot say (treat as "yes").
    pub fn in_use(&self, id: &str) -> Option<bool> {
        let wid = wide(id);
        // SAFETY: COM lookup and plain queries on live interfaces.
        unsafe {
            let dev = self.enumerator.GetDevice(PCWSTR(wid.as_ptr())).ok()?;
            let manager = dev
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                .ok()?;
            let sessions = manager.GetSessionEnumerator().ok()?;
            let count = sessions.GetCount().ok()?;
            for i in 0..count {
                let Ok(session) = sessions.GetSession(i) else {
                    continue;
                };
                if session.GetState().ok() == Some(AudioSessionStateActive) {
                    return Some(true);
                }
            }
            Some(false)
        }
    }

    /// Mute or unmute the endpoint in Windows.
    pub fn set_muted(&self, id: &str, muted: bool) -> Result<(), Error> {
        let vol = self.endpoint_volume(id)?;
        // SAFETY: setter on a live interface; no event context.
        unsafe { vol.SetMute(muted, std::ptr::null()) }.map_err(|e| Error::Com(e.to_string()))
    }
}

/// An endpoint's Windows volume, see [`WinAudio::volume`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EndpointVolume {
    /// 0..1, as the Sound settings slider shows it (0-100).
    pub level: f32,
    pub muted: bool,
}

impl AudioSystem for WinAudio {
    fn endpoints(&self, flow: Flow) -> Vec<Endpoint> {
        self.list(flow, DEVICE_STATE_ACTIVE.0)
    }

    fn default_endpoint(&self, flow: Flow) -> Option<Endpoint> {
        // SAFETY: COM call; console role first, communications as fallback.
        unsafe {
            let dev = self
                .enumerator
                .GetDefaultAudioEndpoint(to_flow(flow), eConsole)
                .or_else(|_| {
                    self.enumerator
                        .GetDefaultAudioEndpoint(to_flow(flow), eCommunications)
                })
                .ok()?;
            self.describe(&dev, flow)
        }
    }

    fn endpoint(&self, id: &str) -> Option<Endpoint> {
        let wid = wide(id);
        // SAFETY: COM lookup by ID string.
        unsafe {
            let dev = self.enumerator.GetDevice(PCWSTR(wid.as_ptr())).ok()?;
            self.describe(&dev, Self::flow_of(id))
        }
    }

    fn set_default(&self, id: &str) -> bool {
        let Ok(policy) = Self::policy() else {
            return false;
        };
        let wid = wide(id);
        // SAFETY: valid wide string for the duration of the calls.
        unsafe {
            [eConsole, eMultimedia, eCommunications]
                .into_iter()
                .map(|role| {
                    policy
                        .SetDefaultEndpoint(PCWSTR(wid.as_ptr()), role)
                        .is_ok()
                })
                // Not `any()`: it would short-circuit and skip the remaining roles.
                .filter(|&ok| ok)
                .count()
                > 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only smoke test against the real machine: listing must not fail or panic.
    #[test]
    fn lists_endpoints_and_defaults() {
        let sys = WinAudio::new().unwrap();
        let caps = sys.endpoints(Flow::Capture);
        let renders = sys.endpoints(Flow::Render);
        for e in caps.iter().chain(renders.iter()) {
            assert!(
                crate::model::is_valid_endpoint_id(&e.id),
                "unexpected endpoint id format: {}",
                e.id
            );
            println!(
                "{:?} {:<60} bus={:?} cable={} virtual={}",
                e.flow,
                e.name,
                e.bus,
                e.is_vb_cable(),
                e.is_virtual()
            );
        }
        if let Some(d) = sys.default_endpoint(Flow::Capture) {
            println!("default mic: {}", d.name);
            assert_eq!(sys.endpoint(&d.id).map(|e| e.id), Some(d.id));
        }
    }

    /// Changes the default microphone's Windows volume for a moment and puts it back:
    /// `cargo test -p vidar-win -- --ignored mic_volume_round_trip`.
    #[test]
    #[ignore = "changes the real microphone volume briefly"]
    fn mic_volume_round_trip() {
        let sys = WinAudio::new().unwrap();
        let mic = sys
            .endpoints(Flow::Capture)
            .into_iter()
            .find(|e| !e.is_virtual())
            .expect("a physical microphone");
        let before = sys.volume(&mic.id).expect("volume control");
        println!("{}: {before:?}", mic.name);
        let probe = if before.level > 0.5 { 0.25 } else { 0.75 };
        sys.set_volume(&mic.id, probe).unwrap();
        let during = sys.volume(&mic.id).unwrap().level;
        sys.set_volume(&mic.id, before.level).unwrap();
        let after = sys.volume(&mic.id).unwrap();
        assert!((during - probe).abs() < 0.02, "{during} != {probe}");
        assert!((after.level - before.level).abs() < 0.02);
        assert_eq!(after.muted, before.muted);
    }

    /// Read-only: whether anything records from each microphone right now.
    /// `cargo test -p vidar-win -- --ignored --nocapture who_is_listening`
    #[test]
    #[ignore = "depends on what runs on the machine"]
    fn who_is_listening() {
        let sys = WinAudio::new().unwrap();
        for e in sys.endpoints(Flow::Capture) {
            println!("{:<60} in use: {:?}", e.name, sys.in_use(&e.id));
            assert!(sys.in_use(&e.id).is_some());
        }
    }

    /// Records from the virtual cable for a moment, as an app would, and checks that
    /// `in_use` sees it: `cargo test -p vidar-win -- --ignored recording_is_detected`.
    #[test]
    #[ignore = "needs the virtual cable"]
    fn recording_is_detected() {
        use windows::Win32::Media::Audio::{IAudioClient, AUDCLNT_SHAREMODE_SHARED};
        let sys = WinAudio::new().unwrap();
        let cable = sys
            .endpoints(Flow::Capture)
            .into_iter()
            .find(|e| e.is_vb_cable())
            .expect("Vidar AI Microphone installed");
        assert_eq!(
            sys.in_use(&cable.id),
            Some(false),
            "close apps recording from it"
        );
        let wid = wide(&cable.id);
        // SAFETY: a plain shared-mode capture stream, started and stopped in this scope;
        // the mix format is freed after use.
        unsafe {
            let dev = sys.enumerator.GetDevice(PCWSTR(wid.as_ptr())).unwrap();
            let client = dev.Activate::<IAudioClient>(CLSCTX_ALL, None).unwrap();
            let format = client.GetMixFormat().unwrap();
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None)
                .unwrap();
            CoTaskMemFree(Some(format as *const c_void));
            client.Start().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
            let during = sys.in_use(&cable.id);
            client.Stop().unwrap();
            drop(client);
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert_eq!(during, Some(true));
            assert_eq!(sys.in_use(&cable.id), Some(false));
        }
    }
}
