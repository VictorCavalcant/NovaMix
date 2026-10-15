//! Look-ahead brick-wall limiter.
//!
//! The required gain for every incoming frame is pushed through a sliding
//! minimum (peak hold) over the look-ahead window and then a moving average
//! of the same length. The average reaches the held minimum exactly when the
//! delayed peak leaves the delay line, so the attack is smooth yet never too
//! late. Release is a one-pole curve. A final clamp guarantees the ceiling
//! even for numerical edge cases.

use crate::Processor;
use crate::params::LimiterParams;
use crate::util::{db_to_gain, gain_to_db, time_constant_coeff};

#[derive(Debug, Clone)]
pub struct Limiter {
    sample_rate: f32,
    params: LimiterParams,
    ceiling: f32,
    release_coeff: f32,
    lookahead: usize,
    max_lookahead: usize,
    // Delay line (stereo), capacity max_lookahead + 1.
    delay: [Vec<f32>; 2],
    delay_pos: usize,
    // Monotonic deque for the sliding minimum: (value, sample index).
    deque_val: Vec<f32>,
    deque_idx: Vec<u64>,
    deque_head: usize,
    deque_len: usize,
    // Moving average of the held gain.
    avg_buf: Vec<f32>,
    avg_pos: usize,
    avg_sum: f64,
    counter: u64,
    gain: f32,
    min_gain_block: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32, params: LimiterParams) -> Self {
        let max_lookahead = ((LimiterParams::MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize).max(1);
        let cap = max_lookahead + 2;
        let mut l = Self {
            sample_rate,
            params,
            ceiling: 1.0,
            release_coeff: 0.0,
            lookahead: 0,
            max_lookahead,
            delay: [vec![0.0; cap], vec![0.0; cap]],
            delay_pos: 0,
            deque_val: vec![1.0; cap],
            deque_idx: vec![0; cap],
            deque_head: 0,
            deque_len: 0,
            avg_buf: vec![1.0; cap],
            avg_pos: 0,
            avg_sum: 0.0,
            counter: 0,
            gain: 1.0,
            min_gain_block: 1.0,
        };
        l.set_params(params);
        l.reset();
        l
    }

    /// Updates parameters. A look-ahead change alters latency, so it resets
    /// the internal state (the chain ducks around such changes).
    pub fn set_params(&mut self, params: LimiterParams) {
        let p = params.sanitized();
        self.params = p;
        self.ceiling = db_to_gain(p.ceiling_db);
        self.release_coeff = time_constant_coeff(p.release_ms, self.sample_rate);
        let la = ((p.lookahead_ms * 0.001 * self.sample_rate).round() as usize).min(self.max_lookahead);
        if la != self.lookahead {
            self.lookahead = la;
            self.reset();
        }
    }

    pub fn params(&self) -> LimiterParams {
        self.params
    }

    /// Latency that a given parameter set would produce (in samples).
    pub fn latency_for(&self, params: &LimiterParams) -> usize {
        ((params.sanitized().lookahead_ms * 0.001 * self.sample_rate).round() as usize).min(self.max_lookahead)
    }

    /// Largest gain reduction applied during the last processed block (dB ≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        gain_to_db(self.min_gain_block)
    }

    #[inline]
    fn window(&self) -> usize {
        self.lookahead + 1
    }

    #[inline]
    fn push_min(&mut self, value: f32) -> f32 {
        let cap = self.deque_val.len();
        let w = self.window() as u64;
        let idx = self.counter;
        // Drop expired entries from the front.
        while self.deque_len > 0 && self.deque_idx[self.deque_head] + w <= idx {
            self.deque_head = (self.deque_head + 1) % cap;
            self.deque_len -= 1;
        }
        // Drop entries from the back that are not smaller than the new value.
        while self.deque_len > 0 {
            let back = (self.deque_head + self.deque_len - 1) % cap;
            if self.deque_val[back] >= value {
                self.deque_len -= 1;
            } else {
                break;
            }
        }
        let pos = (self.deque_head + self.deque_len) % cap;
        self.deque_val[pos] = value;
        self.deque_idx[pos] = idx;
        self.deque_len += 1;
        self.deque_val[self.deque_head]
    }

    #[inline]
    fn push_avg(&mut self, value: f32) -> f32 {
        let w = self.window();
        let old = self.avg_buf[self.avg_pos];
        self.avg_buf[self.avg_pos] = value;
        self.avg_pos = (self.avg_pos + 1) % w;
        self.avg_sum += value as f64 - old as f64;
        // Periodically recompute to cancel floating point drift.
        if self.counter.is_multiple_of(65_536) {
            self.avg_sum = self.avg_buf[..w].iter().map(|&v| v as f64).sum();
        }
        (self.avg_sum / w as f64) as f32
    }
}

impl Processor for Limiter {
    fn reset(&mut self) {
        for d in &mut self.delay {
            d.iter_mut().for_each(|s| *s = 0.0);
        }
        self.delay_pos = 0;
        self.deque_head = 0;
        self.deque_len = 0;
        let w = self.window();
        self.avg_buf.iter_mut().for_each(|v| *v = 1.0);
        self.avg_sum = w as f64;
        self.avg_pos = 0;
        self.counter = 0;
        self.gain = 1.0;
    }

    fn latency(&self) -> usize {
        self.lookahead
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let cap = self.lookahead + 1;
        let ceiling = self.ceiling;
        let mut min_gain = 1.0_f32;
        for i in 0..left.len() {
            let xl = left[i];
            let xr = right.as_deref().map_or(0.0, |r| r[i]);
            let peak = xl.abs().max(xr.abs());
            let needed = if peak > ceiling { ceiling / peak } else { 1.0 };

            let held = self.push_min(needed);
            let smooth = self.push_avg(held);
            self.counter += 1;

            self.gain = if smooth < self.gain { smooth } else { smooth + (self.gain - smooth) * self.release_coeff };
            min_gain = min_gain.min(self.gain);

            // Delay line: write the new frame, read the one `lookahead` ago.
            let wpos = self.delay_pos;
            self.delay[0][wpos] = xl;
            self.delay[1][wpos] = xr;
            let rpos = (wpos + 1) % cap;
            self.delay_pos = rpos;
            let (dl, dr) = if self.lookahead == 0 { (xl, xr) } else { (self.delay[0][rpos], self.delay[1][rpos]) };

            left[i] = (dl * self.gain).clamp(-ceiling, ceiling);
            if let Some(r) = right.as_deref_mut() {
                r[i] = (dr * self.gain).clamp(-ceiling, ceiling);
            }
        }
        self.min_gain_block = min_gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, util};

    const SR: f32 = 48_000.0;

    #[test]
    fn never_exceeds_ceiling() {
        for la in [0.0, 1.0, 5.0, 10.0] {
            let p = LimiterParams { enabled: true, ceiling_db: -3.0, lookahead_ms: la, release_ms: 50.0 };
            let mut l = Limiter::new(SR, p);
            let mut x = signal::white_noise(48_000, 1.5, 7);
            let mut y = signal::sine(200.0, SR, 2.0, 48_000);
            l.process(&mut x, Some(&mut y));
            let ceiling = util::db_to_gain(-3.0);
            assert!(util::peak(&x) <= ceiling + 1e-6, "la {la}");
            assert!(util::peak(&y) <= ceiling + 1e-6, "la {la}");
        }
    }

    #[test]
    fn transparent_below_ceiling_with_exact_latency() {
        let p = LimiterParams { enabled: true, ceiling_db: -1.0, lookahead_ms: 2.0, release_ms: 50.0 };
        let mut l = Limiter::new(SR, p);
        let la = l.latency();
        assert_eq!(la, 96);
        let mut x = signal::impulse(1024, 10, 0.5);
        l.process(&mut x, None);
        assert!((x[10 + la] - 0.5).abs() < 1e-6, "impulse moved by latency");
        assert!(x.iter().enumerate().all(|(i, v)| i == 10 + la || v.abs() < 1e-9));
    }

    #[test]
    fn lookahead_avoids_clipping_distortion() {
        // With look-ahead the gain is already down when the peak arrives, so
        // the final clamp should not need to act (no flat-topped samples).
        let p = LimiterParams { enabled: true, ceiling_db: -6.0, lookahead_ms: 5.0, release_ms: 100.0 };
        let mut l = Limiter::new(SR, p);
        let mut x = signal::sine(100.0, SR, 1.0, 48_000);
        l.process(&mut x, None);
        let ceiling = util::db_to_gain(-6.0);
        // Peaks may touch the ceiling, but there must be no flat tops (runs
        // of consecutive samples pinned at the ceiling, i.e. hard clipping).
        let mut run = 0;
        let mut longest = 0;
        for v in &x[4800..] {
            if (v.abs() - ceiling).abs() < 1e-6 {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        assert!(longest < 3, "flat top of {longest} samples");
        assert!(l.gain_reduction_db() < -5.0);
    }

    #[test]
    fn releases_after_peak() {
        let p = LimiterParams { enabled: true, ceiling_db: -6.0, lookahead_ms: 1.0, release_ms: 20.0 };
        let mut l = Limiter::new(SR, p);
        let mut burst = signal::sine(500.0, SR, 1.0, 4800);
        l.process(&mut burst, None);
        let mut quiet = signal::sine(500.0, SR, 0.1, 48_000);
        for c in quiet.chunks_mut(480) {
            l.process(c, None);
        }
        assert!(l.gain_reduction_db() > -0.05, "{}", l.gain_reduction_db());
    }
}
