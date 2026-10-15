//! Test-signal generators and analysis helpers.
//!
//! Used by unit tests, the integration test suite, the benchmark tool and
//! the engine's synthetic "test mode" source (which plays a local signal
//! without capturing or sending any real audio).

use std::f32::consts::PI;

/// Sine wave.
pub fn sine(freq: f32, sample_rate: f32, amplitude: f32, len: usize) -> Vec<f32> {
    // Phase computed in f64: f32 loses frequency accuracy on long signals.
    let w = 2.0 * std::f64::consts::PI * freq as f64 / sample_rate as f64;
    (0..len).map(|i| amplitude * ((w * i as f64).sin() as f32)).collect()
}

/// Single impulse at `at`.
pub fn impulse(len: usize, at: usize, amplitude: f32) -> Vec<f32> {
    let mut v = vec![0.0; len];
    if at < len {
        v[at] = amplitude;
    }
    v
}

/// Deterministic white noise (xorshift32), uniform in `[-amplitude, amplitude]`.
pub fn white_noise(len: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut rng = XorShift::new(seed);
    (0..len).map(|_| rng.next_bipolar() * amplitude).collect()
}

/// Logarithmic sine sweep from `f0` to `f1`.
pub fn log_sweep(f0: f32, f1: f32, sample_rate: f32, amplitude: f32, len: usize) -> Vec<f32> {
    let t_total = len as f64 / sample_rate as f64;
    let k = (f1 as f64 / f0 as f64).ln();
    (0..len)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            let phase = 2.0 * std::f64::consts::PI * f0 as f64 * t_total / k * ((t / t_total * k).exp() - 1.0);
            amplitude * phase.sin() as f32
        })
        .collect()
}

/// Magnitude of a single frequency component (Goertzel algorithm),
/// normalized so a full-scale sine reads 1.0.
pub fn goertzel_magnitude(buf: &[f32], freq: f32, sample_rate: f32) -> f32 {
    let n = buf.len() as f32;
    let w = 2.0 * std::f64::consts::PI * freq as f64 / sample_rate as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in buf {
        let s0 = x as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    (2.0 * power.max(0.0).sqrt() / n as f64) as f32
}

/// Tiny deterministic PRNG, real-time safe.
#[derive(Debug, Clone)]
pub struct XorShift(u32);

impl XorShift {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Kind of synthetic signal produced by [`TestToneGenerator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ToneKind {
    #[default]
    Sine,
    PinkNoise,
    WhiteNoise,
    /// One short beep per second, useful to check routing and latency.
    Beep,
}

/// Real-time safe synthetic source (no allocation after construction).
#[derive(Debug, Clone)]
pub struct TestToneGenerator {
    sample_rate: f32,
    kind: ToneKind,
    freq: f32,
    amplitude: f32,
    phase: f32,
    rng: XorShift,
    pink: [f32; 7],
    counter: u64,
}

impl TestToneGenerator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            kind: ToneKind::Sine,
            freq: 440.0,
            amplitude: 0.1,
            phase: 0.0,
            rng: XorShift::new(0x1234_5678),
            pink: [0.0; 7],
            counter: 0,
        }
    }

    pub fn set(&mut self, kind: ToneKind, freq: f32, level_db: f32) {
        self.kind = kind;
        self.freq = freq.clamp(20.0, self.sample_rate * 0.45);
        self.amplitude = crate::util::db_to_gain(level_db.clamp(-60.0, 0.0));
    }

    #[inline]
    fn next_pink(&mut self) -> f32 {
        // Paul Kellet's economy pink noise filter.
        let w = self.rng.next_bipolar();
        let b = &mut self.pink;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.153_852;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }

    /// Fills a mono buffer.
    pub fn fill(&mut self, out: &mut [f32]) {
        let inc = 2.0 * PI * self.freq / self.sample_rate;
        let beep_len = (self.sample_rate * 0.1) as u64;
        let period = self.sample_rate as u64;
        for s in out.iter_mut() {
            *s = match self.kind {
                ToneKind::Sine => {
                    let v = self.phase.sin();
                    self.phase = (self.phase + inc) % (2.0 * PI);
                    v
                }
                ToneKind::WhiteNoise => self.rng.next_bipolar(),
                ToneKind::PinkNoise => self.next_pink(),
                ToneKind::Beep => {
                    let pos = self.counter % period.max(1);
                    let v = if pos < beep_len { self.phase.sin() } else { 0.0 };
                    self.phase = (self.phase + inc) % (2.0 * PI);
                    v
                }
            } * self.amplitude;
            self.counter = self.counter.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goertzel_measures_amplitude() {
        let x = sine(1000.0, 48_000.0, 0.25, 48_000);
        assert!((goertzel_magnitude(&x, 1000.0, 48_000.0) - 0.25).abs() < 1e-3);
        assert!(goertzel_magnitude(&x, 3000.0, 48_000.0) < 1e-3);
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = white_noise(1000, 0.5, 42);
        let b = white_noise(1000, 0.5, 42);
        assert_eq!(a, b);
        assert!(a.iter().all(|x| x.abs() <= 0.5));
    }

    #[test]
    fn test_tone_levels() {
        let mut g = TestToneGenerator::new(48_000.0);
        g.set(ToneKind::Sine, 1000.0, -20.0);
        let mut buf = vec![0.0; 48_000];
        g.fill(&mut buf);
        assert!((crate::util::peak(&buf) - 0.1).abs() < 1e-3);
        g.set(ToneKind::PinkNoise, 1000.0, -6.0);
        g.fill(&mut buf);
        assert!(crate::util::peak(&buf) < 1.0);
    }
}
