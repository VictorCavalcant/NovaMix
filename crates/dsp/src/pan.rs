//! Pan and balance laws.

use std::f32::consts::FRAC_PI_4;

/// Constant-power pan for a mono source: returns `(left, right)` gains.
/// `pan` ranges from -1 (hard left) to +1 (hard right); centre is -3 dB on
/// each side so perceived loudness stays constant while panning.
#[inline]
pub fn constant_power(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let theta = (p + 1.0) * FRAC_PI_4;
    (theta.cos(), theta.sin())
}

/// Balance for a stereo source: attenuates the opposite side only, so the
/// centre position is unity gain.
#[inline]
pub fn balance(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

/// Mono-compatible gains: constant-power for mono material and a
/// compensated balance for stereo material. For mono sources the centre is
/// normalized to unity (×√2) so toggling mono/stereo does not change level.
#[inline]
pub fn pan_gains(pan: f32, mono_source: bool) -> (f32, f32) {
    if mono_source {
        let (l, r) = constant_power(pan);
        (l * std::f32::consts::SQRT_2, r * std::f32::consts::SQRT_2)
    } else {
        balance(pan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_power_sums_to_one() {
        for i in -10..=10 {
            let (l, r) = constant_power(i as f32 / 10.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
        let (l, r) = constant_power(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    }

    #[test]
    fn balance_centre_is_unity() {
        assert_eq!(balance(0.0), (1.0, 1.0));
        assert_eq!(balance(1.0), (0.0, 1.0));
        assert_eq!(balance(-0.5), (1.0, 0.5));
    }

    #[test]
    fn mono_pan_centre_is_unity() {
        let (l, r) = pan_gains(0.0, true);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }
}
