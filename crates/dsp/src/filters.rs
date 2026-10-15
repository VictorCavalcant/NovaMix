//! High-pass filter with selectable slope and the parametric equalizer.

use crate::Processor;
use crate::biquad::{Biquad, BiquadCoeffs};
use crate::params::{EqParams, HpfParams, HpfSlope, MAX_EQ_BANDS};

/// Butterworth high-pass with 6/12/18/24 dB per octave slopes.
#[derive(Debug, Clone)]
pub struct HighPassFilter {
    sample_rate: f32,
    params: HpfParams,
    sections: [Biquad; 2],
    active_sections: usize,
}

impl HighPassFilter {
    pub fn new(sample_rate: f32, params: HpfParams) -> Self {
        let mut f = Self { sample_rate, params, sections: [Biquad::default(), Biquad::default()], active_sections: 0 };
        f.update(true);
        f
    }

    pub fn set_params(&mut self, params: HpfParams) {
        let params = params.sanitized();
        let slope_changed = params.slope != self.params.slope;
        self.params = params;
        self.update(slope_changed);
    }

    pub fn params(&self) -> HpfParams {
        self.params
    }

    fn update(&mut self, immediate: bool) {
        let sr = self.sample_rate;
        let f = self.params.frequency_hz;
        let coeffs: [BiquadCoeffs; 2] = match self.params.slope {
            HpfSlope::Db6 => [BiquadCoeffs::first_order_highpass(sr, f), BiquadCoeffs::IDENTITY],
            HpfSlope::Db12 => [BiquadCoeffs::highpass(sr, f, std::f32::consts::FRAC_1_SQRT_2), BiquadCoeffs::IDENTITY],
            HpfSlope::Db18 => [BiquadCoeffs::first_order_highpass(sr, f), BiquadCoeffs::highpass(sr, f, 1.0)],
            // 4th-order Butterworth: Q = 0.5412, 1.3066
            HpfSlope::Db24 => [BiquadCoeffs::highpass(sr, f, 0.541_196_1), BiquadCoeffs::highpass(sr, f, 1.306_563)],
        };
        self.active_sections = match self.params.slope {
            HpfSlope::Db6 | HpfSlope::Db12 => 1,
            HpfSlope::Db18 | HpfSlope::Db24 => 2,
        };
        for (s, c) in self.sections.iter_mut().zip(coeffs) {
            if immediate {
                s.set_coeffs_immediate(c);
                s.reset();
            } else {
                s.set_coeffs(c);
            }
        }
    }

    /// Magnitude response in dB (for UI curves and tests).
    pub fn magnitude_db(&self, freq: f32) -> f32 {
        self.sections[..self.active_sections].iter().map(|s| s.target_coeffs().magnitude_db(freq, self.sample_rate)).sum()
    }
}

impl Processor for HighPassFilter {
    fn reset(&mut self) {
        for s in &mut self.sections {
            s.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        for s in &mut self.sections[..self.active_sections] {
            s.process(left, right.as_deref_mut());
        }
    }
}

/// Parametric equalizer with up to [`MAX_EQ_BANDS`] bands.
#[derive(Debug, Clone)]
pub struct ParametricEq {
    sample_rate: f32,
    params: EqParams,
    bands: [Biquad; MAX_EQ_BANDS],
    active: [bool; MAX_EQ_BANDS],
}

impl ParametricEq {
    pub fn new(sample_rate: f32, params: EqParams) -> Self {
        let mut eq = Self { sample_rate, params, bands: std::array::from_fn(|_| Biquad::default()), active: [false; MAX_EQ_BANDS] };
        eq.apply(true);
        eq
    }

    pub fn set_params(&mut self, params: EqParams) {
        self.params = params.sanitized();
        self.apply(false);
    }

    pub fn params(&self) -> &EqParams {
        &self.params
    }

    fn apply(&mut self, immediate: bool) {
        let count = self.params.band_count as usize;
        for i in 0..MAX_EQ_BANDS {
            let band = self.params.bands[i];
            let used = i < count && band.enabled;
            // A band stays "active" while it ramps back to identity so that
            // disabling a band is as smooth as moving it.
            let c = if used { band.coeffs(self.sample_rate) } else { BiquadCoeffs::IDENTITY };
            if immediate {
                self.bands[i].set_coeffs_immediate(c);
            } else {
                self.bands[i].set_coeffs(c);
            }
            self.active[i] = used || (!immediate && self.active[i]);
        }
    }
}

impl Processor for ParametricEq {
    fn reset(&mut self) {
        for b in &mut self.bands {
            b.reset();
        }
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        for i in 0..MAX_EQ_BANDS {
            if !self.active[i] {
                continue;
            }
            self.bands[i].process(left, right.as_deref_mut());
            if self.bands[i].target_coeffs() == BiquadCoeffs::IDENTITY && !(i < self.params.band_count as usize && self.params.bands[i].enabled) {
                // Finished ramping out; skip from now on.
                self.active[i] = false;
                self.bands[i].reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{EqBand, EqBandKind};
    use crate::{signal, util};

    const SR: f32 = 48_000.0;

    fn tone_gain_db(p: &mut impl Processor, f: f32) -> f32 {
        let mut x = signal::sine(f, SR, 0.5, 48_000);
        p.process(&mut x, None);
        util::gain_to_db(util::rms(&x[24_000..]) * std::f32::consts::SQRT_2 / 0.5)
    }

    #[test]
    fn hpf_slopes_match_nominal_rolloff() {
        for (slope, expected) in [(HpfSlope::Db6, 6.0), (HpfSlope::Db12, 12.0), (HpfSlope::Db18, 18.0), (HpfSlope::Db24, 24.0)] {
            let f = HighPassFilter::new(SR, HpfParams { enabled: true, frequency_hz: 400.0, slope });
            // Corner is -3 dB for all Butterworth orders.
            assert!((f.magnitude_db(400.0) + 3.01).abs() < 0.1, "{slope:?} corner {}", f.magnitude_db(400.0));
            // One octave to two octaves below the corner: slope ≈ nominal.
            let drop = f.magnitude_db(100.0) - f.magnitude_db(50.0);
            assert!((drop - expected).abs() < 1.0, "{slope:?}: {drop}");
            assert!(f.magnitude_db(5000.0).abs() < 0.1);
        }
    }

    #[test]
    fn hpf_removes_dc() {
        let mut f = HighPassFilter::new(SR, HpfParams::default());
        let mut x = vec![0.5; 48_000];
        f.process(&mut x, None);
        assert!(util::peak(&x[24_000..]) < 1e-3);
    }

    #[test]
    fn eq_band_boost_is_measurable() {
        let mut p = EqParams { enabled: true, ..Default::default() };
        p.bands[2] = EqBand::new(EqBandKind::Peak, 1000.0, 6.0, 1.0);
        let mut eq = ParametricEq::new(SR, p);
        let g = tone_gain_db(&mut eq, 1000.0);
        assert!((g - 6.0).abs() < 0.15, "{g}");
        let g = tone_gain_db(&mut eq, 100.0);
        assert!(g.abs() < 0.3, "{g}");
    }

    #[test]
    fn eq_disabled_band_ramps_out() {
        let mut p = EqParams::default();
        p.bands[2] = EqBand::new(EqBandKind::Peak, 1000.0, 12.0, 1.0);
        let mut eq = ParametricEq::new(SR, p);
        p.bands[2].enabled = false;
        eq.set_params(p);
        let g = tone_gain_db(&mut eq, 1000.0);
        assert!(g.abs() < 0.1, "{g}");
    }
}
