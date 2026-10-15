//! Integer-sample stereo delay line with a fixed maximum length.

/// Pre-allocated stereo delay. Changing the delay never allocates; samples
/// beyond the maximum are clamped.
#[derive(Debug, Clone)]
pub struct DelayLine {
    buf: [Vec<f32>; 2],
    pos: usize,
    delay: usize,
}

impl DelayLine {
    pub fn new(max_delay: usize) -> Self {
        let cap = max_delay + 1;
        Self { buf: [vec![0.0; cap], vec![0.0; cap]], pos: 0, delay: 0 }
    }

    pub fn max_delay(&self) -> usize {
        self.buf[0].len() - 1
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Sets the delay in samples (clamped to the maximum).
    pub fn set_delay(&mut self, samples: usize) {
        self.delay = samples.min(self.max_delay());
    }

    pub fn reset(&mut self) {
        self.buf[0].fill(0.0);
        self.buf[1].fill(0.0);
        self.pos = 0;
    }

    /// Delays the buffers in place.
    pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        if self.delay == 0 {
            return;
        }
        let cap = self.buf[0].len();
        for i in 0..left.len() {
            let read = (self.pos + cap - self.delay) % cap;
            self.buf[0][self.pos] = left[i];
            left[i] = self.buf[0][read];
            if let Some(r) = right.as_deref_mut() {
                self.buf[1][self.pos] = r[i];
                r[i] = self.buf[1][read];
            }
            self.pos = (self.pos + 1) % cap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_by_requested_samples() {
        let mut d = DelayLine::new(100);
        d.set_delay(7);
        let mut x: Vec<f32> = (0..50).map(|i| i as f32).collect();
        d.process(&mut x, None);
        assert_eq!(x[7], 0.0);
        assert_eq!(x[8], 1.0);
        assert_eq!(x[49], 42.0);
    }

    #[test]
    fn clamps_to_max() {
        let mut d = DelayLine::new(10);
        d.set_delay(1000);
        assert_eq!(d.delay(), 10);
    }
}
