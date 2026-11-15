/// 2nd-order Butterworth high-pass biquad (Direct Form I).
///
/// Cuts rumble below the cutoff (cooling pads, desk thumps, hum) before the model sees it.
/// State is kept in `f64` for stability on long, quiet decays.
#[derive(Debug, Clone)]
pub struct HighPass {
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
}

impl HighPass {
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> Self {
        let mut f = Self {
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
        };
        f.calculate_coefficients();
        f
    }

    pub fn cutoff_hz(&self) -> f64 {
        self.cutoff_hz
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
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    pub fn process(&mut self, frame: &mut [f32]) {
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

    #[test]
    fn silence_decays_to_exact_zero() {
        let mut f = HighPass::new(80.0, 48_000.0);
        let mut x = vec![0.0f32; 48_000];
        x[0] = 1.0;
        f.process(&mut x);
        assert_eq!(*x.last().unwrap(), 0.0);
    }
}
