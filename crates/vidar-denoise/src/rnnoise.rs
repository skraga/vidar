use std::ffi::c_void;
use std::ptr::NonNull;

use vidar_dsp::{Denoiser, Frame, FRAME_SIZE};

#[repr(C)]
struct DenoiseState {
    _private: [u8; 0],
}

extern "C" {
    fn rnnoise_get_frame_size() -> i32;
    fn rnnoise_create(model: *mut c_void) -> *mut DenoiseState;
    fn rnnoise_destroy(st: *mut DenoiseState);
    fn rnnoise_process_frame(st: *mut DenoiseState, out: *mut f32, input: *const f32) -> f32;
}

/// RNNoise's measured input-to-output delay: two frames (20 ms).
pub const RNNOISE_DELAY_SAMPLES: usize = 2 * FRAME_SIZE;

/// RNNoise noise suppressor with the built-in model.
pub struct RnNoise {
    state: NonNull<DenoiseState>,
    scratch: Frame,
}

// The state is only touched through `&mut self`.
unsafe impl Send for RnNoise {}

impl RnNoise {
    pub fn new() -> Result<Self, crate::Error> {
        // SAFETY: plain C calls; a null model pointer selects the built-in weights.
        let size = unsafe { rnnoise_get_frame_size() };
        if size as usize != FRAME_SIZE {
            return Err(crate::Error::Init(format!(
                "RNNoise frame size {size}, expected {FRAME_SIZE}"
            )));
        }
        let ptr = unsafe { rnnoise_create(std::ptr::null_mut()) };
        let state = NonNull::new(ptr)
            .ok_or_else(|| crate::Error::Init("rnnoise_create returned null".into()))?;
        Ok(Self {
            state,
            scratch: [0.0; FRAME_SIZE],
        })
    }

    /// Run one frame and return the speech probability, keeping the input unchanged.
    /// Used as the voice detector next to DPDFNet.
    pub fn probability_only(&mut self, frame: &Frame) -> f64 {
        self.scratch = *frame;
        // SAFETY: both pointers reference FRAME_SIZE floats; state is valid until drop.
        let p = unsafe {
            rnnoise_process_frame(
                self.state.as_ptr(),
                self.scratch.as_mut_ptr(),
                self.scratch.as_ptr(),
            )
        };
        f64::from(p)
    }
}

impl Denoiser for RnNoise {
    fn process_frame(&mut self, frame: &mut Frame) -> f64 {
        let ptr = frame.as_mut_ptr();
        // SAFETY: in-place processing is supported by RNNoise.
        let p = unsafe { rnnoise_process_frame(self.state.as_ptr(), ptr, ptr) };
        f64::from(p)
    }

    fn delay_samples(&self) -> usize {
        RNNOISE_DELAY_SAMPLES
    }

    fn reset(&mut self) {
        // RNNoise has no reset call; a fresh state is equivalent.
        // SAFETY: the old state is destroyed exactly once and replaced by a valid one.
        unsafe {
            let fresh = rnnoise_create(std::ptr::null_mut());
            if let Some(fresh) = NonNull::new(fresh) {
                rnnoise_destroy(self.state.as_ptr());
                self.state = fresh;
            }
        }
    }

    fn name(&self) -> &'static str {
        "rnnoise"
    }
}

impl Drop for RnNoise {
    fn drop(&mut self) {
        // SAFETY: created in `new`/`reset`, destroyed once here.
        unsafe { rnnoise_destroy(self.state.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_has_low_speech_probability() {
        let mut rn = RnNoise::new().unwrap();
        let mut p = 1.0;
        for _ in 0..50 {
            p = rn.process_frame(&mut [0.0; FRAME_SIZE]);
        }
        assert!(p < 0.1, "{p}");
    }

    #[test]
    fn reset_restores_initial_behaviour() {
        let input: Vec<Frame> = (0..20)
            .map(|k| core::array::from_fn(|i| ((i + k * 480) as f32 * 0.07).sin() * 8000.0))
            .collect();
        let mut rn = RnNoise::new().unwrap();
        let first: Vec<Frame> = input
            .iter()
            .map(|f| {
                let mut f = *f;
                rn.process_frame(&mut f);
                f
            })
            .collect();
        rn.reset();
        let second: Vec<Frame> = input
            .iter()
            .map(|f| {
                let mut f = *f;
                rn.process_frame(&mut f);
                f
            })
            .collect();
        assert_eq!(first, second);
    }
}
