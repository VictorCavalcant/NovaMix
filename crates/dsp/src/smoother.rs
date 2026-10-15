//! Parameter smoothing to avoid zipper noise and clicks.

/// Linear ramp towards a target value over a fixed duration.
///
/// Every gain-like parameter in the engine (input gain, fader, pan, mute,
/// send levels) goes through one of these so changes made from the UI while
/// audio is running never produce discontinuities.
#[derive(Debug, Clone)]
pub struct LinearSmoother {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_samples: u32,
}

impl LinearSmoother {
    /// Creates a smoother resting at `value` with a default 1-sample ramp.
    pub fn new(value: f32) -> Self {
        Self { current: value, target: value, step: 0.0, remaining: 0, ramp_samples: 1 }
    }

    /// Creates a smoother with the ramp time already configured.
    pub fn with_ramp(value: f32, sample_rate: f32, ramp_ms: f32) -> Self {
        let mut s = Self::new(value);
        s.set_ramp(sample_rate, ramp_ms);
        s
    }

    /// Sets the ramp duration used by subsequent [`set_target`](Self::set_target) calls.
    pub fn set_ramp(&mut self, sample_rate: f32, ramp_ms: f32) {
        self.ramp_samples = ((sample_rate * ramp_ms * 0.001).round() as u32).max(1);
    }

    /// Starts a ramp from the current value to `target`.
    pub fn set_target(&mut self, target: f32) {
        if !target.is_finite() {
            return;
        }
        if (target - self.target).abs() <= f32::EPSILON && self.remaining == 0 {
            self.current = target;
            self.target = target;
            return;
        }
        self.target = target;
        self.remaining = self.ramp_samples;
        self.step = (self.target - self.current) / self.ramp_samples as f32;
    }

    /// Jumps immediately to `value` (used when a stream is (re)started).
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.remaining = 0;
        self.step = 0.0;
    }

    /// Returns the next smoothed value.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            // Computed from the target (not accumulated) so the ramp lands
            // exactly and has no floating point drift.
            self.current = self.target - self.step * self.remaining as f32;
        }
        self.current
    }

    /// True while a ramp is in progress.
    #[inline]
    pub fn is_smoothing(&self) -> bool {
        self.remaining > 0
    }

    /// Current (possibly intermediate) value.
    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Final value of the ramp.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Multiplies `buf` by the smoothed value, advancing the ramp.
    pub fn apply(&mut self, buf: &mut [f32]) {
        if !self.is_smoothing() {
            let g = self.current;
            if g == 1.0 {
                return;
            }
            for s in buf.iter_mut() {
                *s *= g;
            }
            return;
        }
        for s in buf.iter_mut() {
            *s *= self.next_value();
        }
    }

    /// Multiplies two buffers by the same smoothed value (stereo).
    pub fn apply_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if !self.is_smoothing() {
            let g = self.current;
            if g == 1.0 {
                return;
            }
            for i in 0..n {
                left[i] *= g;
                right[i] *= g;
            }
            return;
        }
        for i in 0..n {
            let g = self.next_value();
            left[i] *= g;
            right[i] *= g;
        }
    }

    /// Writes the smoothed values into `out` (one per sample).
    pub fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.next_value();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_linearly_and_lands_exactly() {
        let mut s = LinearSmoother::with_ramp(0.0, 1000.0, 10.0); // 10 samples
        s.set_target(1.0);
        let vals: Vec<f32> = (0..12).map(|_| s.next_value()).collect();
        assert!((vals[0] - 0.1).abs() < 1e-6);
        assert!((vals[4] - 0.5).abs() < 1e-6);
        assert_eq!(vals[9], 1.0);
        assert_eq!(vals[11], 1.0);
        assert!(!s.is_smoothing());
    }

    #[test]
    fn no_step_larger_than_ramp_increment() {
        let mut s = LinearSmoother::with_ramp(1.0, 48_000.0, 20.0);
        s.set_target(0.0);
        let mut prev = 1.0;
        for _ in 0..2000 {
            let v = s.next_value();
            assert!((prev - v).abs() <= 1.0 / 960.0 + 1e-6);
            prev = v;
        }
        assert_eq!(prev, 0.0);
    }

    #[test]
    fn retarget_mid_ramp_is_continuous() {
        let mut s = LinearSmoother::with_ramp(0.0, 1000.0, 10.0);
        s.set_target(1.0);
        for _ in 0..5 {
            s.next_value();
        }
        let before = s.current();
        s.set_target(0.0);
        let after = s.next_value();
        assert!((before - after).abs() < 0.11);
    }

    #[test]
    fn ignores_non_finite_targets() {
        let mut s = LinearSmoother::new(0.5);
        s.set_target(f32::NAN);
        assert_eq!(s.next_value(), 0.5);
    }
}
