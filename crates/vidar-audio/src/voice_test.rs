//! Voice test: record the raw microphone once, then process that same recording with any
//! number of settings (offline, faster than real time) to compare them, and play any
//! version on the speakers. Audio stays in memory only.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use vidar_config::EngineSettings;
use vidar_dsp::{levels, AutoTone, Denoiser, Limiter, Studio, VoiceEq, EQ_BANDS, FRAME_SIZE};
use vidar_win::Flow;
use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;

use crate::explain::{explain, Explained};
use crate::processor::Processor;
use crate::shared::{Controls, Meters};
use crate::wasapi::{self, StreamOptions, WinError, SAMPLE_RATE};

/// Peak envelope with `bins` points (0..1), for drawing a waveform.
pub fn peaks(samples: &[f32], bins: usize) -> Vec<f32> {
    if samples.is_empty() || bins == 0 {
        return Vec::new();
    }
    let per = samples.len().div_ceil(bins);
    samples
        .chunks(per)
        .map(|c| c.iter().fold(0.0f32, |m, x| m.max(x.abs())).min(1.0))
        .collect()
}

/// RMS level in dBFS of each of `bins` equal parts of `samples` (the same parts as
/// [`peaks`]), floored at -100.
pub fn levels_db(samples: &[f32], bins: usize) -> Vec<f32> {
    if samples.is_empty() || bins == 0 {
        return Vec::new();
    }
    let per = samples.len().div_ceil(bins);
    samples
        .chunks(per)
        .map(|c| {
            let p = c.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / c.len() as f64;
            (10.0 * p.max(1e-10).log10()) as f32
        })
        .collect()
}

fn err(e: WinError) -> Explained {
    explain(Some(e.code), e.to_string())
}

/// Record up to `seconds` of the raw (mono-mixed) microphone, or until `stop()` returns true;
/// returns what was captured. `progress(fraction of seconds, level_db)`.
pub fn record_raw(
    input_id: Option<&str>,
    seconds: f64,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64, f64),
) -> Result<Vec<f32>, Explained> {
    let total = (f64::from(SAMPLE_RATE) * seconds) as usize;
    let mut out: Vec<f32> = Vec::with_capacity(total + FRAME_SIZE);
    let en = wasapi::enumerator().map_err(err)?;
    let dev = wasapi::device(&en, input_id, Flow::Capture).map_err(err)?;
    let stream = wasapi::open(
        &dev,
        Flow::Capture,
        StreamOptions {
            buffer_ms: 30,
            raw: false,
            low_latency: false,
        },
    )
    .map_err(err)?;
    let client = stream.capture_client().map_err(err)?;
    let ch = stream.channels;
    stream.start().map_err(err)?;
    while out.len() < total && !stop() {
        if !stream.wait(500) {
            continue;
        }
        // SAFETY: standard WASAPI capture loop.
        unsafe {
            while client.GetNextPacketSize().map_err(|e| err(e.into()))? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                client
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| err(e.into()))?;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n(0.0, frames as usize));
                } else {
                    let s = std::slice::from_raw_parts(data as *const f32, frames as usize * ch);
                    // Same downmix as the live engine.
                    out.extend(s.chunks_exact(ch).map(|f| {
                        if ch >= 2 {
                            (f[0] + f[1]) * 0.5
                        } else {
                            f[0]
                        }
                    }));
                }
                let _ = client.ReleaseBuffer(frames);
            }
        }
        let tail = &out[out.len().saturating_sub(FRAME_SIZE)..];
        progress(
            (out.len() as f64 / total as f64).min(1.0),
            levels(tail).rms_db,
        );
    }
    out.truncate(total);
    Ok(out)
}

/// One processed version of a recording.
#[derive(Debug, Clone)]
pub struct Processed {
    /// Same length as the original and aligned with it sample for sample.
    pub samples: Vec<f32>,
    /// Processing time as a percentage of one CPU core in real time.
    pub cpu_percent: f64,
    /// Input-to-output delay of the chain in milliseconds.
    pub delay_ms: f64,
}

/// Frames (10 ms each) processed before CPU timing starts.
const WARMUP_FRAMES: usize = 25;

/// Run `original` through the chain with `settings` and `model`, exactly as the live engine
/// would (same processor), and align the result with the input.
pub fn process(
    original: &[f32],
    settings: &EngineSettings,
    model: Option<Box<dyn Denoiser>>,
) -> Processed {
    let delay = model.as_ref().map_or(0, |m| m.delay_samples()) + FRAME_SIZE;
    let controls = Arc::new(Controls::from_settings(settings));
    let meters = Arc::new(Meters::new());
    let (_tx, rx) = mpsc::channel();
    let (rtx, _rrx) = mpsc::sync_channel(1);
    let mut processor = Processor::new(settings, model, controls, meters, rx, rtx);

    let mut out = Vec::with_capacity(original.len() + delay + FRAME_SIZE);
    // The first frames pay one-off costs (model warm-up, allocations) that the live engine
    // pays once at start, so they are left out of the CPU figure.
    let warm = (WARMUP_FRAMES * FRAME_SIZE).min(original.len() / 4);
    processor.push_interleaved(&original[..warm], 1, &mut |f| out.extend_from_slice(f));
    let t0 = Instant::now();
    processor.push_interleaved(&original[warm..], 1, &mut |f| out.extend_from_slice(f));
    let elapsed = t0.elapsed().as_secs_f64();
    // Flush the delay with silence so the tail comes out.
    processor.push_silence(delay + FRAME_SIZE, &mut |f| out.extend_from_slice(f));

    let samples: Vec<f32> = out.into_iter().skip(delay).take(original.len()).collect();
    let timed_secs = (original.len() - warm) as f64 / f64::from(SAMPLE_RATE);
    Processed {
        samples,
        cpu_percent: if timed_secs > 0.0 {
            elapsed / timed_secs * 100.0
        } else {
            0.0
        },
        delay_ms: delay as f64 * 1000.0 / f64::from(SAMPLE_RATE),
    }
}

/// What a processed version did to the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// How much quieter the pauses got (dB, higher = more noise removed).
    pub noise_removed_db: f64,
    /// Level change on the loudest (speech) frames in dB; near 0 = voice kept intact.
    pub voice_change_db: f64,
    /// Whether the recording has speech at all: its loud parts are at least 12 dB above its
    /// quiet parts. Without it the voice figure means nothing.
    pub speech_detected: bool,
}

fn frame_power(x: &[f32]) -> Vec<f64> {
    x.chunks(FRAME_SIZE)
        .map(|c| c.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / c.len() as f64)
        .collect()
}

/// Compare a processed version with the original, frame by frame.
///
/// `speech_prob` holds a voice-detector probability per 480-sample frame of the original
/// (RNNoise's). Speech frames are those above 0.6, noise frames those below 0.2; levels alone
/// would mistake a mouse click for speech. Without a detector, the loudest 25 % and quietest
/// 30 % of frames are used.
pub fn metrics(original: &[f32], processed: &[f32], speech_prob: Option<&[f64]>) -> Metrics {
    let po = frame_power(original);
    let pp = frame_power(processed);
    let n = po.len().min(pp.len());
    if n < 8 {
        return Metrics {
            noise_removed_db: 0.0,
            voice_change_db: 0.0,
            speech_detected: false,
        };
    }
    let (speech, noise): (Vec<usize>, Vec<usize>) = match speech_prob {
        Some(p) => {
            let m = n.min(p.len());
            (
                (0..m).filter(|&i| p[i] > 0.6).collect(),
                (0..m).filter(|&i| p[i] < 0.2).collect(),
            )
        }
        None => {
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|&a, &b| po[a].total_cmp(&po[b]));
            (
                order[n - (n * 25 / 100).max(1)..].to_vec(),
                order[..(n * 30 / 100).max(1)].to_vec(),
            )
        }
    };
    // Median of per-frame changes: a typical word or pause, not dominated by a few frames
    // still inside the gate's hold time.
    let median_change = |idx: &[usize]| {
        if idx.is_empty() {
            return 0.0;
        }
        let mut d: Vec<f64> = idx
            .iter()
            .map(|&i| 10.0 * ((pp[i] + 1e-12) / (po[i] + 1e-12)).log10())
            .collect();
        d.sort_by(f64::total_cmp);
        d[d.len() / 2]
    };
    let speech_detected = match speech_prob {
        Some(_) => speech.len() >= 40, // at least 0.4 s of confident speech
        None => true,
    };
    Metrics {
        noise_removed_db: (-median_change(&noise)).clamp(0.0, 80.0),
        voice_change_db: median_change(&speech).clamp(-40.0, 20.0),
        speech_detected,
    }
}

/// The finishing touches applied while a test recording plays, so changing them is heard at
/// once: voice tone and Studio sound.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LiveTone {
    pub eq_db: [f64; EQ_BANDS],
    pub studio: bool,
    pub auto_tone: bool,
}

/// What to play: several aligned versions of one clip, of which `current` picks the one heard.
pub struct Playback<'a> {
    /// Equal-length, sample-aligned versions of the clip.
    pub tracks: &'a [&'a [f32]],
    /// Index into `tracks`; may change while playing (switches with a short crossfade).
    pub current: &'a AtomicUsize,
    /// Sample to start at.
    pub start: usize,
    /// Part of the clip to play; with `looping` it repeats until cancelled.
    pub range: Range<usize>,
    pub looping: bool,
    /// Voice tone and Studio sound applied on the fly to every track but the first (the
    /// unprocessed original), read again for each buffer so changes are heard while playing.
    /// The tracks themselves are processed without them.
    pub tone: Option<&'a Mutex<LiveTone>>,
}

/// Crossfade length when switching tracks mid-playback (10 ms).
const SWITCH_FADE: usize = 480;

/// Play `pb` on `output_id` (the speakers). `progress(sample position heard)` while playing.
pub fn play(
    output_id: Option<&str>,
    pb: &Playback,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<(), Explained> {
    let len = pb.tracks.iter().map(|t| t.len()).min().unwrap_or(0);
    let range = pb.range.start.min(len)..pb.range.end.min(len);
    if range.is_empty() {
        return Ok(());
    }
    let span = range.len();
    let start = pb.start.clamp(range.start, range.end - 1) - range.start;
    let total = if pb.looping { usize::MAX } else { span - start };
    // k-th sample written -> position in the clip.
    let at = |k: usize| range.start + (start + k) % span;

    let en = wasapi::enumerator().map_err(err)?;
    let dev = wasapi::device(&en, output_id, Flow::Render).map_err(err)?;
    let stream = wasapi::open(
        &dev,
        Flow::Render,
        StreamOptions {
            buffer_ms: 40,
            raw: false,
            low_latency: false,
        },
    )
    .map_err(err)?;
    let client = stream.render_client().map_err(err)?;
    let ch = stream.channels;
    let track = |i: usize| pb.tracks[i.min(pb.tracks.len() - 1)];
    let (mut written, mut started) = (0usize, false);
    let mut heard_track = pb.current.load(Relaxed);
    // (previous track, samples of crossfade left)
    let mut fade: Option<(usize, usize)> = None;
    let mut eq = VoiceEq::new(f64::from(SAMPLE_RATE));
    let mut studio = Studio::new();
    let mut auto_tone = AutoTone::new();
    let mut limiter = Limiter::new();
    let mut mono: Vec<f32> = Vec::new();
    loop {
        if cancel.load(Relaxed) {
            break;
        }
        // SAFETY: padding / GetBuffer / ReleaseBuffer per the WASAPI render contract.
        let padding =
            unsafe { stream.client.GetCurrentPadding() }.map_err(|e| err(e.into()))? as usize;
        if written >= total {
            if padding == 0 {
                break;
            }
        } else {
            let want = pb.current.load(Relaxed);
            if want != heard_track {
                fade = Some((heard_track, SWITCH_FADE));
                heard_track = want;
            }
            let n = (stream.buffer_frames as usize - padding).min(total - written);
            if n > 0 {
                unsafe {
                    let p = client.GetBuffer(n as u32).map_err(|e| err(e.into()))? as *mut f32;
                    let out = std::slice::from_raw_parts_mut(p, n * ch);
                    mono.clear();
                    for k in 0..n {
                        let i = at(written + k);
                        let mut s = track(heard_track)[i];
                        if let Some((old, left)) = fade {
                            let g = left as f32 / SWITCH_FADE as f32;
                            s = s * (1.0 - g) + track(old)[i] * g;
                            fade = if left > 1 {
                                Some((old, left - 1))
                            } else {
                                None
                            };
                        }
                        mono.push(s);
                    }
                    if let Some(tone) = pb.tone.and_then(|m| m.lock().ok().map(|g| *g)) {
                        auto_tone.set_enabled(tone.auto_tone);
                        let auto = auto_tone.gains_db();
                        eq.set_gains(std::array::from_fn(|i| tone.eq_db[i] + auto[i]));
                        studio.set_enabled(tone.studio);
                        // The filters keep running on the original too, so switching to a
                        // processed version does not start them from silence.
                        let mut shaped = mono.clone();
                        eq.process(&mut shaped);
                        studio.process(&mut shaped);
                        if heard_track != 0 && (!eq.is_flat() || studio.is_active()) {
                            limiter.process(&mut shaped);
                            // Auto tone learns from the cleaned version being heard.
                            for f in shaped.chunks(FRAME_SIZE) {
                                auto_tone.analyze(f, &tone.eq_db);
                            }
                            mono = shaped;
                        }
                    }
                    for (frame, &s) in out.chunks_exact_mut(ch).zip(&mono) {
                        frame.fill(s);
                    }
                    client
                        .ReleaseBuffer(n as u32, 0)
                        .map_err(|e| err(e.into()))?;
                }
                written += n;
            }
            if !started {
                stream.start().map_err(err)?;
                started = true;
            }
        }
        progress(at(written.saturating_sub(padding)));
        stream.wait(30);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidar_config::Config;

    #[test]
    fn peaks_cover_the_clip() {
        let s: Vec<f32> = (0..1000)
            .map(|i| if i == 500 { -0.8 } else { 0.1 })
            .collect();
        let p = peaks(&s, 10);
        assert_eq!(p.len(), 10);
        assert_eq!(p[5], 0.8);
    }

    /// Speech bursts over steady noise: the chain must cut the pauses and keep the bursts.
    #[test]
    fn processing_removes_noise_in_pauses_and_keeps_speech() {
        let mut x = Vec::new();
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        // 0.4 s words and 1.2 s pauses: longer than the gate's 320 ms hold.
        for k in 0..640 {
            let speech = k % 160 >= 120;
            for i in 0..FRAME_SIZE {
                let n = 0.01 * noise();
                let s = if speech {
                    0.3 * ((i as f32) * 0.06).sin()
                } else {
                    0.0
                };
                x.push(n + s);
            }
        }
        let mut settings = EngineSettings::from_config(&Config::default(), false);
        settings.denoise_enabled = false;
        let out = process(&x, &settings, None);
        assert_eq!(out.samples.len(), x.len());
        let m = metrics(&x, &out.samples, None);
        assert!(m.noise_removed_db > 15.0, "noise removed {m:?}");
        assert!(m.voice_change_db.abs() < 3.0, "voice changed {m:?}");
    }
}

#[cfg(test)]
mod speech_tests {
    use super::*;

    #[test]
    fn steady_noise_is_not_speech() {
        let x: Vec<f32> = (0..48_000)
            .map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.02)
            .collect();
        let probs = vec![0.05; x.len() / FRAME_SIZE];
        assert!(!metrics(&x, &x, Some(&probs)).speech_detected);
    }
}
