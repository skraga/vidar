//! The models run on a real-time audio thread, so a frame must not touch the Rust heap.
//! A counting global allocator checks every engine (and Studio sound) after warm-up. (ONNX Runtime's own
//! arena is outside Rust's allocator and is not counted.)

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use vidar_denoise::{create, DenoiseSpec, Engine};
use vidar_dsp::{
    AutoTone, Frame, HighPass, OutputStage, SpeechLeveler, Studio, VoiceEq, EQ_BANDS, FRAME_SIZE,
    PCM_SCALE,
};

struct Counting;

thread_local! {
    /// Counting is per thread: only the thread doing the audio work is checked, not the
    /// test harness printing another test's result meanwhile.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

fn counting() -> bool {
    COUNTING.try_with(Cell::get).unwrap_or(false)
}

// SAFETY: forwards to the system allocator and only adds a counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, SeqCst);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if counting() {
            ALLOCS.fetch_add(1, SeqCst);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The counter itself is shared: tests that count must not run at the same time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn processing_a_frame_does_not_allocate() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/dpdfnet");
    let frames: Vec<Frame> = (0..200)
        .map(|n| {
            core::array::from_fn(|i| {
                let t = (n * FRAME_SIZE + i) as f32 / 48_000.0;
                ((t * 220.0 * std::f32::consts::TAU).sin() * 0.3 + (t * 7919.0).sin() * 0.05)
                    * PCM_SCALE
            })
        })
        .collect();

    for (engine, vad) in [
        (Engine::RnNoise, "rnnoise"),
        (Engine::DpdfNet, "rnnoise"),
        (Engine::DpdfNet, "spectral"),
        (Engine::DpdfNet8, "rnnoise"),
    ] {
        let spec = DenoiseSpec {
            engine,
            dpdfnet_vad: vad.into(),
            model_dirs: vec![models.clone()],
            ..DenoiseSpec::default()
        };
        let mut model = create(&spec).expect("model should load");
        for f in frames.iter().take(20) {
            model.process_frame(&mut f.clone());
        }
        ALLOCS.store(0, SeqCst);
        COUNTING.set(true);
        for f in &frames {
            let mut f = *f;
            model.process_frame(&mut f);
            model.align_level(-30.0);
        }
        COUNTING.set(false);
        let n = ALLOCS.load(SeqCst);
        assert_eq!(
            n,
            0,
            "{} / {vad}: {n} heap allocations in 200 frames",
            engine.as_str()
        );
    }
}

#[test]
fn studio_sound_does_not_allocate() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut studio = Studio::new();
    studio.set_enabled(true);
    let mut frame: Frame = core::array::from_fn(|i| (i as f32 * 0.05).sin() * 0.3);
    studio.process(&mut frame);
    ALLOCS.store(0, SeqCst);
    COUNTING.set(true);
    for k in 0..300 {
        // Switching on and off mid-stream (the crossfade) must not allocate either.
        if k % 50 == 0 {
            studio.set_enabled(k % 100 == 0);
        }
        studio.process(&mut frame);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCS.load(SeqCst), 0);
}

/// Auto tone learns and redesigns the equaliser (and the tone's own) while running.
#[test]
fn auto_tone_does_not_allocate() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut auto = AutoTone::new();
    let mut eq = VoiceEq::new(48_000.0);
    auto.set_enabled(true);
    let mut frame: Frame = core::array::from_fn(|i| (i as f32 * 0.05).sin() * 0.3);
    auto.analyze(&frame, &[0.0; EQ_BANDS]);
    ALLOCS.store(0, SeqCst);
    COUNTING.set(true);
    for k in 0..2000 {
        let tone = if k < 1000 {
            [0.0; EQ_BANDS]
        } else {
            [3.0, 2.0, 0.0, 0.0, -1.0, -2.0, -3.0]
        };
        let g = auto.gains_db();
        eq.set_gains(core::array::from_fn(|i| tone[i] + g[i]));
        eq.process(&mut frame);
        auto.analyze(&frame, &tone);
        frame = core::array::from_fn(|i| ((i + k * 480) as f32 * 0.05).sin() * 0.3);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCS.load(SeqCst), 0);
    assert_ne!(auto.gains_db(), [0.0; EQ_BANDS], "it learned something");
}

#[test]
fn output_stage_and_switches_do_not_allocate() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut output = OutputStage::new();
    let mut hpf = HighPass::new(80.0, 48_000.0);
    let mut leveler = SpeechLeveler::default();
    let mut frame: Frame = core::array::from_fn(|i| (i as f32 * 0.05).sin() * 0.9);
    output.apply_gain(&mut frame, 1.0);
    ALLOCS.store(0, SeqCst);
    COUNTING.set(true);
    for k in 0..300 {
        // Gain changes, limiting, and switching the rumble filter and Auto-level mid-stream.
        hpf.set_enabled(k % 100 < 50);
        leveler.set_enabled(k % 60 < 30);
        hpf.process(&mut frame);
        leveler.process(&mut frame, 0.9, -30.0, 0.0);
        output.apply_gain(&mut frame, 1.0 + (k % 7) as f64 * 0.2);
        output.limit(&mut frame);
    }
    COUNTING.set(false);
    assert_eq!(ALLOCS.load(SeqCst), 0);
}
