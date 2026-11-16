//! Vidar settings: the config file, its defaults, and the engine parameters derived from it.
//!
//! Unknown keys are kept and written back untouched.

mod engine;
mod store;

pub use engine::{close_threshold_for, EngineSettings};
pub use store::{default_path, load, save, LoadOutcome};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONFIG_VERSION: &str = "2.1.0";

/// Keys earlier versions wrote that are now fixed in code. Dropped on load, so a settings
/// file from an older version cannot pin old tuning values.
const RETIRED_KEYS: [&str; 17] = [
    "auto_route",
    "output_endpoint_id",
    "output_device_name",
    "start_minimized",
    "low_latency",
    "high_pass_filter",
    "hpf_cutoff_hz",
    "output_gain",
    "vad_close_threshold",
    "vad_hangover_ms",
    "vad_decay_ms",
    "vad_onset_threshold",
    "vad_onset_snr_db",
    "vad_floor_gain",
    "vad_customized",
    "dpdfnet_vad",
    "dpdfnet_model",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("config file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// User settings. Field names are the JSON keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: String,

    /// Windows endpoint ID of the chosen microphone (`None` = Windows default mic).
    pub input_endpoint_id: Option<String>,
    /// Friendly name of the chosen microphone, used to find it again if its ID changes.
    pub input_device_name: Option<String>,

    /// Strength: the gate's open threshold. The close threshold follows it
    /// ([`close_threshold_for`]); every other gate value is fixed in [`EngineSettings`].
    pub vad_threshold: f64,
    pub mic_boost_db: f64,
    pub auto_level: bool,

    /// `"dpdfnet"` (Voice HD, the default) or `"rnnoise"` (Voice). Anything else reads as
    /// `"rnnoise"`, which is always available.
    pub denoise_engine: String,

    /// True while the Windows default mic is switched to the cable (crash recovery).
    pub is_swapped: bool,
    /// Endpoint ID of the user's own default mic before Vidar switched it.
    pub original_mic_id: Option<String>,

    /// Endpoint IDs of the virtual cable, remembered so it is found even after renaming.
    pub cable_capture_id: Option<String>,
    pub cable_render_id: Option<String>,

    /// Ask Windows for the mic signal without vendor "enhancements".
    pub raw_capture: bool,
    pub start_with_windows: bool,
    /// Window theme: "auto" (follow Windows), "light" or "dark".
    pub ui_theme: String,

    /// Keys this version does not know about, kept as-is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION.into(),
            input_endpoint_id: None,
            input_device_name: None,
            vad_threshold: 0.70,
            mic_boost_db: 0.0,
            auto_level: false,
            denoise_engine: "dpdfnet".into(),
            is_swapped: false,
            original_mic_id: None,
            cable_capture_id: None,
            cable_render_id: None,
            raw_capture: false,
            start_with_windows: false,
            ui_theme: "auto".into(),
            extra: Map::new(),
        }
    }
}

impl Config {
    /// Clamp and normalise values a hand-edited or old file may get wrong.
    pub fn sanitize(&mut self) {
        let engine = self.denoise_engine.trim().to_ascii_lowercase();
        self.denoise_engine = if engine == "dpdfnet" {
            engine
        } else {
            "rnnoise".into()
        };
        if !matches!(self.ui_theme.as_str(), "auto" | "light" | "dark") {
            self.ui_theme = "auto".into();
        }
        self.vad_threshold = finite_or(self.vad_threshold, 0.70).clamp(0.05, 0.99);
        self.mic_boost_db = finite_or(self.mic_boost_db, 0.0).clamp(0.0, 30.0);
        for key in RETIRED_KEYS {
            self.extra.remove(key);
        }
        for id in [&mut self.input_endpoint_id, &mut self.original_mic_id] {
            if id.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *id = None;
            }
        }
    }
}

fn finite_or(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_recommended_settings() {
        let c = Config::default();
        assert_eq!(c.vad_threshold, 0.70);
        assert_eq!(c.mic_boost_db, 0.0);
        assert!(!c.auto_level && !c.raw_capture);
        assert_eq!(c.denoise_engine, "dpdfnet");
    }

    #[test]
    fn retired_keys_are_dropped_from_old_files() {
        let json = r#"{"version":"2.0.0","vad_threshold":0.82,"high_pass_filter":false,
            "auto_route":false,"low_latency":false,"vad_hangover_ms":900.0,
            "dpdfnet_vad":"spectral","something_new":1}"#;
        let mut c: Config = serde_json::from_str(json).unwrap();
        c.sanitize();
        for key in RETIRED_KEYS {
            assert!(!c.extra.contains_key(key), "{key} kept");
        }
        // Settings that still exist and unknown keys survive.
        assert_eq!(c.vad_threshold, 0.82);
        assert_eq!(c.extra["something_new"], 1);
    }

    #[test]
    fn unknown_keys_round_trip() {
        let json = r#"{"version":"2.0.0","something_new":{"a":1}}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.extra["something_new"]["a"], 1);
        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains("something_new"));
    }

    #[test]
    fn sanitize_fixes_bad_values() {
        let mut c = Config {
            denoise_engine: "DPDFNet".into(),
            vad_threshold: f64::NAN,
            mic_boost_db: -5.0,
            input_endpoint_id: Some("  ".into()),
            ..Config::default()
        };
        c.sanitize();
        assert_eq!(c.denoise_engine, "dpdfnet");
        assert_eq!(c.vad_threshold, 0.70);
        assert_eq!(c.mic_boost_db, 0.0);
        assert_eq!(c.input_endpoint_id, None);
    }
}
