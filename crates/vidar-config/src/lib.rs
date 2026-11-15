//! Vidar settings: the config file, its defaults, and the engine parameters derived from it.
//!
//! Unknown keys are kept and written back untouched.

mod engine;
mod store;

pub use engine::{close_threshold_for, EngineSettings};
pub use store::{default_path, load, save, LoadOutcome};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONFIG_VERSION: &str = "2.0.0";

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
    /// Endpoint ID of the playback device audio is sent to (normally the cable's render side).
    pub output_endpoint_id: Option<String>,
    pub output_device_name: Option<String>,

    pub high_pass_filter: bool,
    pub hpf_cutoff_hz: f64,
    pub vad_threshold: f64,
    pub vad_close_threshold: f64,
    pub vad_hangover_ms: f64,
    pub vad_decay_ms: f64,
    pub vad_onset_threshold: f64,
    pub vad_onset_snr_db: f64,
    pub vad_floor_gain: f64,
    /// Set when the user moves the sensitivity slider; stops migrations touching VAD values.
    pub vad_customized: bool,
    pub output_gain: f64,
    pub mic_boost_db: f64,
    pub auto_level: bool,

    pub denoise_engine: String,
    pub dpdfnet_vad: String,
    pub dpdfnet_model: String,

    /// True while the Windows default mic is switched to the cable (crash recovery).
    pub is_swapped: bool,
    /// Endpoint ID of the user's own default mic before Vidar switched it.
    pub original_mic_id: Option<String>,
    pub auto_route: bool,

    /// Endpoint IDs of the virtual cable, remembered so it is found even after renaming.
    pub cable_capture_id: Option<String>,
    pub cable_render_id: Option<String>,

    /// Ask Windows for the mic signal without vendor "enhancements".
    pub raw_capture: bool,
    /// Use the smallest shared-mode period the device allows.
    pub low_latency: bool,
    pub start_with_windows: bool,
    pub start_minimized: bool,
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
            output_endpoint_id: None,
            output_device_name: None,
            high_pass_filter: true,
            hpf_cutoff_hz: 80.0,
            vad_threshold: 0.70,
            vad_close_threshold: 0.52,
            vad_hangover_ms: 320.0,
            vad_decay_ms: 80.0,
            vad_onset_threshold: 0.35,
            vad_onset_snr_db: 10.0,
            vad_floor_gain: 0.06,
            vad_customized: false,
            output_gain: 1.08,
            mic_boost_db: 0.0,
            auto_level: false,
            denoise_engine: "rnnoise".into(),
            dpdfnet_vad: "rnnoise".into(),
            dpdfnet_model: "dpdfnet2_48khz_hr".into(),
            is_swapped: false,
            original_mic_id: None,
            auto_route: true,
            cable_capture_id: None,
            cable_render_id: None,
            raw_capture: false,
            low_latency: true,
            start_with_windows: false,
            start_minimized: false,
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
        let vad = self.dpdfnet_vad.trim().to_ascii_lowercase();
        self.dpdfnet_vad = if vad == "spectral" {
            vad
        } else {
            "rnnoise".into()
        };
        if !matches!(self.ui_theme.as_str(), "auto" | "light" | "dark") {
            self.ui_theme = "auto".into();
        }
        if !matches!(
            self.dpdfnet_model.as_str(),
            "dpdfnet2_48khz_hr" | "dpdfnet8_48khz_hr"
        ) {
            self.dpdfnet_model = "dpdfnet2_48khz_hr".into();
        }
        self.vad_threshold = finite_or(self.vad_threshold, 0.70).clamp(0.05, 0.99);
        self.vad_close_threshold = finite_or(self.vad_close_threshold, 0.52).clamp(0.05, 0.99);
        self.vad_onset_threshold = finite_or(self.vad_onset_threshold, 0.35).clamp(0.0, 1.0);
        self.vad_floor_gain = finite_or(self.vad_floor_gain, 0.06).clamp(0.0, 1.0);
        self.vad_hangover_ms = finite_or(self.vad_hangover_ms, 320.0).clamp(10.0, 5000.0);
        self.vad_decay_ms = finite_or(self.vad_decay_ms, 80.0).clamp(1.0, 5000.0);
        self.hpf_cutoff_hz = finite_or(self.hpf_cutoff_hz, 80.0).clamp(20.0, 400.0);
        self.output_gain = finite_or(self.output_gain, 1.08).clamp(0.1, 8.0);
        self.mic_boost_db = finite_or(self.mic_boost_db, 0.0).clamp(0.0, 30.0);
        self.vad_onset_snr_db = finite_or(self.vad_onset_snr_db, 10.0);
        for id in [
            &mut self.input_endpoint_id,
            &mut self.output_endpoint_id,
            &mut self.original_mic_id,
        ] {
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
    fn defaults_are_the_tuned_values() {
        let c = Config::default();
        assert_eq!(
            (c.vad_threshold, c.vad_close_threshold, c.vad_hangover_ms),
            (0.70, 0.52, 320.0)
        );
        assert_eq!(
            (c.vad_onset_threshold, c.vad_onset_snr_db, c.vad_floor_gain),
            (0.35, 10.0, 0.06)
        );
        assert_eq!((c.output_gain, c.hpf_cutoff_hz), (1.08, 80.0));
        assert!(c.auto_route && c.high_pass_filter);
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
            dpdfnet_vad: "nope".into(),
            dpdfnet_model: "x".into(),
            vad_threshold: f64::NAN,
            mic_boost_db: -5.0,
            input_endpoint_id: Some("  ".into()),
            ..Config::default()
        };
        c.sanitize();
        assert_eq!(c.denoise_engine, "dpdfnet");
        assert_eq!(c.dpdfnet_vad, "rnnoise");
        assert_eq!(c.dpdfnet_model, "dpdfnet2_48khz_hr");
        assert_eq!(c.vad_threshold, 0.70);
        assert_eq!(c.mic_boost_db, 0.0);
        assert_eq!(c.input_endpoint_id, None);
    }
}
