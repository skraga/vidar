//! Pop filter: softens the low "thump" of P and B sounds blown into the microphone.
//!
//! A plosive pushes a burst of air at the capsule, which comes out as a few tens of
//! milliseconds of energy below about 150 Hz, far stronger than the voice above it. A voiced
//! vowel, even a deep one, keeps most of its energy above that. So while the band below the
//! split is more than [`RATIO_DB`] louder than the voice above it, a low shelf at the split
//! turns it down by up to [`MAX_CUT_DB`]. With no pop the output is the input, bit for bit
//! (the shelf's memory is kept current, so it starts cleanly).

use std::f64::consts::PI;

use crate::SAMPLE_RATE;

const SPLIT_HZ: f64 = 150.0;
/// The voice the low band is compared with is heard through a 2nd-order high-pass here.
/// (Not the input minus the low band: that difference falls only 6 dB per octave below the
/// split and would carry much of the pop itself.)
const SENSE_HZ: f64 = 300.0;
/// The band below the split must be this much louder than the rest to count as a pop.
const RATIO_DB: f64 = 10.0;
/// ...and this much above its own recent average: a pop is a sudden burst, while a fan or
/// traffic rumble is steady and is left to the noise model.
const ONSET_DB: f64 = 8.0;
const SLOW_MS: f64 = 300.0;
/// Each dB beyond that cuts this many dB, up to the maximum.
const SLOPE: f64 = 1.5;
const MAX_CUT_DB: f64 = 15.0;
/// Pops below this level are left alone (nothing to hear).
const FLOOR_DB: f64 = -50.0;
const ATTACK_MS: f64 = 1.0;
const RELEASE_MS: f64 = 60.0;
/// Detector envelopes (peak-ish, so the low band's slow cycles are followed).
const ENV_ATTACK_MS: f64 = 2.0;
const ENV_RELEASE_MS: f64 = 30.0;

fn coeff(ms: f64) -> f64 {
    1.0 - (-1.0 / (ms * 1e-3 * f64::from(SAMPLE_RATE))).exp()
}

#[derive(Debug, Clone)]
pub struct PopFilter {
    // 2nd-order Butterworth low-pass (RBJ), direct form I.
    b0: f64,
    b1: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    /// High-pass that hears the voice above the pop: coefficients and state.
    hb: [f64; 3],
    ha: [f64; 2],
    hs: [f64; 4],
    /// The low shelf that does the cutting: its cos and sin at the split, coefficients
    /// (b0, b1, b2, a1, a2), the cut they were designed for, and its memory.
    shelf_cs: (f64, f64),
    shelf: [f64; 5],
    shelf_db: f64,
    ss: [f64; 4],
    env_low: f64,
    env_low_slow: f64,
    env_high: f64,
    cut_db: f64,
    slow: f64,
    env_attack: f64,
    env_release: f64,
    attack: f64,
    release: f64,
    /// Samples since a reset; during the first [`SLOW_MS`] the slow average just follows,
    /// so a noise that is already there when the stream starts is not taken for a pop.
    warm: u32,
}

impl Default for PopFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl PopFilter {
    pub fn new() -> Self {
        let w0 = 2.0 * PI * SPLIT_HZ / f64::from(SAMPLE_RATE);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let w1 = 2.0 * PI * SENSE_HZ / f64::from(SAMPLE_RATE);
        let (sin1, cos1) = w1.sin_cos();
        let alpha1 = sin1 / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let h0 = 1.0 + alpha1;
        Self {
            hb: [
                (1.0 + cos1) / 2.0 / h0,
                -(1.0 + cos1) / h0,
                (1.0 + cos1) / 2.0 / h0,
            ],
            ha: [-2.0 * cos1 / h0, (1.0 - alpha1) / h0],
            hs: [0.0; 4],
            shelf_cs: (cos, sin),
            shelf: [1.0, 0.0, 0.0, 0.0, 0.0],
            shelf_db: 0.0,
            ss: [0.0; 4],
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            env_low: 0.0,
            env_low_slow: 0.0,
            env_high: 0.0,
            cut_db: 0.0,
            slow: coeff(SLOW_MS),
            warm: 0,
            env_attack: coeff(ENV_ATTACK_MS),
            env_release: coeff(ENV_RELEASE_MS),
            attack: coeff(ATTACK_MS),
            release: coeff(RELEASE_MS),
        }
    }

    pub fn reset(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
        self.hs = [0.0; 4];
        self.ss = [0.0; 4];
        self.env_low = 0.0;
        self.env_low_slow = 0.0;
        self.warm = 0;
        self.env_high = 0.0;
        self.cut_db = 0.0;
    }

    /// Cut in use, in dB (0 or positive).
    pub fn cut_db(&self) -> f64 {
        self.cut_db
    }

    pub fn process(&mut self, frame: &mut [f32]) {
        for s in frame.iter_mut() {
            let x = f64::from(*s);
            let mut low =
                self.b0 * (x + self.x2) + self.b1 * self.x1 - self.a1 * self.y1 - self.a2 * self.y2;
            if low.abs() < 1e-15 {
                low = 0.0;
            }
            (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, low);
            let [hx1, hx2, hy1, hy2] = self.hs;
            let mut high = self.hb[0] * x + self.hb[1] * hx1 + self.hb[2] * hx2
                - self.ha[0] * hy1
                - self.ha[1] * hy2;
            if high.abs() < 1e-15 {
                high = 0.0;
            }
            self.hs = [x, hx1, high, hy1];
            let follow = |env: &mut f64, v: f64, a: f64, r: f64| {
                *env += (v - *env) * if v > *env { a } else { r };
            };
            follow(
                &mut self.env_low,
                low.abs(),
                self.env_attack,
                self.env_release,
            );
            follow(
                &mut self.env_high,
                high.abs(),
                self.env_attack,
                self.env_release,
            );
            let warming = f64::from(self.warm) < SLOW_MS * 1e-3 * f64::from(SAMPLE_RATE);
            if warming {
                self.warm += 1;
                self.env_low_slow = self.env_low_slow.max(self.env_low);
            } else {
                self.env_low_slow += (low.abs() - self.env_low_slow) * self.slow;
            }
            let low_db = 20.0 * self.env_low.max(1e-9).log10();
            let want = if warming || low_db < FLOOR_DB {
                0.0
            } else {
                let over_voice = low_db - 20.0 * self.env_high.max(1e-9).log10() - RATIO_DB;
                let over_recent = low_db - 20.0 * self.env_low_slow.max(1e-9).log10() - ONSET_DB;
                (over_voice.min(over_recent) * SLOPE).clamp(0.0, MAX_CUT_DB)
            };
            let k = if want > self.cut_db {
                self.attack
            } else {
                self.release
            };
            self.cut_db += (want - self.cut_db) * k;
            if self.cut_db < 1e-3 {
                self.cut_db = 0.0;
                // The shelf at 0 dB passes x unchanged: keep its memory as if it ran.
                self.ss = [x, self.ss[0], x, self.ss[2]];
                continue;
            }
            if (self.cut_db - self.shelf_db).abs() > 0.1 {
                self.design_shelf(self.cut_db);
            }
            let [b0, b1, b2, a1, a2] = self.shelf;
            let [x1, x2, y1, y2] = self.ss;
            let mut y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            if y.abs() < 1e-15 {
                y = 0.0;
            }
            self.ss = [x, x1, y, y1];
            *s = y as f32;
        }
    }

    /// RBJ low shelf at the split, slope 1, cutting `cut_db`.
    fn design_shelf(&mut self, cut_db: f64) {
        let (cos, sin) = self.shelf_cs;
        let a = 10f64.powf(-cut_db / 40.0);
        let k = 2.0 * a.sqrt() * (sin / 2.0 * std::f64::consts::SQRT_2);
        let (ap, am) = (a + 1.0, a - 1.0);
        let a0 = ap + am * cos + k;
        self.shelf = [
            a * (ap - am * cos + k) / a0,
            2.0 * a * (am - ap * cos) / a0,
            a * (ap - am * cos - k) / a0,
            -2.0 * (am + ap * cos) / a0,
            (ap + am * cos - k) / a0,
        ];
        self.shelf_db = cut_db;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * f * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    /// A filter that has heard half a second of quiet talking.
    fn warmed() -> PopFilter {
        let mut p = PopFilter::new();
        p.process(&mut tone(500.0, 0.05, 24_000));
        p
    }

    fn rms_db(x: &[f32]) -> f64 {
        10.0 * (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).log10()
    }

    /// A deep voiced vowel (120 Hz fundamental with stronger formant harmonics) is left
    /// bit for bit.
    #[test]
    fn a_deep_vowel_passes_untouched() {
        let n = 48_000;
        let x: Vec<f32> = tone(120.0, 0.1, n)
            .iter()
            .zip(tone(480.0, 0.2, n))
            .zip(tone(1_200.0, 0.1, n))
            .map(|((a, b), c)| a + b + c)
            .collect();
        let mut y = x.clone();
        PopFilter::new().process(&mut y);
        assert_eq!(x, y);
    }

    /// A pop: a strong burst of low frequencies over a quieter voice is turned down.
    #[test]
    fn a_plosive_burst_is_turned_down() {
        let n = 4_800; // 100 ms
        let pop: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                let env = (-(t / 0.03)).exp();
                (0.8 * env * (2.0 * PI * 60.0 * t).sin() + 0.05 * (2.0 * PI * 1_000.0 * t).sin())
                    as f32
            })
            .collect();
        let mut y = pop.clone();
        let mut p = warmed();
        p.process(&mut y);
        // With no look-ahead the first few milliseconds pass while the detector reacts;
        // after that the thump is cut deeply.
        let first = rms_db(&pop[..480]) - rms_db(&y[..480]);
        assert!(
            first > 0.5,
            "some cut already in the first 10 ms: {first:.1} dB"
        );
        let rest = rms_db(&pop[480..2400]) - rms_db(&y[480..2400]);
        assert!(rest > 10.0, "pop turned down by {rest:.1} dB after 10 ms");
        assert!(y.iter().all(|v| v.is_finite()));
    }

    /// After the pop it lets go, and the voice comes back untouched.
    #[test]
    fn releases_after_the_pop() {
        let mut p = warmed();
        let mut pop = tone(60.0, 0.8, 2400);
        p.process(&mut pop);
        assert!(p.cut_db() > 3.0);
        let mut voice = tone(500.0, 0.2, 48_000);
        p.process(&mut voice);
        assert_eq!(p.cut_db(), 0.0);
        let mut more = tone(500.0, 0.2, 4800);
        let before = more.clone();
        p.process(&mut more);
        assert_eq!(more, before);
    }
}
