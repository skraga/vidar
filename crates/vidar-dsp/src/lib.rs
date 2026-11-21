//! Vidar signal chain: high-pass, adaptive gate, transient suppressor, leveler, equaliser,
//! Studio sound (compressor, de-esser, saturation), limiter.
//!
//! Sample buffers and gain ramps are `f32`; filter, gate and leveler state and levels are
//! `f64`. The reference vectors in `tests/golden` pin the exact numeric behaviour.
//!
//! Nothing here allocates after construction, so every `process` call is safe on a
//! real-time audio thread.

mod autotone;
mod bands;
mod chain;
mod denormals;
mod eq;
mod gate;
mod hpf;
mod leveler;
mod levels;
mod output;
mod pop;
mod stats;
mod studio;
mod transient;

pub use autotone::AutoTone;
pub use bands::{VoiceBands, BANDS, BAND_CENTRES_HZ};
pub use chain::{process_mono_frame, FrameStats};
pub use denormals::flush_denormals;
pub use eq::{eq_preset, VoiceEq, EQ_BANDS, EQ_FREQS_HZ, EQ_MAX_DB, EQ_POINTS_HZ, EQ_PRESETS};
pub use gate::{AdaptiveGate, GateParams};
pub use hpf::HighPass;
pub use leveler::SpeechLeveler;
pub use levels::{levels, ramp_gain, soft_limit, soft_preclip, Levels};
pub use output::{Limiter, OutputStage, LIMIT};
pub use pop::PopFilter;
pub use studio::Studio;
pub use transient::TransientSuppressor;

/// 10 ms at 48 kHz. Every block in the chain works on frames of this size.
pub const FRAME_SIZE: usize = 480;
pub const SAMPLE_RATE: u32 = 48_000;

/// One mono frame of audio in the -1.0..=1.0 range.
pub type Frame = [f32; FRAME_SIZE];

/// Scale between the chain's -1..1 floats and the int16-range floats neural models take.
pub const PCM_SCALE: f32 = 32767.0;

/// A neural noise suppressor (RNNoise, DPDFNet, ...).
///
/// Frames are passed in place, scaled to the int16 range (`x * 32767`), the convention
/// RNNoise uses. The return value is the model's speech probability for the frame that
/// comes *out*, in 0..=1.
pub trait Denoiser: Send {
    fn process_frame(&mut self, frame: &mut Frame) -> f64;

    /// Delay a per-frame input level so it lines up with the audio this model outputs.
    /// Models with the same lag as RNNoise return the level unchanged.
    fn align_level(&mut self, level_db: f64) -> f64 {
        level_db
    }

    /// Total input-to-output delay in samples (used to trim offline output).
    fn delay_samples(&self) -> usize;

    /// Clear internal state (stream restart, device change).
    fn reset(&mut self);

    /// Stable identifier: `"rnnoise"` or `"dpdfnet"`.
    fn name(&self) -> &'static str;
}
