//! Engine parameters derived from the config.

use vidar_dsp::GateParams;

use crate::Config;

/// Everything the audio engine needs from the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineSettings {
    pub gate: GateParams,
    /// Gain applied after the model: output gain x mic gain (laptop base gain x boost).
    pub total_gain: f64,
    pub mic_boost_db: f64,
    /// Always on in the app; tests turn it off to run the chain without a model.
    pub denoise_enabled: bool,
    pub high_pass: bool,
    pub hpf_cutoff_hz: f64,
    pub auto_level: bool,
}

/// Close threshold that keeps the tuned 0.52/0.70 hysteresis ratio.
pub fn close_threshold_for(threshold: f64) -> f64 {
    let v = (threshold * (0.52 / 0.70)).max(0.20);
    (v * 100.0).round() / 100.0
}

impl EngineSettings {
    /// `is_laptop_mic` widens the gain for built-in laptop arrays (+1.6 dB).
    pub fn from_config(cfg: &Config, is_laptop_mic: bool) -> Self {
        let threshold = cfg.vad_threshold;

        let base_gain = if is_laptop_mic { 1.2 } else { 1.0 };
        let mic_gain = base_gain * 10f64.powf(cfg.mic_boost_db / 20.0);

        Self {
            gate: GateParams {
                threshold,
                close_threshold: cfg.vad_close_threshold,
                hangover_ms: cfg.vad_hangover_ms,
                decay_ms: cfg.vad_decay_ms,
                attack_ms: 28.0,
                frame_ms: 10.0,
                onset_threshold: cfg.vad_onset_threshold,
                onset_snr_db: cfg.vad_onset_snr_db,
                lookahead: true,
                floor_gain: cfg.vad_floor_gain,
                cooldown_ms: 150.0,
                ..GateParams::default()
            },
            total_gain: cfg.output_gain * mic_gain,
            mic_boost_db: cfg.mic_boost_db,
            denoise_enabled: true,
            high_pass: cfg.high_pass_filter,
            hpf_cutoff_hz: cfg.hpf_cutoff_hz,
            auto_level: cfg.auto_level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_give_the_tuned_engine_values() {
        let s = EngineSettings::from_config(&Config::default(), false);
        assert_eq!(s.gate.threshold, 0.70);
        assert_eq!(s.gate.close_threshold, 0.52);
        assert_eq!(s.gate.hangover_ms, 320.0);
        assert_eq!(s.gate.floor_gain, 0.06);
        assert!(s.gate.lookahead);
        assert!((s.total_gain - 1.08).abs() < 1e-12);
        assert!(s.denoise_enabled);
    }

    #[test]
    fn boost_and_laptop_gain_multiply() {
        let cfg = Config {
            mic_boost_db: 10.0,
            ..Config::default()
        };
        let s = EngineSettings::from_config(&cfg, true);
        assert!((s.total_gain - 1.08 * 1.2 * 10f64.powf(0.5)).abs() < 1e-12);
    }

    #[test]
    fn close_threshold_keeps_ratio() {
        assert_eq!(close_threshold_for(0.70), 0.52);
        assert_eq!(close_threshold_for(0.90), 0.67);
        assert_eq!(close_threshold_for(0.10), 0.20);
    }
}
