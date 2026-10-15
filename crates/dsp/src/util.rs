//! Small numeric helpers shared by every DSP block.

/// Floor used when converting very small linear values to decibels.
pub const MIN_DB: f32 = -120.0;

/// Linear amplitude that corresponds to [`MIN_DB`].
pub const MIN_GAIN: f32 = 1.0e-6;

/// Converts decibels to a linear amplitude factor. Values at or below
/// [`MIN_DB`] map to exactly zero so that "-inf" faders are truly silent.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB { 0.0 } else { 10.0_f32.powf(db * 0.05) }
}

/// Converts a linear amplitude factor to decibels, clamped at [`MIN_DB`].
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    let g = gain.abs();
    if g <= MIN_GAIN { MIN_DB } else { 20.0 * g.log10() }
}

/// Converts a power value (squared amplitude) to decibels.
#[inline]
pub fn power_to_db(power: f32) -> f32 {
    if power <= MIN_GAIN * MIN_GAIN { MIN_DB } else { 10.0 * power.log10() }
}

/// One-pole smoothing coefficient for a time constant expressed in
/// milliseconds. `y = coeff * y + (1 - coeff) * x` reaches ~63% of a step
/// after `ms` milliseconds.
#[inline]
pub fn time_constant_coeff(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 || sample_rate <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * sample_rate)).exp() }
}

/// Number of samples that corresponds to `ms` milliseconds.
#[inline]
pub fn ms_to_samples(ms: f32, sample_rate: f32) -> usize {
    (ms.max(0.0) * 0.001 * sample_rate).round() as usize
}

/// Replaces NaN/inf with silence and flushes denormal-range values to zero.
#[inline]
pub fn sanitize(x: f32) -> f32 {
    if x.is_finite() && x.abs() > 1.0e-30 { x } else { 0.0 }
}

/// Sanitizes a whole buffer in place. Used at the boundary between devices
/// and the engine so a misbehaving driver cannot inject NaNs into the mix.
pub fn sanitize_buffer(buf: &mut [f32]) {
    for s in buf {
        *s = sanitize(*s);
    }
}

/// Returns the absolute peak of a buffer.
#[inline]
pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0_f32, |m, &x| m.max(x.abs()))
}

/// Mean of the squared samples.
#[inline]
pub fn mean_square(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    let sum: f64 = buf.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum / buf.len() as f64) as f32
}

/// Root mean square of a buffer.
#[inline]
pub fn rms(buf: &[f32]) -> f32 {
    mean_square(buf).sqrt()
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Enables flush-to-zero and denormals-are-zero for the calling thread.
///
/// Denormal floats can make recursive filters hundreds of times slower when
/// a signal decays towards silence. Audio callbacks call this once per
/// callback (it is two instructions) because the OS may reuse threads.
#[inline]
pub fn enable_flush_to_zero() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: stmxcsr/ldmxcsr only touch the MXCSR register of the current
    // thread; setting FTZ (bit 15) and DAZ (bit 6) is always valid on x86_64.
    unsafe {
        let mut csr: u32 = 0;
        core::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        csr |= 0x8040;
        core::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, preserves_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-60.0_f32, -20.0, -6.0, 0.0, 6.0, 12.0] {
            let g = db_to_gain(db);
            assert!((gain_to_db(g) - db).abs() < 1e-3, "{db}");
        }
        assert_eq!(db_to_gain(MIN_DB), 0.0);
        assert_eq!(gain_to_db(0.0), MIN_DB);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn rms_of_sine_is_minus_3db() {
        let sr = 48_000.0;
        let buf: Vec<f32> = (0..48_000).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()).collect();
        let r = gain_to_db(rms(&buf));
        assert!((r + 3.0103).abs() < 0.01, "rms {r}");
        assert!((peak(&buf) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn sanitize_removes_nan_and_inf() {
        let mut b = [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e-38, 0.5];
        sanitize_buffer(&mut b);
        assert_eq!(b, [0.0, 0.0, 0.0, 0.0, 0.5]);
    }

    #[test]
    fn time_constant_is_sane() {
        let c = time_constant_coeff(10.0, 48_000.0);
        assert!(c > 0.99 && c < 1.0);
        assert_eq!(time_constant_coeff(0.0, 48_000.0), 0.0);
    }
}
