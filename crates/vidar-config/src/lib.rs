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

    /// Voice tone: what corrects the microphone's colouring (the equalizer). `"auto"` (Auto
    /// tone learns it), `"off"`, or one of [`MIC_FIXES`].
    pub mic_fix: String,
    /// Fine-tune curve: band gains in dB (low to high) added on top of the equalizer.
    /// A list of another length (the five-band curve of earlier versions) reads as flat.
    #[serde(deserialize_with = "eq_gains_or_flat")]
    pub eq_custom_db: [f64; vidar_dsp::EQ_BANDS],
    /// The single voice tone of earlier versions (`"off"`, a preset or `"custom"`). Read only
    /// to move it into `mic_fix` / `eq_custom_db`; empty afterwards.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub eq_preset: String,

    /// `"dpdfnet"` (Voice HD, the default), `"dpdfnet8"` (Voice HD+) or `"rnnoise"` (Voice).
    /// Anything else reads as `"rnnoise"`, which is always available.
    pub denoise_engine: String,
    /// Podcast voice: the Broadcast curve plus a compressor, de-esser and saturation.
    pub studio_sound: bool,
    /// Auto tone (a learned correction towards a studio microphone). Follows
    /// `mic_fix == "auto"`.
    pub auto_tone: bool,

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

    /// Look for a newer release on GitHub about once a day. The only network access Vidar
    /// makes; "Check now" in Settings works either way.
    pub check_updates: bool,
    /// When the last automatic check ran (seconds since 1970), so restarts do not re-check.
    pub last_update_check: u64,

    /// Mode, strength, boost and auto level as last used with each microphone, by endpoint
    /// ID. The fields above hold the values of the microphone in use; switching microphones
    /// stores them here and loads the new microphone's.
    pub mic_profiles: std::collections::BTreeMap<String, MicProfile>,
    /// Skip the noise model (and send silence) while no app records from Vidar AI
    /// Microphone, to save processor time and battery.
    pub pause_when_unused: bool,
    /// How far Vidar may step down while the processor cannot keep up: `"off"` (never),
    /// `"voice"` (Voice HD to Voice, no further) or `"any"` (down to the filter and gate
    /// alone).
    pub cpu_fallback: String,

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
            mic_fix: "off".into(),
            eq_custom_db: [0.0; vidar_dsp::EQ_BANDS],
            eq_preset: String::new(),
            denoise_engine: "dpdfnet".into(),
            studio_sound: false,
            auto_tone: false,
            is_swapped: false,
            original_mic_id: None,
            cable_capture_id: None,
            cable_render_id: None,
            raw_capture: false,
            start_with_windows: false,
            ui_theme: "auto".into(),
            check_updates: true,
            last_update_check: 0,
            mic_profiles: std::collections::BTreeMap::new(),
            pause_when_unused: true,
            cpu_fallback: "any".into(),
            extra: Map::new(),
        }
    }
}

/// The settings remembered for one microphone (see [`Config::mic_profiles`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MicProfile {
    /// The microphone's name when last used, for people reading the file.
    pub name: String,
    pub denoise_engine: String,
    pub vad_threshold: f64,
    pub mic_boost_db: f64,
    pub auto_level: bool,
}

impl Default for MicProfile {
    fn default() -> Self {
        let c = Config::default();
        Self {
            name: String::new(),
            denoise_engine: c.denoise_engine,
            vad_threshold: c.vad_threshold,
            mic_boost_db: c.mic_boost_db,
            auto_level: c.auto_level,
        }
    }
}

impl Config {
    /// Store the current mode, strength, boost and auto level for microphone `id`.
    pub fn remember_mic(&mut self, id: &str, name: &str) {
        self.mic_profiles.insert(
            id.to_string(),
            MicProfile {
                name: name.to_string(),
                denoise_engine: self.denoise_engine.clone(),
                vad_threshold: self.vad_threshold,
                mic_boost_db: self.mic_boost_db,
                auto_level: self.auto_level,
            },
        );
    }

    /// Make microphone `id`'s remembered settings current. False (and nothing changed) for a
    /// microphone used for the first time: it starts from the current settings.
    pub fn recall_mic(&mut self, id: &str) -> bool {
        let Some(p) = self.mic_profiles.get(id).cloned() else {
            return false;
        };
        self.denoise_engine = p.denoise_engine;
        self.vad_threshold = p.vad_threshold;
        self.mic_boost_db = p.mic_boost_db;
        self.auto_level = p.auto_level;
        self.sanitize();
        true
    }
}

impl Config {
    /// Clamp and normalise values a hand-edited or old file may get wrong.
    pub fn sanitize(&mut self) {
        self.denoise_engine = engine_name(&self.denoise_engine);
        if !matches!(self.ui_theme.as_str(), "auto" | "light" | "dark") {
            self.ui_theme = "auto".into();
        }
        if !matches!(self.cpu_fallback.as_str(), "off" | "voice" | "any") {
            self.cpu_fallback = "any".into();
        }
        self.vad_threshold = finite_or(self.vad_threshold, 0.70).clamp(0.05, 0.99);
        self.mic_boost_db = finite_or(self.mic_boost_db, 0.0).clamp(0.0, 30.0);
        // Earlier versions had one voice tone: move it into the equalizer choice (Broadcast,
        // the curve Podcast voice uses, is kept only with Podcast voice).
        let legacy = self.eq_preset.trim().to_ascii_lowercase();
        if !legacy.is_empty() {
            if legacy != "custom" {
                self.eq_custom_db = [0.0; vidar_dsp::EQ_BANDS];
            }
            if MIC_FIXES.contains(&legacy.as_str()) {
                self.mic_fix = legacy;
            }
            if self.auto_tone {
                self.mic_fix = "auto".into();
            }
            self.eq_preset.clear();
        }
        let fix = self.mic_fix.trim().to_ascii_lowercase();
        self.mic_fix = if fix == "auto" || fix == "off" || MIC_FIXES.contains(&fix.as_str()) {
            fix
        } else {
            "off".into()
        };
        self.auto_tone = self.mic_fix == "auto";
        for g in &mut self.eq_custom_db {
            *g = finite_or(*g, 0.0).clamp(-vidar_dsp::EQ_MAX_DB, vidar_dsp::EQ_MAX_DB);
        }
        for key in RETIRED_KEYS {
            self.extra.remove(key);
        }
        for id in [&mut self.input_endpoint_id, &mut self.original_mic_id] {
            if id.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *id = None;
            }
        }
        self.mic_profiles.retain(|id, _| !id.trim().is_empty());
        for p in self.mic_profiles.values_mut() {
            p.denoise_engine = engine_name(&p.denoise_engine);
            p.vad_threshold = finite_or(p.vad_threshold, 0.70).clamp(0.05, 0.99);
            p.mic_boost_db = finite_or(p.mic_boost_db, 0.0).clamp(0.0, 30.0);
        }
    }
}

impl Config {
    /// The band gains the equaliser should use now: the chosen equalizer, the Broadcast
    /// curve when Podcast voice is on, and the fine-tune curve, added up (Auto tone adds its
    /// own correction in the engine).
    pub fn eq_gains_db(&self) -> [f64; vidar_dsp::EQ_BANDS] {
        let flat = [0.0; vidar_dsp::EQ_BANDS];
        let fix = vidar_dsp::eq_preset(&self.mic_fix).unwrap_or(flat);
        let podcast = if self.studio_sound {
            vidar_dsp::eq_preset("broadcast").unwrap_or(flat)
        } else {
            flat
        };
        std::array::from_fn(|i| {
            (fix[i] + podcast[i] + self.eq_custom_db[i])
                .clamp(-vidar_dsp::EQ_MAX_DB, vidar_dsp::EQ_MAX_DB)
        })
    }

    /// No voice tone at all: no correction, no Podcast voice, flat fine-tune.
    pub fn clear_tone(&mut self) {
        self.mic_fix = "off".into();
        self.eq_custom_db = [0.0; vidar_dsp::EQ_BANDS];
        self.auto_tone = false;
        self.studio_sound = false;
    }
}

/// Voice tone presets offered in the equalizer (keys of [`vidar_dsp::EQ_PRESETS`]).
pub const MIC_FIXES: [&str; 9] = [
    "laptop", "headset", "close", "bright", "soft", "warm", "clear", "nasal", "gaming",
];

impl Config {}

/// `eq_custom_db` with exactly [`vidar_dsp::EQ_BANDS`] numbers, or flat for anything else, so
/// an old or hand-edited curve resets only itself rather than failing the whole file.
fn eq_gains_or_flat<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<[f64; vidar_dsp::EQ_BANDS], D::Error> {
    let v = Value::deserialize(d)?;
    let gains: Option<Vec<f64>> = v
        .as_array()
        .and_then(|a| a.iter().map(Value::as_f64).collect());
    Ok(gains
        .and_then(|g| g.try_into().ok())
        .unwrap_or([0.0; vidar_dsp::EQ_BANDS]))
}

/// A `denoise_engine` value as stored: one of the known names, `"rnnoise"` for anything else.
fn engine_name(s: &str) -> String {
    let e = s.trim().to_ascii_lowercase();
    if matches!(e.as_str(), "dpdfnet" | "dpdfnet8") {
        e
    } else {
        "rnnoise".into()
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
        assert_eq!(c.mic_fix, "off");
        assert_eq!(c.eq_gains_db(), [0.0; vidar_dsp::EQ_BANDS]);
        assert!(!c.studio_sound);
        assert!(!c.auto_tone);
    }

    #[test]
    fn voice_hd_plus_is_kept_and_unknown_engines_read_as_voice() {
        let mut c = Config {
            denoise_engine: " DPDFNet8 ".into(),
            ..Config::default()
        };
        c.remember_mic("{usb}", "USB");
        c.mic_profiles.get_mut("{usb}").unwrap().denoise_engine = "dpdfnet16".into();
        c.sanitize();
        assert_eq!(c.denoise_engine, "dpdfnet8");
        assert_eq!(c.mic_profiles["{usb}"].denoise_engine, "rnnoise");
    }

    #[test]
    fn tone_settings_are_sanitized_and_added_up() {
        let mut c = Config {
            mic_fix: " Warm ".into(),
            studio_sound: true,
            eq_custom_db: [99.0, f64::NAN, -3.0, 0.0, 1.0, 0.0, -2.0],
            ..Config::default()
        };
        c.sanitize();
        assert_eq!(c.mic_fix, "warm");
        assert_eq!(c.eq_custom_db, [12.0, 0.0, -3.0, 0.0, 1.0, 0.0, -2.0]);
        let (w, b) = (
            vidar_dsp::eq_preset("warm").unwrap(),
            vidar_dsp::eq_preset("broadcast").unwrap(),
        );
        let sum: [f64; vidar_dsp::EQ_BANDS] = std::array::from_fn(|i| {
            (w[i] + b[i] + c.eq_custom_db[i]).clamp(-vidar_dsp::EQ_MAX_DB, vidar_dsp::EQ_MAX_DB)
        });
        assert_eq!(
            c.eq_gains_db(),
            sum,
            "correction + Podcast voice's curve + fine-tune"
        );
        c.mic_fix = "loudness-war".into();
        c.sanitize();
        assert_eq!(c.mic_fix, "off");
        c.mic_fix = "auto".into();
        c.sanitize();
        assert!(c.auto_tone);
    }

    /// The single voice tone of earlier versions moves into the equalizer and the
    /// fine-tune curve, once.
    #[test]
    fn an_old_voice_tone_moves_into_mic_correction() {
        let old = |json: &str| {
            let mut c: Config = serde_json::from_str(json).unwrap();
            c.sanitize();
            c
        };
        let c = old(r#"{"eq_preset":"laptop","eq_custom_db":[1,1,1,1,1,1,1]}"#);
        assert_eq!(c.mic_fix, "laptop");
        assert_eq!(
            c.eq_custom_db,
            [0.0; vidar_dsp::EQ_BANDS],
            "an unused curve is dropped"
        );
        assert!(c.eq_preset.is_empty());
        let c = old(r#"{"eq_preset":"gaming"}"#);
        assert_eq!(c.mic_fix, "gaming");
        let c = old(r#"{"eq_preset":"broadcast","studio_sound":true}"#);
        assert_eq!(c.mic_fix, "off");
        assert!(c.studio_sound, "Podcast voice keeps its Broadcast curve");
        let c = old(r#"{"eq_preset":"custom","eq_custom_db":[1,2,3,4,5,6,7],"auto_tone":true}"#);
        assert_eq!(c.mic_fix, "auto");
        assert_eq!(c.eq_custom_db, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        let c = old(r#"{"eq_preset":"off"}"#);
        assert_eq!(c.mic_fix, "off");
        // Written back, the old key is gone.
        assert!(!serde_json::to_string(&c).unwrap().contains("eq_preset"));
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
    fn a_curve_of_the_wrong_length_reads_as_flat_and_keeps_the_rest() {
        let json = r#"{"version":"2.1.0","vad_threshold":0.82,"eq_preset":"custom",
            "eq_custom_db":[3.0,1.0,0.0,-1.5,-3.0]}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.eq_custom_db, [0.0; vidar_dsp::EQ_BANDS]);
        assert_eq!(c.vad_threshold, 0.82);

        let ok = r#"{"eq_custom_db":[1.0,2.0,3.0,4.0,5.0,6.0,7.0]}"#;
        let c: Config = serde_json::from_str(ok).unwrap();
        assert_eq!(c.eq_custom_db, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn each_microphone_keeps_its_own_settings() {
        // Headset: Voice, Strong.
        let mut c = Config {
            denoise_engine: "rnnoise".into(),
            vad_threshold: 0.82,
            ..Config::default()
        };
        c.remember_mic("{headset}", "Headset");
        // Laptop mic, first time: starts from the current values, then gets its own.
        assert!(!c.recall_mic("{laptop}"));
        assert_eq!(c.vad_threshold, 0.82);
        c.denoise_engine = "dpdfnet".into();
        c.mic_boost_db = 10.0;
        c.auto_level = true;
        c.remember_mic("{laptop}", "Laptop");

        assert!(c.recall_mic("{headset}"));
        assert_eq!(
            (
                c.denoise_engine.as_str(),
                c.vad_threshold,
                c.mic_boost_db,
                c.auto_level
            ),
            ("rnnoise", 0.82, 0.0, false)
        );
        assert!(c.recall_mic("{laptop}"));
        assert_eq!(
            (c.denoise_engine.as_str(), c.mic_boost_db, c.auto_level),
            ("dpdfnet", 10.0, true)
        );

        // Survives the file, and a hand-edited profile is sanitised like the rest.
        let json = serde_json::to_string(&c).unwrap();
        let mut back: Config = serde_json::from_str(&json).unwrap();
        back.sanitize();
        assert_eq!(back.mic_profiles, c.mic_profiles);
        back.mic_profiles.get_mut("{laptop}").unwrap().mic_boost_db = f64::NAN;
        back.sanitize();
        assert_eq!(back.mic_profiles["{laptop}"].mic_boost_db, 0.0);
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
