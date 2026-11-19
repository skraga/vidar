//! The models run on a real-time audio thread, so a frame must not touch the Rust heap.
//! A counting global allocator checks every engine after warm-up. (ONNX Runtime's own
//! arena is outside Rust's allocator and is not counted.)

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

use vidar_denoise::{create, DenoiseSpec, Engine};
use vidar_dsp::{Frame, FRAME_SIZE, PCM_SCALE};

struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator and only adds a counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(SeqCst) {
            ALLOCS.fetch_add(1, SeqCst);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(SeqCst) {
            ALLOCS.fetch_add(1, SeqCst);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn processing_a_frame_does_not_allocate() {
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
        COUNTING.store(true, SeqCst);
        for f in &frames {
            let mut f = *f;
            model.process_frame(&mut f);
            model.align_level(-30.0);
        }
        COUNTING.store(false, SeqCst);
        let n = ALLOCS.load(SeqCst);
        assert_eq!(
            n,
            0,
            "{} / {vad}: {n} heap allocations in 200 frames",
            engine.as_str()
        );
    }
}
