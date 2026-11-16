//! Flush subnormal floats to zero on the calling thread.
//!
//! When the input goes quiet, recurrent model state and filter tails decay toward zero and
//! pass through the subnormal range, where x86 arithmetic is many times slower. On a
//! real-time thread that shows up as CPU spikes, and dropouts, exactly when nobody talks.
//! Setting FTZ (flush results to zero) and DAZ (treat subnormal inputs as zero) removes the
//! slow path. Subnormals are below 1e-38, so flushing them changes nothing audible.
//!
//! The flags belong to the thread, so every thread that runs the chain must set them;
//! [`crate::process_mono_frame`] does so on each call, which costs a few cycles.

/// Enable flush-to-zero and denormals-are-zero for the current thread.
#[inline]
pub fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    {
        const FTZ_DAZ: u32 = (1 << 15) | (1 << 6);
        let mut csr: u32 = 0;
        // SAFETY: stmxcsr/ldmxcsr read and write the SSE control register through a valid
        // pointer to a local; only the FTZ and DAZ bits are changed. (`_mm_setcsr` is
        // deprecated in favour of inline assembly.)
        unsafe {
            core::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
            if csr & FTZ_DAZ != FTZ_DAZ {
                let new = csr | FTZ_DAZ;
                core::arch::asm!("ldmxcsr [{}]", in(reg) &new, options(nostack, preserves_flags, readonly));
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        const FZ: u64 = 1 << 24;
        let fpcr: u64;
        // SAFETY: reads and writes the floating-point control register; only FZ is set.
        unsafe {
            core::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
            if fpcr & FZ == 0 {
                core::arch::asm!("msr fpcr, {}", in(reg) fpcr | FZ, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnormals_become_zero() {
        std::thread::spawn(|| {
            let tiny = std::hint::black_box(f32::MIN_POSITIVE);
            assert!((tiny / std::hint::black_box(4.0)) > 0.0, "subnormal before");
            flush_denormals();
            let tiny = std::hint::black_box(f32::MIN_POSITIVE);
            assert_eq!(tiny / std::hint::black_box(4.0), 0.0, "flushed after");
        })
        .join()
        .unwrap();
    }
}
