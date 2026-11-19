//! The app's core: one thread owns the engine, the router state and the config, and acts on
//! commands from the window and the tray. It publishes a [`View`] for them to draw.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{info, warn};
use vidar_audio::devices::{input_choices, resolve_saved, InputChoice};
use vidar_audio::voice_test;
use vidar_audio::{Engine, EngineConfig, Explained, Health, MeterSnapshot, Meters};
use vidar_config::{Config, EngineSettings};
use vidar_denoise::{create_with_fallback, DenoiseSpec, Engine as Model};
use vidar_win::events::{DeviceEvent, DeviceWatcher};
use vidar_win::router::{self, best_physical, cable_capture, cable_render, RouteState};
use vidar_win::{AudioSystem, Endpoint, Flow, WinAudio};

/// How a status line should be coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Idle,
    Busy,
    Good,
    Warn,
    Bad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    Original,
    /// Index into `TestView::variants`.
    Variant(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TestPhase {
    #[default]
    Empty,
    Recording,
    /// Processing the recording with every option.
    Analyzing,
    Ready,
    Playing(Track),
}

/// One setting combination the test compares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Option_ {
    pub mode: &'static str,
    pub strength: &'static str,
    pub engine: &'static str,
    pub threshold: f64,
}

/// Shortest recording worth comparing (2 s at 48 kHz).
const MIN_TEST_SAMPLES: usize = 96_000;

/// Position of a track in the playback list: the original first, then the variants.
fn track_index(t: Track) -> usize {
    match t {
        Track::Original => 0,
        Track::Variant(i) => i + 1,
    }
}

/// The options compared in the voice test.
pub const TEST_OPTIONS: [Option_; 6] = [
    Option_ {
        mode: "Voice",
        strength: "Gentle",
        engine: "rnnoise",
        threshold: 0.55,
    },
    Option_ {
        mode: "Voice",
        strength: "Balanced",
        engine: "rnnoise",
        threshold: 0.70,
    },
    Option_ {
        mode: "Voice",
        strength: "Strong",
        engine: "rnnoise",
        threshold: 0.82,
    },
    Option_ {
        mode: "Voice HD",
        strength: "Gentle",
        engine: "dpdfnet",
        threshold: 0.55,
    },
    Option_ {
        mode: "Voice HD",
        strength: "Balanced",
        engine: "dpdfnet",
        threshold: 0.70,
    },
    Option_ {
        mode: "Voice HD",
        strength: "Strong",
        engine: "dpdfnet",
        threshold: 0.82,
    },
];

/// Result of one option on the recording.
#[derive(Debug, Clone)]
pub struct VariantView {
    pub option: Option_,
    pub peaks: Arc<Vec<f32>>,
    pub noise_removed_db: f64,
    pub voice_change_db: f64,
    pub cpu_percent: f64,
    pub delay_ms: f64,
    /// Set when the option could not run (e.g. DPDFNet unavailable).
    pub unavailable: Option<String>,
}

/// Voice test state for the window.
#[derive(Debug, Clone, Default)]
pub struct TestView {
    pub phase: TestPhase,
    /// Recording: fraction of `max_seconds` captured. Analysis: fraction done.
    /// Playback: position in the clip, 0..1.
    pub progress: f64,
    pub level_db: f64,
    /// Longest recording allowed.
    pub max_seconds: f64,
    /// Length of the recorded clip (set once recording ends).
    pub seconds: f64,
    pub original_peaks: Option<Arc<Vec<f32>>>,
    /// Input level history while recording (0..1, one point per update).
    pub live: Vec<f32>,
    pub variants: Vec<VariantView>,
    /// False when the recording had no speech (voice figures are then meaningless).
    pub speech_detected: bool,
    pub error: Option<String>,
}

/// The recording and every processed version, kept in memory for playback.
struct TestClip {
    original: Vec<f32>,
    variants: Vec<Vec<f32>>,
}

/// What the update check is doing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    /// Downloading the installer, 0..1.
    Downloading(f64),
    /// The installer is starting (UAC prompt); Vidar quits once it runs.
    Launching,
}

/// Update state for the window.
#[derive(Debug, Clone, Default)]
pub struct UpdateView {
    pub phase: UpdatePhase,
    /// Check automatically about once a day.
    pub enabled: bool,
    /// Installed by setup (can update itself) rather than the portable zip.
    pub installed: bool,
    /// A newer version and its release page.
    pub available: Option<(String, String)>,
    /// Set after a check the user asked for found nothing newer.
    pub up_to_date: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    Idle,
    Running,
    Done(Result<(), String>),
}

/// Everything the window and tray need to draw. Cloned out under a short lock.
#[derive(Debug, Clone)]
pub struct View {
    pub state: RunState,
    pub status: String,
    pub tone: Tone,
    pub detail: Option<String>,
    pub cable_installed: bool,
    pub cable_names: Option<(String, String)>,
    pub inputs: Vec<InputChoice>,
    pub selected_input: Option<String>,
    pub mic_boost_db: f64,
    pub vad_threshold: f64,
    pub engine: String,
    pub active_model: Option<&'static str>,
    pub muted: bool,
    pub start_with_windows: bool,
    pub meters: Option<MeterSnapshot>,
    /// The running engine's meters, read directly by the window at its own frame rate
    /// (`meters` above is a snapshot refreshed only every ~100 ms).
    pub live: Option<Arc<Meters>>,
    pub test: TestView,
    pub auto_level: bool,
    /// Voice tone preset (`"off"`, a preset name or `"custom"`) and the custom band gains.
    pub eq_preset: String,
    pub eq_custom_db: [f64; vidar_dsp::EQ_BANDS],
    pub raw_capture: bool,
    pub ui_theme: String,
    pub install: InstallState,
    /// The driver job in `install` removes the cable rather than installing it.
    pub uninstalling: bool,
    /// Ask the window to show the driver prompt (first run without the cable).
    pub prompt_install: bool,
    pub update: UpdateView,
}

pub enum Cmd {
    Toggle,
    SelectInput(Option<String>),
    SetBoost(f64),
    SetVad(f64),
    SetEngine(String),
    SetMute(bool),
    SetStartWithWindows(bool),
    /// Start recording (up to this many seconds, or until `FinishTest`).
    RecordTest(f64),
    /// End the recording now and compare what was captured.
    FinishTest,
    /// Play `track` from `start` (0..1 of the clip); with `range` (0..1) loop that part.
    PlayTest {
        track: Track,
        start: f64,
        range: Option<(f64, f64)>,
    },
    /// Choose the version heard; switches instantly if playing.
    SelectTrack(Track),
    /// Use a tested option (index into TEST_OPTIONS) as the setting.
    ApplyOption(usize),
    /// Stop playback, or discard a recording or analysis in progress.
    StopTest,
    SetAutoLevel(bool),
    /// Choose a voice tone preset (`"off"`, a preset name or `"custom"`).
    SetEqPreset(String),
    /// Custom band gains; switches to `"custom"`. `save: false` while a slider is dragged.
    SetEqCustom {
        gains_db: [f64; vidar_dsp::EQ_BANDS],
        save: bool,
    },
    SetRawCapture(bool),
    SetTheme(String),
    InstallDriver,
    /// Stop, give the microphone back and remove the virtual cable driver.
    UninstallDriver,
    DismissInstall,
    Rescan,
    /// Look for a newer release now (the user asked: report "up to date" and errors too).
    CheckUpdates,
    /// Turn the daily automatic check on or off.
    SetCheckUpdates(bool),
    /// Download, verify and run the newer release's installer (installed copies only).
    InstallUpdate,
    /// Internal: a check finished. `manual` = the user asked for it.
    UpdateChecked {
        result: Result<crate::update::Release, String>,
        manual: bool,
    },
    /// Internal: the installer is running; quit so it can replace the files.
    UpdateLaunched,
    /// Stop, restore the mic, and end the worker.
    Exit,
}

#[derive(Clone)]
pub struct Handle {
    pub tx: Sender<Cmd>,
    pub view: Arc<Mutex<View>>,
    /// Set by the window so the worker can wake it when something changes.
    pub repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
}

impl Handle {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
    pub fn view(&self) -> View {
        self.view
            .lock()
            .map(|v| v.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }
}

struct Core {
    cfg_path: PathBuf,
    cfg: Config,
    route: RouteState,
    engine: Option<Engine>,
    /// The running engine's microphone is a built-in laptop mic (it gets extra gain).
    laptop_mic: bool,
    view: Arc<Mutex<View>>,
    repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
    /// Restart attempts after a device loss.
    recover: Option<(Instant, u32)>,
    devices_dirty: Option<Instant>,
    cfg_dirty: Option<Instant>,
    test_cancel: Arc<AtomicBool>,
    test_busy: Arc<AtomicBool>,
    test_finish: Arc<AtomicBool>,
    /// Version being heard: 0 = original, i + 1 = variant i.
    test_track: Arc<AtomicUsize>,
    clip: Arc<Mutex<Option<Arc<TestClip>>>>,
    /// Voice tone for test playback: applied live, so tone changes are heard while playing.
    test_eq: Arc<Mutex<[f64; vidar_dsp::EQ_BANDS]>>,
    tx: Sender<Cmd>,
    /// Tells the window and tray to quit (after an update's installer has started).
    ui_tx: Sender<crate::UiCmd>,
    /// The newer release found by the last check.
    latest: Option<crate::update::Release>,
    /// When the next automatic update check is due.
    next_update_check: Option<Instant>,
}

/// Seconds since 1970, for `last_update_check`.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

fn models_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        v.push(dir.join("models"));
    }
    // Development builds run from target\ also find the repo's models.
    v.extend(vidar_denoise::repo_path("models"));
    v
}

impl Core {
    fn audio() -> Option<WinAudio> {
        WinAudio::new().ok()
    }

    fn publish(&self, f: impl FnOnce(&mut View)) {
        if let Ok(mut v) = self.view.lock() {
            f(&mut v);
        }
        if let Ok(ctx) = self.repaint.lock() {
            if let Some(ctx) = ctx.as_ref() {
                ctx.request_repaint();
            }
        }
    }

    fn status(&self, text: impl Into<String>, tone: Tone) {
        let text = text.into();
        info!("status: {text}");
        self.publish(|v| {
            v.status = text;
            v.tone = tone;
            v.detail = None;
        });
    }

    fn error(&self, e: &Explained) {
        warn!("{} [{}] {}", e.title, e.kind, e.technical);
        let (title, advice, tech) = (
            e.title.to_string(),
            e.advice.to_string(),
            e.technical.clone(),
        );
        self.publish(|v| {
            v.status = format!("{title}. {advice}");
            v.tone = Tone::Bad;
            v.detail = (!tech.is_empty()).then_some(tech);
        });
    }

    fn idle_status(&self) -> (String, Tone) {
        let cable = self.view.lock().map(|v| v.cable_installed).unwrap_or(false);
        if !cable {
            (
                "Virtual cable required - install the driver".into(),
                Tone::Warn,
            )
        } else {
            (
                "Ready - Vidar AI Microphone becomes the default microphone when active".into(),
                Tone::Idle,
            )
        }
    }

    fn save_now(&mut self) {
        self.cfg.is_swapped = self.route.is_swapped;
        self.cfg.original_mic_id = self.route.original_mic_id.clone();
        if let Err(e) = vidar_config::save(&self.cfg_path, &self.cfg) {
            warn!("saving config failed: {e}");
        }
        self.cfg_dirty = None;
    }

    fn save_later(&mut self) {
        self.cfg_dirty = Some(Instant::now());
    }

    /// Vidar's names and icon for the cable.
    fn cable_brand() -> vidar_win::branding::Brand {
        let icon = std::env::current_exe()
            .map(|p| format!("{},0", p.display()))
            .ok();
        vidar_win::branding::Brand {
            icon,
            ..Default::default()
        }
    }

    /// Give the cable Vidar's names and icon. Returns the name apps now see.
    fn rename_cable() -> Option<Result<String, vidar_win::Error>> {
        let audio = Self::audio()?;
        Some(
            vidar_win::branding::brand(
                &audio,
                &Self::cable_brand(),
                &vidar_win::driver::default_branding_backup(),
            )
            .map(|(c, _)| c.name),
        )
    }

    /// On start: repair the cable's names and icon if anything changed them (a Windows or
    /// driver update, or an earlier Vidar version), and disable its duplicate devices.
    /// Does nothing when all of that is already right.
    fn repair_names_on_start(&mut self) {
        let Some(audio) = Self::audio() else { return };
        if !vidar_win::branding::needs_brand(&audio, &Self::cable_brand()) {
            return;
        }
        match Self::rename_cable() {
            Some(Ok(name)) => info!(
                "Repaired the cable on start (names, icon, duplicate devices): apps see \"{name}\""
            ),
            Some(Err(e)) => warn!("Could not repair the cable on start: {e}"),
            None => {}
        }
        self.refresh_devices();
    }

    fn refresh_devices(&mut self) {
        let Some(audio) = Self::audio() else { return };
        let caps = audio.endpoints(Flow::Capture);
        let default = audio.default_endpoint(Flow::Capture).map(|e| e.id);
        let choices = input_choices(&caps, default.as_deref());
        let cap = cable_capture(&audio, self.cfg.cable_capture_id.as_deref());
        let ren = cable_render(&audio, self.cfg.cable_render_id.as_deref());
        let names = cap
            .as_ref()
            .zip(ren.as_ref())
            .map(|(c, r)| (c.name.clone(), r.name.clone()));
        if let (Some(c), Some(r)) = (&cap, &ren) {
            if self.cfg.cable_capture_id.as_deref() != Some(&c.id)
                || self.cfg.cable_render_id.as_deref() != Some(&r.id)
            {
                self.cfg.cable_capture_id = Some(c.id.clone());
                self.cfg.cable_render_id = Some(r.id.clone());
                self.save_now();
            }
        }
        // Keep the saved pick if it is still there; otherwise fall back to "Windows default".
        let selected = self
            .cfg
            .input_endpoint_id
            .clone()
            .filter(|id| choices.iter().any(|c| c.id.as_deref() == Some(id)))
            .or_else(|| {
                resolve_saved(&caps, None, self.cfg.input_device_name.as_deref())
                    .map(|e| e.id.clone())
            });
        let installed = cap.is_some() && ren.is_some();
        self.publish(|v| {
            v.inputs = choices;
            v.selected_input = selected;
            v.cable_installed = installed;
            v.cable_names = names;
        });
    }

    /// The microphone to open: the user's pick, else the Windows default unless that is
    /// virtual (the cable while routed), else the remembered or best physical mic.
    fn resolve_input(&self, audio: &WinAudio) -> Option<Endpoint> {
        let caps = audio.endpoints(Flow::Capture);
        if let Some(e) = resolve_saved(
            &caps,
            self.cfg.input_endpoint_id.as_deref(),
            self.cfg.input_device_name.as_deref(),
        ) {
            return Some(e.clone());
        }
        if let Some(d) = audio
            .default_endpoint(Flow::Capture)
            .filter(|d| !d.is_virtual())
        {
            return Some(d);
        }
        self.route
            .original_mic_id
            .as_deref()
            .and_then(|id| audio.endpoint(id))
            .filter(|e| e.is_active() && !e.is_virtual())
            .or_else(|| best_physical(audio, Flow::Capture))
    }

    fn model_spec(&self) -> DenoiseSpec {
        DenoiseSpec {
            engine: Model::parse(&self.cfg.denoise_engine),
            model_dirs: models_dirs(),
            ..DenoiseSpec::default()
        }
    }

    fn start(&mut self) {
        if self.engine.is_some() {
            return;
        }
        let Some(audio) = Self::audio() else {
            self.status("Windows audio is not available", Tone::Bad);
            return;
        };
        let Some(cable_out) = cable_render(&audio, self.cfg.cable_render_id.as_deref()) else {
            self.error(&vidar_audio::explain::cable_missing());
            self.publish(|v| {
                v.cable_installed = false;
                v.prompt_install = true;
            });
            return;
        };
        let Some(input) = self.resolve_input(&audio) else {
            self.status("No microphone found - connect one and try again", Tone::Bad);
            return;
        };

        self.publish(|v| v.state = RunState::Starting);
        self.status(format!("Starting with {}...", input.name), Tone::Busy);

        self.laptop_mic = input.is_laptop_mic();
        let settings = EngineSettings::from_config(&self.cfg, self.laptop_mic);
        let (model, active, errors) = if settings.denoise_enabled {
            create_with_fallback(&self.model_spec())
        } else {
            (None, None, Vec::new())
        };
        for e in &errors {
            warn!("noise model: {e}");
        }
        let cfg = EngineConfig {
            input_id: Some(input.id.clone()),
            output_id: Some(cable_out.id.clone()),
            settings,
            raw_capture: self.cfg.raw_capture,
            // Always ask for the device's smallest period; the stream falls back to the
            // regular one by itself when a device cannot do it.
            low_latency: true,
            bluetooth_buffer: input.bus == vidar_win::Bus::BluetoothHfp,
        };
        match Engine::start(cfg, model) {
            Ok(engine) => {
                info!("engine started: {:?}", engine.info());
                engine
                    .controls()
                    .muted
                    .store(self.view.lock().map(|v| v.muted).unwrap_or(false), Relaxed);
                let live = engine.meters().clone();
                self.engine = Some(engine);
                self.recover = None;
                let model_name = active.map(|m| m.as_str());
                self.publish(|v| {
                    v.state = RunState::Running;
                    v.active_model = model_name;
                    v.live = Some(live);
                });

                // While active, Vidar AI Microphone is the Windows default; stop, quit and
                // crash recovery put the user's own mic back.
                let routed = {
                    let mut route = self.route.clone();
                    let path = self.cfg_path.clone();
                    let mut cfg = self.cfg.clone();
                    let r = router::swap_to_cable(
                        &audio,
                        &mut route,
                        self.cfg.cable_capture_id.as_deref(),
                        &mut |s| {
                            cfg.is_swapped = s.is_swapped;
                            cfg.original_mic_id = s.original_mic_id.clone();
                            let _ = vidar_config::save(&path, &cfg);
                        },
                    );
                    self.route = route;
                    r
                };
                self.save_now();
                let fallback_note = if !errors.is_empty() && active.is_some() {
                    format!(
                        " (using {}, {} unavailable)",
                        active.map(|m| m.label()).unwrap_or(""),
                        Model::parse(&self.cfg.denoise_engine).label()
                    )
                } else {
                    String::new()
                };
                match routed {
                    Ok(cable) => self.status(format!("Active - apps now hear clean audio from {}{fallback_note}", cable.name), Tone::Good),
                    Err(e) => self.status(format!("Active, but switching the default mic failed ({e}). Pick the Vidar AI Microphone in your apps."), Tone::Warn),
                }
            }
            Err(e) => {
                self.publish(|v| v.state = RunState::Stopped);
                self.error(&e);
            }
        }
    }

    fn restore_mic(&mut self) {
        let Some(audio) = Self::audio() else { return };
        let mut route = self.route.clone();
        match router::restore_original(&audio, &mut route, &mut |_| {}) {
            Ok(Some(e)) => info!("default microphone restored to {}", e.name),
            Ok(None) => {}
            Err(e) => warn!("restoring the default microphone failed: {e}"),
        }
        self.route = route;
        self.save_now();
    }

    fn stop(&mut self, quiet: bool) {
        // A pending reconnect must end too, or the watchdog starts the engine again.
        if self.engine.is_none() && !self.route.is_swapped {
            if self.recover.take().is_some() {
                self.publish(|v| v.state = RunState::Stopped);
                if !quiet {
                    let (s, t) = self.idle_status();
                    self.status(s, t);
                }
            }
            return;
        }
        self.publish(|v| v.state = RunState::Stopping);
        if !quiet {
            self.status("Stopping and restoring your microphone...", Tone::Busy);
        }
        if let Some(e) = self.engine.take() {
            e.stop();
        }
        self.restore_mic();
        self.recover = None;
        self.publish(|v| {
            v.state = RunState::Stopped;
            v.meters = None;
            v.live = None;
            v.active_model = None;
        });
        if !quiet {
            let (s, t) = self.idle_status();
            self.status(s, t);
        }
    }

    /// Watchdog: on a lost or stalled stream, restore the mic and try to come back.
    fn check_health(&mut self) {
        if let Some(engine) = &self.engine {
            engine.collect_retired();
            let health = engine.health();
            // No repaint request: only Home shows the meters and it redraws itself while
            // running, so other tabs and a minimised window stay idle.
            let m = engine.meters().snapshot();
            if let Ok(mut v) = self.view.lock() {
                v.meters = Some(m);
            }
            if health == Health::Running {
                return;
            }
            warn!("engine health: {health:?}");
            if let Some(e) = self.engine.take() {
                e.stop();
            }
            if let Ok(mut v) = self.view.lock() {
                v.live = None;
            }
            self.restore_mic();
            let attempts = self.recover.map_or(0, |(_, n)| n);
            self.recover = Some((Instant::now(), attempts + 1));
            let msg = match health {
                Health::DeviceLost => "Microphone or cable disconnected - reconnecting...",
                _ => "Audio stopped responding - restarting...",
            };
            self.status(msg, Tone::Warn);
            self.publish(|v| v.state = RunState::Starting);
            return;
        }
        if let Some((since, attempts)) = self.recover {
            if since.elapsed() >= Duration::from_millis(1500) {
                if attempts > 5 {
                    self.recover = None;
                    self.publish(|v| v.state = RunState::Stopped);
                    self.status("Could not reconnect the audio devices. Check the microphone and press Start.", Tone::Bad);
                } else {
                    self.refresh_devices();
                    self.start();
                    if self.engine.is_none() {
                        self.recover = Some((Instant::now(), attempts + 1));
                    }
                }
            }
        }
    }

    /// The speakers for test playback: never the cable.
    fn speakers(audio: &WinAudio) -> Option<String> {
        audio
            .default_endpoint(Flow::Render)
            .filter(|e| !e.is_virtual())
            .or_else(|| best_physical(audio, Flow::Render))
            .map(|e| e.id)
    }

    fn set_test(&self, f: impl FnOnce(&mut TestView)) {
        self.publish(|v| f(&mut v.test));
    }

    /// Record once, then process the recording with every option in `TEST_OPTIONS`.
    /// Works while noise cancellation runs (the microphone is opened in shared mode).
    fn record_test(&mut self, seconds: f64) {
        if self.test_busy.swap(true, Relaxed) {
            return;
        }
        let Some(audio) = Self::audio() else {
            self.test_busy.store(false, Relaxed);
            return;
        };
        let Some(input) = self.resolve_input(&audio) else {
            self.test_busy.store(false, Relaxed);
            self.status("No microphone found", Tone::Bad);
            return;
        };
        // One settings set per option, starting from the current ones (boost, filter, ...).
        let jobs: Vec<(Option_, EngineSettings, DenoiseSpec)> = TEST_OPTIONS
            .iter()
            .map(|o| {
                let mut cfg = self.cfg.clone();
                cfg.denoise_engine = o.engine.into();
                // Voice tone is added during playback instead, so it can change while you listen.
                cfg.eq_preset = "off".into();
                cfg.vad_threshold = o.threshold;
                let settings = EngineSettings::from_config(&cfg, input.is_laptop_mic());
                let spec = DenoiseSpec {
                    engine: Model::parse(o.engine),
                    ..self.model_spec()
                };
                (*o, settings, spec)
            })
            .collect();
        let (view, repaint, busy, cancel, finish, clip) = (
            self.view.clone(),
            self.repaint.clone(),
            self.test_busy.clone(),
            self.test_cancel.clone(),
            self.test_finish.clone(),
            self.clip.clone(),
        );
        cancel.store(false, Relaxed);
        finish.store(false, Relaxed);
        if let Ok(mut slot) = clip.lock() {
            *slot = None;
        }
        self.set_test(|t| {
            *t = TestView {
                phase: TestPhase::Recording,
                max_seconds: seconds,
                ..TestView::default()
            };
        });
        std::thread::spawn(move || {
            let notify = |f: &dyn Fn(&mut View)| {
                if let Ok(mut v) = view.lock() {
                    f(&mut v);
                }
                if let Ok(c) = repaint.lock() {
                    if let Some(c) = c.as_ref() {
                        c.request_repaint();
                    }
                }
            };
            let stop = || cancel.load(Relaxed) || finish.load(Relaxed);
            let recorded =
                voice_test::record_raw(Some(&input.id), seconds, &stop, &mut |p, level| {
                    notify(&|v| {
                        v.test.progress = p;
                        v.test.level_db = level;
                        v.test
                            .live
                            .push(((level + 60.0) / 50.0).clamp(0.0, 1.0) as f32);
                    })
                });
            let original = match recorded {
                Ok(o) if o.len() >= MIN_TEST_SAMPLES && !cancel.load(Relaxed) => o,
                Ok(o) => {
                    let short = !cancel.load(Relaxed) && !o.is_empty();
                    notify(&|v| {
                        v.test.phase = TestPhase::Empty;
                        if short {
                            v.test.error =
                                Some("That was too short. Record at least 2 seconds.".into());
                        }
                    });
                    busy.store(false, Relaxed);
                    return;
                }
                Err(e) => {
                    let msg = format!("{}. {}", e.title, e.advice);
                    notify(&|v| {
                        v.test.phase = TestPhase::Empty;
                        v.test.error = Some(msg.clone());
                    });
                    busy.store(false, Relaxed);
                    return;
                }
            };
            let original_peaks = Arc::new(voice_test::peaks(&original, 300));
            notify(&|v| {
                v.test.phase = TestPhase::Analyzing;
                v.test.progress = 0.0;
                v.test.seconds =
                    original.len() as f64 / f64::from(vidar_audio::wasapi::SAMPLE_RATE);
                v.test.original_peaks = Some(original_peaks.clone());
            });

            let n = jobs.len();
            let mut processed = Vec::with_capacity(n);
            let mut views = Vec::with_capacity(n);
            let probs = speech_probabilities(&original);
            let speech = probs.iter().filter(|&&p| p > 0.6).count() >= 40;
            for (i, (option, settings, spec)) in jobs.into_iter().enumerate() {
                if cancel.load(Relaxed) {
                    notify(&|v| {
                        v.test = TestView {
                            max_seconds: v.test.max_seconds,
                            ..TestView::default()
                        }
                    });
                    busy.store(false, Relaxed);
                    return;
                }
                let (model, unavailable) = if settings.denoise_enabled {
                    match vidar_denoise::create(&spec) {
                        Ok(m) => (Some(m), None),
                        Err(e) => (None, Some(e.to_string())),
                    }
                } else {
                    (None, None)
                };
                let out = voice_test::process(&original, &settings, model);
                let m = voice_test::metrics(&original, &out.samples, Some(&probs));
                views.push(VariantView {
                    option,
                    peaks: Arc::new(voice_test::peaks(&out.samples, 300)),
                    noise_removed_db: m.noise_removed_db,
                    voice_change_db: m.voice_change_db,
                    cpu_percent: out.cpu_percent,
                    delay_ms: out.delay_ms,
                    unavailable,
                });
                processed.push(out.samples);
                notify(&|v| v.test.progress = (i + 1) as f64 / n as f64);
            }
            // CPU cost depends on the model, not on the strength: report one figure per model
            // (the least disturbed of its runs) instead of per-run timing noise.
            let model_cpu = |o: &Option_| {
                views
                    .iter()
                    .filter(|w| w.unavailable.is_none() && w.option.engine == o.engine)
                    .map(|w| w.cpu_percent)
                    .fold(f64::INFINITY, f64::min)
            };
            let cpus: Vec<f64> = views.iter().map(|w| model_cpu(&w.option)).collect();
            for (w, c) in views.iter_mut().zip(cpus) {
                if c.is_finite() {
                    w.cpu_percent = c;
                }
            }
            if let Ok(mut slot) = clip.lock() {
                *slot = Some(Arc::new(TestClip {
                    original,
                    variants: processed,
                }));
            }
            notify(&|v| {
                v.test.phase = TestPhase::Ready;
                v.test.progress = 0.0;
                v.test.variants = views.clone();
                v.test.speech_detected = speech;
                v.test.error = None;
            });
            busy.store(false, Relaxed);
        });
    }

    fn sync_test_eq(&self) {
        if let Ok(mut g) = self.test_eq.lock() {
            *g = self.cfg.eq_gains_db();
        }
    }

    fn play_test(&mut self, track: Track, start: f64, range: Option<(f64, f64)>) {
        let Some(clip) = self.clip.lock().ok().and_then(|c| c.clone()) else {
            return;
        };
        if let Track::Variant(i) = track {
            if i >= clip.variants.len() {
                return;
            }
        }
        // Already playing something: stop it first.
        self.test_cancel.store(true, Relaxed);
        while self.test_busy.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.test_busy.store(true, Relaxed);
        self.test_cancel.store(false, Relaxed);
        self.test_track.store(track_index(track), Relaxed);
        let speakers = Self::audio().and_then(|a| Self::speakers(&a));
        let (view, repaint, busy, cancel, current, eq) = (
            self.view.clone(),
            self.repaint.clone(),
            self.test_busy.clone(),
            self.test_cancel.clone(),
            self.test_track.clone(),
            self.test_eq.clone(),
        );
        let len = clip.original.len();
        let to_sample = |f: f64| ((f.clamp(0.0, 1.0) * len as f64) as usize).min(len);
        let (range, looping) = match range {
            Some((a, b)) if to_sample(b) > to_sample(a) + 480 => (to_sample(a)..to_sample(b), true),
            _ => (0..len, false),
        };
        let start = to_sample(start);
        // Starting outside the loop (or at the very end) begins at its start instead.
        let start = if range.contains(&start) && start + 480 < range.end {
            start
        } else {
            range.start
        };
        self.set_test(|t| {
            t.phase = TestPhase::Playing(track);
            t.progress = start as f64 / len.max(1) as f64;
        });
        std::thread::spawn(move || {
            let notify = |f: &dyn Fn(&mut View)| {
                if let Ok(mut v) = view.lock() {
                    f(&mut v);
                }
                if let Ok(c) = repaint.lock() {
                    if let Some(c) = c.as_ref() {
                        c.request_repaint();
                    }
                }
            };
            let tracks: Vec<&[f32]> = std::iter::once(clip.original.as_slice())
                .chain(clip.variants.iter().map(Vec::as_slice))
                .collect();
            let pb = voice_test::Playback {
                tracks: &tracks,
                current: &current,
                start,
                range,
                looping,
                eq: Some(&eq),
            };
            let r = voice_test::play(speakers.as_deref(), &pb, &cancel, &mut |pos| {
                notify(&|v| v.test.progress = pos as f64 / len.max(1) as f64)
            });
            notify(&|v| {
                if matches!(v.test.phase, TestPhase::Playing(_)) {
                    v.test.phase = TestPhase::Ready;
                }
                if let Err(e) = &r {
                    v.test.error = Some(format!("{}. {}", e.title, e.advice));
                }
            });
            busy.store(false, Relaxed);
        });
    }

    /// Make a tested option the active setting.
    fn apply_option(&mut self, o: Option_) {
        self.handle(Cmd::SetVad(o.threshold));
        self.handle(Cmd::SetEngine(o.engine.into()));
        self.status(format!("Now using {} - {}", o.mode, o.strength), Tone::Good);
    }

    /// When the next automatic check is due: a day after the last one, but not in the first
    /// half minute after start, so starting with Windows stays quick.
    fn schedule_update_check(&mut self) {
        self.next_update_check = self.cfg.check_updates.then(|| {
            let since = unix_now().saturating_sub(self.cfg.last_update_check);
            let wait = UPDATE_INTERVAL.saturating_sub(Duration::from_secs(since));
            Instant::now() + wait.max(Duration::from_secs(30))
        });
    }

    /// Look for a newer release on a background thread; the result comes back as a command.
    fn check_updates(&mut self, manual: bool) {
        if matches!(
            self.view.lock().map(|v| v.update.phase),
            Ok(UpdatePhase::Checking | UpdatePhase::Downloading(_) | UpdatePhase::Launching)
        ) {
            return;
        }
        self.cfg.last_update_check = unix_now();
        self.save_later();
        self.schedule_update_check();
        self.publish(|v| {
            v.update.phase = UpdatePhase::Checking;
            v.update.error = None;
            v.update.up_to_date = false;
        });
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::update::fetch_latest();
            let _ = tx.send(Cmd::UpdateChecked { result, manual });
        });
    }

    fn update_checked(&mut self, result: Result<crate::update::Release, String>, manual: bool) {
        let current = env!("CARGO_PKG_VERSION");
        match result {
            Ok(rel) if crate::update::is_newer(&rel.version, current) => {
                info!("update available: {} (running {current})", rel.version);
                let found = (rel.version.clone(), rel.page_url.clone());
                self.latest = Some(rel);
                self.publish(|v| {
                    v.update.phase = UpdatePhase::Idle;
                    v.update.available = Some(found);
                });
            }
            Ok(_) => self.publish(|v| {
                v.update.phase = UpdatePhase::Idle;
                v.update.up_to_date = manual;
            }),
            Err(e) => {
                warn!("update check failed: {e}");
                // Automatic checks fail quietly (offline is normal); asked-for ones say why.
                self.publish(|v| {
                    v.update.phase = UpdatePhase::Idle;
                    v.update.error = manual.then_some(e);
                });
            }
        }
    }

    /// Download and verify the newer release's installer, then run it. Vidar quits once it
    /// has started (`UpdateLaunched`); declining the UAC prompt leaves everything as it was.
    fn install_update(&mut self) {
        let Some(rel) = self.latest.clone() else {
            return;
        };
        if !crate::update::installed() {
            return;
        }
        let (view, repaint, tx) = (self.view.clone(), self.repaint.clone(), self.tx.clone());
        let notify = move |f: &dyn Fn(&mut UpdateView)| {
            if let Ok(mut v) = view.lock() {
                f(&mut v.update);
            }
            if let Ok(c) = repaint.lock() {
                if let Some(c) = c.as_ref() {
                    c.request_repaint();
                }
            }
        };
        notify(&|u| {
            u.phase = UpdatePhase::Downloading(0.0);
            u.error = None;
        });
        std::thread::spawn(move || {
            let mut last = 0.0;
            let downloaded = crate::update::download_setup(&rel, &mut |p| {
                if p - last >= 0.01 || p >= 1.0 {
                    last = p;
                    notify(&|u| u.phase = UpdatePhase::Downloading(p));
                }
            });
            let launched = downloaded.and_then(|path| {
                notify(&|u| u.phase = UpdatePhase::Launching);
                crate::update::launch_setup(&path)
            });
            match launched {
                Ok(()) => {
                    let _ = tx.send(Cmd::UpdateLaunched);
                }
                Err(e) => {
                    warn!("update failed: {e}");
                    notify(&|u| {
                        u.phase = UpdatePhase::Idle;
                        u.error = Some(e.clone());
                    });
                }
            }
        });
    }

    /// Install (or with `uninstall`, remove) the virtual cable through the CLI, elevated.
    fn install_driver(&mut self, uninstall: bool) {
        let cli = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("vidar-cli.exe")));
        let Some(cli) = cli.filter(|p| p.is_file()) else {
            self.publish(|v| {
                v.install =
                    InstallState::Done(Err("vidar-cli.exe is missing next to vidar.exe".into()))
            });
            return;
        };
        let icon = std::env::current_exe()
            .map(|p| format!("{},0", p.display()))
            .unwrap_or_default();
        self.publish(|v| {
            v.install = InstallState::Running;
            v.uninstalling = uninstall;
        });
        let (view, repaint, tx) = (self.view.clone(), self.repaint.clone(), self.tx.clone());
        std::thread::spawn(move || {
            // One UAC prompt: the CLI installs, restores the default devices and brands the cable.
            let args = if uninstall {
                "endpoints uninstall-driver".to_string()
            } else {
                format!("endpoints install-driver --icon \"{icon}\"")
            };
            let dir = cli.parent().map(PathBuf::from).unwrap_or_default();
            let result = match vidar_win::driver::run_maybe_elevated(
                &cli,
                &args,
                &dir,
                Duration::from_secs(180),
            ) {
                Ok(0) => Ok(()),
                Ok(code) => Err(format!("the installer reported an error (code {code})")),
                Err(e) => Err(e.to_string()),
            };
            if let Ok(mut v) = view.lock() {
                v.install = InstallState::Done(result);
            }
            if let Ok(c) = repaint.lock() {
                if let Some(c) = c.as_ref() {
                    c.request_repaint();
                }
            }
            let _ = tx.send(Cmd::Rescan);
        });
    }

    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Toggle => {
                if self.engine.is_some() || self.recover.is_some() {
                    self.stop(false);
                } else {
                    self.start();
                }
            }
            Cmd::SelectInput(id) => {
                if self.engine.is_some() {
                    self.status(
                        "Stop noise cancellation first to switch the microphone",
                        Tone::Warn,
                    );
                    return true;
                }
                let name = id
                    .as_deref()
                    .and_then(|id| Self::audio().and_then(|a| a.endpoint(id)))
                    .map(|e| e.name);
                self.cfg.input_endpoint_id = id.clone();
                self.cfg.input_device_name = name;
                self.save_now();
                self.publish(|v| v.selected_input = id);
            }
            Cmd::SetBoost(db) => {
                self.cfg.mic_boost_db = db;
                self.save_now();
                if let Some(e) = &self.engine {
                    let s = EngineSettings::from_config(&self.cfg, self.laptop_mic);
                    e.controls().total_gain.set(s.total_gain);
                    e.controls().mic_boost_db.set(s.mic_boost_db);
                }
                self.publish(|v| v.mic_boost_db = db);
            }
            Cmd::SetVad(t) => {
                let t = (t * 100.0).round() / 100.0;
                self.cfg.vad_threshold = t;
                if let Some(e) = &self.engine {
                    e.controls().set_vad_threshold(t);
                }
                self.save_later();
                self.publish(|v| v.vad_threshold = t);
            }
            Cmd::SetEngine(name) => {
                self.cfg.denoise_engine = name.clone();
                self.cfg.sanitize();
                self.save_now();
                self.publish(|v| v.engine = name);
                if let Some(e) = &self.engine {
                    let (model, active, errors) = create_with_fallback(&self.model_spec());
                    e.set_model(model);
                    let requested = Model::parse(&self.cfg.denoise_engine);
                    self.publish(|v| v.active_model = active.map(|m| m.as_str()));
                    if active != Some(requested) {
                        let why = errors.first().map(|e| e.to_string()).unwrap_or_default();
                        self.status(
                            format!(
                                "{} unavailable ({why}); using {}",
                                requested.label(),
                                active.map(|m| m.label()).unwrap_or("no model")
                            ),
                            Tone::Warn,
                        );
                    }
                }
            }
            Cmd::SetMute(on) => {
                if let Some(e) = &self.engine {
                    e.controls().muted.store(on, Relaxed);
                }
                self.publish(|v| v.muted = on);
            }
            Cmd::SetStartWithWindows(on) => {
                match crate::autostart::set(on) {
                    Ok(()) => {
                        self.cfg.start_with_windows = on;
                        self.save_now();
                    }
                    Err(e) => self.status(format!("Could not change autostart: {e}"), Tone::Warn),
                }
                self.publish(|v| v.start_with_windows = crate::autostart::is_enabled());
            }
            Cmd::RecordTest(s) => self.record_test(s),
            Cmd::FinishTest => self.test_finish.store(true, Relaxed),
            Cmd::PlayTest {
                track,
                start,
                range,
            } => self.play_test(track, start, range),
            Cmd::SelectTrack(t) => {
                self.test_track.store(track_index(t), Relaxed);
                self.set_test(|v| {
                    if matches!(v.phase, TestPhase::Playing(_)) {
                        v.phase = TestPhase::Playing(t);
                    }
                });
            }
            Cmd::ApplyOption(i) => {
                if let Some(o) = TEST_OPTIONS.get(i).copied() {
                    self.apply_option(o);
                }
            }
            Cmd::StopTest => self.test_cancel.store(true, Relaxed),
            Cmd::SetAutoLevel(on) => {
                self.cfg.auto_level = on;
                self.save_now();
                if let Some(e) = &self.engine {
                    e.controls().auto_level.store(on, Relaxed);
                }
                self.publish(|v| v.auto_level = on);
            }
            Cmd::SetEqPreset(name) => {
                self.cfg.eq_preset = name;
                self.cfg.sanitize();
                self.save_now();
                if let Some(e) = &self.engine {
                    e.controls().set_eq(self.cfg.eq_gains_db());
                }
                self.sync_test_eq();
                let preset = self.cfg.eq_preset.clone();
                self.publish(|v| v.eq_preset = preset);
            }
            Cmd::SetEqCustom { gains_db, save } => {
                self.cfg.eq_preset = "custom".into();
                self.cfg.eq_custom_db = gains_db;
                self.cfg.sanitize();
                if save {
                    self.save_now();
                }
                if let Some(e) = &self.engine {
                    e.controls().set_eq(self.cfg.eq_gains_db());
                }
                self.sync_test_eq();
                let g = self.cfg.eq_custom_db;
                self.publish(|v| {
                    v.eq_preset = "custom".into();
                    v.eq_custom_db = g;
                });
            }
            Cmd::SetRawCapture(on) => {
                self.cfg.raw_capture = on;
                self.save_now();
                self.publish(|v| v.raw_capture = on);
                if self.engine.is_some() {
                    self.stop(true);
                    self.start();
                }
            }
            Cmd::SetTheme(t) => {
                self.cfg.ui_theme = t;
                self.cfg.sanitize();
                self.save_now();
                let t = self.cfg.ui_theme.clone();
                self.publish(|v| v.ui_theme = t);
            }
            Cmd::InstallDriver => self.install_driver(false),
            Cmd::UninstallDriver => {
                // The cable cannot be removed while Vidar uses it; stopping also restores the mic.
                if self.engine.is_some() || self.recover.is_some() {
                    self.stop(false);
                }
                self.install_driver(true);
            }
            Cmd::DismissInstall => {
                self.publish(|v| {
                    v.prompt_install = false;
                    v.install = InstallState::Idle;
                });
            }
            Cmd::Rescan => {
                self.refresh_devices();
                if self.engine.is_none() && self.recover.is_none() {
                    let (s, t) = self.idle_status();
                    self.status(s, t);
                }
            }
            Cmd::CheckUpdates => self.check_updates(true),
            Cmd::SetCheckUpdates(on) => {
                self.cfg.check_updates = on;
                self.save_now();
                self.schedule_update_check();
                self.publish(|v| v.update.enabled = on);
            }
            Cmd::InstallUpdate => self.install_update(),
            Cmd::UpdateChecked { result, manual } => self.update_checked(result, manual),
            Cmd::UpdateLaunched => {
                info!("update installer started; quitting so it can replace the files");
                let _ = self.ui_tx.send(crate::UiCmd::Quit);
                if let Ok(c) = self.repaint.lock() {
                    if let Some(c) = c.as_ref() {
                        c.request_repaint();
                    }
                }
            }
            Cmd::Exit => {
                self.test_cancel.store(true, Relaxed);
                self.stop(true);
                if self.cfg_dirty.is_some() {
                    self.save_now();
                }
                return false;
            }
        }
        true
    }
}

/// Start the worker. `cfg_path`/`cfg` come from `main` (already crash-audited); `ui_tx`
/// reaches the window and tray (the worker uses it to quit for an update).
pub fn spawn(
    cfg_path: PathBuf,
    cfg: Config,
    ui_tx: Sender<crate::UiCmd>,
) -> (Handle, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let view = Arc::new(Mutex::new(View {
        state: RunState::Stopped,
        status: "Loading...".into(),
        tone: Tone::Idle,
        detail: None,
        cable_installed: false,
        cable_names: None,
        inputs: Vec::new(),
        selected_input: cfg.input_endpoint_id.clone(),
        mic_boost_db: cfg.mic_boost_db,
        vad_threshold: cfg.vad_threshold,
        engine: cfg.denoise_engine.clone(),
        active_model: None,
        muted: false,
        start_with_windows: crate::autostart::is_enabled(),
        meters: None,
        live: None,
        test: TestView::default(),
        auto_level: cfg.auto_level,
        eq_preset: cfg.eq_preset.clone(),
        eq_custom_db: cfg.eq_custom_db,
        raw_capture: cfg.raw_capture,
        ui_theme: cfg.ui_theme.clone(),
        install: InstallState::Idle,
        uninstalling: false,
        prompt_install: false,
        update: UpdateView {
            enabled: cfg.check_updates,
            installed: crate::update::installed(),
            ..UpdateView::default()
        },
    }));
    let repaint = Arc::new(Mutex::new(None));
    let handle = Handle {
        tx: tx.clone(),
        view: view.clone(),
        repaint: repaint.clone(),
    };
    let join = std::thread::Builder::new()
        .name("vidar-worker".into())
        .spawn(move || run(cfg_path, cfg, view, repaint, tx, rx, ui_tx))
        .expect("worker thread");
    (handle, join)
}

fn run(
    cfg_path: PathBuf,
    cfg: Config,
    view: Arc<Mutex<View>>,
    repaint: Arc<Mutex<Option<eframe::egui::Context>>>,
    tx: Sender<Cmd>,
    rx: Receiver<Cmd>,
    ui_tx: Sender<crate::UiCmd>,
) {
    vidar_win::sys::com_init();
    let route = RouteState {
        is_swapped: cfg.is_swapped,
        original_mic_id: cfg.original_mic_id.clone(),
    };
    let cfg_eq = cfg.eq_gains_db();
    let mut core = Core {
        cfg_path,
        cfg,
        route,
        engine: None,
        laptop_mic: false,
        view,
        repaint,
        recover: None,
        devices_dirty: None,
        cfg_dirty: None,
        test_cancel: Arc::new(AtomicBool::new(false)),
        test_busy: Arc::new(AtomicBool::new(false)),
        test_finish: Arc::new(AtomicBool::new(false)),
        test_track: Arc::new(AtomicUsize::new(0)),
        clip: Arc::new(Mutex::new(None)),
        test_eq: Arc::new(Mutex::new(cfg_eq)),
        tx,
        ui_tx,
        latest: None,
        next_update_check: None,
    };
    core.schedule_update_check();
    core.refresh_devices();
    core.repair_names_on_start();
    let (s, t) = core.idle_status();
    core.status(s, t);
    if !core.view.lock().map(|v| v.cable_installed).unwrap_or(true) {
        core.publish(|v| v.prompt_install = true);
    }

    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<DeviceEvent>();
    let _watcher = DeviceWatcher::start(ev_tx)
        .map_err(|e| warn!("device notifications unavailable: {e}"))
        .ok();

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(cmd) => {
                if !core.handle(cmd) {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                core.stop(true);
                break;
            }
        }
        // Debounce device events: Windows sends bursts while a device settles.
        while ev_rx.try_recv().is_ok() {
            core.devices_dirty = Some(Instant::now());
        }
        if core
            .devices_dirty
            .is_some_and(|t| t.elapsed() > Duration::from_millis(600))
        {
            core.devices_dirty = None;
            core.refresh_devices();
        }
        if core
            .cfg_dirty
            .is_some_and(|t| t.elapsed() > Duration::from_millis(500))
        {
            core.save_now();
        }
        if core.next_update_check.is_some_and(|t| Instant::now() >= t) {
            core.check_updates(false);
        }
        core.check_health();
    }
    info!("worker stopped");
}

/// RNNoise's voice probability for each 480-sample frame: tells the test which frames are
/// speech and which are noise. Levels alone would mistake clicks and bumps for speech.
fn speech_probabilities(samples: &[f32]) -> Vec<f64> {
    use vidar_dsp::{Denoiser, FRAME_SIZE, PCM_SCALE};
    let Ok(mut rn) = vidar_denoise::RnNoise::new() else {
        return Vec::new();
    };
    samples
        .as_chunks::<FRAME_SIZE>()
        .0
        .iter()
        .map(|chunk| {
            let mut f: vidar_dsp::Frame = core::array::from_fn(|i| chunk[i] * PCM_SCALE);
            rn.process_frame(&mut f)
        })
        .collect()
}
