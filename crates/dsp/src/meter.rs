//! Level metering (peak, RMS, clip detection) computed per block on the
//! audio thread. Peak hold and ballistics for display live in the UI.

use crate::util::time_constant_coeff;

/// Samples at or above this magnitude count as clipped (≈ -0.1 dBFS).
pub const CLIP_LEVEL: f32 = 0.989;

/// Result of metering one block.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeterReading {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub clipped: bool,
}

/// Stereo peak/RMS meter with an RMS integration time of ~300 ms.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    ms: [f32; 2],
    coeff: f32,
}

impl LevelMeter {
    pub fn new(sample_rate: f32) -> Self {
        Self { ms: [0.0; 2], coeff: time_constant_coeff(300.0, sample_rate) }
    }

    pub fn reset(&mut self) {
        self.ms = [0.0; 2];
    }

    /// Meters a block. When `right` is `None` the left reading is mirrored.
    pub fn process(&mut self, left: &[f32], right: Option<&[f32]>) -> MeterReading {
        let mut r = MeterReading::default();
        let c = self.coeff;
        let mut ms = self.ms[0];
        let mut pk = 0.0_f32;
        for &x in left {
            pk = pk.max(x.abs());
            ms = c * ms + (1.0 - c) * x * x;
        }
        self.ms[0] = if ms < 1e-20 { 0.0 } else { ms };
        r.peak[0] = pk;
        r.rms[0] = self.ms[0].sqrt();
        match right {
            Some(right) => {
                let mut ms = self.ms[1];
                let mut pk = 0.0_f32;
                for &x in right {
                    pk = pk.max(x.abs());
                    ms = c * ms + (1.0 - c) * x * x;
                }
                self.ms[1] = if ms < 1e-20 { 0.0 } else { ms };
                r.peak[1] = pk;
                r.rms[1] = self.ms[1].sqrt();
            }
            None => {
                self.ms[1] = self.ms[0];
                r.peak[1] = r.peak[0];
                r.rms[1] = r.rms[0];
            }
        }
        r.clipped = r.peak[0] >= CLIP_LEVEL || r.peak[1] >= CLIP_LEVEL;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, util};

    #[test]
    fn reads_sine_peak_and_rms() {
        let mut m = LevelMeter::new(48_000.0);
        let x = signal::sine(1000.0, 48_000.0, 0.5, 96_000);
        let r = m.process(&x, None);
        assert!((r.peak[0] - 0.5).abs() < 1e-3);
        assert!((util::gain_to_db(r.rms[0]) - util::gain_to_db(0.5 / 2f32.sqrt())).abs() < 0.2);
        assert!(!r.clipped);
        assert_eq!(r.peak[0], r.peak[1]);
    }

    #[test]
    fn detects_clipping() {
        let mut m = LevelMeter::new(48_000.0);
        let r = m.process(&[0.0, 1.0, 0.2], Some(&[0.0, 0.1, 0.0]));
        assert!(r.clipped);
    }
}
