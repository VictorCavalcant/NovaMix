//! Spectrum analysis for the optional visualizer. Runs on the UI thread
//! from copies of the audio, never on the audio thread.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

use crate::util::power_to_db;

pub struct SpectrumAnalyzer {
    size: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    history: Vec<f32>,
    write: usize,
    time: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    smoothed: Vec<f32>,
    window_gain: f32,
}

impl SpectrumAnalyzer {
    /// `size` must be a power of two (e.g. 2048).
    pub fn new(size: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size);
        let window: Vec<f32> = (0..size).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()).collect();
        let window_gain = window.iter().sum::<f32>();
        Self {
            size,
            scratch: fft.make_scratch_vec(),
            spec: fft.make_output_vec(),
            fft,
            window,
            history: vec![0.0; size],
            write: 0,
            time: vec![0.0; size],
            smoothed: vec![-120.0; size / 2 + 1],
            window_gain,
        }
    }

    /// Feeds new mono samples (most recent last).
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.history[self.write] = s;
            self.write = (self.write + 1) % self.size;
        }
    }

    /// Computes the spectrum (dBFS per bin, smoothed with `decay` 0..1).
    pub fn compute(&mut self, decay: f32) -> &[f32] {
        for i in 0..self.size {
            let s = self.history[(self.write + i) % self.size];
            self.time[i] = s * self.window[i];
        }
        let _ = self.fft.process_with_scratch(&mut self.time, &mut self.spec, &mut self.scratch);
        let norm = 2.0 / self.window_gain;
        for (k, c) in self.spec.iter().enumerate() {
            let mag = c.norm() * norm;
            let db = power_to_db(mag * mag);
            let prev = self.smoothed[k];
            self.smoothed[k] = if db > prev { db } else { prev * decay + db * (1.0 - decay) };
        }
        &self.smoothed
    }

    /// Maps bins to `bands` log-spaced bands between `f_lo` and `f_hi`,
    /// returning `(centre_frequency, dB)` pairs.
    pub fn log_bands(&self, sample_rate: f32, bands: usize, f_lo: f32, f_hi: f32) -> Vec<(f32, f32)> {
        let bins = self.smoothed.len();
        let hz_per_bin = sample_rate / self.size as f32;
        let ratio = (f_hi / f_lo).powf(1.0 / bands as f32);
        let mut out = Vec::with_capacity(bands);
        let mut f = f_lo;
        for _ in 0..bands {
            let f2 = f * ratio;
            let b0 = ((f / hz_per_bin) as usize).min(bins - 1);
            let b1 = ((f2 / hz_per_bin).ceil() as usize).clamp(b0 + 1, bins);
            let v = self.smoothed[b0..b1].iter().cloned().fold(-120.0, f32::max);
            out.push(((f * f2).sqrt(), v));
            f = f2;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal;

    #[test]
    fn finds_tone_peak() {
        let mut a = SpectrumAnalyzer::new(2048);
        a.push(&signal::sine(1000.0, 48_000.0, 0.5, 4096));
        a.compute(0.0);
        let bands = a.log_bands(48_000.0, 64, 20.0, 20_000.0);
        let (f, db) = bands.iter().cloned().fold((0.0, -200.0), |acc, b| if b.1 > acc.1 { b } else { acc });
        assert!((f / 1000.0 - 1.0).abs() < 0.15, "peak at {f}");
        assert!((db - (-6.0)).abs() < 1.5, "level {db}");
    }
}
