//! Auto tone: makes any microphone sound like a good studio one.
//!
//! It listens to the finished voice (after the model, the equaliser and Studio sound), keeps
//! the average spectrum of speech in the seven equaliser bands, and every few seconds of
//! talking moves an extra equaliser curve half way towards the shape a studio microphone
//! gives the voice, plus whatever voice tone is chosen. Measuring the result rather than the
//! input closes the loop: it allows for the tone, Studio sound and the model, and it settles
//! instead of guessing.
//!
//! The reference shape is the average long-term spectrum of the studio recordings in
//! VoiceBank (one male and one female speaker, 824 clips), measured with the same band
//! filters. The lowest band depends mostly on the speaker's pitch, so there it only takes
//! boom away and never adds bass; the top two bands are lifted by at most a few dB, so a
//! microphone that has no treble (a Bluetooth headset) does not get hiss turned up instead.

use crate::bands::BandPass;
use crate::{VoiceEq, EQ_BANDS, EQ_FREQS_HZ, FRAME_SIZE, SAMPLE_RATE};

/// Studio voice spectrum in the equaliser bands, dB relative to the mean of the 500 Hz to
/// 5 kHz bands. The first value is the male speaker's (the higher of the two): a ceiling for
/// boom, not a target.
const REFERENCE_DB: [f64; EQ_BANDS] = [3.8, 5.9, 7.6, 3.5, -2.7, -8.3, -13.1];
/// Bands whose mean sets the overall level, so the correction does not change loudness.
const MID: std::ops::Range<usize> = 2..6;
/// Most the correction may cut and boost, per band.
const MAX_CUT_DB: f64 = 6.0;
const MAX_BOOST_DB: [f64; EQ_BANDS] = [0.0, 4.0, 4.0, 4.0, 4.0, 3.0, 3.0];
/// Speech heard per update (3 s), and the share of the remaining error each update removes.
const BLOCK_FRAMES: u32 = 300;
const STEP: f64 = 0.5;
/// A 10 ms frame is speech when it is within this many dB of the tracked level and above
/// the floor.
const WINDOW_DB: f64 = 15.0;
const FLOOR_DB: f64 = -55.0;
/// Speech level tracker time constant.
const LEVEL_TAU_MS: f64 = 1500.0;
/// Changes smaller than this are not applied (no needless filter redesigns).
const DEADBAND_DB: f64 = 0.25;

#[derive(Debug, Clone)]
pub struct AutoTone {
    enabled: bool,
    filters: [BandPass; EQ_BANDS],
    acc: [f64; EQ_BANDS],
    frames: u32,
    level_db: f64,
    level_k: f64,
    gains_db: [f64; EQ_BANDS],
    /// The voice tone's own equaliser, to know what it does at each band centre.
    tone_eq: VoiceEq,
    tone_at_bands: [f64; EQ_BANDS],
    /// Target minus measured shape at the last update, in dB.
    residual_db: [f64; EQ_BANDS],
}

impl Default for AutoTone {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoTone {
    pub fn new() -> Self {
        let sr = f64::from(SAMPLE_RATE);
        let frame_ms = FRAME_SIZE as f64 * 1000.0 / sr;
        Self {
            enabled: false,
            filters: EQ_FREQS_HZ.map(|f| BandPass::new(f, 1.4, sr)),
            acc: [0.0; EQ_BANDS],
            frames: 0,
            level_db: -30.0,
            level_k: 1.0 - (-frame_ms / LEVEL_TAU_MS).exp(),
            gains_db: [0.0; EQ_BANDS],
            tone_eq: VoiceEq::new(sr),
            tone_at_bands: [0.0; EQ_BANDS],
            residual_db: [0.0; EQ_BANDS],
        }
    }

    /// Switch on or off. Off drops the correction (the equaliser crossfades it out); on
    /// starts learning again from flat.
    pub fn set_enabled(&mut self, on: bool) {
        if on != self.enabled {
            self.enabled = on;
            self.gains_db = [0.0; EQ_BANDS];
            self.acc = [0.0; EQ_BANDS];
            self.frames = 0;
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The correction to add to the voice tone's gains, in dB at the equaliser points.
    pub fn gains_db(&self) -> [f64; EQ_BANDS] {
        self.gains_db
    }

    /// How far the voice was from its target shape at the last update, in dB per band
    /// (positive: the band is short). Bands at a limit keep a residual.
    pub fn residual_db(&self) -> [f64; EQ_BANDS] {
        self.residual_db
    }

    /// Start with a correction learned earlier (for example on the same microphone).
    pub fn set_gains_db(&mut self, g: [f64; EQ_BANDS]) {
        self.gains_db = std::array::from_fn(|i| clamp_band(i, g[i]));
    }

    /// Listen to one finished frame. `tone_db` is the voice tone in use (its gains), which
    /// is added to the target so a chosen tone is kept, not corrected away.
    pub fn analyze(&mut self, frame: &[f32], tone_db: &[f64; EQ_BANDS]) {
        if !self.enabled {
            return;
        }
        let p = frame
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>()
            / frame.len().max(1) as f64;
        let db = 10.0 * p.max(1e-12).log10();
        let speech = db > FLOOR_DB.max(self.level_db - WINDOW_DB);
        if speech {
            self.level_db += (db - self.level_db) * self.level_k;
        }
        for (f, a) in self.filters.iter_mut().zip(&mut self.acc) {
            let mut e = 0.0;
            for &x in frame {
                let y = f64::from(f.tick(x));
                e += y * y;
            }
            if speech {
                *a += e;
            }
        }
        if !speech {
            return;
        }
        self.frames += 1;
        if self.frames < BLOCK_FRAMES {
            return;
        }
        let measured: [f64; EQ_BANDS] = std::array::from_fn(|i| {
            10.0 * (self.acc[i] / f64::from(self.frames)).max(1e-20).log10()
        });
        if self.tone_eq.gains_db() != *tone_db {
            self.tone_eq.set_gains(*tone_db);
            self.tone_at_bands = EQ_FREQS_HZ.map(|f| self.tone_eq.response_db(f));
        }
        let target: [f64; EQ_BANDS] =
            std::array::from_fn(|i| REFERENCE_DB[i] + self.tone_at_bands[i]);
        let mid = |v: &[f64; EQ_BANDS]| v[MID].iter().sum::<f64>() / MID.len() as f64;
        let (m0, t0) = (mid(&measured), mid(&target));
        let mut next = self.gains_db;
        for i in 0..EQ_BANDS {
            let err = (target[i] - t0) - (measured[i] - m0);
            self.residual_db[i] = err;
            // The lowest band only cuts boom above the reference.
            let err = if i == 0 { err.min(0.0) } else { err };
            next[i] = clamp_band(i, self.gains_db[i] + STEP * err);
        }
        // Keep the correction level-neutral in the mid bands.
        let shift = mid(&next);
        for (i, g) in next.iter_mut().enumerate() {
            *g = clamp_band(i, *g - shift);
        }
        if next
            .iter()
            .zip(&self.gains_db)
            .any(|(a, b)| (a - b).abs() >= DEADBAND_DB)
        {
            self.gains_db = next.map(|g| (g * 4.0).round() / 4.0);
        }
        self.acc = [0.0; EQ_BANDS];
        self.frames = 0;
    }
}

fn clamp_band(i: usize, g: f64) -> f64 {
    if g.is_finite() {
        g.clamp(-MAX_CUT_DB, MAX_BOOST_DB[i])
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VoiceEq;

    /// Speech-like noise: pink noise shaped by a fixed tilt per band, in 10 ms frames with
    /// syllable-rate level changes.
    fn voice(seconds: f64, colour: [f64; EQ_BANDS], seed: u64) -> Vec<f32> {
        // Start from the studio shape itself, so a neutral colour needs little correction.
        let mut eq = VoiceEq::new(48_000.0);
        // Equaliser gains at its points that measure as the reference in the octave bands
        // (the shelves sit at 60 Hz and 13 kHz and the peaks are narrower than an octave).
        const NEUTRAL_FIX: [f64; EQ_BANDS] = [0.0, 2.5, 6.0, 1.0, -1.0, -4.5, -9.0];
        eq.set_gains(std::array::from_fn(|i| REFERENCE_DB[i] + NEUTRAL_FIX[i]));
        // The microphone's colour on top, in a second equaliser (each is limited to ±12 dB).
        let mut mic = VoiceEq::new(48_000.0);
        mic.set_gains(colour);
        let mut s = seed.max(1);
        // Pink noise (Paul Kellet's economy filter): equal energy per octave, like the
        // octave-wide bands measure a voice.
        let (mut p0, mut p1, mut p2) = (0.0f64, 0.0f64, 0.0f64);
        let n = (seconds * 48_000.0) as usize / FRAME_SIZE * FRAME_SIZE;
        let mut x: Vec<f32> = (0..n)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let w = (s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
                p0 = 0.99765 * p0 + w * 0.0990460;
                p1 = 0.96300 * p1 + w * 0.2965164;
                p2 = 0.57000 * p2 + w * 1.0526913;
                let u = (p0 + p1 + p2 + w * 0.1848) * 0.25;
                let syll =
                    0.6 + 0.4 * (2.0 * std::f64::consts::PI * 4.0 * i as f64 / 48_000.0).sin();
                (0.05 * u * syll) as f32
            })
            .collect();
        for f in x.chunks_mut(FRAME_SIZE) {
            eq.process(f);
            mic.process(f);
        }
        x
    }

    /// Run `x` through an equaliser driven by auto tone, as the app does; returns the
    /// learned correction.
    fn learn(x: &[f32], tone: [f64; EQ_BANDS]) -> AutoTone {
        let mut a = AutoTone::new();
        a.set_enabled(true);
        let mut eq = VoiceEq::new(48_000.0);
        for _ in 0..8 {
            for f in x.chunks(FRAME_SIZE) {
                let g: [f64; EQ_BANDS] = std::array::from_fn(|i| tone[i] + a.gains_db()[i]);
                eq.set_gains(g);
                let mut y = [0.0f32; FRAME_SIZE];
                y.copy_from_slice(f);
                eq.process(&mut y);
                a.analyze(&y, &tone);
            }
        }
        a
    }

    /// A dull version of the same voice gets its treble lifted relative to the neutral one,
    /// and a boomy one its bass cut, within the limits.
    #[test]
    fn corrects_dull_and_boomy_microphones() {
        let flat = learn(&voice(10.0, [0.0; EQ_BANDS], 3), [0.0; EQ_BANDS]).gains_db();
        // (8 kHz: this test voice is darker than the correction may lift.)
        for g in &flat[1..EQ_BANDS - 1] {
            assert!(
                g.abs() <= 1.5,
                "a studio-shaped voice is left nearly alone: {flat:?}"
            );
        }
        let dull = learn(
            &voice(10.0, [0.0, 0.0, 0.0, 0.0, -2.0, -6.0, -8.0], 3),
            [0.0; EQ_BANDS],
        )
        .gains_db();
        let boomy = learn(
            &voice(10.0, [8.0, 5.0, 0.0, 0.0, 0.0, 0.0, 0.0], 3),
            [0.0; EQ_BANDS],
        )
        .gains_db();
        assert!(
            dull[5] > flat[5] + 2.0,
            "dull 5k {:.1} vs {:.1}",
            dull[5],
            flat[5]
        );
        assert!(
            boomy[1] < flat[1] - 2.0,
            "boomy 200 {:.1} vs {:.1}",
            boomy[1],
            flat[1]
        );
        assert!(boomy[0] < -2.0, "boom is cut: {:.1}", boomy[0]);
        for g in [flat, dull, boomy] {
            for (i, v) in g.iter().enumerate() {
                assert!(*v >= -MAX_CUT_DB && *v <= MAX_BOOST_DB[i], "{g:?}");
            }
        }
    }

    /// The voice ends up at the studio shape plus the chosen tone: the tone is kept, not
    /// corrected away. (The 8 kHz band of this test voice is darker than the correction may
    /// lift, so it is left out.)
    #[test]
    fn reaches_the_target_and_keeps_the_chosen_tone() {
        for colour in [[0.0; EQ_BANDS], [0.0, 3.0, 0.0, -2.0, 2.0, -3.0, 0.0]] {
            let x = voice(10.0, colour, 5);
            for tone in [[0.0; EQ_BANDS], [3.0, 2.0, 0.5, 0.0, -1.5, -2.0, -3.0]] {
                let r = learn(&x, tone).residual_db();
                for (i, e) in r.iter().enumerate().take(EQ_BANDS - 1).skip(1) {
                    assert!(e.abs() < 1.0, "band {i}: residual {r:?}, tone {tone:?}");
                }
            }
        }
    }

    #[test]
    fn off_learns_nothing_and_silence_changes_nothing() {
        let mut a = AutoTone::new();
        a.analyze(&[0.3; FRAME_SIZE], &[0.0; EQ_BANDS]);
        assert_eq!(a.gains_db(), [0.0; EQ_BANDS]);
        a.set_enabled(true);
        for _ in 0..5000 {
            a.analyze(&[0.0; FRAME_SIZE], &[0.0; EQ_BANDS]);
        }
        assert_eq!(a.gains_db(), [0.0; EQ_BANDS]);
    }
}
