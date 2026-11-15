//! A tiny octave-band analyser for the live logo: how loud each part of the voice spectrum is,
//! once per 10 ms frame. Six band-pass biquads, so it costs next to nothing on the audio thread.

use crate::Frame;

/// Number of bands, low to high.
pub const BANDS: usize = 6;

/// Band centres in Hz, one octave apart: voice fundamental and body (160, 320), first
/// formants (640, 1280), clarity and sibilance (2560, 5120).
pub const BAND_CENTRES_HZ: [f64; BANDS] = [160.0, 320.0, 640.0, 1280.0, 2560.0, 5120.0];

/// RBJ band-pass (0 dB peak gain), transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
struct BandPass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: f32,
    s2: f32,
}

impl BandPass {
    fn new(centre_hz: f64, q: f64, sample_rate: f64) -> Self {
        let w0 = std::f64::consts::TAU * centre_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (alpha / a0) as f32,
            b2: (-alpha / a0) as f32,
            a1: (-2.0 * w0.cos() / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            s1: 0.0,
            s2: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        // b1 is zero for this band-pass.
        let y = self.b0 * x + self.s1;
        self.s1 = -self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Per-band RMS level in dBFS of each frame it is given.
#[derive(Debug, Clone)]
pub struct VoiceBands {
    filters: [BandPass; BANDS],
}

impl Default for VoiceBands {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl VoiceBands {
    pub fn new(sample_rate: f64) -> Self {
        // Q of about 1.4 gives roughly one-octave bands that just touch.
        Self { filters: BAND_CENTRES_HZ.map(|f| BandPass::new(f, 1.4, sample_rate)) }
    }

    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f.s1 = 0.0;
            f.s2 = 0.0;
        }
    }

    /// Band levels of `frame` in dBFS (a full-scale sine in the band reads about -3 dB),
    /// floored at -100.
    pub fn analyze(&mut self, frame: &Frame) -> [f64; BANDS] {
        let mut out = [0.0; BANDS];
        for (f, o) in self.filters.iter_mut().zip(&mut out) {
            let mut sum = 0.0f64;
            for &x in frame {
                let y = f.tick(x);
                sum += f64::from(y) * f64::from(y);
            }
            let rms = (sum / frame.len() as f64).sqrt();
            *o = if rms > 1e-5 { 20.0 * rms.log10() } else { -100.0 };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FRAME_SIZE;

    fn loudest_band(freq: f64) -> usize {
        let mut b = VoiceBands::default();
        let mut last = [0.0; BANDS];
        for k in 0..20 {
            let mut frame = [0.0f32; FRAME_SIZE];
            for (i, s) in frame.iter_mut().enumerate() {
                let n = (k * FRAME_SIZE + i) as f64;
                *s = (0.5 * (std::f64::consts::TAU * freq * n / 48_000.0).sin()) as f32;
            }
            last = b.analyze(&frame);
        }
        (0..BANDS).max_by(|&a, &c| last[a].total_cmp(&last[c])).unwrap()
    }

    #[test]
    fn tones_land_in_their_band() {
        for (band, &centre) in BAND_CENTRES_HZ.iter().enumerate() {
            assert_eq!(loudest_band(centre), band, "{centre} Hz");
        }
    }

    #[test]
    fn silence_is_floor() {
        let mut b = VoiceBands::default();
        assert!(b.analyze(&[0.0; FRAME_SIZE]).iter().all(|&d| d == -100.0));
    }

    #[test]
    fn in_band_level_is_close_to_sine_rms() {
        let mut b = VoiceBands::default();
        let mut db = [0.0; BANDS];
        for k in 0..20 {
            let mut frame = [0.0f32; FRAME_SIZE];
            for (i, s) in frame.iter_mut().enumerate() {
                let n = (k * FRAME_SIZE + i) as f64;
                *s = (std::f64::consts::TAU * 640.0 * n / 48_000.0).sin() as f32;
            }
            db = b.analyze(&frame);
        }
        assert!((db[2] + 3.0).abs() < 0.5, "640 Hz full-scale sine: {} dB", db[2]);
    }
}
