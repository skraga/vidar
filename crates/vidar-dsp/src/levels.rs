/// Peak and RMS level of a frame in dBFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub peak_db: f64,
    pub rms_db: f64,
}

impl Levels {
    pub const SILENT: Levels = Levels {
        peak_db: -100.0,
        rms_db: -100.0,
    };
}

/// Peak and RMS in dBFS, floored at -100 dB.
///
/// Reference level is 1.0 for normalised audio, or 32767 when the frame is clearly in
/// int16 range (peak above 100).
pub fn levels(frame: &[f32]) -> Levels {
    if frame.is_empty() {
        return Levels::SILENT;
    }
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    for &x in frame {
        peak = peak.max(x.abs());
        sum_sq += f64::from(x) * f64::from(x);
    }
    // The RMS is rounded to float32, as in the reference vectors.
    let rms = f64::from(((sum_sq / frame.len() as f64) as f32).sqrt());
    let peak = f64::from(peak);
    let reference = if peak > 100.0 { 32767.0 } else { 1.0 };
    Levels {
        peak_db: 20.0 * (peak / reference).max(1e-5).log10(),
        rms_db: 20.0 * (rms / reference).max(1e-5).log10(),
    }
}

/// Soft-knee saturation above `knee`, so hard ADC clipping does not smear the model's input.
pub fn soft_preclip(frame: &mut [f32], knee: f32) {
    let span = 1.0 - knee;
    for x in frame.iter_mut() {
        let a = x.abs();
        if a > knee {
            let excess = a - knee;
            *x = x.signum() * (knee + span * (excess / span).tanh());
        }
    }
}

/// Transparent below `threshold`; tanh saturation above it, so peaks never hard-clip.
pub fn soft_limit(frame: &mut [f32], threshold: f32) {
    let scale = (1.0f64 - f64::from(threshold)) as f32;
    for x in frame.iter_mut() {
        let a = x.abs();
        if a > threshold {
            *x = x.signum() * (threshold + scale * ((a - threshold) / scale).tanh());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_square_is_zero_db() {
        let l = levels(&[1.0, -1.0, 1.0, -1.0]);
        assert!(l.peak_db.abs() < 1e-9);
        assert!(l.rms_db.abs() < 1e-6);
    }

    #[test]
    fn silence_floors_at_minus_100() {
        assert_eq!(levels(&[0.0; 480]), Levels::SILENT);
        assert_eq!(levels(&[]), Levels::SILENT);
    }

    #[test]
    fn int16_range_uses_pcm_reference() {
        let l = levels(&[32767.0, -32767.0]);
        assert!(l.peak_db.abs() < 1e-9);
    }

    #[test]
    fn limiter_is_transparent_below_threshold_and_bounded_above() {
        let mut x = [0.5f32, -0.84, 0.86, -2.0, 10.0];
        soft_limit(&mut x, 0.85);
        assert_eq!(x[0], 0.5);
        assert_eq!(x[1], -0.84);
        assert!(x[2] > 0.85 && x[2] < 0.87);
        assert!(x[3] < -0.85 && x[3] > -1.0);
        assert!(x[4] <= 1.0);
    }

    #[test]
    fn preclip_keeps_sign_and_stays_below_one() {
        let mut x = [0.7f32, 0.9, -0.95, 1.0];
        soft_preclip(&mut x, 0.75);
        assert_eq!(x[0], 0.7);
        assert!(x[1] > 0.75 && x[1] < 0.9);
        assert!(x[2] < -0.75 && x[2] > -0.95);
        assert!(x[3] < 1.0);
    }
}
