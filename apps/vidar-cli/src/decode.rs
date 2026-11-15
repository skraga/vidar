//! Decode any supported file to 48 kHz float32 with 1 or 2 channels.

use std::fs::File;
use std::path::Path;

use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const SAMPLE_RATE: usize = 48_000;

/// Planar audio: `channels[c][i]`.
pub struct Decoded {
    pub channels: Vec<Vec<f32>>,
}

impl Decoded {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

fn read_interleaved(path: &Path) -> Result<(Vec<f32>, usize, usize), String> {
    let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("unsupported or unreadable audio file: {e}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("no audio track found")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio track found")?;
    let mut channels = params.channels.as_ref().map_or(0, |c| c.count());
    let mut rate = params.sample_rate.unwrap_or(0) as usize;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;

    let mut samples = Vec::new();
    let mut packet_samples: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("read error: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = decoded.spec();
                channels = spec.channels().count();
                rate = spec.rate() as usize;
                decoded.copy_to_vec_interleaved(&mut packet_samples);
                samples.extend_from_slice(&packet_samples);
            }
            // Skip corrupt packets, as most players do.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode error: {e}")),
        }
    }
    if channels == 0 || rate == 0 {
        return Err("could not determine channel count or sample rate".into());
    }
    Ok((samples, channels, rate))
}

/// Convert `channels`-channel interleaved audio to `target` planar channels:
/// to mono by averaging all channels, to stereo by copying mono or taking the first two.
fn to_planar(samples: &[f32], channels: usize, target: usize) -> Vec<Vec<f32>> {
    let frames = samples.len() / channels;
    let mut out = vec![Vec::with_capacity(frames); target];
    for f in samples.chunks_exact(channels) {
        if target == 1 {
            out[0].push(f.iter().sum::<f32>() / channels as f32);
        } else {
            out[0].push(f[0]);
            out[1].push(if channels > 1 { f[1] } else { f[0] });
        }
    }
    out
}

fn resample(planar: Vec<Vec<f32>>, from: usize) -> Result<Vec<Vec<f32>>, String> {
    if from == SAMPLE_RATE {
        return Ok(planar);
    }
    let n_ch = planar.len();
    let n_in = planar[0].len();
    let expected = (n_in as u128 * SAMPLE_RATE as u128 / from as u128) as usize;
    let mut rs = Fft::<f32>::new(from, SAMPLE_RATE, 1024, n_ch, FixedSync::Both)
        .map_err(|e| e.to_string())?;
    let mut out = vec![vec![0.0f32; rs.process_all_needed_output_len(n_in)]; n_ch];
    let out_len = out[0].len();
    let input = SequentialSliceOfVecs::new(&planar, n_ch, n_in).map_err(|e| e.to_string())?;
    let mut output =
        SequentialSliceOfVecs::new_mut(&mut out, n_ch, out_len).map_err(|e| e.to_string())?;
    // Chunks the clip internally and trims the resampler's startup delay.
    let (_, produced) = rs
        .process_all_into_buffer(&input, &mut output, n_in, None)
        .map_err(|e| e.to_string())?;
    let len = expected.min(produced);
    for ch in &mut out {
        ch.truncate(len);
    }
    Ok(out)
}

/// Decode `path` to 48 kHz with `target_channels` (1 or 2).
pub fn decode(path: &Path, target_channels: usize) -> Result<Decoded, String> {
    let (samples, channels, rate) = read_interleaved(path)?;
    let planar = to_planar(&samples, channels, target_channels);
    Ok(Decoded {
        channels: resample(planar, rate)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, rate: u32, channels: u16, frames: &[Vec<i16>]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for f in frames {
            for &s in f {
                w.write_sample(s).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    #[test]
    fn mono_48k_wav_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.wav");
        let frames: Vec<Vec<i16>> = (0..1000).map(|i| vec![(i * 30 - 15000) as i16]).collect();
        write_wav(&p, 48_000, 1, &frames);
        let d = decode(&p, 1).unwrap();
        assert_eq!(d.frames(), 1000);
        assert_eq!(d.channels[0][10], (10 * 30 - 15000) as f32 / 32768.0);
    }

    #[test]
    fn stereo_downmix_averages() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.wav");
        write_wav(&p, 48_000, 2, &vec![vec![16384, 0]; 100]);
        let d = decode(&p, 1).unwrap();
        assert!((d.channels[0][50] - 0.25).abs() < 1e-6);
        let d2 = decode(&p, 2).unwrap();
        assert_eq!(d2.channels.len(), 2);
        assert_eq!(d2.channels[1][50], 0.0);
    }

    #[test]
    fn resampling_keeps_duration_and_pitch() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.wav");
        // 1 s of 1 kHz at 44.1 kHz.
        let frames: Vec<Vec<i16>> = (0..44_100)
            .map(|i| {
                vec![
                    (8000.0 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin())
                        as i16,
                ]
            })
            .collect();
        write_wav(&p, 44_100, 1, &frames);
        let d = decode(&p, 1).unwrap();
        assert_eq!(d.frames(), 48_000);
        // Count zero crossings in the middle: 1 kHz -> ~2 per ms.
        let mid = &d.channels[0][12_000..36_000];
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((990..1010).contains(&crossings), "crossings {crossings}");
    }
}
