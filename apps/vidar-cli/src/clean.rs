//! Offline file cleaning: one file or a whole folder.

use std::path::{Path, PathBuf};
use std::time::Instant;

use vidar_denoise::{create, DenoiseSpec, Engine};
use vidar_dsp::{
    levels, process_mono_frame, soft_limit, soft_preclip, AdaptiveGate, Denoiser, Frame,
    GateParams, HighPass, SpeechLeveler, TransientSuppressor, FRAME_SIZE, PCM_SCALE,
};

use crate::decode::{decode, SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub no_model: bool,
    pub no_gate: bool,
    /// Skip the transient (click) suppressor after the model.
    pub no_declick: bool,
    /// Gate tracks the level after the model instead of the input level.
    pub gate_level_after_model: bool,
    /// DPDFNet attenuation limit in dB (`None`: unlimited).
    pub attn_limit_db: Option<f64>,
    pub auto_level: bool,
    pub gain_db: f64,
    pub stereo: bool,
    pub engine: Engine,
    pub dpdfnet_vad: String,
    pub dpdfnet_model: String,
    pub model_dirs: Vec<PathBuf>,
    pub report_json: Option<PathBuf>,
    /// Fixed dither seed (tests); `None` seeds from the clock.
    pub dither_seed: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CleanResult {
    pub input: PathBuf,
    pub output: PathBuf,
    pub samples: usize,
    pub duration_sec: f64,
    pub elapsed_sec: f64,
    pub in_peak_db: f64,
    pub in_rms_db: f64,
    pub out_peak_db: f64,
    pub out_rms_db: f64,
    /// Engine actually used (a DPDFNet request may fall back).
    pub model: Option<&'static str>,
}

impl CleanResult {
    pub fn speed(&self) -> f64 {
        self.duration_sec / self.elapsed_sec.max(1e-4)
    }
}

/// xorshift64*: fast, good enough for dither, no dependency.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn cli_gate(level_after_model: bool) -> AdaptiveGate {
    AdaptiveGate::new(GateParams {
        level_after_model,
        threshold: 0.70,
        close_threshold: 0.52,
        hangover_ms: 320.0,
        decay_ms: 80.0,
        lookahead: true,
        calibrate_startup: false,
        ..GateParams::default()
    })
}

pub fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    input.with_file_name(format!("{stem}_cleaned.wav"))
}

/// Clean one file. `progress(done, total)` is called every few hundred frames.
pub fn clean_file(
    input: &Path,
    output: &Path,
    opt: &CleanOptions,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<CleanResult, String> {
    let t0 = Instant::now();
    // The no-gate and stereo paths below do not go through `process_mono_frame`.
    vidar_dsp::flush_denormals();
    let input = std::fs::canonicalize(input)
        .map_err(|e| format!("input file not found: {} ({e})", input.display()))?;
    let output_abs = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(output)
    };
    if output_abs == input || std::fs::canonicalize(&output_abs).is_ok_and(|o| o == input) {
        return Err(format!(
            "output file cannot be the input file: {}",
            input.display()
        ));
    }
    if let Some(parent) = output_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let channels = if opt.stereo { 2 } else { 1 };
    let audio = decode(&input, channels)?;
    let total = audio.frames();
    if total == 0 {
        return Err(format!(
            "audio file contains no samples: {}",
            input.display()
        ));
    }

    let gain = 10f64.powf(opt.gain_db / 20.0);
    let mut use_model = !opt.no_model;
    let use_gate = !opt.no_gate;

    let mut hpf: Vec<HighPass> = (0..channels)
        .map(|_| HighPass::new(80.0, SAMPLE_RATE as f64))
        .collect();
    let mut ts: Vec<TransientSuppressor> = (0..channels)
        .map(|_| TransientSuppressor::default())
        .collect();
    let mut models: Vec<Box<dyn Denoiser>> = Vec::new();
    if use_model {
        let mut spec = DenoiseSpec {
            engine: opt.engine,
            dpdfnet_vad: opt.dpdfnet_vad.clone(),
            dpdfnet_model: opt.dpdfnet_model.clone(),
            model_dirs: opt.model_dirs.clone(),
            dpdfnet_attn_limit_db: opt.attn_limit_db,
        };
        for _ in 0..channels {
            let mut made = create(&spec);
            // Like the app: if DPDFNet (Voice HD, the default) cannot load, use RNNoise.
            if let (Err(e), true) = (&made, models.is_empty() && spec.engine != Engine::RnNoise) {
                eprintln!("[WARN] {} unavailable ({e}); using RNNoise.", spec.engine.label());
                spec.engine = Engine::RnNoise;
                made = create(&spec);
            }
            match made {
                Ok(m) => models.push(m),
                Err(e) => {
                    eprintln!(
                        "[WARN] {} unavailable ({e}); continuing without a noise model.",
                        spec.engine.label()
                    );
                    use_model = false;
                    models.clear();
                    break;
                }
            }
        }
    }
    let mut gates: Vec<AdaptiveGate> = if use_gate {
        (0..channels)
            .map(|_| cli_gate(opt.gate_level_after_model))
            .collect()
    } else {
        Vec::new()
    };
    let mut levelers: Vec<SpeechLeveler> = if opt.auto_level {
        (0..channels).map(|_| SpeechLeveler::default()).collect()
    } else {
        Vec::new()
    };

    // Latency to trim: the model's delay, plus one frame of gate lookahead.
    let mut delay = 0;
    if use_model {
        delay += models[0].delay_samples();
    }
    if use_gate {
        delay += FRAME_SIZE;
    }
    let pad = (FRAME_SIZE - total % FRAME_SIZE) % FRAME_SIZE;
    let work = total + pad + (delay / FRAME_SIZE) * FRAME_SIZE;
    let n_frames = work / FRAME_SIZE;

    let mono_in: Vec<f32> = if channels == 1 {
        audio.channels[0].clone()
    } else {
        audio.channels[0]
            .iter()
            .zip(&audio.channels[1])
            .map(|(a, b)| (a + b) * 0.5)
            .collect()
    };
    let in_lv = levels(&mono_in);

    let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(work); channels];
    let mut telemetry = Vec::new();
    let sample = |c: usize, i: usize| audio.channels[c].get(i).copied().unwrap_or(0.0);

    for k in 0..n_frames {
        let base = k * FRAME_SIZE;
        if channels == 1 {
            let mut f: Frame = core::array::from_fn(|i| sample(0, base + i));
            let model = models.first_mut().map(|m| m.as_mut() as &mut dyn Denoiser);
            let (sp, rms) = if let Some(gate) = gates.first_mut() {
                let s = process_mono_frame(
                    &mut f,
                    model,
                    Some(&mut hpf[0]),
                    gate,
                    gain,
                    levelers.first_mut(),
                    (!opt.no_declick).then_some(&mut ts[0]),
                    0.0,
                );
                (s.speech_prob, s.input_rms_db)
            } else {
                hpf[0].process(&mut f);
                let mut rms = levels(&f).rms_db;
                soft_preclip(&mut f, 0.75);
                f.iter_mut().for_each(|x| *x = x.clamp(-1.0, 1.0));
                let mut sp = 0.0;
                if let Some(m) = model {
                    f.iter_mut().for_each(|x| *x *= PCM_SCALE);
                    sp = m.process_frame(&mut f);
                    f.iter_mut().for_each(|x| *x /= PCM_SCALE);
                    rms = m.align_level(rms);
                }
                if !opt.no_declick {
                    ts[0].process(&mut f, sp);
                }
                if let Some(l) = levelers.first_mut() {
                    l.process(&mut f, sp, rms, 0.0);
                }
                let g = gain as f32;
                f.iter_mut().for_each(|x| *x *= g);
                soft_limit(&mut f, 0.85);
                (sp, rms)
            };
            out[0].extend_from_slice(&f);
            if opt.report_json.is_some() {
                telemetry.push((k, sp, rms));
            }
        } else {
            // Linked stereo: both channels share one speech decision and gate level.
            let mut ch: [Frame; 2] = [
                core::array::from_fn(|i| sample(0, base + i)),
                core::array::from_fn(|i| sample(1, base + i)),
            ];
            for c in 0..2 {
                hpf[c].process(&mut ch[c]);
                soft_preclip(&mut ch[c], 0.75);
            }
            let mut master_rms = levels(&ch[0]).rms_db.max(levels(&ch[1]).rms_db);
            let master_sp = if use_model {
                let mut sp = [0.0; 2];
                for c in 0..2 {
                    ch[c].iter_mut().for_each(|x| *x *= PCM_SCALE);
                    sp[c] = models[c].process_frame(&mut ch[c]);
                    ch[c].iter_mut().for_each(|x| *x /= PCM_SCALE);
                }
                master_rms = models[0].align_level(master_rms);
                sp[0].max(sp[1])
            } else {
                ((master_rms + 45.0) / 20.0).clamp(0.0, 1.0)
            };
            for c in 0..2 {
                if !opt.no_declick {
                    ts[c].process(&mut ch[c], master_sp);
                }
                if let Some(l) = levelers.get_mut(c) {
                    l.process(&mut ch[c], master_sp, master_rms, 0.0);
                }
                let g = gain as f32;
                ch[c].iter_mut().for_each(|x| *x *= g);
                if let Some(gate) = gates.get_mut(c) {
                    gate.process(&mut ch[c], master_sp, Some(master_rms));
                }
                soft_limit(&mut ch[c], 0.85);
                out[c].extend_from_slice(&ch[c]);
            }
            if opt.report_json.is_some() {
                telemetry.push((k, master_sp, master_rms));
            }
        }
        if k % 200 == 0 || k + 1 == n_frames {
            progress(k + 1, n_frames);
        }
    }

    // Drop the processing latency and match the input length exactly.
    let trimmed: Vec<&[f32]> = out.iter().map(|ch| &ch[delay..delay + total]).collect();

    // TPDF dither, then 16-bit PCM, rounding half to even.
    let seed = opt.dither_seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    });
    let mut rng = Rng::new(seed);
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&output_abs, spec)
        .map_err(|e| format!("cannot write {}: {e}", output_abs.display()))?;
    for i in 0..total {
        for ch in &trimmed {
            let d = (rng.next_f32() - rng.next_f32()) / 32768.0;
            let v = ((ch[i] + d) * 32767.0)
                .round_ties_even()
                .clamp(-32768.0, 32767.0) as i16;
            w.write_sample(v).map_err(|e| e.to_string())?;
        }
    }
    w.finalize().map_err(|e| e.to_string())?;

    let mono_out: Vec<f32> = if channels == 1 {
        trimmed[0].to_vec()
    } else {
        trimmed[0]
            .iter()
            .zip(trimmed[1])
            .map(|(a, b)| (a + b) * 0.5)
            .collect()
    };
    let out_lv = levels(&mono_out);
    let elapsed = t0.elapsed().as_secs_f64().max(1e-4);
    let duration = total as f64 / SAMPLE_RATE as f64;

    if let Some(report) = &opt.report_json {
        let frames: Vec<serde_json::Value> = telemetry
            .iter()
            .map(|(k, sp, rms)| serde_json::json!({"frame_idx": k, "speech_prob": (sp * 1e4).round() / 1e4, "rms_db": (rms * 100.0).round() / 100.0}))
            .collect();
        let doc = serde_json::json!({
            "input_file": input.display().to_string(),
            "output_file": output_abs.display().to_string(),
            "samples_processed": total,
            "duration_seconds": (duration * 1000.0).round() / 1000.0,
            "speed_factor": ((duration / elapsed) * 100.0).round() / 100.0,
            "frames": frames,
        });
        if let Some(parent) = report.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(
            report,
            serde_json::to_string_pretty(&doc).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(CleanResult {
        input,
        output: output_abs,
        samples: total,
        duration_sec: duration,
        elapsed_sec: elapsed,
        in_peak_db: in_lv.peak_db,
        in_rms_db: in_lv.rms_db,
        out_peak_db: out_lv.peak_db,
        out_rms_db: out_lv.rms_db,
        model: models.first().map(|m| m.name()),
    })
}

/// Files in `dir` that the cleaner handles, sorted.
pub fn audio_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .filter(|p| {
                    let ext = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_ascii_lowercase)
                        .unwrap_or_default();
                    matches!(ext.as_str(), "wav" | "mp3" | "flac" | "ogg" | "m4a")
                })
                .filter(|p| {
                    !p.file_stem()
                        .is_some_and(|s| s.to_string_lossy().ends_with("_cleaned"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

pub fn print_stats(results: &[CleanResult]) {
    if results.is_empty() {
        return;
    }
    println!("\n{}", "=".repeat(78));
    println!(
        "{:<24} {:<8} {:<10} {:<11} {:<11} {:<8}",
        "FILE", "DUR", "SPEED", "IN RMS", "OUT RMS", "ATTEN"
    );
    println!("{}", "-".repeat(78));
    for r in results {
        let mut name = r
            .input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.chars().count() > 22 {
            name = format!("{}...", name.chars().take(19).collect::<String>());
        }
        println!(
            "{:<24} {:<8} {:<10} {:<11} {:<11} {:<8}",
            name,
            format!("{:.1}s", r.duration_sec),
            format!("{:.1}x", r.speed()),
            format!("{:.1} dB", r.in_rms_db),
            format!("{:.1} dB", r.out_rms_db),
            format!("{:.1} dB", r.in_rms_db - r.out_rms_db)
        );
    }
    println!("{}\n", "=".repeat(78));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> CleanOptions {
        CleanOptions {
            no_model: false,
            no_gate: false,
            no_declick: false,
            gate_level_after_model: false,
            attn_limit_db: None,
            auto_level: false,
            gain_db: 0.0,
            stereo: false,
            engine: Engine::RnNoise,
            dpdfnet_vad: "rnnoise".into(),
            dpdfnet_model: "dpdfnet2_48khz_hr".into(),
            model_dirs: vec![],
            report_json: None,
            dither_seed: Some(7),
        }
    }

    fn golden_clip() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/clips/keys_zira.wav")
    }

    #[test]
    fn output_length_matches_input_for_all_modes() {
        let dir = tempfile::tempdir().unwrap();
        let input = golden_clip();
        let n = hound::WavReader::open(&input).unwrap().duration() as usize;
        for (i, o) in [
            opts(),
            CleanOptions {
                no_gate: true,
                ..opts()
            },
            CleanOptions {
                no_model: true,
                ..opts()
            },
            CleanOptions {
                stereo: true,
                ..opts()
            },
            CleanOptions {
                no_model: true,
                auto_level: true,
                gain_db: 3.0,
                ..opts()
            },
        ]
        .iter()
        .enumerate()
        {
            let out = dir.path().join(format!("o{i}.wav"));
            let r = clean_file(&input, &out, o, &mut |_, _| {}).unwrap();
            let rd = hound::WavReader::open(&out).unwrap();
            assert_eq!(rd.duration() as usize, n, "mode {i}");
            assert_eq!(r.samples, n);
        }
    }

    #[test]
    fn refuses_to_overwrite_input() {
        let input = golden_clip();
        assert!(clean_file(&input, &input, &opts(), &mut |_, _| {}).is_err());
    }

    #[test]
    fn same_seed_same_output() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        clean_file(&golden_clip(), &a, &opts(), &mut |_, _| {}).unwrap();
        clean_file(&golden_clip(), &b, &opts(), &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    }

    #[test]
    fn output_name_and_file_listing() {
        assert_eq!(
            default_output(Path::new("C:/x/talk.mp3")),
            PathBuf::from("C:/x/talk_cleaned.wav")
        );
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.wav", "a.flac", "c.txt", "a_cleaned.wav"] {
            std::fs::write(dir.path().join(n), b"").unwrap();
        }
        let names: Vec<String> = audio_files(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.flac", "b.wav"]);
    }
}
