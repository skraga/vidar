//! Audio endpoint description and classification (no OS calls).

/// Recording or playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flow {
    Capture,
    Render,
}

/// Hardware bus, from the endpoint's enumerator name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bus {
    /// Bluetooth hands-free profile (16 kHz mono, needs a bigger buffer).
    BluetoothHfp,
    BluetoothA2dp,
    BluetoothLe,
    Usb,
    /// Onboard HD Audio (Realtek and friends).
    HdAudio,
    IntelAudio,
    /// Software device (virtual drivers).
    Software,
    /// Root-enumerated (VB-Cable and other virtual drivers).
    Root,
    Other(String),
    Unknown,
}

impl Bus {
    pub fn from_enumerator(name: &str) -> Bus {
        let n = name.trim().to_ascii_uppercase();
        if n.is_empty() {
            Bus::Unknown
        } else if n.contains("BTHHFENUM") {
            Bus::BluetoothHfp
        } else if n.contains("BTHLEENUM") {
            Bus::BluetoothLe
        } else if n.contains("BTHENUM") {
            Bus::BluetoothA2dp
        } else if n.contains("USB") {
            Bus::Usb
        } else if n.contains("HDAUDIO") {
            Bus::HdAudio
        } else if n.contains("INTELAUDIO") {
            Bus::IntelAudio
        } else if n.contains("SWD") {
            Bus::Software
        } else if n == "ROOT" {
            Bus::Root
        } else {
            Bus::Other(n)
        }
    }

    pub fn is_bluetooth(&self) -> bool {
        matches!(
            self,
            Bus::BluetoothHfp | Bus::BluetoothA2dp | Bus::BluetoothLe
        )
    }
}

/// One Windows audio endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    /// Endpoint ID, e.g. `{0.0.1.00000000}.{guid}`.
    pub id: String,
    pub flow: Flow,
    /// Full name as apps show it: "Microphone (USB Audio)".
    pub name: String,
    /// The renamable part: "Microphone".
    pub description: String,
    /// The driver/interface part: "USB Audio". Not affected by renaming.
    pub interface_name: String,
    pub bus: Bus,
    /// `DEVICE_STATE_*`: 1 active, 2 disabled, 4 not present, 8 unplugged.
    pub state: u32,
}

/// Interface name of VB-Audio's main virtual cable. Renaming an endpoint changes only its
/// description, so this stays stable after Vidar brands the cable.
pub const VB_CABLE_INTERFACE: &str = "VB-Audio Virtual Cable";

impl Endpoint {
    pub fn is_active(&self) -> bool {
        self.state == 1
    }

    /// The VB-Audio Virtual Cable endpoint (either side), even after renaming.
    pub fn is_vb_cable(&self) -> bool {
        if self.interface_name.eq_ignore_ascii_case(VB_CABLE_INTERFACE) {
            return true;
        }
        // Fallback for systems where the interface name is not reported.
        let n = self.name.to_ascii_lowercase();
        n.contains("cable output")
            || n.contains("cable input")
            || n.contains("vb-audio virtual cable")
    }

    /// Any virtual audio bridge (VB-Cable, VB-Audio Cable A/B, Voicemeeter, other "virtual" drivers).
    pub fn is_virtual(&self) -> bool {
        // Real hardware is never root-enumerated; software audio devices are
        // (VB-Cable, Voicemeeter, NVIDIA Broadcast, Steam streaming, ...).
        if self.is_vb_cable() || self.bus == Bus::Root {
            return true;
        }
        let n = format!("{} {}", self.name, self.interface_name).to_ascii_lowercase();
        ["cable", "vb-audio", "voicemeeter", "virtual"]
            .iter()
            .any(|k| n.contains(k))
    }

    /// Built-in laptop microphone array (gets a small gain lift in the engine).
    pub fn is_laptop_mic(&self) -> bool {
        if self.is_virtual() || self.bus.is_bluetooth() || self.bus == Bus::Usb {
            return false;
        }
        let n = self.name.to_ascii_lowercase();
        [
            "microphone array",
            "mic array",
            "internal mic",
            "built-in",
            "integrated",
            "digital microphone",
            "intel® smart sound",
            "intel smart sound",
        ]
        .iter()
        .any(|k| n.contains(k))
    }

    /// External headsets and microphones, preferred when restoring a physical default.
    pub fn looks_external(&self) -> bool {
        if self.bus.is_bluetooth() || self.bus == Bus::Usb {
            return true;
        }
        let n = self.name.to_ascii_lowercase();
        [
            "headset",
            "usb",
            "wireless",
            "mpow",
            "hyperx",
            "razer",
            "logitech",
            "corsair",
            "steelseries",
            "yeti",
            "fifine",
            "elgato",
            "rode",
            "sennheiser",
            "audio-technica",
        ]
        .iter()
        .any(|k| n.contains(k))
    }
}

/// Endpoint IDs look like `{0.0.1.00000000}.{xxxxxxxx-xxxx-...}`; reject anything else before
/// handing it to Windows (it came from a config file).
pub fn is_valid_endpoint_id(id: &str) -> bool {
    let id = id.trim();
    let braced = |s: &str, ok: fn(char) -> bool| {
        s.len() > 2 && s.starts_with('{') && s.ends_with('}') && s[1..s.len() - 1].chars().all(ok)
    };
    let guid_char = |c: char| c.is_ascii_alphanumeric() || c == '-';
    let prefix_char = |c: char| c.is_ascii_hexdigit() || c == '.' || c == '-';
    match id.find("}.{") {
        Some(i) => braced(&id[..=i], prefix_char) && braced(&id[i + 2..], guid_char),
        None => braced(id, guid_char),
    }
}

#[cfg(test)]
pub(crate) fn ep(id: &str, flow: Flow, desc: &str, iface: &str, bus: Bus) -> Endpoint {
    Endpoint {
        id: id.into(),
        flow,
        name: format!("{desc} ({iface})"),
        description: desc.into(),
        interface_name: iface.into(),
        bus,
        state: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vb_cable_is_recognised_before_and_after_branding() {
        let stock = ep(
            "{0.0.1.00000000}.{a}",
            Flow::Capture,
            "CABLE Output",
            VB_CABLE_INTERFACE,
            Bus::Root,
        );
        let branded = ep(
            "{0.0.1.00000000}.{a}",
            Flow::Capture,
            "Vidar AI Microphone",
            VB_CABLE_INTERFACE,
            Bus::Root,
        );
        assert!(stock.is_vb_cable() && branded.is_vb_cable());
        assert!(branded.is_virtual());
    }

    #[test]
    fn other_virtual_devices_are_virtual_but_not_the_cable() {
        let vm = ep(
            "{x}",
            Flow::Capture,
            "VoiceMeeter Output",
            "VB-Audio VoiceMeeter VAIO",
            Bus::Root,
        );
        assert!(vm.is_virtual());
        assert!(!vm.is_vb_cable());
        let nv = ep(
            "{n}",
            Flow::Capture,
            "Microphone",
            "NVIDIA Broadcast",
            Bus::Root,
        );
        assert!(
            nv.is_virtual(),
            "root-enumerated software devices are virtual"
        );
    }

    #[test]
    fn physical_mics_are_classified() {
        let realtek = ep(
            "{r}",
            Flow::Capture,
            "Microphone Array",
            "Realtek(R) Audio",
            Bus::HdAudio,
        );
        assert!(!realtek.is_virtual());
        assert!(realtek.is_laptop_mic());
        assert!(!realtek.looks_external());
        let bt = ep(
            "{b}",
            Flow::Capture,
            "Headset",
            "MPOW Wireless Gaming Headset",
            Bus::BluetoothHfp,
        );
        assert!(bt.looks_external());
        assert!(!bt.is_laptop_mic());
    }

    #[test]
    fn bus_from_enumerator() {
        assert_eq!(Bus::from_enumerator("BTHHFENUM"), Bus::BluetoothHfp);
        assert_eq!(Bus::from_enumerator("bthenum"), Bus::BluetoothA2dp);
        assert_eq!(Bus::from_enumerator("USB"), Bus::Usb);
        assert_eq!(Bus::from_enumerator("HDAUDIO"), Bus::HdAudio);
        assert_eq!(Bus::from_enumerator("ROOT"), Bus::Root);
        assert_eq!(Bus::from_enumerator(""), Bus::Unknown);
    }

    #[test]
    fn endpoint_id_validation() {
        assert!(is_valid_endpoint_id(
            "{0.0.1.00000000}.{4c5be058-7443-489e-bb45-46faebd06431}"
        ));
        assert!(is_valid_endpoint_id(
            "{4c5be058-7443-489e-bb45-46faebd06431}"
        ));
        assert!(!is_valid_endpoint_id("CABLE Output"));
        assert!(!is_valid_endpoint_id("{0.0.1}.{bad id}"));
        assert!(!is_valid_endpoint_id(""));
    }
}
