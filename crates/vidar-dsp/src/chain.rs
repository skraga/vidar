use crate::levels::{levels, soft_limit, soft_preclip};
use crate::{
    AdaptiveGate, Denoiser, Frame, HighPass, SpeechLeveler, TransientSuppressor, PCM_SCALE,
};

/// Per-frame measurements returned by [`process_mono_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    pub speech_prob: f64,
    /// Input level after the high-pass, before any processing.
    pub input_peak_db: f64,
    pub input_rms_db: f64,
    /// Gain the gate applied at the end of the frame.
    pub gate_gain: f64,
}

/// The single-frame chain shared by the live engine, the voice test and the file cleaner:
///
/// high-pass → metering and soft pre-clip → neural model (or a level-based probability
/// when bypassed) → transient suppression → optional leveler → gain → adaptive gate →
/// soft limiter.
///
/// Sets flush-to-zero on the calling thread first (see [`crate::flush_denormals`]), so the
/// live engine, the voice test and the CLI all run the models without subnormal slowdowns.
#[allow(clippy::too_many_arguments)]
pub fn process_mono_frame(
    frame: &mut Frame,
    denoiser: Option<&mut dyn Denoiser>,
    hpf: Option<&mut HighPass>,
    gate: &mut AdaptiveGate,
    total_gain: f64,
    leveler: Option<&mut SpeechLeveler>,
    transient: Option<&mut TransientSuppressor>,
    mic_boost_db: f64,
) -> FrameStats {
    crate::flush_denormals();
    if let Some(h) = hpf {
        h.process(frame);
    }

    let input = levels(frame);
    soft_preclip(frame, 0.75);
    for x in frame.iter_mut() {
        *x = x.clamp(-1.0, 1.0);
    }

    let mut level_db = input.rms_db;
    let mut gate_level_db = None;
    let speech_prob = match denoiser {
        Some(d) => {
            for x in frame.iter_mut() {
                *x *= PCM_SCALE;
            }
            let p = d.process_frame(frame);
            for x in frame.iter_mut() {
                *x /= PCM_SCALE;
            }
            level_db = d.align_level(input.rms_db);
            if gate.level_after_model() {
                // Already lined up with the audio: it is measured on the model's output.
                gate_level_db = Some(levels(frame).rms_db);
            }
            p
        }
        None => ((input.rms_db + 45.0) / 20.0).clamp(0.0, 1.0),
    };

    if let Some(t) = transient {
        t.process(frame, speech_prob);
    }
    if let Some(l) = leveler {
        l.process(frame, speech_prob, level_db, mic_boost_db);
    }
    if total_gain != 1.0 {
        let g = total_gain as f32;
        frame.iter_mut().for_each(|x| *x *= g);
    }

    let gate_gain = gate.process(frame, speech_prob, Some(gate_level_db.unwrap_or(level_db)));
    soft_limit(frame, 0.85);

    FrameStats {
        speech_prob,
        input_peak_db: input.peak_db,
        input_rms_db: input.rms_db,
        gate_gain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GateParams, FRAME_SIZE};

    /// Passes audio through and reports a fixed probability; lags like DPDFNet.
    struct FakeModel {
        prob: f64,
        delay: Vec<f64>,
    }
    impl Denoiser for FakeModel {
        fn process_frame(&mut self, _frame: &mut Frame) -> f64 {
            self.prob
        }
        fn align_level(&mut self, level_db: f64) -> f64 {
            self.delay.push(level_db);
            self.delay.remove(0)
        }
        fn delay_samples(&self) -> usize {
            960
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    #[test]
    fn bypass_derives_probability_from_level() {
        let mut gate = AdaptiveGate::new(GateParams::default());
        let mut f = [0.0f32; FRAME_SIZE];
        let s = process_mono_frame(&mut f, None, None, &mut gate, 1.0, None, None, 0.0);
        assert_eq!(s.speech_prob, 0.0);
        let mut f = [0.5f32; FRAME_SIZE];
        let s = process_mono_frame(&mut f, None, None, &mut gate, 1.0, None, None, 0.0);
        assert_eq!(s.speech_prob, 1.0);
    }

    #[test]
    fn model_level_alignment_feeds_the_gate() {
        let mut gate = AdaptiveGate::new(GateParams::default());
        let mut m = FakeModel {
            prob: 0.9,
            delay: vec![-100.0],
        };
        let mut f = [0.3f32; FRAME_SIZE];
        let s = process_mono_frame(&mut f, Some(&mut m), None, &mut gate, 1.0, None, None, 0.0);
        assert_eq!(s.speech_prob, 0.9);
        assert!(s.input_rms_db > -11.0, "input level is reported unaligned");
    }

    #[test]
    fn output_never_exceeds_full_scale() {
        let mut gate = AdaptiveGate::new(GateParams {
            floor_gain: 1.0,
            ..GateParams::default()
        });
        let mut f = [0.99f32; FRAME_SIZE];
        process_mono_frame(&mut f, None, None, &mut gate, 4.0, None, None, 0.0);
        assert!(f.iter().all(|x| x.abs() <= 1.0));
    }
}
