//! Plain-language explanations for start failures, from WASAPI error codes.

/// What went wrong, for the status line and a details popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained {
    pub kind: &'static str,
    pub title: &'static str,
    pub advice: &'static str,
    /// Raw error text for logs and the details view.
    pub technical: String,
}

const AUDCLNT_E_DEVICE_INVALIDATED: u32 = 0x8889_0004;
const AUDCLNT_E_UNSUPPORTED_FORMAT: u32 = 0x8889_0008;
const AUDCLNT_E_DEVICE_IN_USE: u32 = 0x8889_000A;
const AUDCLNT_E_ENDPOINT_CREATE_FAILED: u32 = 0x8889_000F;
const AUDCLNT_E_SERVICE_NOT_RUNNING: u32 = 0x8889_0010;
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: u32 = 0x8889_000E;
const E_ACCESSDENIED: u32 = 0x8007_0005;
const E_NOTFOUND: u32 = 0x8007_0490;
const ERROR_SET_NOT_FOUND: u32 = 0x8007_0492;

/// Explain a failure. `code` is the HRESULT when known.
pub fn explain(code: Option<u32>, technical: impl Into<String>) -> Explained {
    let technical = technical.into();
    let make = |kind, title, advice| Explained {
        kind,
        title,
        advice,
        technical: technical.clone(),
    };
    match code {
        Some(ERROR_SET_NOT_FOUND) => make(
            "jack_unplugged",
            "No active connection on this audio input",
            "Make sure the mic cable is firmly plugged in (a headset with one combo plug needs a 3.5 mm Y-splitter), or pick another microphone.",
        ),
        Some(E_ACCESSDENIED) => make(
            "privacy",
            "Windows is blocking microphone access",
            "Open Settings > Privacy & security > Microphone and turn on 'Let desktop apps access your microphone'.",
        ),
        Some(AUDCLNT_E_DEVICE_IN_USE) | Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => make(
            "busy",
            "The microphone is busy",
            "Another app has exclusive control of it. Close apps that may be using it, or in Windows Sound settings turn off 'Allow applications to take exclusive control' for the microphone and the virtual cable.",
        ),
        Some(AUDCLNT_E_DEVICE_INVALIDATED) | Some(E_NOTFOUND) => make(
            "gone",
            "The selected audio device is no longer available",
            "Check that the microphone is plugged in, then pick it again.",
        ),
        Some(AUDCLNT_E_UNSUPPORTED_FORMAT) => make(
            "format",
            "The device does not accept the audio format",
            "In Windows Sound settings > device Properties > Advanced, set the default format to 48000 Hz.",
        ),
        Some(AUDCLNT_E_SERVICE_NOT_RUNNING) => make(
            "service",
            "The Windows audio service is not running",
            "Restart the computer, or start the 'Windows Audio' service.",
        ),
        Some(AUDCLNT_E_ENDPOINT_CREATE_FAILED) => make(
            "driver",
            "The audio driver could not open the device",
            "Unplug and reconnect the device, or update its driver.",
        ),
        _ => make("unknown", "Could not start the microphone", "Pick your microphone again, or check Windows Sound settings."),
    }
}

/// The cable is not installed or disabled.
pub fn cable_missing() -> Explained {
    Explained {
        kind: "no_cable",
        title: "The virtual cable is not installed",
        advice: "Install the bundled VB-Audio Virtual Cable driver (Vidar can do this for you), then restart Vidar.",
        technical: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_have_specific_advice() {
        assert_eq!(explain(Some(0x8007_0492), "x").kind, "jack_unplugged");
        assert_eq!(explain(Some(0x8889_000A), "x").kind, "busy");
        assert_eq!(explain(Some(0x8007_0005), "x").kind, "privacy");
        assert_eq!(explain(Some(0x8889_0004), "x").kind, "gone");
        assert_eq!(explain(None, "x").kind, "unknown");
        assert_eq!(explain(Some(1), "detail").technical, "detail");
    }
}
