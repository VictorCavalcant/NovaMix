//! Channel strip: source → input gain → (mono) → voice chain → pan →
//! fader/mute → sends.

use std::sync::atomic::Ordering;
use std::time::Instant;

use novamix_diagnostics::{RtEvent, RtEventSender};
use novamix_dsp::chain::VoiceChain;
use novamix_dsp::meter::{CLIP_LEVEL, LevelMeter};
use novamix_dsp::params::ChainParams;
use novamix_dsp::signal::TestToneGenerator;
use novamix_dsp::util::{db_to_gain, peak, sanitize_buffer};
use novamix_dsp::{LinearSmoother, pan};
use novamix_mixer_graph::MAX_BUSES;

use crate::bridge::{BridgeReader, ReadStatus};
use crate::meters::StripMeter;

pub struct ChannelStrip {
    pub(crate) id: u8,
    pub(crate) enabled: bool,
    pub(crate) input: Option<Box<BridgeReader>>,
    pub(crate) tone: TestToneGenerator,
    pub(crate) tone_enabled: bool,
    pub(crate) input_gain: LinearSmoother,
    pub(crate) fader: LinearSmoother,
    pub(crate) mute: LinearSmoother,
    pub(crate) pan_l: LinearSmoother,
    pub(crate) pan_r: LinearSmoother,
    pub(crate) pan: f32,
    pub(crate) mono: bool,
    pub(crate) chain: VoiceChain,
    pub(crate) in_meter: LevelMeter,
    pub(crate) out_meter: LevelMeter,
    pub(crate) sends: [LinearSmoother; MAX_BUSES],
    pub(crate) send_pre: [bool; MAX_BUSES],
    /// Post-fader signal (what most sends use).
    pub(crate) post: [Vec<f32>; 2],
    /// Post-DSP, pre-fader signal (only filled when a pre-fader send is active).
    pub(crate) pre: [Vec<f32>; 2],
    pub(crate) has_pre_sends: bool,
    was_starved: bool,
    clip_hold: u32,
}

impl ChannelStrip {
    pub fn new(id: u8, sample_rate: f32, max_block: usize, smoothing_ms: f32) -> Self {
        let sm = |v: f32| LinearSmoother::with_ramp(v, sample_rate, smoothing_ms);
        let (pl, pr) = pan::pan_gains(0.0, false);
        Self {
            id,
            enabled: false,
            input: None,
            tone: TestToneGenerator::new(sample_rate),
            tone_enabled: false,
            input_gain: sm(1.0),
            fader: sm(1.0),
            mute: sm(1.0),
            pan_l: sm(pl),
            pan_r: sm(pr),
            pan: 0.0,
            mono: false,
            chain: VoiceChain::new(sample_rate, max_block, ChainParams::default()),
            in_meter: LevelMeter::new(sample_rate),
            out_meter: LevelMeter::new(sample_rate),
            sends: std::array::from_fn(|_| sm(0.0)),
            send_pre: [false; MAX_BUSES],
            post: [vec![0.0; max_block], vec![0.0; max_block]],
            pre: [vec![0.0; max_block], vec![0.0; max_block]],
            has_pre_sends: false,
            was_starved: false,
            clip_hold: 0,
        }
    }

    pub(crate) fn set_smoothing(&mut self, sample_rate: f32, ms: f32) {
        for s in [&mut self.input_gain, &mut self.fader, &mut self.mute, &mut self.pan_l, &mut self.pan_r] {
            s.set_ramp(sample_rate, ms);
        }
        for s in &mut self.sends {
            s.set_ramp(sample_rate, ms);
        }
    }

    pub(crate) fn update_pan(&mut self) {
        let (l, r) = pan::pan_gains(self.pan, self.mono);
        self.pan_l.set_target(l);
        self.pan_r.set_target(r);
    }

    pub(crate) fn set_fader_db(&mut self, db: f32) {
        self.fader.set_target(if db <= -90.0 { 0.0 } else { db_to_gain(db) });
    }

    /// True when the strip contributes nothing and can be skipped.
    pub(crate) fn is_silent_and_settled(&self) -> bool {
        !self.enabled
    }

    /// Processes `n` frames into `self.post` (and `self.pre`).
    pub(crate) fn process(&mut self, n: usize, meter: &StripMeter, events: &mut RtEventSender, budget_us: f32) {
        let [l, r] = &mut self.post;
        let (l, r) = (&mut l[..n], &mut r[..n]);

        // 1. Source.
        let mut active = false;
        if self.tone_enabled {
            self.tone.fill(l);
            r.copy_from_slice(l);
            active = true;
        } else if let Some(input) = self.input.as_mut() {
            match input.read(l, r) {
                ReadStatus::Full => {
                    active = true;
                    if self.was_starved {
                        self.was_starved = false;
                        events.send(RtEvent::InputRecovered { channel: self.id });
                    }
                }
                ReadStatus::Underrun => {
                    active = true;
                    if !self.was_starved {
                        events.send(RtEvent::InputUnderrun { channel: self.id });
                    }
                    self.was_starved = true;
                }
                ReadStatus::Priming | ReadStatus::Idle => {}
            }
        } else {
            l.fill(0.0);
            r.fill(0.0);
        }
        meter.source_active.store(active, Ordering::Relaxed);
        sanitize_buffer(l);
        sanitize_buffer(r);

        // Source clipping is detected before any gain is applied.
        if peak(l).max(peak(r)) >= CLIP_LEVEL {
            meter.source_clips.fetch_add(1, Ordering::Relaxed);
            if self.clip_hold == 0 {
                events.send(RtEvent::InputClipped { channel: self.id });
            }
            self.clip_hold = 50;
        } else if self.clip_hold > 0 {
            self.clip_hold -= 1;
        }

        // 2. Input gain and mono fold.
        self.input_gain.apply_stereo(l, r);
        if self.mono {
            for i in 0..n {
                l[i] = 0.5 * (l[i] + r[i]);
            }
        }

        // 3. Input meter.
        let reading = if self.mono { self.in_meter.process(l, None) } else { self.in_meter.process(l, Some(r)) };
        for c in 0..2 {
            meter.input_peak[c].fetch_max(reading.peak[c]);
            meter.input_rms[c].store(reading.rms[c]);
        }

        // 4. Voice chain (timed for the per-channel CPU indicator).
        let t0 = Instant::now();
        if self.mono {
            self.chain.process(l, None);
            r.copy_from_slice(l);
        } else {
            self.chain.process(l, Some(r));
        }
        let us = t0.elapsed().as_secs_f32() * 1e6;
        if budget_us > 0.0 {
            let load = us / budget_us * 100.0;
            let prev = meter.dsp_load_pct.load();
            meter.dsp_load_pct.store(prev * 0.95 + load * 0.05);
        }

        // 5. Pre-fader copy.
        if self.has_pre_sends {
            self.pre[0][..n].copy_from_slice(l);
            self.pre[1][..n].copy_from_slice(r);
        }

        // 6. Pan, fader, mute.
        let smoothing = self.pan_l.is_smoothing() || self.pan_r.is_smoothing() || self.fader.is_smoothing() || self.mute.is_smoothing();
        if smoothing {
            for i in 0..n {
                let g = self.fader.next_value() * self.mute.next_value();
                l[i] *= self.pan_l.next_value() * g;
                r[i] *= self.pan_r.next_value() * g;
            }
        } else {
            let g = self.fader.current() * self.mute.current();
            let gl = self.pan_l.current() * g;
            let gr = self.pan_r.current() * g;
            if gl != 1.0 || gr != 1.0 {
                for i in 0..n {
                    l[i] *= gl;
                    r[i] *= gr;
                }
            }
        }

        // 7. Output meter and chain indicators.
        let reading = self.out_meter.process(l, Some(r));
        for c in 0..2 {
            meter.output_peak[c].fetch_max(reading.peak[c]);
            meter.output_rms[c].store(reading.rms[c]);
        }
        if reading.clipped {
            meter.output_clips.fetch_add(1, Ordering::Relaxed);
        }
        let cm = self.chain.meters();
        meter.gate_open.store(cm.gate_open, Ordering::Relaxed);
        meter.gate_gain_db.store(cm.gate_gain_db);
        meter.expander_gr_db.store(cm.expander_gr_db);
        meter.compressor_gr_db.store(cm.compressor_gr_db);
        meter.deesser_gr_db.store(cm.deesser_gr_db);
        meter.limiter_gr_db.store(cm.limiter_gr_db);
        meter.agc_gain_db.store(cm.agc_gain_db);
        meter.nr_reduction_db.store(cm.nr_reduction_db);
        meter.vad.store(cm.vad);
        meter.neural_active.store(cm.neural_active, Ordering::Relaxed);
        meter.latency_samples.store(cm.latency_samples as u32, Ordering::Relaxed);
    }
}
