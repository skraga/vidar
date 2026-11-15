//! Time the noise models alone, without file I/O or the rest of the chain.
//!
//!     cargo run --release -p vidar-denoise --example bench [clip.wav]
//!
//! Prints the cost of each model as a share of one CPU core in real time (10 ms frames).

use std::path::Path;
use std::time::Instant;

use vidar_denoise::{create, DenoiseSpec, Engine};
use vidar_dsp::{
    process_mono_frame, AdaptiveGate, Frame, GateParams, HighPass, SpeechLeveler,
    TransientSuppressor, FRAME_SIZE, PCM_SCALE,
};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let clip = std::env::args()
        .nth(1)
        .map(Into::into)
        .unwrap_or_else(|| root.join("tests/golden/clips/fan_david.wav"));
    let mut r = hound::WavReader::open(&clip).expect("open clip");
    let audio: Vec<f32> = r
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let frames: Vec<Frame> = audio
        .chunks_exact(FRAME_SIZE)
        .map(|c| c.try_into().unwrap())
        .collect();
    // The models see what the chain gives them: samples scaled to PCM range. RNNoise's cost
    // depends on the level, so +-1.0 input would understate it about fivefold.
    let model_frames: Vec<Frame> = frames
        .iter()
        .map(|f| f.map(|x| x * PCM_SCALE))
        .collect();

    // The rest of the chain (filter, gate, leveler, click suppressor) with no model.
    {
        let mut hpf = HighPass::new(80.0, 48_000.0);
        let mut gate = AdaptiveGate::new(GateParams::default());
        let mut lev = SpeechLeveler::default();
        let mut ts = TransientSuppressor::default();
        let t = Instant::now();
        for _ in 0..5 {
            for f in &frames {
                process_mono_frame(
                    &mut f.clone(),
                    None,
                    Some(&mut hpf),
                    &mut gate,
                    1.0,
                    Some(&mut lev),
                    Some(&mut ts),
                    0.0,
                );
            }
        }
        let secs = t.elapsed().as_secs_f64() / 5.0;
        let audio_secs = frames.len() as f64 * FRAME_SIZE as f64 / 48_000.0;
        println!(
            "{:<34} {:>6.2}% of a core  ({:.1} us/frame)",
            "Chain without model",
            secs / audio_secs * 100.0,
            secs / frames.len() as f64 * 1e6
        );
    }

    let cases = [
        ("RNNoise (Voice)", Engine::RnNoise, "rnnoise"),
        (
            "DPDFNet + RNNoise VAD (Voice HD)",
            Engine::DpdfNet,
            "rnnoise",
        ),
        ("DPDFNet + spectral VAD", Engine::DpdfNet, "spectral"),
    ];
    for (name, engine, vad) in cases {
        let spec = DenoiseSpec {
            engine,
            dpdfnet_vad: vad.into(),
            model_dirs: vec![root.join("models")],
            ..DenoiseSpec::default()
        };
        let mut model = match create(&spec) {
            Ok(m) => m,
            Err(e) => {
                println!("{name:<34} unavailable: {e}");
                continue;
            }
        };
        // Warm up, then take the best of several passes so background load does not count.
        for f in model_frames.iter().take(100) {
            model.process_frame(&mut f.clone());
        }
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            for f in &model_frames {
                model.process_frame(&mut f.clone());
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        let audio_secs = frames.len() as f64 * FRAME_SIZE as f64 / 48_000.0;
        println!(
            "{name:<34} {:>6.2}% of a core  ({:.1} us/frame)",
            best / audio_secs * 100.0,
            best / frames.len() as f64 * 1e6
        );
    }
}
