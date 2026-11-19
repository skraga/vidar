//! Capture-side processing: interleaved packets in, processed mono frames out.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use vidar_config::EngineSettings;
use vidar_dsp::{
    levels, process_mono_frame, AdaptiveGate, Denoiser, Frame, HighPass, SpeechLeveler,
    TransientSuppressor, VoiceBands, VoiceEq, BANDS, FRAME_SIZE,
};

use crate::shared::{Controls, Meters};

/// A model swap: `Some(model)` to switch, `None` to run without one.
pub type ModelSwap = Option<Box<dyn Denoiser>>;

pub struct Processor {
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    hpf: HighPass,
    gate: AdaptiveGate,
    leveler: SpeechLeveler,
    transient: TransientSuppressor,
    eq: VoiceEq,
    bands: VoiceBands,
    model: Option<Box<dyn Denoiser>>,
    model_rx: Receiver<ModelSwap>,
    retire_tx: SyncSender<Box<dyn Denoiser>>,
    frame: Frame,
    filled: usize,
    was_muted: bool,
    /// Muted frames left in which the model still runs on silence (see `process_frame`).
    mute_flush: usize,
}

/// Longer than the slowest model's delay (DPDFNet, 50 ms = 5 frames), with margin.
const MUTE_FLUSH_FRAMES: usize = 8;

impl Processor {
    pub fn new(
        settings: &EngineSettings,
        model: Option<Box<dyn Denoiser>>,
        controls: Arc<Controls>,
        meters: Arc<Meters>,
        model_rx: Receiver<ModelSwap>,
        retire_tx: SyncSender<Box<dyn Denoiser>>,
    ) -> Self {
        let mut gate = AdaptiveGate::new(settings.gate);
        gate.recalibrate();
        Self {
            controls,
            meters,
            hpf: HighPass::new(settings.hpf_cutoff_hz, 48_000.0),
            gate,
            leveler: SpeechLeveler::default(),
            transient: TransientSuppressor::default(),
            eq: {
                let mut eq = VoiceEq::new(48_000.0);
                eq.set_gains(settings.eq_db);
                eq
            },
            bands: VoiceBands::default(),
            model,
            model_rx,
            retire_tx,
            frame: [0.0; FRAME_SIZE],
            filled: 0,
            was_muted: false,
            mute_flush: 0,
        }
    }

    /// Take interleaved `channels`-channel samples; call `emit` with each finished 480-sample
    /// mono frame. Stereo is averaged; more channels use the first two.
    pub fn push_interleaved(
        &mut self,
        data: &[f32],
        channels: usize,
        emit: &mut dyn FnMut(&Frame),
    ) {
        let ch = channels.max(1);
        for chunk in data.chunks_exact(ch) {
            let s = if ch >= 2 {
                (chunk[0] + chunk[1]) * 0.5
            } else {
                chunk[0]
            };
            self.frame[self.filled] = s;
            self.filled += 1;
            if self.filled == FRAME_SIZE {
                self.filled = 0;
                self.process_frame();
                emit(&self.frame);
            }
        }
    }

    /// Feed `n` frames of silence (Windows flagged the packet silent).
    pub fn push_silence(&mut self, n: usize, emit: &mut dyn FnMut(&Frame)) {
        for _ in 0..n {
            self.frame[self.filled] = 0.0;
            self.filled += 1;
            if self.filled == FRAME_SIZE {
                self.filled = 0;
                self.process_frame();
                emit(&self.frame);
            }
        }
    }

    fn apply_pending_swaps(&mut self) {
        while let Ok(next) = self.model_rx.try_recv() {
            // Swapped-in models are freshly created, so no reset (RNNoise's would allocate).
            let old = std::mem::replace(&mut self.model, next);
            if let Some(old) = old {
                // Free the old model off the audio thread; if nobody collects, drop here.
                if let Err(TrySendError::Full(old) | TrySendError::Disconnected(old)) =
                    self.retire_tx.try_send(old)
                {
                    drop(old);
                }
            }
        }
    }

    fn process_frame(&mut self) {
        self.apply_pending_swaps();
        let c = &self.controls;

        if c.muted.load(Relaxed) {
            if !self.was_muted {
                self.gate.reset();
                self.hpf.reset();
                self.eq.reset();
                self.bands.reset();
                self.was_muted = true;
                self.mute_flush = MUTE_FLUSH_FRAMES;
            }
            self.frame.fill(0.0);
            // Run the model on silence for a few frames so its delay line holds no speech from
            // before the mute; otherwise unmuting would replay it. (No reset: RNNoise's allocates.)
            if self.mute_flush > 0 {
                self.mute_flush -= 1;
                if let Some(m) = self.model.as_mut() {
                    m.process_frame(&mut self.frame);
                }
                self.frame.fill(0.0);
            }
            self.meters.publish(0.0, -100.0, -100.0, -100.0, 0.0);
            self.meters.publish_bands(&[-100.0; BANDS]);
            return;
        }
        self.was_muted = false;

        self.gate.threshold = c.vad_threshold.get();
        self.gate.close_threshold = c.vad_close_threshold.get();

        let model: Option<&mut dyn Denoiser> = if c.denoise.load(Relaxed) {
            match self.model.as_mut() {
                Some(m) => Some(m.as_mut()),
                None => None,
            }
        } else {
            None
        };
        let hpf = if c.high_pass.load(Relaxed) {
            Some(&mut self.hpf)
        } else {
            None
        };
        self.eq.set_gains(c.eq());
        let eq = if self.eq.is_flat() {
            None
        } else {
            Some(&mut self.eq)
        };
        let leveler = if c.auto_level.load(Relaxed) {
            Some(&mut self.leveler)
        } else {
            None
        };

        let stats = process_mono_frame(
            &mut self.frame,
            model,
            hpf,
            &mut self.gate,
            c.total_gain.get(),
            leveler,
            Some(&mut self.transient),
            c.mic_boost_db.get(),
            eq,
        );
        for x in self.frame.iter_mut() {
            *x = x.clamp(-1.0, 1.0);
        }
        let out = levels(&self.frame);
        self.meters.publish(
            stats.speech_prob,
            out.peak_db,
            out.rms_db,
            stats.input_peak_db,
            stats.gate_gain,
        );
        // What apps hear, split into bands, for the live logo.
        self.meters.publish_bands(&self.bands.analyze(&self.frame));
    }

    pub fn active_model(&self) -> Option<&'static str> {
        self.model.as_ref().map(|m| m.name())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, sync_channel};

    use super::*;
    use vidar_config::Config;

    struct Halve {
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Denoiser for Halve {
        fn process_frame(&mut self, f: &mut Frame) -> f64 {
            f.iter_mut().for_each(|x| *x *= 0.5);
            0.99
        }
        fn delay_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "halve"
        }
    }
    impl Drop for Halve {
        fn drop(&mut self) {
            self.dropped.store(true, Relaxed);
        }
    }

    type Harness = (
        Processor,
        Arc<Controls>,
        Arc<Meters>,
        std::sync::mpsc::Sender<ModelSwap>,
        Receiver<Box<dyn Denoiser>>,
    );

    fn setup(model: Option<Box<dyn Denoiser>>) -> Harness {
        let settings = EngineSettings::from_config(&Config::default(), false);
        let controls = Arc::new(Controls::from_settings(&settings));
        let meters = Arc::new(Meters::new());
        let (tx, rx) = channel();
        let (rtx, rrx) = sync_channel(4);
        (
            Processor::new(&settings, model, controls.clone(), meters.clone(), rx, rtx),
            controls,
            meters,
            tx,
            rrx,
        )
    }

    #[test]
    fn odd_packet_sizes_produce_whole_frames() {
        let (mut p, _, meters, _, _) = setup(None);
        let mut frames = 0;
        let data = vec![0.1f32; 2 * 1000];
        for chunk in data.chunks(2 * 333) {
            p.push_interleaved(chunk, 2, &mut |_| frames += 1);
        }
        assert_eq!(frames, 1000 / FRAME_SIZE);
        assert_eq!(meters.snapshot().frames, 2);
    }

    #[test]
    fn stereo_is_averaged() {
        let (mut p, controls, _, _, _) = setup(None);
        controls.high_pass.store(false, Relaxed);
        let mut seen = Vec::new();
        // Run long enough for the gate to open on a loud signal, then check the level.
        for _ in 0..100 {
            let data: Vec<f32> = (0..FRAME_SIZE).flat_map(|_| [0.6f32, 0.0]).collect();
            p.push_interleaved(&data, 2, &mut |f| seen.push(f[100]));
        }
        let last = *seen.last().unwrap();
        assert!(
            (last - 0.3 * 1.08).abs() < 0.01,
            "expected (0.6+0)/2 x 1.08 gain, got {last}"
        );
    }

    #[test]
    fn mute_outputs_silence() {
        let (mut p, controls, _, _, _) = setup(None);
        controls.muted.store(true, Relaxed);
        let mut out = Vec::new();
        p.push_interleaved(&vec![0.5f32; FRAME_SIZE], 1, &mut |f| {
            out.extend_from_slice(f)
        });
        assert!(out.iter().all(|&x| x == 0.0));
    }

    /// Counts the frames it sees and whether any of them held sound.
    struct Spy {
        frames: Arc<std::sync::atomic::AtomicUsize>,
        heard_sound: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Denoiser for Spy {
        fn process_frame(&mut self, f: &mut Frame) -> f64 {
            self.frames.fetch_add(1, Relaxed);
            if f.iter().any(|&x| x != 0.0) {
                self.heard_sound.store(true, Relaxed);
            }
            0.0
        }
        fn delay_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "spy"
        }
    }

    #[test]
    fn mute_flushes_the_model_with_silence_then_stops_running_it() {
        let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let heard_sound = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (mut p, controls, _, _, _) = setup(Some(Box::new(Spy {
            frames: frames.clone(),
            heard_sound: heard_sound.clone(),
        })));
        controls.muted.store(true, Relaxed);
        for _ in 0..20 {
            p.push_interleaved(&vec![0.5f32; FRAME_SIZE], 1, &mut |_| {});
        }
        assert_eq!(frames.load(Relaxed), MUTE_FLUSH_FRAMES);
        assert!(
            !heard_sound.load(Relaxed),
            "muted input must not reach the model"
        );
    }

    #[test]
    fn model_swap_applies_and_old_model_is_retired() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (mut p, _, _, tx, retired) = setup(Some(Box::new(Halve {
            dropped: dropped.clone(),
        })));
        assert_eq!(p.active_model(), Some("halve"));
        tx.send(None).unwrap();
        p.push_interleaved(&vec![0.0f32; FRAME_SIZE], 1, &mut |_| {});
        assert_eq!(p.active_model(), None);
        let old = retired.try_recv().expect("old model handed back");
        assert!(!dropped.load(Relaxed), "not dropped on the audio thread");
        drop(old);
        assert!(dropped.load(Relaxed));
    }

    #[test]
    fn eq_changes_reach_the_chain() {
        let (mut p, controls, _, _, _) = setup(None);
        assert!(p.eq.is_flat());
        controls.set_eq([0.0, 0.0, 6.0, 0.0, 0.0]);
        p.push_interleaved(&vec![0.0f32; FRAME_SIZE], 1, &mut |_| {});
        assert_eq!(p.eq.gains_db(), [0.0, 0.0, 6.0, 0.0, 0.0]);
    }

    #[test]
    fn threshold_changes_reach_the_gate() {
        let (mut p, controls, _, _, _) = setup(None);
        controls.set_vad_threshold(0.9);
        p.push_interleaved(&vec![0.0f32; FRAME_SIZE], 1, &mut |_| {});
        assert_eq!(p.gate.threshold, 0.9);
        assert_eq!(p.gate.close_threshold, 0.67);
    }
}
