//! Reference vectors: run the chain over the clips in tests/golden and compare every frame
//! with the stored reference output, so any change in behaviour is caught.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use vidar_denoise::{create, DenoiseSpec, Engine};
use vidar_dsp::{
    process_mono_frame, AdaptiveGate, Denoiser, Frame, GateParams, HighPass, SpeechLeveler,
    TransientSuppressor, FRAME_SIZE,
};

const CLIPS: [&str; 5] = [
    "clean_david",
    "fan_david",
    "keys_zira",
    "hiss_silence",
    "loud_zira",
];

struct Config {
    name: &'static str,
    model: Option<(Engine, &'static str)>,
    cli_gate: bool,
    music: bool,
    hpf: bool,
    leveler: bool,
    boost_db: f64,
    gain: f64,
}

fn configs() -> Vec<Config> {
    let db = |d: f64| 10f64.powf(d / 20.0);
    let base = |name, model| Config {
        name,
        model,
        cli_gate: false,
        music: false,
        hpf: true,
        leveler: false,
        boost_db: 0.0,
        gain: 1.08,
    };
    let rn = Some((Engine::RnNoise, "rnnoise"));
    vec![
        base("engine_rnnoise", rn),
        base("engine_bypass", None),
        Config {
            music: true,
            ..base("engine_music", None)
        },
        Config {
            leveler: true,
            ..base("engine_autolevel", rn)
        },
        Config {
            leveler: true,
            boost_db: 5.0,
            gain: 1.08 * db(5.0),
            ..base("engine_boost5", rn)
        },
        Config {
            hpf: false,
            ..base("engine_nohpf", rn)
        },
        Config {
            cli_gate: true,
            gain: 1.0,
            ..base("cli_rnnoise", rn)
        },
        base("engine_dpdfnet_rnnvad", Some((Engine::DpdfNet, "rnnoise"))),
        base(
            "engine_dpdfnet_spectral",
            Some((Engine::DpdfNet, "spectral")),
        ),
    ]
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn read_clip(name: &str) -> Vec<f32> {
    let mut r =
        hound::WavReader::open(golden_dir().join("clips").join(format!("{name}.wav"))).unwrap();
    r.samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect()
}

fn read_csv(path: &Path) -> Vec<Vec<f64>> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| l.split(',').map(|v| v.parse::<f64>().unwrap()).collect())
        .collect()
}

fn gate_for(cfg: &Config) -> AdaptiveGate {
    let p = if cfg.cli_gate {
        GateParams {
            lookahead: true,
            calibrate_startup: false,
            ..GateParams::default()
        }
    } else {
        GateParams {
            hangover_ms: if cfg.music { 800.0 } else { 320.0 },
            decay_ms: if cfg.music { 250.0 } else { 80.0 },
            lookahead: true,
            floor_gain: 0.06,
            ..GateParams::default()
        }
    };
    let mut g = AdaptiveGate::new(p);
    g.recalibrate();
    g
}

fn make_model(cfg: &Config) -> Option<Box<dyn Denoiser>> {
    let (engine, vad) = cfg.model?;
    let spec = DenoiseSpec {
        engine,
        dpdfnet_vad: vad.into(),
        model_dirs: vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/dpdfnet")],
        ..DenoiseSpec::default()
    };
    Some(create(&spec).expect("model should load"))
}

/// Per-column worst absolute difference and count of gate open/closed disagreements.
#[derive(Default)]
struct Diff {
    max: [f64; 9],
    gate_flips: usize,
    frames: usize,
    sample_max: f64,
}

fn run(cfg: &Config, clip: &str) -> (Diff, f64) {
    let audio = read_clip(clip);
    let expected = read_csv(&golden_dir().join(cfg.name).join(format!("{clip}.csv")));
    let mut model = make_model(cfg);
    let mut hpf = cfg.hpf.then(|| HighPass::new(80.0, 48_000.0));
    let mut gate = gate_for(cfg);
    let mut leveler = cfg.leveler.then(SpeechLeveler::default);
    let mut ts = TransientSuppressor::default();

    let full = std::fs::read(golden_dir().join(cfg.name).join(format!("{clip}.f32")))
        .ok()
        .map(|b| {
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect::<Vec<f32>>()
        });

    let mut d = Diff::default();
    let n = audio.len() / FRAME_SIZE;
    assert_eq!(n, expected.len(), "{}/{clip}: frame count", cfg.name);
    let t0 = Instant::now();
    for i in 0..n {
        let mut f: Frame = audio[i * FRAME_SIZE..(i + 1) * FRAME_SIZE]
            .try_into()
            .unwrap();
        let s = process_mono_frame(
            &mut f,
            model.as_mut().map(|m| m.as_mut() as &mut dyn Denoiser),
            hpf.as_mut(),
            &mut gate,
            cfg.gain,
            leveler.as_mut(),
            Some(&mut ts),
            cfg.boost_db,
            None,
        );
        let peak = f.iter().map(|x| f64::from(x.abs())).fold(0.0, f64::max);
        let rms = (f.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / FRAME_SIZE as f64)
            .sqrt();
        let got = [
            i as f64,
            s.speech_prob,
            s.input_peak_db,
            s.input_rms_db,
            gate.current_gain(),
            if gate.is_open() { 1.0 } else { 0.0 },
            gate.noise_floor_db(),
            peak,
            rms,
        ];
        let want = &expected[i];
        for c in 1..9 {
            d.max[c] = d.max[c].max((got[c] - want[c]).abs());
        }
        if got[5] != want[5] {
            d.gate_flips += 1;
        }
        if let Some(full) = &full {
            for (k, &x) in f.iter().enumerate() {
                d.sample_max = d
                    .sample_max
                    .max(f64::from((x - full[i * FRAME_SIZE + k]).abs()));
            }
        }
    }
    d.frames = n;
    let rt_factor = (n as f64 * 0.01) / t0.elapsed().as_secs_f64();
    (d, rt_factor)
}

#[test]
fn chain_matches_reference_vectors() {
    let mut report = String::new();
    let mut failures = Vec::new();
    for cfg in configs() {
        for clip in CLIPS {
            let (d, speed) = run(&cfg, clip);
            let _ = writeln!(
                report,
                "{:<26} {:<13} prob {:.1e} in_rms {:.1e} gain {:.1e} floor {:.1e} out_rms {:.1e} samples {:.1e} gate_flips {}/{} speed {:.0}x",
                cfg.name, clip, d.max[1], d.max[3], d.max[4], d.max[6], d.max[8], d.sample_max, d.gate_flips, d.frames, speed
            );
            // RNNoise picks scalar, SSE4.1 or AVX2/FMA layers at run time, so its sums are
            // rounded differently on different CPUs and the differences compound through its
            // recurrent state. The bounds allow for that (about -65 dB on the audio); the gate
            // must still make exactly the same decisions.
            let ok = d.max[1] <= 5e-3
                && d.max[3] <= 1e-3
                && d.max[8] <= 3e-4
                && d.sample_max <= 1e-3
                && d.gate_flips == 0;
            if !ok {
                failures.push(format!("{}/{clip}", cfg.name));
            }
        }
    }
    println!("{report}");
    assert!(
        failures.is_empty(),
        "golden mismatch in: {failures:?}\n{report}"
    );
}
