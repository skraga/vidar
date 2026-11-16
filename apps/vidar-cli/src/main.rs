//! `vidar-cli`: clean audio files offline, and manage audio endpoints for the installer.

mod clean;
mod decode;
mod endpoints;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use vidar_denoise::Engine;

use crate::clean::{
    audio_files, clean_file, default_output, print_stats, CleanOptions, CleanResult,
};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum EngineArg {
    Rnnoise,
    Dpdfnet,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum VadArg {
    Rnnoise,
    Spectral,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum GateLevelArg {
    Input,
    Output,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[allow(non_camel_case_types)]
enum ModelArg {
    #[value(name = "dpdfnet2_48khz_hr")]
    Dpdfnet2,
    #[value(name = "dpdfnet8_48khz_hr")]
    Dpdfnet8,
}

#[derive(Parser, Debug)]
#[command(
    name = "vidar-cli",
    version,
    about = "Vidar noise cancellation - offline audio cleaner and audio device tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input audio file or folder to clean
    #[arg(long, value_name = "PATH")]
    clean: Option<PathBuf>,
    /// Destination file or folder (default: <name>_cleaned.wav next to the input)
    output_path: Option<PathBuf>,

    /// Skip the neural noise model (filter and gate only)
    #[arg(long = "no-rnnoise", alias = "no-model")]
    no_model: bool,
    /// Neural noise model (falls back to rnnoise if DPDFNet cannot load)
    #[arg(long, value_enum, default_value = "dpdfnet")]
    engine: EngineArg,
    /// DPDFNet speech detector
    #[arg(long = "dpdfnet-vad", value_enum, default_value = "rnnoise")]
    dpdfnet_vad: VadArg,
    /// DPDFNet model variant
    #[arg(
        long = "dpdfnet-model",
        value_enum,
        default_value = "dpdfnet2_48khz_hr"
    )]
    dpdfnet_model: ModelArg,
    /// Folder with DPDFNet .onnx models (default: next to the program)
    #[arg(long = "models-dir", value_name = "DIR")]
    models_dir: Option<PathBuf>,
    /// Skip the adaptive noise gate
    #[arg(long = "no-gate")]
    no_gate: bool,
    /// Skip the click suppressor after the noise model
    #[arg(long = "no-declick")]
    no_declick: bool,
    /// Level the gate tracks: the input, or the output of the noise model (experimental)
    #[arg(long = "gate-level", value_enum, default_value = "input")]
    gate_level: GateLevelArg,
    /// Voice HD: most the model may turn anything down, in dB (default: no limit)
    #[arg(long = "attn-limit-db", value_name = "DB")]
    attn_limit_db: Option<f64>,
    /// Level quiet speech automatically
    #[arg(long = "auto-level")]
    auto_level: bool,
    /// Make-up gain in dB
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    gain: f64,
    /// Keep stereo, with linked gating on both channels
    #[arg(long)]
    stereo: bool,
    /// Show a progress bar
    #[arg(long, overrides_with = "no_progress", default_value_t = true)]
    progress: bool,
    #[arg(long = "no-progress", hide = true)]
    no_progress: bool,
    /// Print a summary table
    #[arg(long, overrides_with = "no_stats", default_value_t = true)]
    stats: bool,
    #[arg(long = "no-stats", hide = true)]
    no_stats: bool,
    /// Write per-frame speech probability and level to a JSON file (single file only)
    #[arg(long = "report-json", value_name = "FILE")]
    report_json: Option<PathBuf>,
    /// Files to clean at once in folder mode (default: all CPU cores)
    #[arg(long, short = 'j', value_name = "N")]
    jobs: Option<usize>,

    /// Print the audio device report and exit
    #[arg(long = "check-devices")]
    check_devices: bool,
    /// Install the bundled VB-Audio Virtual Cable driver and exit
    #[arg(long = "install-driver")]
    install_driver: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Audio endpoint tools (used by the installer)
    Endpoints {
        #[command(subcommand)]
        action: endpoints::Action,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(Command::Endpoints { action }) = cli.command {
        return endpoints::run(action);
    }
    if cli.check_devices {
        return endpoints::run(endpoints::Action::Report);
    }
    if cli.install_driver {
        return endpoints::run(endpoints::Action::InstallDriver {
            driver_dir: None,
            icon: None,
            no_brand: false,
        });
    }

    let Some(target) = cli.clean.clone() else {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        return ExitCode::from(1);
    };

    let show_progress = cli.progress && !cli.no_progress;
    let show_stats = cli.stats && !cli.no_stats;
    let mut model_dirs = Vec::new();
    if let Some(d) = &cli.models_dir {
        model_dirs.push(d.clone());
    }
    // Development builds: use the repo's models folder.
    model_dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"));

    let opts = CleanOptions {
        no_model: cli.no_model,
        no_gate: cli.no_gate,
        no_declick: cli.no_declick,
        gate_level_after_model: matches!(cli.gate_level, GateLevelArg::Output),
        attn_limit_db: cli.attn_limit_db,
        auto_level: cli.auto_level,
        gain_db: cli.gain,
        stereo: cli.stereo,
        engine: match cli.engine {
            EngineArg::Rnnoise => Engine::RnNoise,
            EngineArg::Dpdfnet => Engine::DpdfNet,
        },
        dpdfnet_vad: match cli.dpdfnet_vad {
            VadArg::Rnnoise => "rnnoise".into(),
            VadArg::Spectral => "spectral".into(),
        },
        dpdfnet_model: match cli.dpdfnet_model {
            ModelArg::Dpdfnet2 => "dpdfnet2_48khz_hr".into(),
            ModelArg::Dpdfnet8 => "dpdfnet8_48khz_hr".into(),
        },
        model_dirs,
        report_json: cli.report_json.clone(),
        dither_seed: None,
    };

    let style = ProgressStyle::with_template("{prefix:<24} [{bar:30}] {percent:>3}%")
        .unwrap()
        .progress_chars("#>-");

    let results: Vec<CleanResult> = if target.is_dir() {
        let files = audio_files(&target);
        if files.is_empty() {
            println!("[CLI] No audio files found in {}", target.display());
            return ExitCode::SUCCESS;
        }
        let out_dir = cli.output_path.clone().unwrap_or_else(|| target.clone());
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            eprintln!("[ERROR] cannot create {}: {e}", out_dir.display());
            return ExitCode::from(1);
        }
        let jobs = cli
            .jobs
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
            .max(1);
        println!(
            "[CLI] Cleaning {} files in {} ({} at a time)...",
            files.len(),
            target.display(),
            jobs.min(files.len())
        );
        let multi = MultiProgress::new();
        if !show_progress {
            multi.set_draw_target(indicatif::ProgressDrawTarget::hidden());
        }
        let failures = Mutex::new(Vec::new());
        let opts = CleanOptions {
            report_json: None,
            ..opts
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .expect("thread pool");
        let done: Vec<CleanResult> = pool.install(|| {
            files
                .par_iter()
                .filter_map(|f| {
                    let stem = f
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let out = out_dir.join(format!("{stem}_cleaned.wav"));
                    let bar = multi.add(ProgressBar::new(100).with_style(style.clone()));
                    bar.set_prefix(
                        f.file_name()
                            .map(|n| n.to_string_lossy().chars().take(24).collect::<String>())
                            .unwrap_or_default(),
                    );
                    let r = clean_file(f, &out, &opts, &mut |d, t| {
                        bar.set_position((d * 100 / t.max(1)) as u64)
                    });
                    bar.finish();
                    match r {
                        Ok(r) => Some(r),
                        Err(e) => {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("{}: {e}", f.display()));
                            None
                        }
                    }
                })
                .collect()
        });
        for f in failures.into_inner().unwrap() {
            eprintln!("[ERROR] {f}");
        }
        done
    } else if target.is_file() {
        let out = cli
            .output_path
            .clone()
            .unwrap_or_else(|| default_output(&target));
        let bar = ProgressBar::new(100).with_style(style);
        bar.set_prefix(
            target
                .file_name()
                .map(|n| n.to_string_lossy().chars().take(24).collect::<String>())
                .unwrap_or_default(),
        );
        if !show_progress {
            bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());
        }
        match clean_file(&target, &out, &opts, &mut |d, t| {
            bar.set_position((d * 100 / t.max(1)) as u64)
        }) {
            Ok(r) => {
                bar.finish();
                if r.model.is_none() && !opts.no_model {
                    eprintln!("[WARN] No noise model was used.");
                }
                println!(
                    "[OK] {} ({} samples, peak {:.1} dB -> {:.1} dB)",
                    r.output.display(),
                    r.samples,
                    r.in_peak_db,
                    r.out_peak_db
                );
                vec![r]
            }
            Err(e) => {
                bar.abandon();
                eprintln!("\n[ERROR] Audio processing failed: {e}");
                return ExitCode::from(1);
            }
        }
    } else {
        eprintln!("[ERROR] Path does not exist: {}", target.display());
        return ExitCode::from(1);
    };

    if show_stats {
        print_stats(&results);
    }
    ExitCode::SUCCESS
}
