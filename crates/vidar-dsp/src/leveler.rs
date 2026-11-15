/// Slow automatic gain for quiet microphones.
///
/// Tracks the level of voiced speech only (frozen during silence and noise), pulls it
/// toward `target_rms_db` with at most `max_boost_db` of gain, and subtracts the user's
/// manual mic boost from the target so the two do not stack. Gain drops immediately
/// and rises at most 5 dB per second.
#[derive(Debug, Clone)]
pub struct SpeechLeveler {
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
    }

    pub fn current_gain(&self) -> f64 {
        self.current_gain
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
        let desired_gain_db = 0.0f64.max(self.max_boost_db.min(error_db));
        let desired_gain = 10f64.powf(desired_gain_db / 20.0);

        if desired_gain < self.current_gain {
            self.current_gain = desired_gain;
        } else {
            let current_db = 20.0 * self.current_gain.max(1e-4).log10();
            let next_db = desired_gain_db.min(current_db + self.max_slew_up_db);
            self.current_gain = 10f64.powf(next_db / 20.0);
        }

        if self.current_gain != 1.0 {
            let g = self.current_gain as f32;
            frame.iter_mut().for_each(|x| *x *= g);
            self.current_gain
        } else {
            1.0
        }
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
