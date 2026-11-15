/// In-speech de-clicker: dips sharp keyboard clicks and desk thumps by 6-8 dB
/// with a 1 ms Hann-shaped envelope, so the dip itself does not click.
///
/// The derivative and crest factor run in f64; the envelope runs in f32.
#[derive(Debug, Clone)]
pub struct TransientSuppressor {
    threshold_crest: f64,
    max_attenuation_db: f64,
    prev_sample: f32,
    diff: Vec<f64>,
    env: Vec<f32>,
}

const RADIUS: usize = 24; // 0.5 ms each side at 48 kHz

impl Default for TransientSuppressor {
    fn default() -> Self {
        Self::new(4.5, 8.0)
    }
}

impl TransientSuppressor {
    pub fn new(threshold_crest: f64, max_attenuation_db: f64) -> Self {
        Self {
            threshold_crest,
            max_attenuation_db,
            prev_sample: 0.0,
            diff: Vec::with_capacity(crate::FRAME_SIZE),
            env: Vec::with_capacity(crate::FRAME_SIZE),
        }
    }

    pub fn reset(&mut self) {
        self.prev_sample = 0.0;
    }

    pub fn process(&mut self, frame: &mut [f32], _speech_prob: f64) {
        let n = frame.len();
        if n == 0 {
            return;
        }
        // Buffers are sized once for the frame length; the steady state does not allocate.
        self.diff.resize(n, 0.0);
        let mut prev = f64::from(self.prev_sample);
        let mut sum_sq = 0.0;
        for (d, &x) in self.diff.iter_mut().zip(frame.iter()) {
            let x = f64::from(x);
            *d = x - prev;
            sum_sq += *d * *d;
            prev = x;
        }
        self.prev_sample = frame[n - 1];

        let rms_diff = (sum_sq / n as f64).sqrt();
        if rms_diff < 1e-4 {
            return;
        }
        let denom = rms_diff + 1e-6;
        if !self
            .diff
            .iter()
            .any(|d| d.abs() / denom >= self.threshold_crest)
        {
            return;
        }

        self.env.clear();
        self.env.resize(n, 1.0);
        for s_idx in 0..n {
            let crest = self.diff[s_idx].abs() / denom;
            if crest < self.threshold_crest {
                continue;
            }
            let att_db = self
                .max_attenuation_db
                .min(6.0 + (crest - self.threshold_crest) * 0.8);
            let spike_gain = 10f64.powf(-att_db / 20.0);
            let depth = (1.0 - spike_gain) as f32;

            let i_start = s_idx.saturating_sub(RADIUS);
            let i_end = (s_idx + RADIUS + 1).min(n);
            let win_len = i_end - i_start;
            // np.linspace(0, pi, win_len, dtype=float32), then sin^2 in f32.
            let step = if win_len > 1 {
                std::f64::consts::PI / (win_len - 1) as f64
            } else {
                0.0
            };
            for k in 0..win_len {
                let t = if k == win_len - 1 && win_len > 1 {
                    std::f64::consts::PI as f32
                } else {
                    (k as f64 * step) as f32
                };
                let s = t.sin();
                let local = 1.0 - depth * (s * s);
                let e = &mut self.env[i_start + k];
                if local < *e {
                    *e = local;
                }
            }
        }
        for (x, g) in frame.iter_mut().zip(self.env.iter()) {
            *x *= *g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_signal_passes_unchanged() {
        let mut ts = TransientSuppressor::default();
        let mut x: Vec<f32> = (0..480)
            .map(|i| (0.3 * (i as f64 * 0.05).sin()) as f32)
            .collect();
        let before = x.clone();
        ts.process(&mut x, 1.0);
        assert_eq!(x, before);
    }

    #[test]
    fn click_is_attenuated_locally() {
        let mut ts = TransientSuppressor::default();
        let mut x: Vec<f32> = (0..480)
            .map(|i| (0.05 * (i as f64 * 0.05).sin()) as f32)
            .collect();
        x[240] = 0.9;
        let before = x.clone();
        ts.process(&mut x, 1.0);
        assert!(x[240] < before[240] * 0.6, "click not reduced: {}", x[240]);
        assert_eq!(x[100], before[100], "far samples must be untouched");
    }
}
