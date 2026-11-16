//! DPDFNet streaming noise suppressor (Ceva-IP, Apache-2.0).
//!
//! Per 10 ms frame: 960-sample causal Vorbis window (50 % overlap) -> real FFT (481 bins)
//! -> model(spec, flat state) -> inverse FFT -> windowed overlap-add -> 480 samples out.
//!
//! The model has no voice-activity output, so the speech probability comes from either an
//! RNNoise side-chain (`Vad::RnNoise`) or from how much speech-band energy the model kept
//! (`Vad::Spectral`). Output lags input by 50 ms vs RNNoise's 20 ms; the probability and the
//! level handed to the gate are delayed by the difference so the gate tuning still applies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use vidar_dsp::{Denoiser, Frame, FRAME_SIZE, SAMPLE_RATE};

use crate::onnx::SpecModel;
use crate::rnnoise::RnNoise;
use crate::Error;

pub const WIN_LEN: usize = 2 * FRAME_SIZE;
pub const FREQ_BINS: usize = WIN_LEN / 2 + 1;

pub const DEFAULT_MODEL: &str = "dpdfnet2_48khz_hr";
pub const MODELS: [&str; 2] = ["dpdfnet2_48khz_hr", "dpdfnet8_48khz_hr"];

/// Internal look-ahead of each model in frames, beyond the one-hop STFT delay.
/// Measured: both lag 2400 samples = 1 hop + 4 frames.
fn model_delay_frames(model: &str) -> usize {
    match model {
        "dpdfnet2_48khz_hr" | "dpdfnet8_48khz_hr" => 4,
        _ => 0,
    }
}

/// RNNoise's delay in frames; the gate was tuned against it.
const RNNOISE_DELAY_FRAMES: usize = 2;

const BIN_HZ: f64 = SAMPLE_RATE as f64 / WIN_LEN as f64;

/// Source of the speech probability that drives the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vad {
    /// RNNoise side-chain on the same input (its audio is discarded).
    RnNoise,
    /// Ratio of speech-band energy kept by DPDFNet, gated by the kept level.
    Spectral,
}

impl Vad {
    pub fn parse(s: &str) -> Vad {
        match s.trim().to_ascii_lowercase().as_str() {
            "spectral" => Vad::Spectral,
            _ => Vad::RnNoise,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Vad::RnNoise => "rnnoise",
            Vad::Spectral => "spectral",
        }
    }
}

/// Power-complementary window: w[i]^2 + w[i + n/2]^2 == 1.
pub fn vorbis_window() -> [f32; WIN_LEN] {
    let half = WIN_LEN as f64 / 2.0;
    core::array::from_fn(|i| {
        let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half).sin();
        (0.5 * std::f64::consts::PI * s * s).sin() as f32
    })
}

/// Look for `<model>.onnx` in `dirs`, then next to the executable (`models/`).
pub fn find_model_path(model: &str, dirs: &[PathBuf]) -> Result<PathBuf, Error> {
    let file = format!("{model}.onnx");
    let mut candidates: Vec<PathBuf> = dirs.iter().map(|d| d.join(&file)).collect();
    if let Ok(dir) = std::env::var("VIDAR_MODELS_DIR") {
        candidates.push(Path::new(&dir).join(&file));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("models").join(&file));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or(Error::ModelMissing(file))
}

/// Fixed delay line for one value per frame.
struct FrameDelay {
    buf: Vec<f64>,
    pos: usize,
    fill: f64,
}

impl FrameDelay {
    fn new(frames: usize, fill: f64) -> Self {
        Self {
            buf: vec![fill; frames],
            pos: 0,
            fill,
        }
    }
    fn push(&mut self, v: f64) -> f64 {
        if self.buf.is_empty() {
            return v;
        }
        let out = self.buf[self.pos];
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
    fn reset(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = self.fill);
        self.pos = 0;
    }
}

/// Option B: speech probability from the energy DPDFNet kept in the speech band.
struct SpectralDetector {
    kept_lo_db: f64,
    kept_hi_db: f64,
    level_lo_db: f64,
    level_hi_db: f64,
    release: f64,
    win_energy: f64,
    lo_bin: usize,
    hi_bin: usize,
    prob: f64,
}

impl SpectralDetector {
    fn new() -> Self {
        let w = vorbis_window();
        // Window energy summed in float32, as in the reference vectors.
        let win_energy = f64::from(w.iter().map(|x| x * x).sum::<f32>());
        Self {
            kept_lo_db: -15.0,
            kept_hi_db: -4.0,
            level_lo_db: -62.0,
            level_hi_db: -48.0,
            release: 0.55,
            win_energy,
            lo_bin: (250.0 / BIN_HZ).round() as usize,
            hi_bin: (4000.0 / BIN_HZ).round() as usize,
            prob: 0.0,
        }
    }

    fn band_power_db(&self, spec: &[Complex<f64>]) -> f64 {
        let sum: f64 = spec[self.lo_bin..self.hi_bin]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let ms = 2.0 * sum / (WIN_LEN as f64 * self.win_energy);
        10.0 * (ms + 1e-12).log10()
    }

    fn update(&mut self, spec_in: &[Complex<f64>], spec_out: &[Complex<f64>]) -> f64 {
        let in_db = self.band_power_db(spec_in);
        let out_db = self.band_power_db(spec_out);
        let kept = out_db - in_db;
        let p_kept = (kept - self.kept_lo_db) / (self.kept_hi_db - self.kept_lo_db);
        let p_lvl = (out_db - self.level_lo_db) / (self.level_hi_db - self.level_lo_db);
        let raw = p_kept.clamp(0.0, 1.0) * p_lvl.clamp(0.0, 1.0);
        if raw >= self.prob {
            self.prob = raw;
        } else {
            self.prob = self.release * self.prob + (1.0 - self.release) * raw;
        }
        self.prob
    }
}

pub struct DpdfNet {
    model: Box<dyn SpecModel>,

    window: [f32; WIN_LEN],
    in_buf: [f32; WIN_LEN],
    ola: [f32; WIN_LEN],

    fft: Arc<dyn RealToComplex<f64>>,
    ifft: Arc<dyn ComplexToReal<f64>>,
    fft_in: Vec<f64>,
    spec_in: Vec<Complex<f64>>,
    spec_out: Vec<Complex<f64>>,
    spec_out_vad: Vec<Complex<f64>>,
    time_out: Vec<f64>,
    fft_scratch: Vec<Complex<f64>>,
    ifft_scratch: Vec<Complex<f64>>,

    vad_rnnoise: Option<RnNoise>,
    spectral: Option<SpectralDetector>,
    spec_hist: Vec<Vec<Complex<f64>>>,
    hist_len: usize,
    hist_pos: usize,

    prob_delay: FrameDelay,
    level_delay: FrameDelay,
    delay_samples: usize,
    /// Share of the noisy spectrum mixed back in (attenuation limit); 0 = off.
    attn_alpha: f64,
}

impl DpdfNet {
    /// Load `model` (e.g. `"dpdfnet2_48khz_hr"`) from `path` and build the streaming state.
    pub fn load(path: &Path, model_name: &str, vad: Vad) -> Result<Self, Error> {
        let model = crate::onnx::load(path)?;
        Self::with_model(model, model_name, vad)
    }

    pub fn with_model(
        model: Box<dyn SpecModel>,
        model_name: &str,
        vad: Vad,
    ) -> Result<Self, Error> {
        let k = model_delay_frames(model_name);
        let align = (1 + k).saturating_sub(RNNOISE_DELAY_FRAMES);

        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(WIN_LEN);
        let ifft = planner.plan_fft_inverse(WIN_LEN);
        let fft_scratch = fft.make_scratch_vec();
        let ifft_scratch = ifft.make_scratch_vec();

        Ok(Self {
            model,
            window: vorbis_window(),
            in_buf: [0.0; WIN_LEN],
            ola: [0.0; WIN_LEN],
            fft_in: fft.make_input_vec(),
            spec_in: fft.make_output_vec(),
            spec_out: ifft.make_input_vec(),
            spec_out_vad: ifft.make_input_vec(),
            time_out: ifft.make_output_vec(),
            fft,
            ifft,
            fft_scratch,
            ifft_scratch,
            vad_rnnoise: if vad == Vad::RnNoise {
                Some(RnNoise::new()?)
            } else {
                None
            },
            spectral: if vad == Vad::Spectral {
                Some(SpectralDetector::new())
            } else {
                None
            },
            spec_hist: vec![vec![Complex::new(0.0, 0.0); FREQ_BINS]; k + 1],
            hist_len: 0,
            hist_pos: 0,
            prob_delay: FrameDelay::new(align, 0.0),
            level_delay: FrameDelay::new(align, -100.0),
            delay_samples: FRAME_SIZE * (1 + k),
            attn_alpha: 0.0,
        })
    }
}

impl DpdfNet {
    /// Limit how far the model may turn anything down, as upstream's `attn_limit_db`:
    /// `out = a * noisy + (1 - a) * enhanced` per bin, `a = 10^(-dB/20)`, with the noisy
    /// spectrum taken from the frame the output belongs to (k frames back). `None` or
    /// infinity disables it.
    pub fn set_attn_limit_db(&mut self, db: Option<f64>) {
        self.attn_alpha = match db {
            Some(d) if d.is_finite() && d >= 0.0 => 10f64.powf(-d / 20.0),
            _ => 0.0,
        };
    }
}

impl Denoiser for DpdfNet {
    fn process_frame(&mut self, frame: &mut Frame) -> f64 {
        let prob_now = match self.vad_rnnoise.as_mut() {
            Some(rn) => rn.probability_only(frame),
            None => 0.0,
        };

        // Analysis: causal 20 ms window ending at the newest sample.
        let scale = (1.0f64 / 32767.0) as f32;
        self.in_buf.copy_within(FRAME_SIZE.., 0);
        for (dst, &x) in self.in_buf[FRAME_SIZE..].iter_mut().zip(frame.iter()) {
            *dst = x * scale;
        }
        for i in 0..WIN_LEN {
            self.fft_in[i] = f64::from(self.in_buf[i] * self.window[i]);
        }
        if self
            .fft
            .process_with_scratch(&mut self.fft_in, &mut self.spec_in, &mut self.fft_scratch)
            .is_err()
        {
            return 0.0;
        }
        // Input spectra of the last k + 1 frames: the output of this frame belongs to the
        // oldest of them (the model's internal look-ahead).
        let cap = self.spec_hist.len();
        self.spec_hist[self.hist_pos].copy_from_slice(&self.spec_in);
        self.hist_pos = (self.hist_pos + 1) % cap;
        self.hist_len = (self.hist_len + 1).min(cap);
        let full = self.hist_len == cap;
        let oldest = if full { self.hist_pos } else { 0 };

        for (dst, c) in self
            .model
            .spec_in()
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(&self.spec_in)
        {
            dst[0] = c.re as f32;
            dst[1] = c.im as f32;
        }

        if self.model.step().is_ok() {
            for (c, src) in self
                .spec_out
                .iter_mut()
                .zip(self.model.spec_out().as_chunks::<2>().0)
            {
                *c = Complex::new(f64::from(src[0]), f64::from(src[1]));
            }
        } else {
            // Keep the stream alive and in time: this hop contributes silence, so the
            // overlap-add fades out the previous frame instead of jumping to raw input.
            self.spec_out.fill(Complex::new(0.0, 0.0));
        }
        // A real signal has no imaginary part at DC and Nyquist; drop any the model emits.
        self.spec_out[0].im = 0.0;
        self.spec_out[FREQ_BINS - 1].im = 0.0;
        // The inverse FFT uses its input as scratch, so keep a copy for the detector.
        if self.spectral.is_some() {
            self.spec_out_vad.copy_from_slice(&self.spec_out);
        }
        // Attenuation limit, after the detector's copy so that it still sees what the
        // model kept. Until the history is full the aligned input is silence, as upstream.
        if self.attn_alpha > 0.0 && full {
            let a = self.attn_alpha;
            for (o, n) in self.spec_out.iter_mut().zip(&self.spec_hist[oldest]) {
                *o = *n * a + *o * (1.0 - a);
            }
        } else if self.attn_alpha > 0.0 {
            let a = 1.0 - self.attn_alpha;
            self.spec_out.iter_mut().for_each(|o| *o *= a);
        }
        if self
            .ifft
            .process_with_scratch(
                &mut self.spec_out,
                &mut self.time_out,
                &mut self.ifft_scratch,
            )
            .is_err()
        {
            return 0.0;
        }

        // Synthesis: windowed overlap-add (inverse FFT normalised by 1/n).
        let norm = 1.0 / WIN_LEN as f64;
        for i in 0..WIN_LEN {
            self.ola[i] += (self.time_out[i] * norm) as f32 * self.window[i];
        }
        for (dst, &v) in frame.iter_mut().zip(self.ola[..FRAME_SIZE].iter()) {
            *dst = v * 32767.0;
        }
        self.ola.copy_within(FRAME_SIZE.., 0);
        self.ola[FRAME_SIZE..].fill(0.0);

        match self.spectral.as_mut() {
            // Output frame t holds the input from k frames ago; compare like with like.
            Some(det) => det.update(&self.spec_hist[oldest], &self.spec_out_vad),
            None => self.prob_delay.push(prob_now),
        }
    }

    fn align_level(&mut self, level_db: f64) -> f64 {
        self.level_delay.push(level_db)
    }

    fn delay_samples(&self) -> usize {
        self.delay_samples
    }

    fn reset(&mut self) {
        self.model.reset();
        self.in_buf = [0.0; WIN_LEN];
        self.ola = [0.0; WIN_LEN];
        self.prob_delay.reset();
        self.level_delay.reset();
        self.hist_len = 0;
        self.hist_pos = 0;
        if let Some(d) = self.spectral.as_mut() {
            d.prob = 0.0;
        }
        if let Some(rn) = self.vad_rnnoise.as_mut() {
            rn.reset();
        }
    }

    fn name(&self) -> &'static str {
        "dpdfnet"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_power_complementary() {
        let w = vorbis_window();
        for i in 0..FRAME_SIZE {
            let s = f64::from(w[i]).powi(2) + f64::from(w[i + FRAME_SIZE]).powi(2);
            assert!((s - 1.0).abs() < 1e-6);
        }
    }

    /// Identity model: returns the spectrum unchanged, so the STFT/OLA path must
    /// reconstruct the input delayed by exactly one hop.
    struct Identity {
        spec_in: Vec<f32>,
        spec_out: Vec<f32>,
    }
    impl Identity {
        fn new() -> Self {
            Self {
                spec_in: vec![0.0; FREQ_BINS * 2],
                spec_out: vec![0.0; FREQ_BINS * 2],
            }
        }
    }
    impl SpecModel for Identity {
        fn spec_in(&mut self) -> &mut [f32] {
            &mut self.spec_in
        }
        fn step(&mut self) -> Result<(), Error> {
            self.spec_out.copy_from_slice(&self.spec_in);
            Ok(())
        }
        fn spec_out(&self) -> &[f32] {
            &self.spec_out
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn identity_model_reconstructs_input_one_hop_late() {
        let mut d =
            DpdfNet::with_model(Box::new(Identity::new()), "unknown", Vad::Spectral).unwrap();
        let input: Vec<f32> = (0..FRAME_SIZE * 6)
            .map(|i| ((i as f32) * 0.031).sin() * 10_000.0)
            .collect();
        let mut out = Vec::new();
        for chunk in input.chunks(FRAME_SIZE) {
            let mut f: Frame = chunk.try_into().unwrap();
            d.process_frame(&mut f);
            out.extend_from_slice(&f);
        }
        for i in FRAME_SIZE..input.len() {
            let err = (out[i] - input[i - FRAME_SIZE]).abs();
            assert!(
                err < 0.05,
                "sample {i}: {} vs {}",
                out[i],
                input[i - FRAME_SIZE]
            );
        }
        assert_eq!(d.delay_samples(), FRAME_SIZE);
    }

    #[test]
    fn frame_delay_shifts_by_n() {
        let mut fd = FrameDelay::new(3, -1.0);
        let out: Vec<f64> = (0..5).map(|i| fd.push(i as f64)).collect();
        assert_eq!(out, vec![-1.0, -1.0, -1.0, 0.0, 1.0]);
    }
}
