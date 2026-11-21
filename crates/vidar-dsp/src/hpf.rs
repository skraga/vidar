/// 2nd-order Butterworth high-pass biquad (Direct Form I).
///
/// Cuts rumble below the cutoff (cooling pads, desk thumps, hum) before the model sees it.
/// State is kept in `f64` for stability on long, quiet decays.
///
/// Switching it on or off ([`HighPass::set_enabled`]) crossfades over 10 ms. While off it
/// keeps filtering in the background (one biquad, next to nothing) and only its output is
/// unused, so switching back on does not restart it from stale memory.
#[derive(Debug, Clone)]
pub struct HighPass {
    enabled: bool,
    /// Share of the filtered signal heard now (ramps for the crossfade).
    mix: f32,
    cutoff_hz: f64,
    sample_rate: f64,
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    /// Softens P and B pops; runs after the filter while it is on.
    pop: crate::PopFilter,
}

impl HighPass {
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> Self {
        let mut f = Self {
            enabled: true,
            mix: 1.0,
            cutoff_hz,
            sample_rate,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            pop: crate::PopFilter::new(),
        };
        f.calculate_coefficients();
        f
    }

    pub fn cutoff_hz(&self) -> f64 {
        self.cutoff_hz
    }

    /// Switch on or off; the change crossfades over the next 10 ms processed.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Switch on or off at once, with no crossfade (before any audio has been processed).
    pub fn with_enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self.mix = if on { 1.0 } else { 0.0 };
        self
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn calculate_coefficients(&mut self) {
        let w0 = 2.0 * std::f64::consts::PI * self.cutoff_hz / self.sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let q = 1.0 / 2.0f64.sqrt();
        let alpha = sin_w0 / (2.0 * q);

        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Zero the filter memory.
    pub fn reset(&mut self) {
        self.pop.reset();
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    pub fn process(&mut self, frame: &mut [f32]) {
        self.process_filter(frame);
        if self.enabled {
            self.pop.process(frame);
        } else {
            self.pop.reset();
        }
    }

    fn process_filter(&mut self, frame: &mut [f32]) {
        if !(self.enabled && self.mix == 1.0) {
            self.process_fading(frame);
            return;
        }
        let (b0, b1, b2, a1, a2) = (self.b0, self.b1, self.b2, self.a1, self.a2);
        let (mut x1, mut x2, mut y1, mut y2) = (self.x1, self.x2, self.y1, self.y2);
        for s in frame.iter_mut() {
            let x0 = f64::from(*s);
            let mut y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            // Flush denormals so long silent decays do not stall the CPU.
            if y0 > -1e-15 && y0 < 1e-15 {
                y0 = 0.0;
            }
            *s = y0 as f32;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = y0;
        }
        self.x1 = x1;
        self.x2 = x2;
        self.y1 = y1;
        self.y2 = y2;
    }

    /// Off, or fading in or out: filter, then mix the filtered and the unfiltered signal.
    fn process_fading(&mut self, frame: &mut [f32]) {
        let target = if self.enabled { 1.0 } else { 0.0 };
        let step = 1.0 / crate::FRAME_SIZE as f32;
        for s in frame.iter_mut() {
            let dry = *s;
            let x0 = f64::from(dry);
            let mut y0 = self.b0 * x0 + self.b1 * self.x1 + self.b2 * self.x2
                - self.a1 * self.y1
                - self.a2 * self.y2;
            if y0 > -1e-15 && y0 < 1e-15 {
                y0 = 0.0;
            }
            (self.x2, self.x1, self.y2, self.y1) = (self.x1, x0, self.y1, y0);
            if self.mix != target {
                self.mix = if target > self.mix {
                    (self.mix + step).min(1.0)
                } else {
                    (self.mix - step).max(0.0)
                };
            }
            if self.mix > 0.0 {
                *s = dry + (y0 as f32 - dry) * self.mix;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn passes_speech_band_and_cuts_rumble() {
        let mut f = HighPass::new(80.0, 48_000.0);
        let mut low = sine(20.0, 48_000);
        f.process(&mut low);
        f.reset();
        let mut mid = sine(1000.0, 48_000);
        f.process(&mut mid);
        // Measure after the filter has settled.
        let low_rms = rms(&low[24_000..]);
        let mid_rms = rms(&mid[24_000..]);
        assert!(
            low_rms < 0.5 * std::f64::consts::FRAC_1_SQRT_2 * 0.1,
            "20 Hz not attenuated: {low_rms}"
        );
        assert!(
            (mid_rms - 0.5 * std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "1 kHz changed: {mid_rms}"
        );
    }

    #[test]
    fn block_size_does_not_change_output() {
        let input = sine(150.0, 4800);
        let mut a = input.clone();
        HighPass::new(80.0, 48_000.0).process(&mut a);
        let mut b = input;
        let mut f = HighPass::new(80.0, 48_000.0);
        for chunk in b.chunks_mut(480) {
            f.process(chunk);
        }
        assert_eq!(a, b);
    }

    /// Switching off glides from filtered to unfiltered over 10 ms, and back on again
    /// without starting the filter from stale memory.
    #[test]
    fn switching_crossfades() {
        // 40 Hz rumble: the filter turns it well down, so the two states differ clearly.
        let x = sine(40.0, 48_000);
        let mut f = HighPass::new(80.0, 48_000.0);
        let mut y = x.clone();
        f.process(&mut y[..24_000]);
        f.set_enabled(false);
        f.process(&mut y[24_000..24_480]);
        assert!(f.mix == 0.0, "faded out within one frame");
        f.process(&mut y[24_480..36_000]);
        assert_eq!(
            &y[24_480..36_000],
            &x[24_480..36_000],
            "off leaves audio alone"
        );
        f.set_enabled(true);
        f.process(&mut y[36_000..]);
        // No jumps: a hard switch moves one sample by up to the gap between the filtered
        // and unfiltered sine (about 0.3 here); a crossfade adds at most a 480th of it
        // to the sine's own slope.
        let max_slope = 0.5 * 2.0 * std::f64::consts::PI * 40.0 / 48_000.0;
        for w in y.windows(2) {
            let d = f64::from((w[1] - w[0]).abs());
            assert!(d < max_slope * 2.0, "jump of {d}");
        }
        // Back on, it matches a filter that never stopped.
        let mut always = x.clone();
        HighPass::new(80.0, 48_000.0).process(&mut always);
        assert!((y[47_999] - always[47_999]).abs() < 1e-6);
    }

    #[test]
    fn starts_off_without_a_fade() {
        let x = sine(40.0, 480);
        let mut y = x.clone();
        HighPass::new(80.0, 48_000.0)
            .with_enabled(false)
            .process(&mut y);
        assert_eq!(x, y);
    }

    #[test]
    fn silence_decays_to_exact_zero() {
        let mut f = HighPass::new(80.0, 48_000.0);
        let mut x = vec![0.0f32; 48_000];
        x[0] = 1.0;
        f.process(&mut x);
        assert_eq!(*x.last().unwrap(), 0.0);
    }
}
