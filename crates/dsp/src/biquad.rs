//! Second-order IIR sections (RBJ "Audio EQ Cookbook" designs) in
//! transposed direct form II, with click-free coefficient interpolation.

use std::f32::consts::PI;

/// Normalized biquad coefficients (`a0 == 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for BiquadCoeffs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[inline]
fn clamp_freq(freq: f32, sample_rate: f32) -> f32 {
    freq.clamp(1.0, sample_rate * 0.49)
}

#[inline]
fn clamp_q(q: f32) -> f32 {
    if q.is_finite() { q.clamp(0.025, 40.0) } else { std::f32::consts::FRAC_1_SQRT_2 }
}

impl BiquadCoeffs {
    /// Pass-through filter.
    pub const IDENTITY: Self = Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    fn normalized(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        let inv = 1.0 / a0;
        Self { b0: b0 * inv, b1: b1 * inv, b2: b2 * inv, a1: a1 * inv, a2: a2 * inv }
    }

    fn omega(freq: f32, sample_rate: f32, q: f32) -> (f32, f32, f32) {
        let w0 = 2.0 * PI * clamp_freq(freq, sample_rate) / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * clamp_q(q));
        (cos, alpha, sin)
    }

    /// Second-order low-pass.
    pub fn lowpass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        let b1 = 1.0 - cos;
        Self::normalized(b1 * 0.5, b1, b1 * 0.5, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Second-order high-pass.
    pub fn highpass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        let b0 = (1.0 + cos) * 0.5;
        Self::normalized(b0, -(1.0 + cos), b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Band-pass with 0 dB peak gain. Its complement (`x - bandpass(x)`) is
    /// exactly the [`notch`](Self::notch) with the same frequency and Q,
    /// which the de-esser relies on.
    pub fn bandpass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        Self::normalized(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Band-reject (notch).
    pub fn notch(sample_rate: f32, freq: f32, q: f32) -> Self {
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        Self::normalized(1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Peaking (bell) equalizer.
    pub fn peaking(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        Self::normalized(1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
    }

    /// Low shelf.
    pub fn low_shelf(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) - (a - 1.0) * cos + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sa),
            (a + 1.0) + (a - 1.0) * cos + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sa,
        )
    }

    /// High shelf.
    pub fn high_shelf(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let (cos, alpha, _) = Self::omega(freq, sample_rate, q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) + (a - 1.0) * cos + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sa),
            (a + 1.0) - (a - 1.0) * cos + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sa,
        )
    }

    /// First-order high-pass (6 dB/octave), bilinear transform.
    pub fn first_order_highpass(sample_rate: f32, freq: f32) -> Self {
        let k = (PI * clamp_freq(freq, sample_rate) / sample_rate).tan();
        let norm = 1.0 / (1.0 + k);
        Self { b0: norm, b1: -norm, b2: 0.0, a1: (k - 1.0) * norm, a2: 0.0 }
    }

    /// First-order low-pass (6 dB/octave), bilinear transform.
    pub fn first_order_lowpass(sample_rate: f32, freq: f32) -> Self {
        let k = (PI * clamp_freq(freq, sample_rate) / sample_rate).tan();
        let norm = 1.0 / (1.0 + k);
        Self { b0: k * norm, b1: k * norm, b2: 0.0, a1: (k - 1.0) * norm, a2: 0.0 }
    }

    /// Magnitude response in dB at `freq` Hz. Used by the EQ curve display
    /// and by the frequency-response tests.
    pub fn magnitude_db(&self, freq: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * freq as f64 / sample_rate as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (b0, b1, b2, a1, a2) = (self.b0 as f64, self.b1 as f64, self.b2 as f64, self.a1 as f64, self.a2 as f64);
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30);
        (10.0 * mag2.max(1e-24).log10()) as f32
    }

    /// True when the poles lie inside the unit circle.
    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && self.a1.abs() < 1.0 + self.a2
    }
}

/// Number of samples over which coefficient changes are interpolated.
const COEFF_RAMP: u32 = 64;

/// Stereo biquad section with per-channel state and smooth coefficient updates.
#[derive(Debug, Clone)]
pub struct Biquad {
    coeffs: BiquadCoeffs,
    target: BiquadCoeffs,
    delta: BiquadCoeffs,
    ramp: u32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(BiquadCoeffs::IDENTITY)
    }
}

impl Biquad {
    pub fn new(coeffs: BiquadCoeffs) -> Self {
        Self { coeffs, target: coeffs, delta: BiquadCoeffs { b0: 0.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }, ramp: 0, z1: [0.0; 2], z2: [0.0; 2] }
    }

    /// Sets new coefficients, interpolating over a short ramp so moving an
    /// EQ band while audio plays does not click.
    pub fn set_coeffs(&mut self, target: BiquadCoeffs) {
        if target == self.target {
            return;
        }
        self.target = target;
        let n = COEFF_RAMP as f32;
        self.delta = BiquadCoeffs {
            b0: (target.b0 - self.coeffs.b0) / n,
            b1: (target.b1 - self.coeffs.b1) / n,
            b2: (target.b2 - self.coeffs.b2) / n,
            a1: (target.a1 - self.coeffs.a1) / n,
            a2: (target.a2 - self.coeffs.a2) / n,
        };
        self.ramp = COEFF_RAMP;
    }

    /// Sets coefficients immediately (no interpolation).
    pub fn set_coeffs_immediate(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = coeffs;
        self.target = coeffs;
        self.ramp = 0;
    }

    /// Coefficients the filter is ramping towards (or has reached).
    pub fn target_coeffs(&self) -> BiquadCoeffs {
        self.target
    }

    pub fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    #[inline]
    fn step_ramp(&mut self) {
        if self.ramp > 0 {
            self.ramp -= 1;
            if self.ramp == 0 {
                self.coeffs = self.target;
            } else {
                self.coeffs.b0 += self.delta.b0;
                self.coeffs.b1 += self.delta.b1;
                self.coeffs.b2 += self.delta.b2;
                self.coeffs.a1 += self.delta.a1;
                self.coeffs.a2 += self.delta.a2;
            }
        }
    }

    /// Processes one sample of channel `ch` without advancing the
    /// coefficient ramp (callers that interleave channels use
    /// [`tick`](Self::tick) once per frame).
    #[inline]
    pub fn process_sample(&mut self, x: f32, ch: usize) -> f32 {
        let c = &self.coeffs;
        let y = c.b0 * x + self.z1[ch];
        self.z1[ch] = c.b1 * x - c.a1 * y + self.z2[ch];
        self.z2[ch] = c.b2 * x - c.a2 * y;
        y
    }

    /// Advances the coefficient interpolation by one frame.
    #[inline]
    pub fn tick(&mut self) {
        self.step_ramp();
    }

    /// Processes a mono buffer (channel 0).
    pub fn process_mono(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            self.step_ramp();
            *s = self.process_sample(*s, 0);
        }
        self.flush_denormals();
    }

    /// Processes left (and optionally right) buffers in place.
    pub fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        match right {
            None => self.process_mono(left),
            Some(right) => {
                let n = left.len().min(right.len());
                for i in 0..n {
                    self.step_ramp();
                    left[i] = self.process_sample(left[i], 0);
                    right[i] = self.process_sample(right[i], 1);
                }
                self.flush_denormals();
            }
        }
    }

    #[inline]
    fn flush_denormals(&mut self) {
        for ch in 0..2 {
            if self.z1[ch].abs() < 1e-25 {
                self.z1[ch] = 0.0;
            }
            if self.z2[ch].abs() < 1e-25 {
                self.z2[ch] = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal;

    const SR: f32 = 48_000.0;

    fn measured_gain_db(coeffs: BiquadCoeffs, freq: f32) -> f32 {
        let mut f = Biquad::new(coeffs);
        let mut buf = signal::sine(freq, SR, 1.0, 24_000);
        f.process_mono(&mut buf);
        // Skip the transient.
        crate::util::gain_to_db(crate::util::rms(&buf[12_000..]) * std::f32::consts::SQRT_2)
    }

    #[test]
    fn highpass_attenuates_lows_and_passes_highs() {
        let c = BiquadCoeffs::highpass(SR, 100.0, std::f32::consts::FRAC_1_SQRT_2);
        assert!((c.magnitude_db(100.0, SR) + 3.01).abs() < 0.05);
        assert!(c.magnitude_db(25.0, SR) < -23.0);
        assert!(c.magnitude_db(1000.0, SR).abs() < 0.1);
        let measured = measured_gain_db(c, 1000.0);
        assert!(measured.abs() < 0.1, "{measured}");
        let measured = measured_gain_db(c, 30.0);
        assert!(measured < -20.0, "{measured}");
    }

    #[test]
    fn peaking_hits_requested_gain() {
        for gain in [-12.0, -3.0, 3.0, 12.0] {
            let c = BiquadCoeffs::peaking(SR, 1000.0, 1.0, gain);
            assert!((c.magnitude_db(1000.0, SR) - gain).abs() < 0.01);
            assert!(c.magnitude_db(20.0, SR).abs() < 0.1);
            let measured = measured_gain_db(c, 1000.0);
            assert!((measured - gain).abs() < 0.1, "gain {gain}: {measured}");
        }
    }

    #[test]
    fn shelves_reach_gain_far_from_corner() {
        let ls = BiquadCoeffs::low_shelf(SR, 200.0, 0.707, 6.0);
        assert!((ls.magnitude_db(20.0, SR) - 6.0).abs() < 0.2);
        assert!(ls.magnitude_db(10_000.0, SR).abs() < 0.1);
        let hs = BiquadCoeffs::high_shelf(SR, 4000.0, 0.707, -6.0);
        assert!((hs.magnitude_db(20_000.0, SR) + 6.0).abs() < 0.3);
        assert!(hs.magnitude_db(100.0, SR).abs() < 0.1);
    }

    #[test]
    fn bandpass_plus_notch_is_identity() {
        let bp = BiquadCoeffs::bandpass(SR, 6000.0, 2.0);
        let no = BiquadCoeffs::notch(SR, 6000.0, 2.0);
        for f in [100.0, 1000.0, 6000.0, 12_000.0] {
            let mut a = Biquad::new(bp);
            let mut b = Biquad::new(no);
            let x = signal::sine(f, SR, 0.5, 2048);
            let mut ya = x.clone();
            let mut yb = x.clone();
            a.process_mono(&mut ya);
            b.process_mono(&mut yb);
            for i in 0..x.len() {
                assert!((ya[i] + yb[i] - x[i]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn coefficients_are_stable_over_parameter_space() {
        for &f in &[20.0, 100.0, 1000.0, 10_000.0, 23_000.0] {
            for &q in &[0.1, 0.707, 4.0, 18.0] {
                for &g in &[-18.0, 0.0, 18.0] {
                    assert!(BiquadCoeffs::peaking(SR, f, q, g).is_stable());
                    assert!(BiquadCoeffs::low_shelf(SR, f, q, g).is_stable());
                    assert!(BiquadCoeffs::high_shelf(SR, f, q, g).is_stable());
                }
                assert!(BiquadCoeffs::highpass(SR, f, q).is_stable());
                assert!(BiquadCoeffs::lowpass(SR, f, q).is_stable());
            }
        }
    }

    #[test]
    fn coefficient_ramp_has_no_discontinuity() {
        let mut f = Biquad::new(BiquadCoeffs::peaking(SR, 1000.0, 1.0, 0.0));
        let mut buf = signal::sine(1000.0, SR, 0.5, 4800);
        f.process_mono(&mut buf[..2400]);
        f.set_coeffs(BiquadCoeffs::peaking(SR, 1000.0, 1.0, 12.0));
        f.process_mono(&mut buf[2400..]);
        let max_jump = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 1 kHz sine at amplitude 2.0 (after +12 dB... ~1.99) moves at most ~0.27 per sample.
        assert!(max_jump < 0.3, "max jump {max_jump}");
    }

    #[test]
    fn first_order_filters_have_3db_corner() {
        let hp = BiquadCoeffs::first_order_highpass(SR, 200.0);
        let lp = BiquadCoeffs::first_order_lowpass(SR, 200.0);
        assert!((hp.magnitude_db(200.0, SR) + 3.01).abs() < 0.05);
        assert!((lp.magnitude_db(200.0, SR) + 3.01).abs() < 0.05);
    }
}
