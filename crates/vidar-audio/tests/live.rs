//! Live engine smoke test on the real default devices. Opt-in: set VIDAR_LIVE_TESTS=1.
//! Runs muted, so the output device only ever receives silence.

#![cfg(windows)]

use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use vidar_audio::{Engine, EngineConfig, Health};
use vidar_config::{Config, EngineSettings};

#[test]
fn engine_runs_on_default_devices_muted() {
    if std::env::var("VIDAR_LIVE_TESTS").is_err() {
        return;
    }
    let cfg = EngineConfig {
        input_id: None,
        output_id: None,
        settings: EngineSettings::from_config(&Config::default(), false),
        raw_capture: false,
        low_latency: true,
        bluetooth_buffer: false,
    };
    let engine = Engine::start(cfg, None).expect("engine starts");
    engine.controls().muted.store(true, Relaxed);
    println!("{:#?}", engine.info());
    std::thread::sleep(Duration::from_secs(3));
    let m = engine.meters().snapshot();
    println!("{m:#?}");
    assert_eq!(engine.health(), Health::Running);
    assert!(
        m.frames >= 250,
        "about 300 frames expected in 3 s, got {}",
        m.frames
    );
    assert_eq!(m.errors, 0);
    engine.stop();
}

#[test]
fn capture_packet_flags() {
    if std::env::var("VIDAR_LIVE_TESTS").is_err() {
        return;
    }
    use vidar_audio::wasapi::{self, StreamOptions};
    use vidar_win::Flow;
    let en = wasapi::enumerator().unwrap();
    let dev = wasapi::device(&en, None, Flow::Capture).unwrap();
    for low_latency in [false, true] {
        let s = wasapi::open(
            &dev,
            Flow::Capture,
            StreamOptions {
                buffer_ms: 20,
                raw: false,
                low_latency,
            },
        )
        .unwrap();
        let c = s.capture_client().unwrap();
        s.start().unwrap();
        let mut seen = Vec::new();
        while seen.len() < 12 {
            if !s.wait(500) {
                continue;
            }
            unsafe {
                while c.GetNextPacketSize().unwrap() > 0 {
                    let (mut d, mut n, mut f, mut pos) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                    c.GetBuffer(&mut d, &mut n, &mut f, Some(&mut pos), None)
                        .unwrap();
                    seen.push((n, f, pos));
                    c.ReleaseBuffer(n).unwrap();
                }
            }
        }
        println!(
            "low_latency={low_latency} buffer={} ll={:?} packets (frames, flags, devpos): {:?}",
            s.buffer_frames,
            s.low_latency_period,
            &seen[..12]
        );
    }
}
