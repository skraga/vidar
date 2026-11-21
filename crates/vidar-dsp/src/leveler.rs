/// Slow automatic gain for quiet microphones.
///
/// Tracks the level of voiced speech only (frozen during silence and noise), pulls it
/// toward `target_rms_db` with at most `max_boost_db` of gain, and subtracts the user's
/// manual mic boost from the target so the two do not stack. Gain drops within one frame
/// and rises at most 5 dB per second. Every change is ramped across the frame, so even a
/// fast drop never clicks.
///
/// Switched off ([`SpeechLeveler::set_enabled`]), it glides back to unity gain over one
/// frame instead of jumping there, after which callers may skip it
/// ([`SpeechLeveler::is_bypassed`]); switched back on, it rises again at the normal rate.
#[derive(Debug, Clone)]
pub struct SpeechLeveler {
    enabled: bool,
    /// Gain applied at the end of the previous frame (where the next ramp starts).
    applied: f64,
    target_rms_db: f64,
    max_boost_db: f64,
    voicing_threshold: f64,
    alpha_attack: f64,
    alpha_decay: f64,
    max_slew_up_db: f64,
    voiced_rms_db: f64,
    current_gain: f64,
}

impl Default for SpeechLeveler {
    fn default() -> Self {
        Self::new(-24.0, 9.0, 0.60, 10.0, 40.0, 500.0, 5.0)
    }
}

impl SpeechLeveler {
    pub fn new(
        target_rms_db: f64,
        max_boost_db: f64,
        voicing_threshold: f64,
        frame_ms: f64,
        attack_ms: f64,
        decay_ms: f64,
        max_slew_up_db_per_sec: f64,
    ) -> Self {
        let max_boost_mult = 10f64.powf(max_boost_db / 20.0);
        Self {
            enabled: true,
            applied: 1.0,
            target_rms_db,
            // dB -> linear -> dB, as in the reference vectors.
            max_boost_db: 20.0 * max_boost_mult.log10(),
            voicing_threshold,
            alpha_attack: (-frame_ms / attack_ms).exp(),
            alpha_decay: (-frame_ms / decay_ms).exp(),
            max_slew_up_db: max_slew_up_db_per_sec * (frame_ms / 1000.0),
            voiced_rms_db: target_rms_db,
            current_gain: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.voiced_rms_db = self.target_rms_db;
        self.current_gain = 1.0;
        self.applied = 1.0;
    }

    pub fn current_gain(&self) -> f64 {
        self.current_gain
    }

    /// Switch on or off. Off glides to unity gain over the next frame processed.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Off and back at unity gain: [`SpeechLeveler::process`] would leave the audio
    /// unchanged.
    pub fn is_bypassed(&self) -> bool {
        !self.enabled && self.applied == 1.0
    }

    /// Apply leveling to one frame in place. Returns the gain used.
    pub fn process(
        &mut self,
        frame: &mut [f32],
        speech_prob: f64,
        input_rms_db: f64,
        mic_boost_db: f64,
    ) -> f64 {
        if speech_prob >= self.voicing_threshold && input_rms_db > -60.0 {
            let alpha = if input_rms_db > self.voiced_rms_db {
                self.alpha_attack
            } else {
                self.alpha_decay
            };
            self.voiced_rms_db = alpha * self.voiced_rms_db + (1.0 - alpha) * input_rms_db;
        }

        let effective_target_db = self.target_rms_db - mic_boost_db;
        let error_db = effective_target_db - self.voiced_rms_db;
        let desired_gain_db = if self.enabled {
            0.0f64.max(self.max_boost_db.min(error_db))
        } else {
            0.0
        };
        let desired_gain = 10f64.powf(desired_gain_db / 20.0);

        if desired_gain < self.current_gain {
            self.current_gain = desired_gain;
        } else {
            let current_db = 20.0 * self.current_gain.max(1e-4).log10();
            let next_db = desired_gain_db.min(current_db + self.max_slew_up_db);
            self.current_gain = 10f64.powf(next_db / 20.0);
        }

        let from = self.applied;
        self.applied = self.current_gain;
        crate::levels::ramp_gain(frame, from as f32, self.current_gain as f32);
        self.current_gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boosts_quiet_speech_slowly_up_to_limit() {
        let mut l = SpeechLeveler::default();
        let mut f = [0.01f32; 480];
        let first = l.process(&mut f, 0.9, -40.0, 0.0);
        assert!(
            first > 1.0 && first < 1.01,
            "slew limit is 0.05 dB per frame: {first}"
        );
        let mut g = 0.0;
        for _ in 0..2000 {
            g = l.process(&mut [0.01f32; 480], 0.9, -40.0, 0.0);
        }
        assert!(
            (20.0 * g.log10() - 9.0).abs() < 1e-6,
            "capped at +9 dB, got {g}"
        );
    }

    /// Let the gain settle on speech at `level_db` (a constant 0.01 input).
    fn boosted(level_db: f64) -> SpeechLeveler {
        let mut l = SpeechLeveler::default();
        for _ in 0..2000 {
            l.process(&mut [0.01f32; 480], 0.9, level_db, 0.0);
        }
        l
    }

    /// Largest change in gain between two neighbouring samples of a constant input.
    fn worst_step(frame: &[f32], before: f32) -> f32 {
        let mut prev = before;
        let mut worst = 0.0f32;
        for &x in frame {
            worst = worst.max((x - prev).abs());
            prev = x;
        }
        worst
    }

    /// A loud word after quiet ones lowers the gain quickly, but across the frame: the
    /// several dB a frame used to drop between two samples (a click) are now spread out.
    #[test]
    fn a_fast_drop_is_ramped_not_stepped() {
        // Settled at +7 dB; one frame at -10 dB pulls the tracked level up by about 4.6 dB.
        let mut l = boosted(-31.0);
        let g0 = l.current_gain() as f32;
        let mut f = [0.01f32; 480];
        let g1 = l.process(&mut f, 0.9, -10.0, 0.0) as f32;
        assert!(g1 < g0 * 0.9, "the gain should drop: {g0} -> {g1}");
        assert!((f[479] - 0.01 * g1).abs() < 1e-7, "ends at the new gain");
        let step = worst_step(&f, 0.01 * g0);
        assert!(
            step < 0.01 * (g0 - g1) / 100.0,
            "jumped {step} in one sample"
        );
    }

    /// Switching off glides back to unity over one frame, then leaves audio untouched.
    #[test]
    fn switching_off_glides_to_unity() {
        let mut l = boosted(-40.0);
        let g0 = l.current_gain() as f32;
        l.set_enabled(false);
        assert!(!l.is_bypassed(), "still at the boosted gain");
        let mut f = [0.01f32; 480];
        assert_eq!(l.process(&mut f, 0.9, -40.0, 0.0), 1.0);
        assert!(worst_step(&f, 0.01 * g0) < 0.01 * (g0 - 1.0) / 100.0);
        assert!((f[479] - 0.01).abs() < 1e-9);
        assert!(l.is_bypassed());
        let mut f = [0.01f32; 480];
        l.process(&mut f, 0.9, -40.0, 0.0);
        assert!(f.iter().all(|&x| x == 0.01));
        // Back on: it rises again at the normal 0.05 dB per frame, not in one jump.
        l.set_enabled(true);
        let g = l.process(&mut [0.01f32; 480], 0.9, -40.0, 0.0);
        assert!(g > 1.0 && g < 1.01, "{g}");
    }

    #[test]
    fn ignores_noise_frames() {
        let mut l = SpeechLeveler::default();
        for _ in 0..500 {
            l.process(&mut [0.01f32; 480], 0.1, -40.0, 0.0);
        }
        assert_eq!(l.current_gain(), 1.0);
    }

    #[test]
    fn manual_boost_lowers_target() {
        let mut a = SpeechLeveler::default();
        let mut b = SpeechLeveler::default();
        for _ in 0..2000 {
            a.process(&mut [0.01f32; 480], 0.9, -30.0, 0.0);
            b.process(&mut [0.01f32; 480], 0.9, -30.0, 5.0);
        }
        assert!(b.current_gain() < a.current_gain());
    }
}
