//! "Studio sound": the polish a broadcast voice chain adds after the noise is gone.
//!
//! Three stages, in the order a studio would use them:
//!
//! 1. **Compressor**: evens out loud and soft syllables (4:1, soft knee, about 7 to 12 dB on
//!    words), so every word sits at the same close distance. Its threshold follows your speaking level, so it works the
//!    same on a quiet laptop mic and a hot USB mic, and a slow make-up gain brings speech to a
//!    steady broadcast level. Pauses are turned down by as much as speech is compressed, so
//!    the leftover noise between words does not come up with the make-up gain.
//! 2. **De-esser**: turns the band above about 5 kHz down only while S and T sounds poke out,
//!    which compression and a presence boost would otherwise make sharp.
//! 3. **Saturation**: a little asymmetric soft clipping mixed in, which adds the low, even
//!    harmonics heard as warmth and density.
//!
//! Every setting is fixed: the point is one switch, not a mixing desk. Turning it on or off
//! crossfades over 10 ms, and nothing allocates after construction.

use std::f64::consts::PI;

use crate::{FRAME_SIZE, SAMPLE_RATE};

/// Compression ratio above the threshold.
const RATIO: f64 = 4.0;
/// Width of the soft knee in dB, centred on the threshold.
const KNEE_DB: f64 = 6.0;
/// The threshold sits this far below the tracked speech level, so nearly every word is
/// compressed (about 7 dB at your usual level, more on loud syllables) and only the soft
/// tails are not.
const THRESHOLD_BELOW_SPEECH_DB: f64 = 10.0;
/// Level detector: RMS over about 5 ms, short enough to follow syllables.
const DETECT_MS: f64 = 5.0;
/// Gain smoothing in dB: how fast compression starts, and how fast it lets go. Smoothed in
/// dB (not in power), so a soft syllable after a loud one recovers in about this time
/// whatever the level difference.
const ATTACK_MS: f64 = 5.0;
const RELEASE_MS: f64 = 70.0;
/// Pauses: below the speech level by more than the first value the gain is lowered too,
/// reaching at the second the average compression speech gets, so the make-up gain does not
/// raise the noise between words relative to the voice.
const PAUSE_START_BELOW_DB: f64 = 16.0;
const PAUSE_FULL_BELOW_DB: f64 = 26.0;

/// Speech level the make-up gain aims for (RMS of 10 ms frames while talking).
const TARGET_DB: f64 = -20.0;
/// Make-up gain range: never more than this boost, never more than this cut.
const MAKEUP_MAX_DB: f64 = 15.0;
const MAKEUP_MIN_DB: f64 = -6.0;
/// How fast the make-up gain may move, in dB per second (up, down).
const MAKEUP_UP_DB_S: f64 = 3.0;
const MAKEUP_DOWN_DB_S: f64 = 6.0;
/// Speech level tracker: time constant, and where it starts before it has heard you.
const LEVEL_TAU_MS: f64 = 1500.0;
const LEVEL_START_DB: f64 = -26.0;
/// A 10 ms frame counts as speech when it is within this many dB of the tracked level (and
/// above [`LEVEL_FLOOR_DB`]). After the gate, pauses are far below that.
const LEVEL_WINDOW_DB: f64 = 18.0;
const LEVEL_FLOOR_DB: f64 = -55.0;

/// De-esser: split frequency, detector time constants, when it starts acting (the share of
/// the level in the top band, in dB) and the most it takes away.
const DEESS_HZ: f64 = 5_500.0;
const DEESS_ATTACK_MS: f64 = 1.0;
const DEESS_RELEASE_MS: f64 = 40.0;
const DEESS_START_DB: f64 = -9.0;
const DEESS_MAX_DB: f64 = 8.0;
/// Below this level the top band is left alone (breaths and leftover hiss).
const DEESS_FLOOR_DB: f64 = -50.0;

/// Saturation: drive, asymmetry (even harmonics) and how much of it is mixed in.
const SAT_DRIVE: f32 = 2.5;
const SAT_BIAS: f32 = 0.2;
const SAT_MIX: f32 = 0.25;

/// Crossfade when switching on or off.
const FADE_SAMPLES: f32 = 480.0;

fn coeff(ms: f64) -> f64 {
    1.0 - (-1.0 / (ms * 1e-3 * f64::from(SAMPLE_RATE))).exp()
}

fn db(power_or_amp: f64, power: bool) -> f64 {
    let v = power_or_amp.max(1e-12);
    if power {
        10.0 * v.log10()
    } else {
        20.0 * v.log10()
    }
}

/// Gain change in dB the compressor applies at `over` dB above the threshold.
fn compression_db(over: f64) -> f64 {
    let slope = 1.0 - 1.0 / RATIO;
    if over <= -KNEE_DB / 2.0 {
        0.0
    } else if over >= KNEE_DB / 2.0 {
        -slope * over
    } else {
        let x = over + KNEE_DB / 2.0;
        -slope * x * x / (2.0 * KNEE_DB)
    }
}

/// Second-order Butterworth biquad (RBJ), low- or high-pass.
///
/// The de-esser *cuts* the input minus the low-pass, so turning that band down and adding
/// the two back is exact: at full gain the sum is the input itself, with no phase smear
/// around the split. That difference band falls only 6 dB per octave below the split,
/// though (−6 dB at 2 kHz, −2 dB at 3 kHz), so it is not used to *decide* when to cut:
/// a bright vowel would read as an S. The decision listens through the high-pass, which
/// falls 12 dB per octave (−18 dB at 2 kHz, −11 dB at 3 kHz).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad2 {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad2 {
    fn design(freq: f64, high: bool) -> Self {
        let w0 = 2.0 * PI * freq / f64::from(SAMPLE_RATE);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    fn low_pass(freq: f64) -> Self {
        Self::design(freq, false)
    }

    fn high_pass(freq: f64) -> Self {
        Self::design(freq, true)
    }

    fn tick(&mut self, x: f64) -> f64 {
        let mut y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        if y.abs() < 1e-15 {
            y = 0.0;
        }
        (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, y);
        y
    }

    fn reset(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// The Studio sound stage. See the module documentation.
#[derive(Debug, Clone)]
pub struct Studio {
    enabled: bool,
    /// Share of the processed signal heard now (ramps for the crossfade).
    mix: f32,

    // Compressor.
    env: f64,
    detect: f64,
    /// Smoothed compression in dB (0 or negative).
    gr_db: f64,
    attack: f64,
    release: f64,
    speech_db: f64,
    /// Mean power of speech before and after compression (without make-up gain). Power,
    /// not dB: loud syllables, which are compressed most, dominate how loud speech sounds.
    speech_pow: f64,
    comp_pow: f64,
    makeup_db: f64,
    block_sum: f64,
    block_comp_sum: f64,
    block_len: usize,
    level_k: f64,

    // De-esser: `low` splits the band that is cut, `sense` is what decides the cut.
    low: Biquad2,
    sense: Biquad2,
    env_high: f64,
    env_all: f64,
    ds_attack: f64,
    ds_release: f64,

    // DC blocker after the (asymmetric) saturation.
    dc_x1: f32,
    dc_y1: f32,
}

impl Default for Studio {
    fn default() -> Self {
        Self::new()
    }
}

impl Studio {
    pub fn new() -> Self {
        let frame_ms = FRAME_SIZE as f64 * 1000.0 / f64::from(SAMPLE_RATE);
        Self {
            enabled: false,
            mix: 0.0,
            env: 0.0,
            detect: coeff(DETECT_MS),
            gr_db: 0.0,
            attack: coeff(ATTACK_MS),
            release: coeff(RELEASE_MS),
            speech_db: LEVEL_START_DB,
            speech_pow: 10f64.powf(LEVEL_START_DB / 10.0),
            comp_pow: 10f64
                .powf((LEVEL_START_DB + compression_db(THRESHOLD_BELOW_SPEECH_DB)) / 10.0),
            makeup_db: Self::makeup_for(LEVEL_START_DB + compression_db(THRESHOLD_BELOW_SPEECH_DB)),
            block_sum: 0.0,
            block_comp_sum: 0.0,
            block_len: 0,
            level_k: 1.0 - (-frame_ms / LEVEL_TAU_MS).exp(),
            low: Biquad2::low_pass(DEESS_HZ),
            sense: Biquad2::high_pass(DEESS_HZ),
            env_high: 0.0,
            env_all: 0.0,
            ds_attack: coeff(DEESS_ATTACK_MS),
            ds_release: coeff(DEESS_RELEASE_MS),
            dc_x1: 0.0,
            dc_y1: 0.0,
        }
    }

    /// Switch on or off; the change crossfades over the next 10 ms processed.
    pub fn set_enabled(&mut self, on: bool) {
        if on && !self.enabled && self.mix == 0.0 {
            // Coming back from fully off: start the detectors from silence, not from
            // whatever they held when it was switched off.
            self.reset();
        }
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// On, or still fading out: [`Studio::process`] changes the audio. When false, callers
    /// may skip it.
    pub fn is_active(&self) -> bool {
        self.enabled || self.mix > 0.0
    }

    /// The speech level the compressor follows, in dBFS.
    pub fn speech_level_db(&self) -> f64 {
        self.speech_db
    }

    /// The make-up gain in use, in dB.
    pub fn makeup_db(&self) -> f64 {
        self.makeup_db
    }

    /// Clear the detectors and filters (stream restart, mute). The learned speech level and
    /// make-up gain stay: your voice has not changed.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.gr_db = 0.0;
        self.block_sum = 0.0;
        self.block_comp_sum = 0.0;
        self.block_len = 0;
        self.low.reset();
        self.sense.reset();
        self.env_high = 0.0;
        self.env_all = 0.0;
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }

    /// Make-up gain that brings compressed speech at `comp_db` to [`TARGET_DB`].
    fn makeup_for(comp_db: f64) -> f64 {
        (TARGET_DB - comp_db).clamp(MAKEUP_MIN_DB, MAKEUP_MAX_DB)
    }

    /// Average compression speech gets, in dB (0 or negative).
    fn speech_reduction_db(&self) -> f64 {
        db(self.comp_pow / self.speech_pow.max(1e-12), true).min(0.0)
    }

    /// One 10 ms block finished: update the speech level and move the make-up gain.
    fn end_block(&mut self) {
        let len = self.block_len.max(1) as f64;
        let (pow, comp_pow) = (self.block_sum / len, self.block_comp_sum / len);
        let rms_db = db(pow, true);
        self.block_sum = 0.0;
        self.block_comp_sum = 0.0;
        self.block_len = 0;
        if rms_db > LEVEL_FLOOR_DB.max(self.speech_db - LEVEL_WINDOW_DB) {
            self.speech_db += (rms_db - self.speech_db) * self.level_k;
            self.speech_db = self.speech_db.clamp(-50.0, -6.0);
            self.speech_pow += (pow - self.speech_pow) * self.level_k;
            self.comp_pow += (comp_pow - self.comp_pow) * self.level_k;
        }
        let want = Self::makeup_for(db(self.comp_pow, true));
        let dt = FRAME_SIZE as f64 / f64::from(SAMPLE_RATE);
        let step = (want - self.makeup_db).clamp(-MAKEUP_DOWN_DB_S * dt, MAKEUP_UP_DB_S * dt);
        self.makeup_db += step;
    }

    fn compress(&mut self, x: f64) -> f64 {
        let p = x * x;
        self.env += (p - self.env) * self.detect;
        let level = db(self.env, true);
        let threshold = self.speech_db - THRESHOLD_BELOW_SPEECH_DB;
        let mut want = compression_db(level - threshold);
        // Downward expansion in the pauses, matching the compression of speech.
        let below = self.speech_db - level;
        if below > PAUSE_START_BELOW_DB {
            let t = ((below - PAUSE_START_BELOW_DB) / (PAUSE_FULL_BELOW_DB - PAUSE_START_BELOW_DB))
                .min(1.0);
            want = want.min(t * self.speech_reduction_db());
        }
        // More compression: attack. Less: release.
        let k = if want < self.gr_db {
            self.attack
        } else {
            self.release
        };
        self.gr_db += (want - self.gr_db) * k;
        let compressed = x * 10f64.powf(self.gr_db / 20.0);
        self.block_sum += p;
        self.block_comp_sum += compressed * compressed;
        self.block_len += 1;
        let y = compressed * 10f64.powf(self.makeup_db / 20.0);
        if self.block_len == FRAME_SIZE {
            self.end_block();
        }
        y
    }

    fn deess(&mut self, x: f64) -> f64 {
        let hf = x - self.low.tick(x);
        let sensed = self.sense.tick(x);
        let follow = |env: &mut f64, v: f64, a: f64, r: f64| {
            let k = if v > *env { a } else { r };
            *env += (v - *env) * k;
        };
        follow(
            &mut self.env_high,
            sensed.abs(),
            self.ds_attack,
            self.ds_release,
        );
        follow(&mut self.env_all, x.abs(), self.ds_attack, self.ds_release);
        let high_db = db(self.env_high, false);
        if high_db < DEESS_FLOOR_DB {
            return x;
        }
        let share_db = high_db - db(self.env_all, false);
        let cut_db = (share_db - DEESS_START_DB).clamp(0.0, DEESS_MAX_DB);
        if cut_db == 0.0 {
            return x;
        }
        let g = 10f64.powf(-cut_db / 20.0);
        x - (1.0 - g) * hf
    }

    fn saturate(&mut self, x: f32) -> f32 {
        let t0 = (SAT_DRIVE * SAT_BIAS).tanh();
        // Normalised so the slope at zero is 1: quiet sound passes unchanged.
        let slope = SAT_DRIVE * (1.0 - t0 * t0);
        let sat = ((SAT_DRIVE * (x + SAT_BIAS)).tanh() - t0) / slope;
        let y = x + SAT_MIX * (sat - x);
        // The asymmetry leaves a little DC; a 10 Hz one-pole high-pass removes it.
        let out = y - self.dc_x1 + 0.9987 * self.dc_y1;
        self.dc_x1 = y;
        self.dc_y1 = if out.abs() < 1e-20 { 0.0 } else { out };
        out
    }

    /// Process `samples` in place. Any length; allocation-free.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.is_active() {
            return;
        }
        let target = if self.enabled { 1.0 } else { 0.0 };
        for s in samples.iter_mut() {
            let dry = *s;
            let c = self.compress(f64::from(dry));
            let d = self.deess(c) as f32;
            let wet = self.saturate(d);
            if self.mix != target {
                let step = 1.0 / FADE_SAMPLES;
                self.mix = if target > self.mix {
                    (self.mix + step).min(1.0)
                } else {
                    (self.mix - step).max(0.0)
                };
            }
            *s = dry + (wet - dry) * self.mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, amp: f64, n: usize, start: usize) -> Vec<f32> {
        (start..start + n)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        let p = x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64;
        db(p, true)
    }

    /// A sine of RMS `level_db` dBFS.
    fn amp(level_db: f64) -> f64 {
        10f64.powf(level_db / 20.0) * std::f64::consts::SQRT_2
    }

    #[test]
    fn off_is_a_bit_exact_bypass() {
        let mut s = Studio::new();
        let mut x = tone(300.0, 0.5, 4800, 0);
        let before = x.clone();
        s.process(&mut x);
        assert_eq!(x, before);
        assert!(!s.is_active());
    }

    #[test]
    fn the_knee_is_continuous_and_reaches_the_ratio() {
        assert_eq!(compression_db(-10.0), 0.0);
        let edge = KNEE_DB / 2.0;
        assert!((compression_db(edge - 1e-9) - compression_db(edge + 1e-9)).abs() < 1e-6);
        assert!(compression_db(-edge + 1e-9).abs() < 1e-6);
        // Well above the knee every RATIO dB in gives 1 dB out.
        let d = compression_db(20.0) - compression_db(20.0 - RATIO);
        assert!((d - (1.0 - RATIO)).abs() < 1e-9, "{d}");
    }

    /// Loud and soft syllables come out closer together than they went in.
    #[test]
    fn loud_and_soft_speech_move_closer() {
        let mut s = Studio::new();
        s.set_enabled(true);
        let seg = 9600; // 200 ms
        let (mut loud_out, mut soft_out) = (Vec::new(), Vec::new());
        for k in 0..40 {
            let (level, out) = if k % 2 == 0 {
                (-14.0, &mut loud_out)
            } else {
                (-24.0, &mut soft_out)
            };
            let mut x = tone(220.0, amp(level), seg, k * seg);
            s.process(&mut x);
            // Skip each segment's first 50 ms (the detector moving) and the first 2 s.
            if k >= 10 {
                out.extend_from_slice(&x[2400..]);
            }
        }
        let spread = rms_db(&loud_out) - rms_db(&soft_out);
        assert!(spread < 5.0, "10 dB in, {spread:.1} dB out");
        assert!(spread > 2.0, "not flattened completely: {spread:.1} dB");
    }

    /// Speech ends up near the target level; a very quiet mic gets the most make-up gain
    /// allowed (no more, so noise is not raised without limit).
    #[test]
    fn make_up_gain_brings_speech_to_a_steady_level() {
        for level in [-34.0, -26.0, -12.0] {
            let mut s = Studio::new();
            s.set_enabled(true);
            let mut out = Vec::new();
            for k in 0..60 {
                // 100 ms of "speech", 100 ms of near-silence (a closed gate).
                let mut x = tone(220.0, amp(level), 4800, k * 9600);
                x.extend(tone(220.0, amp(-70.0), 4800, k * 9600 + 4800));
                s.process(&mut x);
                if k >= 40 {
                    out.extend_from_slice(&x[1200..4800]);
                }
            }
            let got = rms_db(&out);
            if level < -30.0 {
                assert!((s.makeup_db() - MAKEUP_MAX_DB).abs() < 1e-9);
                assert!(got > level + 5.0, "{level} dB in: {got:.1} dB out");
                continue;
            }
            assert!(
                (got - TARGET_DB).abs() < 4.0,
                "{level} dB in: {got:.1} dB out, makeup {:.1}",
                s.makeup_db()
            );
            assert!(s.makeup_db() <= MAKEUP_MAX_DB && s.makeup_db() >= MAKEUP_MIN_DB);
        }
    }

    /// Silence between words does not drag the speech level down (pauses are not speech).
    #[test]
    fn pauses_do_not_count_as_speech() {
        let mut s = Studio::new();
        s.set_enabled(true);
        for k in 0..100 {
            let mut x = tone(220.0, amp(-20.0), 4800, k * 4800);
            s.process(&mut x);
        }
        let talking = s.speech_level_db();
        for _ in 0..300 {
            s.process(&mut [0.0f32; 480]);
        }
        assert!((s.speech_level_db() - talking).abs() < 0.01);
    }

    /// The make-up gain lifts speech, but the noise left between words is turned down by
    /// as much as speech is compressed: relative to the voice, it does not come up.
    #[test]
    fn pauses_do_not_come_up_with_the_make_up_gain() {
        let mut s = Studio::new();
        s.set_enabled(true);
        let (mut speech_in, mut speech_out, mut pause_in, mut pause_out) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for k in 0..60 {
            // 300 ms of speech-level tone (loud and soft syllables), 300 ms of noise floor.
            let level = if k % 2 == 0 { -26.0 } else { -34.0 };
            let mut x = tone(220.0, amp(level), 14_400, k * 28_800);
            x.extend(tone(220.0, amp(level - 30.0), 14_400, k * 28_800 + 14_400));
            let dry = x.clone();
            s.process(&mut x);
            if k >= 30 {
                speech_in.extend_from_slice(&dry[2400..14_400]);
                speech_out.extend_from_slice(&x[2400..14_400]);
                pause_in.extend_from_slice(&dry[19_200..]);
                pause_out.extend_from_slice(&x[19_200..]);
            }
        }
        let before = rms_db(&pause_in) - rms_db(&speech_in);
        let after = rms_db(&pause_out) - rms_db(&speech_out);
        assert!(
            s.makeup_db() > 6.0,
            "quiet speech is lifted: {:.1}",
            s.makeup_db()
        );
        assert!(
            after < before + 1.5,
            "pause vs speech {before:.1} dB in, {after:.1} dB out"
        );
    }

    /// The de-esser turns a loud S-like tone down, and leaves a voice-band tone alone.
    #[test]
    fn deesser_tames_sibilance_only() {
        let gain = |freq: f64| {
            let mut on = Studio::new();
            on.set_enabled(true);
            let mut x = tone(freq, amp(-20.0), 48_000, 0);
            let input = rms_db(&x[24_000..]);
            on.process(&mut x);
            rms_db(&x[24_000..]) - input
        };
        let (voice, ess) = (gain(300.0), gain(8_000.0));
        assert!(ess < voice - 4.0, "300 Hz {voice:.1} dB, 8 kHz {ess:.1} dB");
    }

    /// Amplitude of the `freq` component of `x` (sample `start` is time zero).
    fn component(x: &[f32], freq: f64, start: usize) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let w = 2.0 * PI * freq * (start + i) as f64 / 48_000.0;
            re += f64::from(v) * w.cos();
            im += f64::from(v) * w.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    /// A bright vowel is not an S. Its strong upper formant (here 3 kHz, often lifted further
    /// by a presence boost) must not make the de-esser take the air above 5.5 kHz away.
    /// Measuring through the 6 dB/octave split band read the 3 kHz formant only 2.3 dB below
    /// the input and cut the air by about 7 dB.
    #[test]
    fn deesser_leaves_bright_vowels_alone() {
        let n = 48_000;
        let (formant, air) = (amp(-20.0), amp(-40.0));
        let input: Vec<f32> = tone(3_000.0, formant, n, 0)
            .iter()
            .zip(tone(8_000.0, air, n, 0))
            .map(|(a, b)| a + b)
            .collect();
        let mut s = Studio::new();
        s.set_enabled(true);
        let mut out = input.clone();
        s.process(&mut out);
        let tail = n / 2;
        let gain_db = |f: f64| {
            20.0 * (component(&out[tail..], f, tail) / component(&input[tail..], f, tail)).log10()
        };
        let (g_formant, g_air) = (gain_db(3_000.0), gain_db(8_000.0));
        assert!(
            (g_air - g_formant).abs() < 1.5,
            "3 kHz {g_formant:.1} dB, 8 kHz {g_air:.1} dB"
        );
    }

    #[test]
    fn saturation_adds_harmonics_without_dc() {
        let mut s = Studio::new();
        let n = 48_000;
        let x = tone(200.0, 0.4, n, 0);
        let y: Vec<f32> = x.iter().map(|&v| s.saturate(v)).collect();
        let tail = &y[n / 2..];
        let mean = tail.iter().map(|&v| f64::from(v)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 1e-3, "DC {mean}");
        // Second harmonic (400 Hz) present: correlate with it.
        let h2: f64 = tail
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let t = (n / 2 + i) as f64 / 48_000.0;
                f64::from(v) * (2.0 * PI * 400.0 * t).cos()
            })
            .sum::<f64>()
            .abs()
            * 2.0
            / tail.len() as f64;
        assert!(h2 > 0.002, "2nd harmonic {h2}");
        // Quiet sound passes almost unchanged.
        let q = Studio::new().saturate(0.001);
        assert!((q - 0.001).abs() < 1e-4, "{q}");
    }

    /// Switching on or off glides: no sample jumps by more than the signal itself moves.
    #[test]
    fn switching_crossfades_and_then_bypasses() {
        let mut s = Studio::new();
        let mut warm = tone(220.0, 0.2, 4800, 0);
        s.process(&mut warm);
        s.set_enabled(true);
        let x = tone(220.0, 0.2, 480, 4800);
        let mut y = x.clone();
        s.process(&mut y);
        // The first sample is (almost) the dry one.
        assert!((y[0] - x[0]).abs() < 0.01, "{} vs {}", y[0], x[0]);
        assert!(s.is_active());

        s.set_enabled(false);
        assert!(s.is_active(), "still fading out");
        let mut z = tone(220.0, 0.2, 480, 5280);
        s.process(&mut z);
        assert!(!s.is_active());
        let mut w = tone(220.0, 0.2, 480, 5760);
        let before = w.clone();
        s.process(&mut w);
        assert_eq!(w, before);
    }

    #[test]
    fn output_stays_finite_and_bounded() {
        let mut s = Studio::new();
        s.set_enabled(true);
        let mut silence = [0.0f32; 4800];
        s.process(&mut silence);
        assert!(silence.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
        let mut loud: Vec<f32> = (0..48_000)
            .map(|i| if (i / 7) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        s.process(&mut loud);
        assert!(loud.iter().all(|v| v.is_finite() && v.abs() < 4.0));
    }
}
