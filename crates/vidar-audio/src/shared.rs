//! State shared between the UI and the audio threads, all lock-free.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

/// An `f64` in an `AtomicU64`.
#[derive(Debug, Default)]
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Relaxed))
    }
    pub fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Relaxed)
    }
}

/// Settings the UI can change while running. Read once per 10 ms frame.
#[derive(Debug)]
pub struct Controls {
    pub muted: AtomicBool,
    /// No app is recording from the virtual microphone: output silence and skip the model,
    /// like `muted`, but without the user having muted anything.
    pub idle: AtomicBool,
    pub denoise: AtomicBool,
    pub high_pass: AtomicBool,
    pub auto_level: AtomicBool,
    pub vad_threshold: AtomicF64,
    pub vad_close_threshold: AtomicF64,
    pub total_gain: AtomicF64,
    pub mic_boost_db: AtomicF64,
    /// Equaliser band gains in dB, low to high.
    pub eq_db: [AtomicF64; vidar_dsp::EQ_BANDS],
}

impl Controls {
    pub fn from_settings(s: &vidar_config::EngineSettings) -> Self {
        Self {
            muted: AtomicBool::new(false),
            idle: AtomicBool::new(false),
            denoise: AtomicBool::new(s.denoise_enabled),
            high_pass: AtomicBool::new(s.high_pass),
            auto_level: AtomicBool::new(s.auto_level),
            vad_threshold: AtomicF64::new(s.gate.threshold),
            vad_close_threshold: AtomicF64::new(s.gate.close_threshold),
            total_gain: AtomicF64::new(s.total_gain),
            mic_boost_db: AtomicF64::new(s.mic_boost_db),
            eq_db: std::array::from_fn(|i| AtomicF64::new(s.eq_db[i])),
        }
    }

    pub fn set_eq(&self, gains_db: [f64; vidar_dsp::EQ_BANDS]) {
        for (a, g) in self.eq_db.iter().zip(gains_db) {
            a.set(g);
        }
    }

    pub fn eq(&self) -> [f64; vidar_dsp::EQ_BANDS] {
        std::array::from_fn(|i| self.eq_db[i].get())
    }

    /// Sensitivity slider: sets the open threshold and the matching close threshold.
    pub fn set_vad_threshold(&self, threshold: f64) {
        self.vad_threshold.set(threshold);
        self.vad_close_threshold
            .set(vidar_config::close_threshold_for(threshold));
    }
}

/// A frame that takes this share of its 10 ms leaves too little room for the rest of the
/// system: counted in [`Meters::slow_frames`].
pub const SLOW_FRAME: f64 = 0.7;

/// Live measurements for meters, the tray icon and the watchdog.
#[derive(Debug, Default)]
pub struct Meters {
    pub speech_prob: AtomicU32,
    pub out_peak_db: AtomicU32,
    pub out_rms_db: AtomicU32,
    pub in_peak_db: AtomicU32,
    pub gate_gain: AtomicU32,
    /// Output level per voice band (dBFS, low to high), for the live logo.
    pub bands: [AtomicU32; vidar_dsp::BANDS],
    /// Frames processed since start.
    pub frames: AtomicU64,
    /// Processing time per frame as a share of its 10 ms (f32 bits), smoothed over about
    /// half a second. Above 1 the processor cannot keep up.
    pub load: AtomicU32,
    /// Frames whose processing took more than [`SLOW_FRAME`] of their 10 ms.
    pub slow_frames: AtomicU64,
    /// Capture packets Windows flagged as discontinuous (data lost before we read it).
    pub capture_glitches: AtomicU64,
    /// Render callbacks that found the buffer empty.
    pub underruns: AtomicU64,
    /// Processed frames dropped because the render side did not keep up.
    pub overruns: AtomicU64,
    pub drift_ppm: AtomicI64,
    /// `Instant`-style milliseconds since engine start of the last capture packet.
    pub last_capture_ms: AtomicU64,
    pub errors: AtomicU64,
    /// Set by a stream thread when its device disappeared.
    pub device_lost: AtomicBool,
    /// Set when a stream thread stopped for any reason.
    pub stopped: AtomicBool,
}

fn f32_set(a: &AtomicU32, v: f64) {
    a.store((v as f32).to_bits(), Relaxed)
}
fn f32_get(a: &AtomicU32) -> f64 {
    f64::from(f32::from_bits(a.load(Relaxed)))
}

/// A consistent-enough copy of the meters for display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterSnapshot {
    pub speech_prob: f64,
    pub out_peak_db: f64,
    pub out_rms_db: f64,
    pub in_peak_db: f64,
    pub gate_gain: f64,
    pub bands: [f64; vidar_dsp::BANDS],
    pub frames: u64,
    /// See [`Meters::load`].
    pub load: f64,
    pub slow_frames: u64,
    pub capture_glitches: u64,
    pub underruns: u64,
    pub overruns: u64,
    pub drift_ppm: i64,
    pub errors: u64,
}

impl Meters {
    pub fn new() -> Self {
        let m = Self::default();
        for a in [&m.out_peak_db, &m.out_rms_db, &m.in_peak_db]
            .into_iter()
            .chain(&m.bands)
        {
            f32_set(a, -100.0);
        }
        m
    }

    pub(crate) fn publish(
        &self,
        speech_prob: f64,
        out_peak_db: f64,
        out_rms_db: f64,
        in_peak_db: f64,
        gate_gain: f64,
    ) {
        f32_set(&self.speech_prob, speech_prob);
        f32_set(&self.out_peak_db, out_peak_db);
        f32_set(&self.out_rms_db, out_rms_db);
        f32_set(&self.in_peak_db, in_peak_db);
        f32_set(&self.gate_gain, gate_gain);
        self.frames.fetch_add(1, Relaxed);
    }

    /// Record how long one frame took, as a share of its 10 ms.
    pub(crate) fn publish_load(&self, share: f64) {
        let smoothed = f32_get(&self.load);
        // About half a second (50 frames) of smoothing.
        f32_set(&self.load, smoothed + (share - smoothed) * 0.02);
        if share > SLOW_FRAME {
            self.slow_frames.fetch_add(1, Relaxed);
        }
    }

    pub(crate) fn publish_bands(&self, bands: &[f64; vidar_dsp::BANDS]) {
        for (a, &db) in self.bands.iter().zip(bands) {
            f32_set(a, db);
        }
    }

    /// Just the band levels, for drawing at the window's frame rate.
    pub fn bands(&self) -> [f64; vidar_dsp::BANDS] {
        std::array::from_fn(|i| f32_get(&self.bands[i]))
    }

    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            speech_prob: f32_get(&self.speech_prob),
            out_peak_db: f32_get(&self.out_peak_db),
            out_rms_db: f32_get(&self.out_rms_db),
            in_peak_db: f32_get(&self.in_peak_db),
            gate_gain: f32_get(&self.gate_gain),
            bands: self.bands(),
            frames: self.frames.load(Relaxed),
            load: f32_get(&self.load),
            slow_frames: self.slow_frames.load(Relaxed),
            capture_glitches: self.capture_glitches.load(Relaxed),
            underruns: self.underruns.load(Relaxed),
            overruns: self.overruns.load(Relaxed),
            drift_ppm: self.drift_ppm.load(Relaxed),
            errors: self.errors.load(Relaxed),
        }
    }
}
