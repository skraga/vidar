//! Adaptive hysteresis noise gate.
//!
//! Opens on a confident speech probability, or early on a weaker probability plus a
//! clear level rise over the tracked noise floor. Holds open through word endings
//! (hangover and energy sustain), closes to `floor_gain` rather than silence, and
//! ramps every gain change across the frame with a C3 smootherstep so it never clicks.

use crate::stats::{median_in_place, percentile_in_place};
use crate::{Frame, FRAME_SIZE};

const FLOOR_INIT_DB: f64 = -60.0;
const FLOOR_MIN_DB: f64 = -75.0;
const FLOOR_MAX_DB: f64 = -36.0;
const AMBIENT_PROB_MAX: f64 = 0.20;
const FLOOR_FALL_RATE: f64 = 0.15;
const FLOOR_SLOW_RISE_RATE: f64 = 0.02;
const FLOOR_FAST_RISE_RATE: f64 = 0.10;
const FLOOR_RISE_CAP_DB: f64 = 0.5;

const CALIBRATION_TARGET: usize = 50;
const CALIBRATION_TIMEOUT: u32 = 150;
const CALIBRATION_MIN_SAMPLES: usize = 10;

const FLUTTER_BURST_FRAMES: u32 = 15;
const FLUTTER_EVENT_THRESHOLD: usize = 3;
const FLUTTER_WINDOW_FRAMES: u64 = 300;
/// Flutter events can only be added once per close, and closes are at least a hangover
/// apart, so a few dozen slots cover the 3 s window with a wide margin.
const FLUTTER_CAPACITY: usize = 64;

const IDLE_FRAMES: u64 = 1_000_000;
const ANCHOR_CAPACITY: usize = 150;
const ANCHOR_INIT_DB: f64 = -24.0;
const NEARFIELD_MARGIN_DB: f64 = 14.0;
const NEARFIELD_HOLD_FRAMES: u32 = 40;
const SUSTAIN_WINDOW_FRAMES: u32 = 40;

/// Tunable gate settings. Defaults are the tuned values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateParams {
    pub threshold: f64,
    pub close_threshold: f64,
    pub hangover_ms: f64,
    pub decay_ms: f64,
    pub attack_ms: f64,
    pub frame_ms: f64,
    pub onset_threshold: f64,
    pub onset_snr_db: f64,
    pub lookahead: bool,
    pub floor_gain: f64,
    pub cooldown_ms: f64,
    pub flutter_cooldown_ms: f64,
    pub calibrate_startup: bool,
    pub sustain_snr_db: f64,
    pub sustain_threshold: f64,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold: 0.70,
            close_threshold: 0.52,
            hangover_ms: 320.0,
            decay_ms: 80.0,
            attack_ms: 28.0,
            frame_ms: 10.0,
            onset_threshold: 0.35,
            onset_snr_db: 10.0,
            lookahead: false,
            floor_gain: 0.0,
            cooldown_ms: 150.0,
            flutter_cooldown_ms: 300.0,
            calibrate_startup: true,
            sustain_snr_db: 6.0,
            sustain_threshold: 0.25,
        }
    }
}

/// Fixed-capacity FIFO of `f64` that drops the oldest value when full (a `deque(maxlen=N)`).
#[derive(Clone)]
struct Ring<const N: usize> {
    buf: [f64; N],
    start: usize,
    len: usize,
}

impl<const N: usize> Ring<N> {
    fn new() -> Self {
        Self {
            buf: [0.0; N],
            start: 0,
            len: 0,
        }
    }
    fn push(&mut self, v: f64) {
        if self.len < N {
            self.buf[(self.start + self.len) % N] = v;
            self.len += 1;
        } else {
            self.buf[self.start] = v;
            self.start = (self.start + 1) % N;
        }
    }
    fn front(&self) -> Option<f64> {
        (self.len > 0).then(|| self.buf[self.start])
    }
    fn pop_front(&mut self) {
        if self.len > 0 {
            self.start = (self.start + 1) % N;
            self.len -= 1;
        }
    }
    fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }
    fn copy_into<'a>(&self, out: &'a mut [f64; N]) -> &'a mut [f64] {
        for i in 0..self.len {
            out[i] = self.buf[(self.start + i) % N];
        }
        &mut out[..self.len]
    }
}

/// C3 smootherstep table for one 480-sample frame:
/// computed in f64, clipped, cast to f32, then made monotonic.
fn s_curve_table() -> [f32; FRAME_SIZE] {
    let mut table = [0.0f32; FRAME_SIZE];
    let last = (FRAME_SIZE - 1) as f64;
    let mut running = f32::MIN;
    for (i, slot) in table.iter_mut().enumerate() {
        // np.linspace(0, 1, 480): i * step, with the final point pinned to exactly 1.0.
        let t = if i == FRAME_SIZE - 1 {
            1.0
        } else {
            i as f64 * (1.0 / last)
        };
        let s = t.powi(4) * (35.0 + t * (-84.0 + t * (70.0 - 20.0 * t)));
        let v = s.clamp(0.0, 1.0) as f32;
        running = running.max(v);
        *slot = running;
    }
    table
}

#[derive(Clone)]
pub struct AdaptiveGate {
    // Settings (threshold and close_threshold can change live).
    pub threshold: f64,
    pub close_threshold: f64,
    hangover_frames: u32,
    decay_rate: f64,
    attack_rate: f64,
    onset_threshold: f64,
    onset_snr_db: f64,
    sustain_snr_db: f64,
    sustain_threshold: f64,
    lookahead: bool,
    floor_gain: f64,
    cooldown_frames: u64,
    max_cooldown_frames: u64,
    calibrate_startup: bool,

    // State.
    noise_floor_db: f64,
    frames_since_close: u64,
    is_open: bool,
    current_gain: f64,
    frames_since_speech: u32,
    delay: Frame,

    frame_idx: u64,
    burst_voiced: u32,
    frames_since_voiced: u32,
    flutter_events: Ring<FLUTTER_CAPACITY>,

    calibrated: bool,
    cal_elapsed: u32,
    cal_buffer: [f64; CALIBRATION_TARGET],
    cal_len: usize,

    user_anchor_db: f64,
    anchor: Ring<ANCHOR_CAPACITY>,
    frames_since_nearfield: u32,

    // Scratch space and tables, so `process` never allocates.
    anchor_scratch: [f64; ANCHOR_CAPACITY],
    s_curve: [f32; FRAME_SIZE],
}

impl AdaptiveGate {
    pub fn new(p: GateParams) -> Self {
        let hangover_frames = ((p.hangover_ms / p.frame_ms) as u32).max(1);
        let cooldown_frames = ((p.cooldown_ms / p.frame_ms) as u64).max(1);
        Self {
            threshold: p.threshold,
            close_threshold: p.close_threshold,
            hangover_frames,
            decay_rate: (-(p.frame_ms / p.decay_ms)).exp(),
            attack_rate: 1.0 - (-(p.frame_ms / p.attack_ms)).exp(),
            onset_threshold: p.onset_threshold,
            onset_snr_db: p.onset_snr_db,
            sustain_snr_db: p.sustain_snr_db,
            sustain_threshold: p.sustain_threshold,
            lookahead: p.lookahead,
            floor_gain: p.floor_gain,
            cooldown_frames,
            max_cooldown_frames: cooldown_frames.max((p.flutter_cooldown_ms / p.frame_ms) as u64),
            calibrate_startup: p.calibrate_startup,

            noise_floor_db: FLOOR_INIT_DB,
            frames_since_close: IDLE_FRAMES,
            is_open: false,
            current_gain: p.floor_gain,
            frames_since_speech: hangover_frames + 10,
            delay: [0.0; FRAME_SIZE],

            frame_idx: 0,
            burst_voiced: 0,
            frames_since_voiced: 1000,
            flutter_events: Ring::new(),

            calibrated: !p.calibrate_startup,
            cal_elapsed: 0,
            cal_buffer: [0.0; CALIBRATION_TARGET],
            cal_len: 0,

            user_anchor_db: ANCHOR_INIT_DB,
            anchor: Ring::new(),
            frames_since_nearfield: 1000,

            anchor_scratch: [0.0; ANCHOR_CAPACITY],
            s_curve: s_curve_table(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }
    pub fn current_gain(&self) -> f64 {
        self.current_gain
    }
    pub fn noise_floor_db(&self) -> f64 {
        self.noise_floor_db
    }
    pub fn user_anchor_db(&self) -> f64 {
        self.user_anchor_db
    }

    /// Return to a clean closed state (on mute). Keeps the learned noise floor,
    /// zeroes the lookahead buffer and clears flutter history.
    pub fn reset(&mut self) {
        self.is_open = false;
        self.current_gain = self.floor_gain;
        self.frames_since_speech = self.hangover_frames + 10;
        self.frames_since_close = IDLE_FRAMES;
        self.burst_voiced = 0;
        self.frames_since_voiced = 1000;
        self.frames_since_nearfield = 1000;
        self.user_anchor_db = ANCHOR_INIT_DB;
        self.anchor.clear();
        self.flutter_events.clear();
        self.delay = [0.0; FRAME_SIZE];
    }

    /// Restart the startup ambient profiler (stream start, device change).
    pub fn recalibrate(&mut self) {
        self.noise_floor_db = FLOOR_INIT_DB;
        self.cal_elapsed = 0;
        self.cal_len = 0;
        self.calibrated = !self.calibrate_startup;
        self.user_anchor_db = ANCHOR_INIT_DB;
        self.anchor.clear();
        self.frames_since_nearfield = 1000;
    }

    fn current_cooldown_frames(&mut self) -> u64 {
        while let Some(front) = self.flutter_events.front() {
            if self.frame_idx - front as u64 > FLUTTER_WINDOW_FRAMES {
                self.flutter_events.pop_front();
            } else {
                break;
            }
        }
        if self.flutter_events.len >= FLUTTER_EVENT_THRESHOLD {
            self.max_cooldown_frames
        } else {
            self.cooldown_frames
        }
    }

    fn finish_calibration(&mut self, value_db: f64) {
        self.noise_floor_db = value_db.clamp(FLOOR_MIN_DB, FLOOR_MAX_DB);
        self.calibrated = true;
        self.cal_len = 0;
    }

    fn calibrate_step(&mut self, input_rms_db: Option<f64>, speech_prob: f64) {
        self.cal_elapsed += 1;
        if let Some(db) = input_rms_db {
            if db > -100.0
                && db < -20.0
                && speech_prob < AMBIENT_PROB_MAX
                && self.cal_len < CALIBRATION_TARGET
            {
                self.cal_buffer[self.cal_len] = db;
                self.cal_len += 1;
            }
        }
        if self.cal_len >= CALIBRATION_TARGET {
            let n = self.cal_len;
            let v = median_in_place(&mut self.cal_buffer[..n]);
            self.finish_calibration(v);
        } else if self.cal_elapsed >= CALIBRATION_TIMEOUT {
            if self.cal_len >= CALIBRATION_MIN_SAMPLES {
                let n = self.cal_len;
                let v = percentile_in_place(&mut self.cal_buffer[..n], 25.0);
                self.finish_calibration(v);
            } else {
                // Too few clean ambient frames: keep the default seed.
                self.calibrated = true;
                self.cal_len = 0;
            }
        }
    }

    fn track_floor(&mut self, input_rms_db: Option<f64>, speech_prob: f64, in_cooldown: bool) {
        let Some(db) = input_rms_db else { return };
        if !(db > -100.0 && db < -20.0) {
            return;
        }
        let diff = db - self.noise_floor_db;
        if diff < 0.0 {
            self.noise_floor_db = FLOOR_MIN_DB.max(self.noise_floor_db + diff * FLOOR_FALL_RATE);
        } else {
            let rate = if in_cooldown && speech_prob < AMBIENT_PROB_MAX {
                FLOOR_FAST_RISE_RATE
            } else {
                FLOOR_SLOW_RISE_RATE
            };
            self.noise_floor_db += (diff * rate).min(FLOOR_RISE_CAP_DB);
        }
    }

    /// Gate one frame in place. Returns the gain applied at the end of the frame.
    ///
    /// With lookahead on, the frame written back is the *previous* input frame, so the
    /// gate decision made on this frame leads the audio by 10 ms.
    pub fn process(
        &mut self,
        frame: &mut Frame,
        speech_prob: f64,
        input_rms_db: Option<f64>,
    ) -> f64 {
        self.frame_idx += 1;
        if !self.calibrated {
            self.calibrate_step(input_rms_db, speech_prob);
        }

        if self.lookahead {
            std::mem::swap(frame, &mut self.delay);
        }
        // From here `frame` holds the audio to gate.

        let start_gain = self.current_gain;

        // 1. Track the near-field user's voiced level.
        if let Some(db) = input_rms_db {
            if speech_prob >= 0.80 && db - self.noise_floor_db >= 15.0 {
                self.anchor.push(db);
                if self.anchor.len >= 10 {
                    let values = self.anchor.copy_into(&mut self.anchor_scratch);
                    self.user_anchor_db = percentile_in_place(values, 90.0).clamp(-36.0, -12.0);
                }
            }
        }

        // 2. Near-field qualification.
        let is_nearfield = match input_rms_db {
            None => true,
            Some(db) => db >= self.user_anchor_db - NEARFIELD_MARGIN_DB,
        };
        if is_nearfield && speech_prob >= self.close_threshold {
            self.frames_since_nearfield = 0;
        } else {
            self.frames_since_nearfield = self.frames_since_nearfield.saturating_add(1);
        }

        let target_gain;
        if !self.is_open {
            self.frames_since_close = self.frames_since_close.saturating_add(1);
            let in_cooldown = self.frames_since_close <= self.current_cooldown_frames();

            if self.calibrated {
                self.track_floor(input_rms_db, speech_prob, in_cooldown);
            }

            let mut is_onset = false;
            if let Some(db) = input_rms_db {
                let snr = db - self.noise_floor_db;
                let can_trigger_onset =
                    !in_cooldown || (snr >= self.onset_snr_db + 8.0 && speech_prob >= 0.55);
                if can_trigger_onset
                    && snr >= self.onset_snr_db
                    && speech_prob >= self.onset_threshold
                {
                    is_onset = true;
                }
            }

            let is_qualified = is_nearfield
                || self.frames_since_nearfield <= NEARFIELD_HOLD_FRAMES
                || (self.anchor.len < 10 && input_rms_db.is_none_or(|db| db >= -38.0));

            if (speech_prob >= self.threshold || is_onset) && is_qualified {
                self.is_open = true;
                self.frames_since_speech = 0;
                let voiced = speech_prob >= self.close_threshold;
                self.frames_since_voiced = if voiced { 0 } else { 1000 };
                self.burst_voiced = u32::from(voiced);
                target_gain = 1.0;
            } else {
                target_gain = self.floor_gain;
            }
        } else {
            let voiced = speech_prob >= self.close_threshold;
            self.frames_since_voiced = if voiced {
                0
            } else {
                self.frames_since_voiced.saturating_add(1)
            };
            let mut is_sustained = voiced;
            if !voiced && self.frames_since_voiced <= SUSTAIN_WINDOW_FRAMES {
                if let Some(db) = input_rms_db {
                    if db - self.noise_floor_db >= self.sustain_snr_db
                        && speech_prob >= self.sustain_threshold
                    {
                        is_sustained = true;
                    }
                }
            }

            if is_sustained {
                self.frames_since_speech = 0;
                if voiced {
                    self.burst_voiced += 1;
                }
                if is_nearfield || self.frames_since_nearfield <= NEARFIELD_HOLD_FRAMES {
                    target_gain = 1.0;
                } else {
                    // Distant chatter during pauses: expand downward instead of passing it.
                    let exp_db = match input_rms_db {
                        Some(db) => db - (self.user_anchor_db - NEARFIELD_MARGIN_DB),
                        None => -14.0,
                    };
                    target_gain =
                        0.15f64.min(self.floor_gain.max(0.15 * 10f64.powf(exp_db / 20.0)));
                }
            } else {
                self.frames_since_speech += 1;
                if self.frames_since_speech <= self.hangover_frames {
                    target_gain = 1.0;
                } else {
                    self.is_open = false;
                    self.frames_since_close = 0;
                    if self.burst_voiced < FLUTTER_BURST_FRAMES {
                        self.flutter_events.push(self.frame_idx as f64);
                    }
                    target_gain = self.floor_gain;
                }
            }
        }

        // Exponential attack / decay.
        if target_gain > self.current_gain {
            self.current_gain += (target_gain - self.current_gain) * self.attack_rate;
            if self.current_gain >= 0.999 {
                self.current_gain = 1.0;
            }
        } else if target_gain < self.current_gain {
            self.current_gain =
                self.floor_gain + (self.current_gain - self.floor_gain) * self.decay_rate;
            if self.current_gain <= self.floor_gain + 0.001 {
                self.current_gain = self.floor_gain;
            }
        }
        let end_gain = self.current_gain;

        // Steady silence.
        if end_gain <= self.floor_gain + 0.0001 && start_gain <= self.floor_gain + 0.0001 {
            if self.floor_gain > 0.0 {
                let g = self.floor_gain as f32;
                frame.iter_mut().for_each(|x| *x *= g);
                return self.floor_gain;
            }
            frame.fill(0.0);
            return 0.0;
        }

        // Steady speech.
        if end_gain >= 0.999 && start_gain >= 0.999 {
            return 1.0;
        }

        // Transition: smootherstep ramp across the frame (computed in f32).
        if (end_gain - start_gain).abs() > 1e-5 {
            let base = start_gain as f32;
            let span = (end_gain - start_gain) as f32;
            for (x, s) in frame.iter_mut().zip(self.s_curve.iter()) {
                *x *= base + span * *s;
            }
        } else {
            let g = end_gain as f32;
            frame.iter_mut().for_each(|x| *x *= g);
        }
        end_gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: f32) -> Frame {
        [v; FRAME_SIZE]
    }

    fn engine_gate() -> AdaptiveGate {
        AdaptiveGate::new(GateParams {
            lookahead: true,
            floor_gain: 0.06,
            ..GateParams::default()
        })
    }

    #[test]
    fn s_curve_runs_zero_to_one_monotonic() {
        let t = s_curve_table();
        assert_eq!(t[0], 0.0);
        assert_eq!(t[FRAME_SIZE - 1], 1.0);
        assert!(t.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn closed_gate_applies_floor_gain() {
        let mut g = engine_gate();
        let mut f = frame(0.5);
        for _ in 0..3 {
            f = frame(0.5);
            g.process(&mut f, 0.0, Some(-60.0));
        }
        assert!((f[0] - 0.5 * 0.06).abs() < 1e-7);
        assert!(!g.is_open());
    }

    #[test]
    fn opens_on_speech_and_holds_through_hangover() {
        let mut g = engine_gate();
        for _ in 0..60 {
            g.process(&mut frame(0.01), 0.05, Some(-60.0));
        }
        g.process(&mut frame(0.3), 0.95, Some(-20.0));
        assert!(g.is_open());
        // 320 ms hangover = 32 frames of silence before it closes.
        for i in 0..32 {
            g.process(&mut frame(0.01), 0.0, Some(-60.0));
            assert!(g.is_open(), "closed early at frame {i}");
        }
        g.process(&mut frame(0.01), 0.0, Some(-60.0));
        assert!(!g.is_open());
    }

    #[test]
    fn reaches_unity_gain_during_speech() {
        let mut g = engine_gate();
        let mut last = 0.0;
        for _ in 0..40 {
            last = g.process(&mut frame(0.3), 0.95, Some(-20.0));
        }
        assert_eq!(last, 1.0);
        let mut f = frame(0.3);
        g.process(&mut f, 0.95, Some(-20.0));
        assert_eq!(f[0], 0.3);
    }

    #[test]
    fn lookahead_delays_audio_by_one_frame() {
        let mut g = AdaptiveGate::new(GateParams {
            lookahead: true,
            floor_gain: 1.0,
            ..GateParams::default()
        });
        let mut a = frame(0.25);
        g.process(&mut a, 0.0, None);
        assert_eq!(a[0], 0.0, "first output is the empty delay line");
        let mut b = frame(0.5);
        g.process(&mut b, 0.0, None);
        assert_eq!(b[0], 0.25);
    }

    #[test]
    fn startup_calibration_learns_ambient_floor() {
        let mut g = engine_gate();
        for _ in 0..CALIBRATION_TARGET {
            g.process(&mut frame(0.001), 0.05, Some(-50.0));
        }
        assert!((g.noise_floor_db() - (-50.0)).abs() < 1e-9);
    }

    #[test]
    fn calibration_falls_back_to_seed_without_ambient_frames() {
        let mut g = engine_gate();
        for _ in 0..CALIBRATION_TIMEOUT {
            g.process(&mut frame(0.3), 0.9, Some(-10.0));
        }
        assert_eq!(g.noise_floor_db(), FLOOR_INIT_DB);
    }

    #[test]
    fn reset_closes_and_clears_lookahead() {
        let mut g = engine_gate();
        for _ in 0..5 {
            g.process(&mut frame(0.3), 0.95, Some(-20.0));
        }
        g.reset();
        assert!(!g.is_open());
        assert_eq!(g.current_gain(), 0.06);
        let mut f = frame(0.3);
        g.process(&mut f, 0.0, Some(-60.0));
        assert_eq!(f[0], 0.0);
    }

    #[test]
    fn ring_drops_oldest_when_full() {
        let mut r: Ring<3> = Ring::new();
        for v in 1..=5 {
            r.push(v as f64);
        }
        let mut out = [0.0; 3];
        assert_eq!(r.copy_into(&mut out), &[3.0, 4.0, 5.0]);
    }
}
