//! The end of the chain: the output gain, and the limiter that keeps Vidar AI Microphone
//! from clipping.

use crate::levels::{ramp_gain, soft_limit};
use crate::SAMPLE_RATE;

/// Level the limiter holds peaks to: 85% of full scale (about -1.4 dBFS).
pub const LIMIT: f32 = 0.85;
/// How fast the limiter turns down a peak above [`LIMIT`], and lets go after it.
const LIMIT_ATTACK_MS: f64 = 1.0;
const LIMIT_RELEASE_MS: f64 = 80.0;

fn coeff(ms: f64) -> f64 {
    1.0 - (-1.0 / (ms * 1e-3 * f64::from(SAMPLE_RATE))).exp()
}

/// Peak limiter with no delay.
///
/// A gain that drops within about 1 ms when a peak goes over [`LIMIT`] and recovers over
/// about 80 ms, followed by the soft clipper ([`soft_limit`]) for the first moments of a
/// peak, before the gain has come down. Loud passages are turned down rather than
/// clipped into distortion, and nothing below [`LIMIT`] is touched: audio that never gets
/// there passes bit for bit. (A look-ahead limiter would catch even the first moments of a
/// peak, but would add its look-ahead to the microphone's delay.)
#[derive(Debug, Clone)]
pub struct Limiter {
    /// Gain reduction in dB (0 or negative).
    gr_db: f64,
    attack: f64,
    release: f64,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        Self {
            gr_db: 0.0,
            attack: coeff(LIMIT_ATTACK_MS),
            release: coeff(LIMIT_RELEASE_MS),
        }
    }

    pub fn reset(&mut self) {
        self.gr_db = 0.0;
    }

    /// Gain reduction in use, in dB (0 or negative).
    pub fn reduction_db(&self) -> f64 {
        self.gr_db
    }

    pub fn process(&mut self, frame: &mut [f32]) {
        for x in frame.iter_mut() {
            let a = x.abs();
            if a <= LIMIT && self.gr_db == 0.0 {
                continue;
            }
            let want = if a > LIMIT {
                20.0 * (f64::from(LIMIT) / f64::from(a)).log10()
            } else {
                0.0
            };
            let k = if want < self.gr_db {
                self.attack
            } else {
                self.release
            };
            self.gr_db += (want - self.gr_db) * k;
            if self.gr_db > -1e-3 {
                self.gr_db = 0.0;
            } else {
                *x *= 10f64.powf(self.gr_db / 20.0) as f32;
            }
        }
        soft_limit(frame, LIMIT);
    }
}

/// The chain's output gain (glides from one frame's value to the next, so moving a gain
/// slider does not crackle) and its [`Limiter`].
#[derive(Debug, Clone, Default)]
pub struct OutputStage {
    /// Gain at the end of the previous frame; `None` before the first.
    gain: Option<f32>,
    limiter: Limiter,
}

impl OutputStage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the limiter. The gain keeps its value, so it does not glide in from unity.
    pub fn reset(&mut self) {
        self.limiter.reset();
    }

    /// Apply `gain`, ramping from the previous frame's gain if it changed.
    pub fn apply_gain(&mut self, frame: &mut [f32], gain: f64) {
        let to = gain as f32;
        let from = self.gain.unwrap_or(to);
        self.gain = Some(to);
        ramp_gain(frame, from, to);
    }

    pub fn limit(&mut self, frame: &mut [f32]) {
        self.limiter.process(frame);
    }

    pub fn limiter(&self) -> &Limiter {
        &self.limiter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * 200.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    #[test]
    fn below_the_limit_is_bit_exact() {
        let x = sine(0.8, 4800);
        let mut y = x.clone();
        Limiter::new().process(&mut y);
        assert_eq!(x, y);
    }

    /// A sustained loud passage is turned down, not clipped: after the first peak the
    /// output is a clean, scaled sine, with little left for the soft clipper to do.
    #[test]
    fn sustained_overs_are_turned_down_not_clipped() {
        let n = 48_000;
        let x = sine(1.6, n);
        let mut lim = x.clone();
        Limiter::new().process(&mut lim);
        let mut clip = x.clone();
        soft_limit(&mut clip, LIMIT);
        // Distortion: how far each output is from the best-fitting scaled input, over the
        // second half.
        let residual = |y: &[f32]| {
            let (xs, ys) = (&x[n / 2..], &y[n / 2..]);
            let dot: f64 = xs.iter().zip(ys).map(|(&a, &b)| f64::from(a * b)).sum();
            let xx: f64 = xs.iter().map(|&a| f64::from(a * a)).sum();
            let g = dot / xx;
            let err: f64 = xs
                .iter()
                .zip(ys)
                .map(|(&a, &b)| (f64::from(b) - g * f64::from(a)).powi(2))
                .sum();
            10.0 * (err / ys.iter().map(|&b| f64::from(b * b)).sum::<f64>()).log10()
        };
        let (r_lim, r_clip) = (residual(&lim), residual(&clip));
        assert!(
            r_lim < r_clip - 15.0,
            "limiter {r_lim:.1} dB, clipper {r_clip:.1} dB"
        );
        assert!(lim.iter().all(|v| v.abs() < 1.0));
    }

    #[test]
    fn recovers_after_a_peak() {
        let mut l = Limiter::new();
        let mut loud = sine(1.6, 4800);
        l.process(&mut loud);
        assert!(l.reduction_db() < -3.0);
        let mut quiet = sine(0.1, 48_000);
        l.process(&mut quiet);
        assert_eq!(l.reduction_db(), 0.0);
    }

    #[test]
    fn a_gain_change_glides_across_the_frame() {
        let mut o = OutputStage::new();
        let mut f = [0.5f32; 480];
        o.apply_gain(&mut f, 1.0);
        assert!(
            f.iter().all(|&v| v == 0.5),
            "first frame: no ramp from nothing"
        );
        let mut f = [0.5f32; 480];
        o.apply_gain(&mut f, 2.0);
        assert!((f[0] - 0.5 * (1.0 + 1.0 / 480.0)).abs() < 1e-6);
        assert_eq!(f[479], 1.0);
        let mut f = [0.5f32; 480];
        o.apply_gain(&mut f, 2.0);
        assert!(f.iter().all(|&v| v == 1.0));
    }
}
