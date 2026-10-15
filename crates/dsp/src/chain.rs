//! The per-channel voice/effects chain.
//!
//! Design goals (from the real-time rules of the project):
//! * every stage is pre-allocated; reordering, enabling or re-tuning never
//!   allocates on the audio thread;
//! * zero-latency stages fade in/out with a short crossfade when toggled;
//! * changes that alter latency (noise reduction, limiter look-ahead,
//!   stage order) duck the output for a few milliseconds, swap at silence,
//!   and fade back in, so nothing clicks;
//! * global bypass and A/B comparison crossfade against a dry signal that is
//!   delayed by exactly the chain latency, optionally loudness matched.

use crate::dynamics::{Agc, Compressor, DeEsser, Expander, GainStage, NoiseGate};
use crate::filters::{HighPassFilter, ParametricEq};
use crate::limiter::Limiter;
use crate::noise::NoiseReducer;
use crate::params::{ChainParams, NoiseReductionMode, STAGE_COUNT, StageKind, StageParams};
use crate::smoother::LinearSmoother;
use crate::util::{db_to_gain, mean_square};
use crate::{DelayLine, Processor};

/// Snapshot of the chain's internal state for meters and indicators.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChainMeters {
    pub gate_open: bool,
    pub gate_gain_db: f32,
    pub expander_gr_db: f32,
    pub compressor_gr_db: f32,
    pub deesser_gr_db: f32,
    pub limiter_gr_db: f32,
    pub agc_gain_db: f32,
    pub nr_reduction_db: f32,
    pub vad: f32,
    pub latency_samples: usize,
    pub neural_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DuckPhase {
    Idle,
    Out,
    /// Silent while freshly reset latency stages fill up, so their
    /// start-up transient is never heard.
    Hold,
    In,
}

const STAGE_FADE_MS: f32 = 10.0;
const DUCK_MS: f32 = 5.0;
const BYPASS_FADE_MS: f32 = 15.0;

pub struct VoiceChain {
    sample_rate: f32,
    max_block: usize,
    params: ChainParams,
    pending: Option<ChainParams>,

    hpf: HighPassFilter,
    nr: NoiseReducer,
    gate: NoiseGate,
    expander: Expander,
    agc: Agc,
    eq: ParametricEq,
    deesser: DeEsser,
    compressor: Compressor,
    output_gain: GainStage,
    limiter: Limiter,

    stage_mix: [LinearSmoother; STAGE_COUNT],
    duck: LinearSmoother,
    duck_phase: DuckPhase,
    duck_hold: usize,

    ab_listen_dry: bool,
    loudness_match: bool,
    bypass_mix: LinearSmoother,
    wet_ms: f32,
    dry_ms: f32,
    match_gain: LinearSmoother,
    skipping: bool,

    dry_delay: DelayLine,
    dry: [Vec<f32>; 2],
    scratch: [Vec<f32>; 2],
    meters: ChainMeters,
}

impl VoiceChain {
    /// Builds a chain. `max_block` is the largest block that will be passed
    /// to [`process`](Self::process) in one call (larger blocks are split).
    pub fn new(sample_rate: f32, max_block: usize, params: ChainParams) -> Self {
        let params = params.sanitized();
        let max_block = max_block.max(16);
        let nr_max = 2048; // > largest STFT (1024) and neural latency (960)
        let lim_max = (crate::params::LimiterParams::MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 1;
        let mut chain = Self {
            sample_rate,
            max_block,
            params,
            pending: None,
            hpf: HighPassFilter::new(sample_rate, params.hpf),
            nr: NoiseReducer::new(sample_rate, params.noise_reduction),
            gate: NoiseGate::new(sample_rate, params.gate),
            expander: Expander::new(sample_rate, params.expander),
            agc: Agc::new(sample_rate, params.agc),
            eq: ParametricEq::new(sample_rate, params.eq),
            deesser: DeEsser::new(sample_rate, params.deesser),
            compressor: Compressor::new(sample_rate, params.compressor),
            output_gain: GainStage::new(sample_rate, params.output_gain),
            limiter: Limiter::new(sample_rate, params.limiter),
            stage_mix: std::array::from_fn(|_| LinearSmoother::with_ramp(0.0, sample_rate, STAGE_FADE_MS)),
            duck: LinearSmoother::with_ramp(1.0, sample_rate, DUCK_MS),
            duck_phase: DuckPhase::Idle,
            duck_hold: 0,
            ab_listen_dry: false,
            loudness_match: true,
            bypass_mix: LinearSmoother::with_ramp(if params.bypass { 1.0 } else { 0.0 }, sample_rate, BYPASS_FADE_MS),
            wet_ms: 0.0,
            dry_ms: 0.0,
            match_gain: LinearSmoother::with_ramp(1.0, sample_rate, 200.0),
            skipping: false,
            dry_delay: DelayLine::new(nr_max + lim_max),
            dry: [vec![0.0; max_block], vec![0.0; max_block]],
            scratch: [vec![0.0; max_block], vec![0.0; max_block]],
            meters: ChainMeters::default(),
        };
        for k in StageKind::DEFAULT_ORDER {
            chain.stage_mix[k.index()].snap(if params.is_enabled(k) { 1.0 } else { 0.0 });
        }
        chain.dry_delay.set_delay(chain.current_latency());
        chain.meters.latency_samples = chain.current_latency();
        chain
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Parameters as currently applied (pending changes not included).
    pub fn params(&self) -> &ChainParams {
        &self.params
    }

    /// Parameters including any change waiting for the duck to complete.
    pub fn target_params(&self) -> ChainParams {
        self.pending.unwrap_or(self.params)
    }

    pub fn meters(&self) -> ChainMeters {
        self.meters
    }

    /// Total latency of the active chain, in samples.
    pub fn current_latency(&self) -> usize {
        let mut l = 0;
        if self.params.noise_reduction.enabled {
            l += self.nr.latency_for(&self.params.noise_reduction);
        }
        if self.params.limiter.enabled {
            l += self.limiter.latency_for(&self.params.limiter);
        }
        l
    }

    fn latency_relevant_change(old: &ChainParams, new: &ChainParams) -> bool {
        let nr_o = &old.noise_reduction;
        let nr_n = &new.noise_reduction;
        old.order != new.order
            || nr_o.enabled != nr_n.enabled
            || (nr_n.enabled && (nr_o.mode != nr_n.mode || nr_o.quality != nr_n.quality))
            || old.limiter.enabled != new.limiter.enabled
            || (new.limiter.enabled && old.limiter.lookahead_ms != new.limiter.lookahead_ms)
    }

    /// Applies one stage's parameters.
    pub fn set_stage(&mut self, params: StageParams) {
        let params = params.sanitized();
        let mut next = self.target_params();
        next.set_stage(params);
        self.set_params(next);
    }

    /// Sets the stage order (limiter stays last).
    pub fn set_order(&mut self, order: [StageKind; STAGE_COUNT]) {
        let mut next = self.target_params();
        next.order = order;
        self.set_params(next);
    }

    /// Global bypass (dry, latency compensated).
    pub fn set_bypass(&mut self, bypass: bool) {
        let mut next = self.target_params();
        next.bypass = bypass;
        self.set_params(next);
    }

    /// A/B comparison: listen to the dry signal without changing the
    /// stored bypass state. `loudness_match` scales the dry signal to the
    /// loudness of the processed one so the comparison is fair.
    pub fn set_ab_listen(&mut self, listen_dry: bool, loudness_match: bool) {
        self.ab_listen_dry = listen_dry;
        self.loudness_match = loudness_match;
        self.update_bypass_target();
    }

    fn update_bypass_target(&mut self) {
        let dry = self.target_params().bypass || self.ab_listen_dry;
        self.bypass_mix.set_target(if dry { 1.0 } else { 0.0 });
    }

    /// Replaces all parameters, deciding per stage whether the change can be
    /// applied immediately or needs the duck/swap sequence.
    pub fn set_params(&mut self, params: ChainParams) {
        let params = params.sanitized();
        if Self::latency_relevant_change(&self.params, &params) {
            self.pending = Some(params);
            if matches!(self.duck_phase, DuckPhase::Idle | DuckPhase::In) {
                self.duck_phase = DuckPhase::Out;
                self.duck.set_target(0.0);
            }
            // Still apply non-structural parameters of unchanged stages now
            // so knobs feel immediate.
            self.apply_immediate(&params, false);
        } else {
            if self.pending.is_some() {
                self.pending = Some(params);
            }
            self.apply_immediate(&params, true);
        }
        self.update_bypass_target();
    }

    /// Pushes parameters to the stage processors. When `structural` is
    /// false, latency-affecting stages keep their current structure.
    fn apply_immediate(&mut self, p: &ChainParams, structural: bool) {
        self.hpf.set_params(p.hpf);
        self.gate.set_params(p.gate);
        self.expander.set_params(p.expander);
        self.agc.set_params(p.agc);
        self.eq.set_params(p.eq);
        self.deesser.set_params(p.deesser);
        self.compressor.set_params(p.compressor);
        self.output_gain.set_params(p.output_gain);

        let old = self.params;
        let mut applied = old;
        applied.bypass = p.bypass;
        applied.hpf = p.hpf;
        applied.gate = p.gate;
        applied.expander = p.expander;
        applied.agc = p.agc;
        applied.eq = p.eq;
        applied.deesser = p.deesser;
        applied.compressor = p.compressor;
        applied.output_gain = p.output_gain;
        // Noise reduction strength / limiter ceiling can change live.
        let same_nr_structure = old.noise_reduction.enabled == p.noise_reduction.enabled
            && old.noise_reduction.mode == p.noise_reduction.mode
            && old.noise_reduction.quality == p.noise_reduction.quality;
        if structural || same_nr_structure {
            applied.noise_reduction = p.noise_reduction;
            self.nr.set_params(p.noise_reduction);
        }
        let same_lim_structure = old.limiter.enabled == p.limiter.enabled && old.limiter.lookahead_ms == p.limiter.lookahead_ms;
        if structural || same_lim_structure {
            applied.limiter = p.limiter;
            self.limiter.set_params(p.limiter);
        }
        if structural {
            applied.order = p.order;
        }
        self.params = applied;
        for k in StageKind::DEFAULT_ORDER {
            if !k.has_latency() {
                self.stage_mix[k.index()].set_target(if p.is_enabled(k) { 1.0 } else { 0.0 });
            }
        }
    }

    /// Called at the bottom of a duck: apply structural changes in silence.
    /// Returns how many samples to stay silent while reset stages refill.
    fn apply_pending(&mut self) -> usize {
        let mut hold = 0;
        if let Some(p) = self.pending.take() {
            let nr_changed = p.noise_reduction.enabled != self.params.noise_reduction.enabled
                || p.noise_reduction.mode != self.params.noise_reduction.mode
                || p.noise_reduction.quality != self.params.noise_reduction.quality;
            let lim_changed = p.limiter.enabled != self.params.limiter.enabled || p.limiter.lookahead_ms != self.params.limiter.lookahead_ms;
            self.apply_immediate(&p, true);
            if nr_changed {
                self.nr.reset();
            }
            if lim_changed {
                self.limiter.reset();
            }
            for k in [StageKind::NoiseReduction, StageKind::Limiter] {
                self.stage_mix[k.index()].snap(if p.is_enabled(k) { 1.0 } else { 0.0 });
            }
            self.dry_delay.set_delay(self.current_latency());
            self.dry_delay.reset();
            self.meters.latency_samples = self.current_latency();
            hold = self.current_latency();
        }
        hold
    }

    /// Resets all internal state (stream restart).
    pub fn reset(&mut self) {
        self.hpf.reset();
        self.nr.reset();
        self.gate.reset();
        self.expander.reset();
        self.agc.reset();
        self.eq.reset();
        self.deesser.reset();
        self.compressor.reset();
        self.output_gain.reset();
        self.limiter.reset();
        self.dry_delay.reset();
        self.wet_ms = 0.0;
        self.dry_ms = 0.0;
    }

    fn run_processor(&mut self, kind: StageKind, left: &mut [f32], right: Option<&mut [f32]>) {
        match kind {
            StageKind::Hpf => self.hpf.process(left, right),
            StageKind::NoiseReduction => self.nr.process(left, right),
            StageKind::Gate => self.gate.process(left, right),
            StageKind::Expander => self.expander.process(left, right),
            StageKind::Agc => self.agc.process(left, right),
            StageKind::Eq => self.eq.process(left, right),
            StageKind::DeEsser => self.deesser.process(left, right),
            StageKind::Compressor => self.compressor.process(left, right),
            StageKind::OutputGain => self.output_gain.process(left, right),
            StageKind::Limiter => self.limiter.process(left, right),
        }
    }

    fn run_stage(&mut self, kind: StageKind, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let idx = kind.index();
        if kind.has_latency() {
            if self.params.is_enabled(kind) {
                self.run_processor(kind, left, right);
            }
            return;
        }
        let mix = &self.stage_mix[idx];
        if !mix.is_smoothing() {
            if mix.current() >= 1.0 {
                self.run_processor(kind, left, right);
            }
            return;
        }
        // Crossfading: keep the stage input as the dry reference.
        let n = left.len();
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch[0][..n].copy_from_slice(left);
        if let Some(r) = right.as_deref() {
            scratch[1][..n].copy_from_slice(r);
        }
        self.run_processor(kind, left, right.as_deref_mut());
        let mix = &mut self.stage_mix[idx];
        for i in 0..n {
            let m = mix.next_value();
            left[i] = scratch[0][i] + m * (left[i] - scratch[0][i]);
            if let Some(r) = right.as_deref_mut() {
                r[i] = scratch[1][i] + m * (r[i] - scratch[1][i]);
            }
        }
        self.scratch = scratch;
    }

    /// Processes a block in place. `right == None` means a mono channel.
    pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let total = left.len();
        let mut start = 0;
        while start < total {
            let end = (start + self.max_block).min(total);
            let r = right.as_deref_mut().map(|r| &mut r[start..end]);
            self.process_chunk(&mut left[start..end], r);
            start = end;
        }
    }

    fn process_chunk(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let n = left.len();
        let stereo = right.is_some();

        // 1. Keep the dry input.
        self.dry[0][..n].copy_from_slice(left);
        if let Some(r) = right.as_deref() {
            self.dry[1][..n].copy_from_slice(r);
        }

        // 2. Wet path (skipped when a hard bypass has settled, to save CPU).
        let settled_bypass = self.params.bypass && !self.bypass_mix.is_smoothing() && self.bypass_mix.current() >= 1.0;
        if settled_bypass {
            if !self.skipping {
                self.skipping = true;
            }
        } else {
            if self.skipping {
                // Coming back from a long bypass: start from clean state.
                self.skipping = false;
                self.hpf.reset();
                self.nr.reset();
                self.gate.reset();
                self.expander.reset();
                self.agc.reset();
                self.eq.reset();
                self.deesser.reset();
                self.compressor.reset();
                self.limiter.reset();
            }
            let order = self.params.order;
            for kind in order {
                self.run_stage(kind, left, right.as_deref_mut());
            }
        }

        // 3. Align the dry path with the chain latency.
        {
            let [dl, dr] = &mut self.dry;
            let dr = if stereo { Some(&mut dr[..n]) } else { None };
            self.dry_delay.process(&mut dl[..n], dr);
        }

        // 4. Loudness tracking for A/B matching (only while the wet path runs).
        if !self.skipping {
            let k = (-(n as f32) / (self.sample_rate * 1.5)).exp();
            self.wet_ms = k * self.wet_ms + (1.0 - k) * mean_square(left);
            self.dry_ms = k * self.dry_ms + (1.0 - k) * mean_square(&self.dry[0][..n]);
        }
        let want_match = self.ab_listen_dry && self.loudness_match && !self.params.bypass;
        let target_gain = if want_match && self.dry_ms > 1e-9 && self.wet_ms > 1e-9 {
            (self.wet_ms / self.dry_ms).sqrt().clamp(db_to_gain(-12.0), db_to_gain(12.0))
        } else {
            1.0
        };
        self.match_gain.set_target(target_gain);

        // 5. Wet/dry crossfade.
        if self.skipping {
            left.copy_from_slice(&self.dry[0][..n]);
            if let Some(r) = right.as_deref_mut() {
                r.copy_from_slice(&self.dry[1][..n]);
            }
        } else if self.bypass_mix.is_smoothing() || self.bypass_mix.current() > 0.0 || self.match_gain.is_smoothing() {
            for i in 0..n {
                let m = self.bypass_mix.next_value();
                let g = self.match_gain.next_value();
                left[i] = left[i] * (1.0 - m) + self.dry[0][i] * m * g;
                if let Some(r) = right.as_deref_mut() {
                    r[i] = r[i] * (1.0 - m) + self.dry[1][i] * m * g;
                }
            }
        }

        // 6. Duck around structural changes.
        if self.duck_phase != DuckPhase::Idle {
            match right {
                Some(r) => self.duck.apply_stereo(left, r),
                None => self.duck.apply(left),
            }
            if !self.duck.is_smoothing() {
                match self.duck_phase {
                    DuckPhase::Out => {
                        self.duck_hold = self.apply_pending();
                        if self.duck_hold > 0 {
                            self.duck_phase = DuckPhase::Hold;
                        } else {
                            self.duck_phase = DuckPhase::In;
                            self.duck.set_target(1.0);
                        }
                    }
                    DuckPhase::Hold => {
                        self.duck_hold = self.duck_hold.saturating_sub(n);
                        if self.duck_hold == 0 {
                            if self.pending.is_some() {
                                // Another structural change arrived meanwhile.
                                self.duck_phase = DuckPhase::Out;
                            } else {
                                self.duck_phase = DuckPhase::In;
                                self.duck.set_target(1.0);
                            }
                        }
                    }
                    DuckPhase::In => self.duck_phase = DuckPhase::Idle,
                    DuckPhase::Idle => {}
                }
            }
        }

        // 7. Meters.
        let m = &mut self.meters;
        m.gate_open = !self.params.gate.enabled || self.gate.is_open();
        m.gate_gain_db = if self.params.gate.enabled { self.gate.gain_db() } else { 0.0 };
        m.expander_gr_db = if self.params.expander.enabled { self.expander.gain_reduction_db() } else { 0.0 };
        m.compressor_gr_db = if self.params.compressor.enabled { self.compressor.gain_reduction_db() } else { 0.0 };
        m.deesser_gr_db = if self.params.deesser.enabled { self.deesser.gain_reduction_db() } else { 0.0 };
        m.limiter_gr_db = if self.params.limiter.enabled { self.limiter.gain_reduction_db() } else { 0.0 };
        m.agc_gain_db = if self.params.agc.enabled { self.agc.gain_db() } else { 0.0 };
        m.nr_reduction_db = if self.params.noise_reduction.enabled { self.nr.reduction_db() } else { 0.0 };
        m.vad = if self.params.noise_reduction.enabled { self.nr.vad() } else { 0.0 };
        m.neural_active = self.params.noise_reduction.enabled && self.nr.effective_mode() == NoiseReductionMode::Neural;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{CompressorParams, LimiterParams, NoiseReductionParams, VoicePreset};
    use crate::{signal, util};

    const SR: f32 = 48_000.0;

    fn run(chain: &mut VoiceChain, x: &mut [f32], block: usize) {
        for c in x.chunks_mut(block) {
            chain.process(c, None);
        }
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn raw_chain_is_transparent() {
        let mut c = VoiceChain::new(SR, 512, VoicePreset::Raw.chain());
        let x = signal::white_noise(4800, 0.5, 1);
        let mut y = x.clone();
        run(&mut c, &mut y, 128);
        assert_eq!(c.current_latency(), 0);
        for i in 0..x.len() {
            assert!((x[i] - y[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn every_preset_is_stable_on_speechlike_signal() {
        for preset in VoicePreset::ALL {
            let mut c = VoiceChain::new(SR, 1024, preset.chain());
            let mut x = signal::log_sweep(80.0, 8000.0, SR, 0.4, 48_000);
            let noise = signal::white_noise(48_000, 0.01, 2);
            for (a, b) in x.iter_mut().zip(noise) {
                *a += b;
            }
            run(&mut c, &mut x, 480);
            assert!(x.iter().all(|v| v.is_finite()), "{preset:?}");
            assert!(util::peak(&x) <= 1.0, "{preset:?}");
        }
    }

    #[test]
    fn bypass_is_latency_compensated_and_click_free() {
        let mut p = VoicePreset::Natural.chain();
        p.noise_reduction.enabled = true;
        let mut c = VoiceChain::new(SR, 512, p);
        let lat = c.current_latency();
        assert!(lat > 0);
        let x = signal::sine(500.0, SR, 0.3, 48_000);
        let mut y = x.clone();
        run(&mut c, &mut y[..24_000], 256);
        c.set_bypass(true);
        run(&mut c, &mut y[24_000..], 256);
        // After the crossfade the output equals the input delayed by the latency.
        for i in 30_000..48_000 {
            assert!((y[i] - x[i - lat]).abs() < 1e-4, "i {i}");
        }
        assert!(max_step(&y[20_000..]) < 0.06, "step {}", max_step(&y[20_000..]));
    }

    #[test]
    fn toggling_noise_reduction_ducks_instead_of_clicking() {
        let mut c = VoiceChain::new(SR, 512, VoicePreset::Raw.chain());
        let mut y = signal::sine(300.0, SR, 0.5, 48_000);
        run(&mut c, &mut y[..12_000], 256);
        c.set_stage(StageParams::NoiseReduction(NoiseReductionParams { enabled: true, ..Default::default() }));
        run(&mut c, &mut y[12_000..], 256);
        assert_eq!(c.current_latency(), 512);
        // A 300 Hz sine at 0.5 changes at most ~0.02 per sample; allow some
        // margin for the ducking envelope.
        assert!(max_step(&y) < 0.05, "max step {}", max_step(&y));
    }

    #[test]
    fn enabling_compressor_crossfades() {
        let mut c = VoiceChain::new(SR, 512, VoicePreset::Raw.chain());
        let mut y = signal::sine(300.0, SR, 0.8, 24_000);
        run(&mut c, &mut y[..12_000], 256);
        c.set_stage(StageParams::Compressor(CompressorParams {
            enabled: true,
            threshold_db: -30.0,
            ratio: 10.0,
            attack_ms: 0.1,
            ..Default::default()
        }));
        run(&mut c, &mut y[12_000..], 256);
        assert!(max_step(&y) < 0.05, "max step {}", max_step(&y));
        assert!(util::peak(&y[20_000..]) < 0.3);
    }

    #[test]
    fn ab_listen_matches_loudness() {
        let mut p = VoicePreset::Raw.chain();
        p.output_gain.gain_db = -12.0;
        let mut c = VoiceChain::new(SR, 512, p);
        let mut y = signal::sine(440.0, SR, 0.5, 96_000);
        run(&mut c, &mut y[..48_000], 480);
        c.set_ab_listen(true, true);
        run(&mut c, &mut y[48_000..], 480);
        let wet = util::rms(&y[30_000..46_000]);
        let dry = util::rms(&y[80_000..96_000]);
        let diff = util::gain_to_db(dry / wet);
        assert!(diff.abs() < 1.0, "A/B loudness mismatch {diff} dB");
    }

    #[test]
    fn limiter_lookahead_change_updates_latency() {
        let mut p = VoicePreset::Raw.chain();
        p.limiter = LimiterParams { enabled: true, lookahead_ms: 1.0, ..Default::default() };
        let mut c = VoiceChain::new(SR, 512, p);
        assert_eq!(c.current_latency(), 48);
        p.limiter.lookahead_ms = 5.0;
        c.set_params(p);
        let mut y = vec![0.0; 4800];
        run(&mut c, &mut y, 256);
        assert_eq!(c.current_latency(), 240);
    }

    #[test]
    fn stereo_and_large_blocks() {
        let mut c = VoiceChain::new(SR, 256, VoicePreset::Stream.chain());
        let mut l = signal::sine(220.0, SR, 0.3, 10_000);
        let mut r = signal::sine(330.0, SR, 0.3, 10_000);
        c.process(&mut l, Some(&mut r));
        assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
    }
}
