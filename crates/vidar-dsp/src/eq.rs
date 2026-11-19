//! Seven-band voice equaliser: a low shelf, five peaks and a high shelf (RBJ cookbook biquads).
//!
//! The bands sit where a voice actually changes character, rather than on a music EQ's
//! octave grid: body (100 Hz), warmth (200 Hz), boxiness (500 Hz), nasal tone (1 kHz),
//! presence (2.5 kHz), sibilance (5 kHz) and air (8 kHz). It runs after the noise model, so
//! the model always hears the natural voice.

use std::f64::consts::{PI, SQRT_2};

pub const EQ_BANDS: usize = 7;
/// Centre (peaks) or corner (shelves) frequency of each band.
pub const EQ_FREQS_HZ: [f64; EQ_BANDS] = [100.0, 200.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 8_000.0];
/// Where each band's gain is measured: the peak centres, and for the shelves a point out on
/// their plateau (a shelf has only half its gain at its corner). [`VoiceEq::set_gains`]
/// makes the combined response pass through these points, so a curve drawn through them is
/// what you hear.
pub const EQ_POINTS_HZ: [f64; EQ_BANDS] = [60.0, 200.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 13_000.0];
/// Largest boost or cut per band.
pub const EQ_MAX_DB: f64 = 12.0;
/// Largest gain a single filter may get after correction (neighbouring bands overlap, so a
/// filter sometimes needs a little more than the gain asked for at its point).
const FILTER_MAX_DB: f64 = 18.0;
/// Q of the peaking bands: about one octave wide. Broad enough to sound natural, narrow
/// enough that neighbouring bands (1 to 1.3 octaves apart) can still be set independently.
const PEAK_Q: f64 = 1.4;

/// Built-in tones (`name`, gains in dB from low to high, at [`EQ_POINTS_HZ`]). `"off"` is flat.
pub const EQ_PRESETS: [(&str, [f64; EQ_BANDS]); 11] = [
    ("off", [0.0; EQ_BANDS]),
    // Fuller and softer: more body, less edge and hiss. Thin headset mics.
    ("warm", [3.0, 2.0, 0.5, 0.0, -1.5, -2.0, -3.0]),
    // Easier to understand: less boom and mud, more presence and air. Boomy mics.
    ("clear", [-2.0, -2.5, -1.5, 0.0, 3.0, 2.5, 2.0]),
    // Radio-style: a little weight, scooped boxiness, forward presence.
    ("broadcast", [2.0, -1.0, -2.5, -1.0, 2.5, 2.0, 1.5]),
    // Opens up dull or muffled mics (cheap webcams, a mic under clothing).
    ("bright", [0.0, -1.0, -1.0, 0.0, 2.0, 4.0, 5.0]),
    // Built-in laptop mics: less boxy room sound, more presence, a little less hiss.
    ("laptop", [1.0, 1.5, -3.0, -1.0, 2.0, 1.0, -1.0]),
    // Tames harsh, sibilant mics: softer S sounds and edge, the voice itself unchanged.
    ("soft", [0.0, 0.5, 0.0, 0.0, -2.0, -4.0, -3.0]),
    // A focused cut of the honky, nasal region around 1 kHz.
    ("nasal", [0.0, 0.0, -1.0, -4.0, -0.5, 0.0, 0.0]),
    // Cuts through game sound and compressed voice chat: lean lows, strong presence.
    ("gaming", [-3.0, -2.0, -1.0, 1.0, 4.0, 2.5, 0.0]),
    // Tinny gaming headsets: back the missing lows, soften the harsh upper mids.
    ("headset", [3.5, 2.5, 0.5, -1.5, -1.0, -1.5, -1.0]),
    // Talking very close to the mic: removes the boom (proximity effect), keeps clarity.
    ("close", [-4.0, -2.0, -0.5, 0.0, 1.0, 0.5, 1.0]),
];

/// Gains for a preset name, or `None` if it is not one (e.g. `"custom"`).
pub fn eq_preset(name: &str) -> Option<[f64; EQ_BANDS]> {
    EQ_PRESETS.iter().find(|(n, _)| *n == name).map(|(_, g)| *g)
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    LowShelf,
    Peak,
    HighShelf,
}

const KINDS: [Kind; EQ_BANDS] = [
    Kind::LowShelf,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::HighShelf,
];

/// One biquad, Direct Form I with `f64` state (tolerates coefficient changes while running).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
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

impl Biquad {
    fn design(kind: Kind, freq: f64, gain_db: f64, sample_rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Peak => {
                let alpha = sin / (2.0 * PEAK_Q);
                (
                    1.0 + alpha * a,
                    -2.0 * cos,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos,
                    1.0 - alpha / a,
                )
            }
            Kind::LowShelf | Kind::HighShelf => {
                // Shelf slope S = 1: the steepest without a bump.
                let k = 2.0 * a.sqrt() * (sin / 2.0 * SQRT_2);
                let (ap, am) = (a + 1.0, a - 1.0);
                if matches!(kind, Kind::LowShelf) {
                    (
                        a * (ap - am * cos + k),
                        2.0 * a * (am - ap * cos),
                        a * (ap - am * cos - k),
                        ap + am * cos + k,
                        -2.0 * (am + ap * cos),
                        ap + am * cos - k,
                    )
                } else {
                    (
                        a * (ap + am * cos + k),
                        -2.0 * a * (am + ap * cos),
                        a * (ap + am * cos - k),
                        ap - am * cos + k,
                        2.0 * (am - ap * cos),
                        ap - am * cos - k,
                    )
                }
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            ..Self::default()
        }
    }

    fn set_coefficients(&mut self, from: &Biquad) {
        (self.b0, self.b1, self.b2, self.a1, self.a2) =
            (from.b0, from.b1, from.b2, from.a1, from.a2);
    }

    fn process(&mut self, frame: &mut [f32]) {
        let (b0, b1, b2, a1, a2) = (self.b0, self.b1, self.b2, self.a1, self.a2);
        let (mut x1, mut x2, mut y1, mut y2) = (self.x1, self.x2, self.y1, self.y2);
        for s in frame.iter_mut() {
            let x0 = f64::from(*s);
            let mut y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            if y0.abs() < 1e-15 {
                y0 = 0.0;
            }
            *s = y0 as f32;
            (x2, x1, y2, y1) = (x1, x0, y1, y0);
        }
        (self.x1, self.x2, self.y1, self.y2) = (x1, x2, y1, y2);
    }

    /// Magnitude response in dB at `freq`.
    fn response_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        // |H(e^jw)| with z^-1 = e^-jw.
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).log10()
    }

    fn reset(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// Samples over which a gain change crossfades from the old curve to the new one
/// (10 ms at 48 kHz): a preset switch glides instead of clicking.
pub const EQ_FADE_SAMPLES: usize = 480;

/// The seven-band equaliser. Flat bands are skipped, so a flat EQ costs nothing.
#[derive(Debug, Clone)]
pub struct VoiceEq {
    sample_rate: f64,
    /// The gains asked for, at [`EQ_POINTS_HZ`].
    gains_db: [f64; EQ_BANDS],
    /// The filter gains that produce them.
    filter_db: [f64; EQ_BANDS],
    bands: [Biquad; EQ_BANDS],
    /// The filters before the last change, with their memory; set while a crossfade is due.
    fade_from: Option<([Biquad; EQ_BANDS], [f64; EQ_BANDS])>,
    /// Scratch for the old curve's output during a crossfade (no allocation on the audio thread).
    fade_buf: [f32; EQ_FADE_SAMPLES],
}

impl VoiceEq {
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = Self {
            sample_rate,
            gains_db: [0.0; EQ_BANDS],
            filter_db: [0.0; EQ_BANDS],
            bands: [Biquad::default(); EQ_BANDS],
            fade_from: None,
            fade_buf: [0.0; EQ_FADE_SAMPLES],
        };
        for i in 0..EQ_BANDS {
            eq.bands[i] = Biquad::design(KINDS[i], EQ_FREQS_HZ[i], 0.0, sample_rate);
        }
        eq
    }

    /// Set the band gains (clamped to ±[`EQ_MAX_DB`]): the response at each of
    /// [`EQ_POINTS_HZ`]. Nothing is recomputed while the gains stay the same and the filter
    /// memory is kept, so this can be called every frame while a slider moves. A change is
    /// crossfaded over the next [`EQ_FADE_SAMPLES`] processed. Allocation-free.
    pub fn set_gains(&mut self, gains_db: [f64; EQ_BANDS]) {
        let target: [f64; EQ_BANDS] = std::array::from_fn(|i| {
            let g = gains_db[i];
            if g.is_finite() {
                g.clamp(-EQ_MAX_DB, EQ_MAX_DB)
            } else {
                0.0
            }
        });
        if target == self.gains_db {
            return;
        }
        self.gains_db = target;
        // Fade from what is heard now. If a fade is still pending, its start stays: the
        // listener has not heard anything in between.
        if self.fade_from.is_none() {
            self.fade_from = Some((self.bands, self.filter_db));
        }
        let filter = if target.iter().all(|&g| g == 0.0) {
            [0.0; EQ_BANDS]
        } else {
            self.correct(&target)
        };
        for (i, &g) in filter.iter().enumerate() {
            if g != self.filter_db[i] {
                self.filter_db[i] = g;
                let d = Biquad::design(KINDS[i], EQ_FREQS_HZ[i], g, self.sample_rate);
                self.bands[i].set_coefficients(&d);
            }
        }
    }

    /// Filter gains whose combined response hits `target` at [`EQ_POINTS_HZ`]: neighbouring
    /// bands overlap, so setting each filter to its target would overshoot where they meet.
    /// A few Newton steps on the (nearly linear in dB) band responses converge closely.
    fn correct(&self, target: &[f64; EQ_BANDS]) -> [f64; EQ_BANDS] {
        let sr = self.sample_rate;
        let mut g = *target;
        for _ in 0..4 {
            // m[i][j]: dB at point i per dB of filter j, around filter j's current gain.
            let mut m = [[0.0; EQ_BANDS]; EQ_BANDS];
            let mut err = *target;
            for j in 0..EQ_BANDS {
                let gj = if g[j].abs() > 0.5 { g[j] } else { 1.0 };
                let unit = Biquad::design(KINDS[j], EQ_FREQS_HZ[j], gj, sr);
                let now = Biquad::design(KINDS[j], EQ_FREQS_HZ[j], g[j], sr);
                for i in 0..EQ_BANDS {
                    m[i][j] = unit.response_db(EQ_POINTS_HZ[i], sr) / gj;
                    err[i] -= now.response_db(EQ_POINTS_HZ[i], sr);
                }
            }
            let Some(step) = solve(m, err) else {
                break;
            };
            for (gi, d) in g.iter_mut().zip(step) {
                *gi = (*gi + d).clamp(-FILTER_MAX_DB, FILTER_MAX_DB);
            }
        }
        g
    }

    pub fn gains_db(&self) -> [f64; EQ_BANDS] {
        self.gains_db
    }

    /// Flat and not fading: [`VoiceEq::process`] would leave the audio unchanged, so callers
    /// may skip it. A switch to flat is still faded out first.
    pub fn is_flat(&self) -> bool {
        self.fade_from.is_none() && self.gains_db.iter().all(|&g| g == 0.0)
    }

    /// Run `frame` through the bands in use. Bands at 0 dB are skipped and their memory is
    /// cleared, so a band switched on later starts from silence instead of from stale samples
    /// (which would pop).
    fn run(bands: &mut [Biquad; EQ_BANDS], filter_db: &[f64; EQ_BANDS], frame: &mut [f32]) {
        for (band, &g) in bands.iter_mut().zip(filter_db) {
            if g != 0.0 {
                band.process(frame);
            } else {
                band.reset();
            }
        }
    }

    pub fn process(&mut self, frame: &mut [f32]) {
        if let Some((mut old, old_db)) = self.fade_from.take() {
            let n = frame.len().min(EQ_FADE_SAMPLES);
            let faded = &mut self.fade_buf[..n];
            faded.copy_from_slice(&frame[..n]);
            Self::run(&mut old, &old_db, faded);
            Self::run(&mut self.bands, &self.filter_db, frame);
            for (i, (new, &was)) in frame[..n].iter_mut().zip(faded.iter()).enumerate() {
                let t = (i + 1) as f32 / n as f32;
                *new = was + (*new - was) * t;
            }
        } else {
            Self::run(&mut self.bands, &self.filter_db, frame);
        }
    }

    /// Combined magnitude response in dB at `freq` (for drawing the curve).
    pub fn response_db(&self, freq: f64) -> f64 {
        self.bands
            .iter()
            .zip(&self.filter_db)
            .filter(|(_, &g)| g != 0.0)
            .map(|(b, _)| b.response_db(freq, self.sample_rate))
            .sum()
    }

    pub fn reset(&mut self) {
        self.bands.iter_mut().for_each(Biquad::reset);
        self.fade_from = None;
    }
}

/// Solve `m x = y` by Gaussian elimination with partial pivoting; `None` if singular.
fn solve(mut m: [[f64; EQ_BANDS]; EQ_BANDS], mut y: [f64; EQ_BANDS]) -> Option<[f64; EQ_BANDS]> {
    for c in 0..EQ_BANDS {
        let p = (c..EQ_BANDS).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-9 {
            return None;
        }
        m.swap(c, p);
        y.swap(c, p);
        for r in 0..EQ_BANDS {
            if r != c {
                let k = m[r][c] / m[c][c];
                for col in c..EQ_BANDS {
                    m[r][col] -= k * m[c][col];
                }
                y[r] -= k * y[c];
            }
        }
    }
    Some(std::array::from_fn(|i| y[i] / m[i][i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_gain_db(eq: &mut VoiceEq, freq: f64) -> f64 {
        let n = 48_000;
        let mut x: Vec<f32> = (0..n)
            .map(|i| (0.25 * (2.0 * PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect();
        eq.reset();
        eq.process(&mut x);
        let tail = &x[n / 2..];
        let rms =
            (tail.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        20.0 * (rms / (0.25 / SQRT_2)).log10()
    }

    fn sine_frame(start: usize) -> Vec<f32> {
        (start..start + 480)
            .map(|i| (0.5 * (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    /// A preset switch while sound is playing must not jump: the frame after the change
    /// starts where the old curve was and only reaches the new one at its end.
    #[test]
    fn a_change_crossfades_instead_of_jumping() {
        let mut eq = VoiceEq::new(48_000.0);
        eq.set_gains([0.0, 0.0, 0.0, -12.0, 0.0, 0.0, 0.0]);
        for f in 0..20 {
            eq.process(&mut sine_frame(f * 480));
        }
        let mut old = eq.clone();
        let mut abrupt = eq.clone();
        eq.set_gains([0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0]);
        abrupt.set_gains([0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0]);
        abrupt.fade_from = None;

        let (mut y, mut y_old, mut y_new) = (sine_frame(9600), sine_frame(9600), sine_frame(9600));
        eq.process(&mut y);
        old.process(&mut y_old);
        abrupt.process(&mut y_new);
        // First sample: almost exactly the old curve. Last sample: exactly the new one.
        let jump = (y_new[0] - y_old[0]).abs();
        assert!(
            (y[0] - y_old[0]).abs() <= jump / 400.0 + 1e-6,
            "{} vs {}",
            y[0],
            y_old[0]
        );
        assert_eq!(y[479], y_new[479]);
        assert!(!eq.is_flat() && eq.fade_from.is_none());
    }

    /// Switching to Off fades out too, and only then may callers skip the EQ.
    #[test]
    fn switching_off_fades_out_before_bypassing() {
        let mut eq = VoiceEq::new(48_000.0);
        eq.set_gains([6.0; EQ_BANDS]);
        eq.process(&mut sine_frame(0));
        eq.set_gains([0.0; EQ_BANDS]);
        assert!(!eq.is_flat(), "the fade-out still has to run");
        eq.process(&mut sine_frame(480));
        assert!(eq.is_flat());
    }

    /// A band that is off keeps no memory, so turning it back on cannot replay old samples.
    #[test]
    fn bands_that_are_off_forget_their_memory() {
        let mut eq = VoiceEq::new(48_000.0);
        eq.set_gains([0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0]);
        eq.process(&mut sine_frame(0));
        assert!(eq.bands[3].y1 != 0.0);
        // (Off, not another band: the overlap correction would nudge band 3 too.)
        eq.set_gains([0.0; EQ_BANDS]);
        eq.process(&mut sine_frame(480));
        let b = eq.bands[3];
        assert_eq!((b.x1, b.x2, b.y1, b.y2), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn flat_eq_is_a_bit_exact_bypass() {
        let mut eq = VoiceEq::new(48_000.0);
        let mut x: Vec<f32> = (0..480).map(|i| (i as f32 * 0.37).sin()).collect();
        let before = x.clone();
        eq.process(&mut x);
        assert!(eq.is_flat());
        assert_eq!(x, before);
    }

    #[test]
    fn each_band_hits_its_gain_at_its_point() {
        for (i, &f) in EQ_POINTS_HZ.iter().enumerate() {
            let mut g = [0.0; EQ_BANDS];
            g[i] = 6.0;
            let mut eq = VoiceEq::new(48_000.0);
            eq.set_gains(g);
            let measured = sine_gain_db(&mut eq, f);
            assert!(
                (measured - 6.0).abs() < 0.3,
                "band {i} at {f} Hz: {measured:.2} dB"
            );
            assert!(
                (eq.response_db(f) - measured).abs() < 0.2,
                "response curve disagrees"
            );
        }
    }

    /// The overlap of neighbouring bands is corrected: the curve goes through every point,
    /// and does not run far past the range when all bands are at the limit.
    #[test]
    fn response_passes_through_every_point() {
        let mut cases = vec![
            [-12.0; EQ_BANDS],
            [12.0; EQ_BANDS],
            [12.0, -12.0, 12.0, -12.0, 12.0, -12.0, 12.0],
            [-12.0, 12.0, -12.0, 12.0, -12.0, 12.0, -12.0],
        ];
        cases.extend(EQ_PRESETS.iter().map(|(_, g)| *g));
        for gains in cases {
            let mut eq = VoiceEq::new(48_000.0);
            eq.set_gains(gains);
            for (i, &f) in EQ_POINTS_HZ.iter().enumerate() {
                let r = eq.response_db(f);
                assert!((r - gains[i]).abs() < 0.2, "{gains:?}: {r:.2} dB at {f} Hz");
            }
            for k in 0..=100 {
                let f = 50.0 * (16_000.0f64 / 50.0).powf(f64::from(k) / 100.0);
                assert!(eq.response_db(f).abs() < 14.5, "{gains:?} at {f:.0} Hz");
            }
        }
    }

    /// Equal gains on every band give a nearly even lift. Peaks one octave wide leave about
    /// 2.2 dB of ripple between points at +-6 dB (the earlier five-band layout left 3.1 dB).
    #[test]
    fn equal_gains_are_smooth_between_points() {
        for db in [6.0, -6.0] {
            let mut eq = VoiceEq::new(48_000.0);
            eq.set_gains([db; EQ_BANDS]);
            for k in 0..=200 {
                let f = 60.0 * (13_000.0f64 / 60.0).powf(f64::from(k) / 200.0);
                let r = eq.response_db(f);
                assert!(
                    (r - db).abs() < 2.5,
                    "{db} dB everywhere: {r:.2} dB at {f:.0} Hz"
                );
            }
        }
    }

    #[test]
    fn gains_are_clamped_and_cleaned() {
        let mut eq = VoiceEq::new(48_000.0);
        eq.set_gains([40.0, -40.0, f64::NAN, 0.0, 1.0, 0.0, -2.0]);
        assert_eq!(
            eq.gains_db(),
            [EQ_MAX_DB, -EQ_MAX_DB, 0.0, 0.0, 1.0, 0.0, -2.0]
        );
    }

    #[test]
    fn presets_are_within_range_and_off_is_flat() {
        assert_eq!(eq_preset("off"), Some([0.0; EQ_BANDS]));
        assert_eq!(eq_preset("custom"), None);
        for (_, g) in EQ_PRESETS {
            assert!(g.iter().all(|v| v.abs() <= EQ_MAX_DB));
        }
    }

    #[test]
    fn far_away_frequencies_are_untouched_by_a_peak() {
        let mut eq = VoiceEq::new(48_000.0);
        eq.set_gains([0.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0]);
        assert!(sine_gain_db(&mut eq, 60.0).abs() < 0.5);
        assert!(sine_gain_db(&mut eq, 15_000.0).abs() < 0.5);
    }
}
