//! Bus strip: summed sends → fader/mute → mono → delay → safety limiter.

use std::sync::atomic::Ordering;

use novamix_dsp::limiter::Limiter;
use novamix_dsp::meter::LevelMeter;
use novamix_dsp::params::LimiterParams;
use novamix_dsp::util::{db_to_gain, ms_to_samples};
use novamix_dsp::{DelayLine, LinearSmoother, Processor};

use crate::meters::BusMeter;

/// Maximum bus delay.
pub const MAX_BUS_DELAY_MS: f32 = 500.0;

pub struct BusStrip {
    pub(crate) enabled: bool,
    pub(crate) buf: [Vec<f32>; 2],
    pub(crate) fader: LinearSmoother,
    pub(crate) mute: LinearSmoother,
    pub(crate) mono: bool,
    pub(crate) delay: DelayLine,
    pub(crate) limiter: Limiter,
    pub(crate) limiter_enabled: bool,
    pub(crate) meter: LevelMeter,
    pub(crate) output: Option<u8>,
    /// Contribution of this bus to its own output when it is the monitor
    /// bus (fades to 0 while a cue is active).
    pub(crate) monitor_contrib: LinearSmoother,
    /// Contribution of this bus to the monitor output while cued.
    pub(crate) cue_contrib: LinearSmoother,
    pub(crate) touched: bool,
    sample_rate: f32,
}

impl BusStrip {
    pub fn new(sample_rate: f32, max_block: usize, smoothing_ms: f32) -> Self {
        let sm = |v: f32| LinearSmoother::with_ramp(v, sample_rate, smoothing_ms);
        Self {
            enabled: false,
            buf: [vec![0.0; max_block], vec![0.0; max_block]],
            fader: sm(1.0),
            mute: sm(1.0),
            mono: false,
            delay: DelayLine::new(ms_to_samples(MAX_BUS_DELAY_MS, sample_rate) + 1),
            limiter: Limiter::new(sample_rate, LimiterParams::default()),
            limiter_enabled: false,
            meter: LevelMeter::new(sample_rate),
            output: None,
            monitor_contrib: LinearSmoother::with_ramp(1.0, sample_rate, 30.0),
            cue_contrib: LinearSmoother::with_ramp(0.0, sample_rate, 30.0),
            touched: false,
            sample_rate,
        }
    }

    pub(crate) fn set_smoothing(&mut self, ms: f32) {
        self.fader.set_ramp(self.sample_rate, ms);
        self.mute.set_ramp(self.sample_rate, ms);
    }

    pub(crate) fn set_fader_db(&mut self, db: f32) {
        self.fader.set_target(if db <= -90.0 { 0.0 } else { db_to_gain(db) });
    }

    pub(crate) fn set_delay_ms(&mut self, ms: f32) {
        let s = ms_to_samples(ms.clamp(0.0, MAX_BUS_DELAY_MS), self.sample_rate);
        if s != self.delay.delay() {
            self.delay.set_delay(s);
            self.delay.reset();
        }
    }

    pub(crate) fn set_limiter(&mut self, p: LimiterParams) {
        self.limiter_enabled = p.enabled;
        self.limiter.set_params(p);
    }

    pub(crate) fn clear(&mut self, n: usize) {
        self.buf[0][..n].fill(0.0);
        self.buf[1][..n].fill(0.0);
        self.touched = false;
    }

    /// Adds `src * gain` (gain ramped per sample by `send`).
    #[inline]
    pub(crate) fn accumulate(&mut self, l: &[f32], r: &[f32], send: &mut LinearSmoother) {
        let n = l.len();
        let [bl, br] = &mut self.buf;
        let (bl, br) = (&mut bl[..n], &mut br[..n]);
        if send.is_smoothing() {
            for i in 0..n {
                let g = send.next_value();
                bl[i] += l[i] * g;
                br[i] += r[i] * g;
            }
        } else {
            let g = send.current();
            for i in 0..n {
                bl[i] += l[i] * g;
                br[i] += r[i] * g;
            }
        }
        self.touched = true;
    }

    pub(crate) fn process(&mut self, n: usize, meter: &BusMeter) {
        let [l, r] = &mut self.buf;
        let (l, r) = (&mut l[..n], &mut r[..n]);
        if self.fader.is_smoothing() || self.mute.is_smoothing() {
            for i in 0..n {
                let g = self.fader.next_value() * self.mute.next_value();
                l[i] *= g;
                r[i] *= g;
            }
        } else {
            let g = self.fader.current() * self.mute.current();
            if g != 1.0 {
                for i in 0..n {
                    l[i] *= g;
                    r[i] *= g;
                }
            }
        }
        if self.mono {
            for i in 0..n {
                let m = 0.5 * (l[i] + r[i]);
                l[i] = m;
                r[i] = m;
            }
        }
        self.delay.process(l, Some(r));
        if self.limiter_enabled {
            self.limiter.process(l, Some(r));
            meter.limiter_gr_db.store(self.limiter.gain_reduction_db());
        } else {
            meter.limiter_gr_db.store(0.0);
        }
        let reading = self.meter.process(l, Some(r));
        for c in 0..2 {
            meter.peak[c].fetch_max(reading.peak[c]);
            meter.rms[c].store(reading.rms[c]);
        }
        if reading.clipped {
            meter.clips.fetch_add(1, Ordering::Relaxed);
        }
    }
}
