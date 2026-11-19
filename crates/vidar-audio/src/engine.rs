//! The live engine: microphone -> processing chain -> virtual cable.
//!
//! Two threads, both at MMCSS "Pro Audio" priority:
//! - capture: reads WASAPI packets, runs the chain per 10 ms frame, pushes samples into a
//!   lock-free ring buffer;
//! - render: pulls from the ring into the cable, with drift correction.
//!
//! Nothing on either thread allocates, locks or logs after start-up. Model swaps arrive
//! through a channel and the replaced model is sent back to be freed elsewhere.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::RingBuffer;
use vidar_config::EngineSettings;
use vidar_dsp::{Denoiser, FRAME_SIZE};
use vidar_win::Flow;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED,
};

use crate::drift::{Correction, DriftControl};
use crate::explain::{explain, Explained};
use crate::processor::{ModelSwap, Processor};
use crate::shared::{Controls, Meters};
use crate::wasapi::{self, MmcssGuard, StreamOptions, WinError};

/// What to open.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Microphone endpoint ID; `None` = Windows default microphone.
    pub input_id: Option<String>,
    /// Playback endpoint receiving the processed audio (the cable's render side).
    pub output_id: Option<String>,
    pub settings: EngineSettings,
    pub raw_capture: bool,
    pub low_latency: bool,
    /// Use a 35 ms buffer to ride out Bluetooth hands-free radio retries.
    pub bluetooth_buffer: bool,
}

/// What actually opened, for logs and the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub input_name: String,
    pub output_name: String,
    pub input_channels: usize,
    pub output_channels: usize,
    pub input_low_latency_frames: Option<u32>,
    pub output_low_latency_frames: Option<u32>,
    pub output_buffer_frames: u32,
}

/// Health as seen by the watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Running,
    /// No capture packet for over 2 seconds.
    Stalled,
    /// A device was unplugged or disabled.
    DeviceLost,
    /// A stream thread ended.
    Stopped,
}

pub struct Engine {
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    model_tx: Sender<ModelSwap>,
    retired: Receiver<Box<dyn Denoiser>>,
    info: StreamInfo,
    started: Instant,
}

const RING_CAPACITY: usize = 48_000; // 1 s, far more than ever needed

fn now_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

impl Engine {
    /// Open both devices and start processing. On failure nothing is left running.
    pub fn start(cfg: EngineConfig, model: Option<Box<dyn Denoiser>>) -> Result<Engine, Explained> {
        let controls = Arc::new(Controls::from_settings(&cfg.settings));
        let meters = Arc::new(Meters::new());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Instant::now();

        let (model_tx, model_rx) = mpsc::channel::<ModelSwap>();
        let (retire_tx, retired) = mpsc::sync_channel::<Box<dyn Denoiser>>(4);
        let (producer, consumer) = RingBuffer::<f32>::new(RING_CAPACITY);

        let buffer_ms = if cfg.bluetooth_buffer { 35 } else { 20 };
        let in_opts = StreamOptions {
            buffer_ms,
            raw: cfg.raw_capture,
            low_latency: cfg.low_latency && !cfg.bluetooth_buffer,
        };
        let out_opts = StreamOptions {
            buffer_ms: 20,
            raw: false,
            low_latency: cfg.low_latency,
        };

        // Each thread opens its own device (COM objects stay on the thread that made them)
        // and reports back before entering its loop.
        let (ready_tx, ready_rx) =
            mpsc::channel::<Result<(Flow, String, usize, Option<u32>, u32), WinError>>();

        let processor = Processor::new(
            &cfg.settings,
            model,
            controls.clone(),
            meters.clone(),
            model_rx,
            retire_tx,
        );
        let capture = {
            let (stop, meters, ready_tx, id) = (
                stop.clone(),
                meters.clone(),
                ready_tx.clone(),
                cfg.input_id.clone(),
            );
            std::thread::Builder::new()
                .name("vidar-capture".into())
                .spawn(move || {
                    capture_thread(
                        id, in_opts, processor, producer, meters, stop, ready_tx, started,
                    )
                })
                .map_err(|e| explain(None, e.to_string()))?
        };
        let render = {
            let (stop, meters, id) = (stop.clone(), meters.clone(), cfg.output_id.clone());
            std::thread::Builder::new()
                .name("vidar-render".into())
                .spawn(move || render_thread(id, out_opts, consumer, meters, stop, ready_tx))
        };
        let render = match render {
            Ok(t) => t,
            Err(e) => {
                // Don't leave the capture thread holding the microphone.
                stop.store(true, Relaxed);
                let _ = capture.join();
                return Err(explain(None, e.to_string()));
            }
        };

        let mut info = StreamInfo {
            input_name: String::new(),
            output_name: String::new(),
            input_channels: 0,
            output_channels: 0,
            input_low_latency_frames: None,
            output_low_latency_frames: None,
            output_buffer_frames: 0,
        };
        let mut failure = None;
        for _ in 0..2 {
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok((Flow::Capture, name, ch, ll, _))) => {
                    info.input_name = name;
                    info.input_channels = ch;
                    info.input_low_latency_frames = ll;
                }
                Ok(Ok((Flow::Render, name, ch, ll, buf))) => {
                    info.output_name = name;
                    info.output_channels = ch;
                    info.output_low_latency_frames = ll;
                    info.output_buffer_frames = buf;
                }
                Ok(Err(e)) => {
                    failure.get_or_insert(explain(Some(e.code), e.to_string()));
                }
                Err(_) => {
                    failure.get_or_insert(explain(None, "audio device did not respond within 5 s"));
                }
            }
        }

        let engine = Engine {
            controls,
            meters,
            stop,
            threads: vec![capture, render],
            model_tx,
            retired,
            info,
            started,
        };
        match failure {
            None => Ok(engine),
            Some(e) => {
                engine.shutdown();
                Err(e)
            }
        }
    }

    pub fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }
    pub fn meters(&self) -> &Arc<Meters> {
        &self.meters
    }
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Switch the noise model without stopping (`None` = DSP only).
    pub fn set_model(&self, model: Option<Box<dyn Denoiser>>) {
        let _ = self.model_tx.send(model);
        self.collect_retired();
    }

    /// Free models the audio thread has replaced. Called from the UI tick.
    pub fn collect_retired(&self) {
        while self.retired.try_recv().is_ok() {}
    }

    pub fn health(&self) -> Health {
        if self.meters.device_lost.load(Relaxed) {
            return Health::DeviceLost;
        }
        if self.meters.stopped.load(Relaxed) {
            return Health::Stopped;
        }
        let last = self.meters.last_capture_ms.load(Relaxed);
        if now_ms(self.started).saturating_sub(last) > 2000 {
            return Health::Stalled;
        }
        Health::Running
    }

    fn shutdown(mut self) {
        self.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        self.collect_retired();
    }

    /// Stop both streams and wait for the threads.
    pub fn stop(self) {
        self.shutdown();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

type Ready = Sender<Result<(Flow, String, usize, Option<u32>, u32), WinError>>;

#[allow(clippy::too_many_arguments)]
fn capture_thread(
    id: Option<String>,
    opts: StreamOptions,
    mut processor: Processor,
    mut ring: rtrb::Producer<f32>,
    meters: Arc<Meters>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    ready: Ready,
    started: Instant,
) {
    let opened = (|| -> Result<_, WinError> {
        let en = wasapi::enumerator()?;
        let dev = wasapi::device(&en, id.as_deref(), Flow::Capture)?;
        let stream = wasapi::open(&dev, Flow::Capture, opts)?;
        let client = stream.capture_client()?;
        Ok((stream, client))
    })();
    let (stream, client) = match opened {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            meters.stopped.store(true, Relaxed);
            return;
        }
    };
    if let Err(e) = stream.start() {
        let _ = ready.send(Err(e));
        meters.stopped.store(true, Relaxed);
        return;
    }
    let _ = ready.send(Ok((
        Flow::Capture,
        stream.device_name.clone(),
        stream.channels,
        stream.low_latency_period,
        stream.buffer_frames,
    )));
    drop(ready);
    let _mmcss = MmcssGuard::pro_audio();
    meters.last_capture_ms.store(now_ms(started), Relaxed);

    let channels = stream.channels;
    let mut emit = |frame: &[f32; FRAME_SIZE]| {
        if ring.slots() >= FRAME_SIZE {
            for &s in frame.iter() {
                let _ = ring.push(s);
            }
        } else {
            meters.overruns.fetch_add(1, Relaxed);
        }
    };

    let mut next_position: Option<u64> = None;
    while !stop.load(Relaxed) {
        if !stream.wait(200) {
            continue;
        }
        loop {
            // SAFETY: standard WASAPI capture loop; every GetBuffer is matched by ReleaseBuffer.
            let packet = unsafe { client.GetNextPacketSize() };
            let n = match packet {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                        meters.device_lost.store(true, Relaxed);
                    }
                    meters.errors.fetch_add(1, Relaxed);
                    meters.stopped.store(true, Relaxed);
                    return;
                }
            };
            let _ = n;
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let mut position = 0u64;
            unsafe {
                if let Err(e) = client.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    None,
                ) {
                    if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                        meters.device_lost.store(true, Relaxed);
                    }
                    meters.errors.fetch_add(1, Relaxed);
                    meters.stopped.store(true, Relaxed);
                    return;
                }
                // Some USB drivers set DATA_DISCONTINUITY on every packet, so a glitch is
                // counted only when the device position actually skips ahead.
                let skipped = next_position.is_some_and(|expected| position > expected);
                let flagged_without_position =
                    position == 0 && flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
                if skipped || (flagged_without_position && next_position.is_some()) {
                    meters.capture_glitches.fetch_add(1, Relaxed);
                }
                next_position = (position != 0).then(|| position + u64::from(frames));
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    processor.push_silence(frames as usize, &mut emit);
                } else {
                    let samples =
                        std::slice::from_raw_parts(data as *const f32, frames as usize * channels);
                    processor.push_interleaved(samples, channels, &mut emit);
                }
                let _ = client.ReleaseBuffer(frames);
            }
            meters.last_capture_ms.store(now_ms(started), Relaxed);
        }
    }
}

fn render_thread(
    id: Option<String>,
    opts: StreamOptions,
    mut ring: rtrb::Consumer<f32>,
    meters: Arc<Meters>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    ready: Ready,
) {
    let opened = (|| -> Result<_, WinError> {
        let en = wasapi::enumerator()?;
        let dev = wasapi::device(&en, id.as_deref(), Flow::Render)?;
        let stream = wasapi::open(&dev, Flow::Render, opts)?;
        let client = stream.render_client()?;
        Ok((stream, client))
    })();
    let (stream, client) = match opened {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            meters.stopped.store(true, Relaxed);
            return;
        }
    };

    // Keep about one processing frame plus one render period queued.
    let period = stream.low_latency_period.unwrap_or(FRAME_SIZE as u32) as usize;
    let mut drift = DriftControl::new(FRAME_SIZE + period, FRAME_SIZE / 2);
    let channels = stream.channels;
    let buffer = stream.buffer_frames;

    // Pre-fill with silence so the device starts cleanly.
    // SAFETY: requesting the whole buffer before Start is allowed.
    unsafe {
        if let Ok(p) = client.GetBuffer(buffer) {
            std::ptr::write_bytes(p, 0, buffer as usize * channels * 4);
            let _ = client.ReleaseBuffer(buffer, 0);
        }
    }
    if let Err(e) = stream.start() {
        let _ = ready.send(Err(e));
        meters.stopped.store(true, Relaxed);
        return;
    }
    let _ = ready.send(Ok((
        Flow::Render,
        stream.device_name.clone(),
        channels,
        stream.low_latency_period,
        buffer,
    )));
    drop(ready);
    let _mmcss = MmcssGuard::pro_audio();
    let mut last = 0.0f32;

    while !stop.load(Relaxed) {
        if !stream.wait(200) {
            continue;
        }
        // SAFETY: padding/GetBuffer/ReleaseBuffer follow the WASAPI render contract.
        let padding = match unsafe { stream.client.GetCurrentPadding() } {
            Ok(p) => p,
            Err(e) => {
                if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                    meters.device_lost.store(true, Relaxed);
                }
                meters.errors.fetch_add(1, Relaxed);
                meters.stopped.store(true, Relaxed);
                return;
            }
        };
        let avail = buffer.saturating_sub(padding) as usize;
        if avail == 0 {
            continue;
        }
        let ptr = match unsafe { client.GetBuffer(avail as u32) } {
            Ok(p) => p as *mut f32,
            Err(e) => {
                if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                    meters.device_lost.store(true, Relaxed);
                }
                meters.errors.fetch_add(1, Relaxed);
                meters.stopped.store(true, Relaxed);
                return;
            }
        };
        // SAFETY: WASAPI gave us `avail * channels` writable floats.
        let out = unsafe { std::slice::from_raw_parts_mut(ptr, avail * channels) };

        let correction = drift.update(ring.slots(), avail);
        if !drift.primed() {
            out.fill(0.0);
        } else {
            if correction == Correction::Drop {
                let _ = ring.pop();
            }
            let mut underrun = false;
            for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
                let s = if i == 0 && correction == Correction::Repeat {
                    last
                } else {
                    match ring.pop() {
                        Ok(s) => s,
                        Err(_) => {
                            underrun = true;
                            0.0
                        }
                    }
                };
                last = s;
                frame.fill(s);
            }
            if underrun {
                meters.underruns.fetch_add(1, Relaxed);
                drift.reprime();
            }
            meters.drift_ppm.store(drift.ppm() as i64, Relaxed);
        }
        unsafe {
            let _ = client.ReleaseBuffer(avail as u32, 0);
        }
    }
}

/// Keep a retire channel type nameable for callers that want to own it.
pub type Retire = SyncSender<Box<dyn Denoiser>>;
