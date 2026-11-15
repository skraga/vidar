//! Clock-drift correction between the microphone and the cable.
//!
//! The two devices run on separate clocks, so over minutes the buffer between them slowly
//! fills or drains. The render side asks [`DriftControl`] once per callback what to do: keep
//! going, drop one sample, or repeat one sample. Corrections are at most one sample per
//! callback (about 100 samples per second at 10 ms periods, 2000 ppm), far more than real
//! clock drift (typically under 100 ppm), and a single sample is inaudible.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    None,
    /// Skip one input sample (buffer too full).
    Drop,
    /// Output the last sample twice (buffer too empty).
    Repeat,
}

#[derive(Debug, Clone)]
pub struct DriftControl {
    target: f64,
    band: f64,
    avg: f64,
    alpha: f64,
    primed: bool,
    corrections: i64,
    samples: u64,
}

impl DriftControl {
    /// `target` buffer fill in samples, `band` dead zone either side.
    pub fn new(target: usize, band: usize) -> Self {
        Self {
            target: target as f64,
            band: band as f64,
            avg: target as f64,
            // ~1 s time constant at 100 callbacks per second.
            alpha: 0.01,
            primed: false,
            corrections: 0,
            samples: 0,
        }
    }

    /// True once the buffer first reached the target; before that the render side outputs
    /// silence so playback starts with a full cushion.
    pub fn primed(&self) -> bool {
        self.primed
    }

    /// Feed the buffer fill seen at the start of a render callback that will consume
    /// `consume` samples.
    pub fn update(&mut self, fill: usize, consume: usize) -> Correction {
        self.samples += consume as u64;
        if !self.primed {
            if fill as f64 >= self.target {
                self.primed = true;
                self.avg = fill as f64;
            }
            return Correction::None;
        }
        self.avg += self.alpha * (fill as f64 - self.avg);
        if self.avg > self.target + self.band {
            self.corrections += 1;
            Correction::Drop
        } else if self.avg < self.target - self.band {
            self.corrections -= 1;
            Correction::Repeat
        } else {
            Correction::None
        }
    }

    /// Net correction rate in parts per million (positive: mic clock faster than cable).
    pub fn ppm(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.corrections as f64 * 1e6 / self.samples as f64
        }
    }

    /// Start over (after an underrun empties the buffer).
    pub fn reprime(&mut self) {
        self.primed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate a producer slightly faster or slower than the consumer and check the
    /// buffer stays near the target.
    fn simulate(producer_ppm: f64) -> (DriftControl, Vec<usize>) {
        let mut ctl = DriftControl::new(960, 240);
        let mut fill = 0.0f64;
        let mut fills = Vec::new();
        for _ in 0..(100 * 600) {
            fill += 480.0 * (1.0 + producer_ppm * 1e-6);
            let mut consume = 480usize;
            fills.push(fill as usize);
            match ctl.update(fill as usize, consume) {
                Correction::Drop => consume += 1,
                Correction::Repeat => consume -= 1,
                Correction::None => {}
            }
            if ctl.primed() {
                fill = (fill - consume as f64).max(0.0);
            }
        }
        (ctl, fills)
    }

    #[test]
    fn holds_buffer_near_target_with_fast_mic() {
        let (ctl, fills) = simulate(300.0);
        let tail = &fills[fills.len() - 1000..];
        assert!(
            tail.iter().all(|&f| (600..1300).contains(&f)),
            "fill drifted: {:?}",
            &tail[..5]
        );
        assert!(ctl.ppm() > 200.0 && ctl.ppm() < 400.0, "ppm {}", ctl.ppm());
    }

    #[test]
    fn holds_buffer_near_target_with_slow_mic() {
        let (ctl, fills) = simulate(-300.0);
        let tail = &fills[fills.len() - 1000..];
        assert!(tail.iter().all(|&f| (600..1300).contains(&f)));
        assert!(ctl.ppm() < -200.0);
    }

    #[test]
    fn no_corrections_when_clocks_match() {
        let (ctl, _) = simulate(0.0);
        assert_eq!(ctl.ppm(), 0.0);
    }

    #[test]
    fn waits_for_prime() {
        let mut ctl = DriftControl::new(960, 240);
        assert_eq!(ctl.update(100, 480), Correction::None);
        assert!(!ctl.primed());
        ctl.update(960, 480);
        assert!(ctl.primed());
    }
}
