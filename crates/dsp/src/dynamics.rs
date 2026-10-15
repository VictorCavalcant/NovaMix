//! Dynamics processors: gate, expander, compressor, de-esser, AGC and the
//! plain gain stage. All are stereo-linked (the louder channel drives the
//! gain computer) so the stereo image never shifts.

use crate::Processor;
use crate::biquad::{Biquad, BiquadCoeffs};
use crate::params::{AgcParams, CompressorParams, DeEsserParams, DetectorMode, ExpanderParams, GateParams, OutputGainParams};
use crate::smoother::LinearSmoother;
use crate::util::{MIN_DB, db_to_gain, gain_to_db, power_to_db, time_constant_coeff};

#[inline]
fn frame_level(left: &[f32], right: &Option<&mut [f32]>, i: usize) -> f32 {
    match right {
        Some(r) => left[i].abs().max(r[i].abs()),
        None => left[i].abs(),
    }
}

#[inline]
fn apply_gain(left: &mut [f32], right: &mut Option<&mut [f32]>, i: usize, g: f32) {
    left[i] *= g;
    if let Some(r) = right {
        r[i] *= g;
    }
}

// ---------------------------------------------------------------------------
// Noise gate

/// Noise gate with hysteresis, hold and smooth attack/release.
#[derive(Debug, Clone)]
pub struct NoiseGate {
    sample_rate: f32,
    params: GateParams,
    env: f32,
    env_attack: f32,
    env_release: f32,
    gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    hold_samples: u32,
    hold_counter: u32,
    open: bool,
    open_threshold: f32,
    close_threshold: f32,
    floor_gain: f32,
}

impl NoiseGate {
    pub fn new(sample_rate: f32, params: GateParams) -> Self {
        let mut g = Self {
            sample_rate,
            params,
            env: 0.0,
            env_attack: time_constant_coeff(0.5, sample_rate),
            env_release: time_constant_coeff(25.0, sample_rate),
            gain: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            hold_samples: 0,
            hold_counter: 0,
            open: false,
            open_threshold: 0.0,
            close_threshold: 0.0,
            floor_gain: 0.0,
        };
        g.set_params(params);
        g.gain = g.floor_gain;
        g
    }

    pub fn set_params(&mut self, params: GateParams) {
        let p = params.sanitized();
        self.params = p;
        self.attack_coeff = time_constant_coeff(p.attack_ms, self.sample_rate);
        self.release_coeff = time_constant_coeff(p.release_ms / 4.0, self.sample_rate);
        self.hold_samples = (p.hold_ms * 0.001 * self.sample_rate) as u32;
        self.open_threshold = db_to_gain(p.threshold_db);
        self.close_threshold = db_to_gain(p.threshold_db - p.hysteresis_db);
        self.floor_gain = db_to_gain(p.range_db);
    }

    /// True while the gate is open (for the UI indicator).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Current attenuation in dB (0 = fully open).
    pub fn gain_db(&self) -> f32 {
        gain_to_db(self.gain).max(self.params.range_db)
    }
}

impl Processor for NoiseGate {
    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = self.floor_gain;
        self.open = false;
        self.hold_counter = 0;
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        for i in 0..left.len() {
            let x = frame_level(left, &right, i);
            let c = if x > self.env { self.env_attack } else { self.env_release };
            self.env = c * self.env + (1.0 - c) * x;

            if self.open {
                if self.env < self.close_threshold {
                    if self.hold_counter > 0 {
                        self.hold_counter -= 1;
                    } else {
                        self.open = false;
                    }
                } else {
                    self.hold_counter = self.hold_samples;
                }
            } else if self.env > self.open_threshold {
                self.open = true;
                self.hold_counter = self.hold_samples;
            }

            let target = if self.open { 1.0 } else { self.floor_gain };
            let coeff = if target > self.gain { self.attack_coeff } else { self.release_coeff };
            self.gain = target + (self.gain - target) * coeff;
            apply_gain(left, &mut right, i, self.gain);
        }
    }
}

// ---------------------------------------------------------------------------
// Downward expander

#[derive(Debug, Clone)]
pub struct Expander {
    sample_rate: f32,
    params: ExpanderParams,
    env: f32,
    env_attack: f32,
    env_release: f32,
    gr_db: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl Expander {
    pub fn new(sample_rate: f32, params: ExpanderParams) -> Self {
        let mut e = Self {
            sample_rate,
            params,
            env: 0.0,
            env_attack: time_constant_coeff(1.0, sample_rate),
            env_release: time_constant_coeff(40.0, sample_rate),
            gr_db: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
        };
        e.set_params(params);
        e
    }

    pub fn set_params(&mut self, params: ExpanderParams) {
        let p = params.sanitized();
        self.params = p;
        self.attack_coeff = time_constant_coeff(p.attack_ms, self.sample_rate);
        self.release_coeff = time_constant_coeff(p.release_ms, self.sample_rate);
    }

    /// Static curve: gain reduction (≤ 0 dB) for an input level.
    pub fn gain_reduction_for(&self, level_db: f32) -> f32 {
        let p = &self.params;
        let over = level_db - p.threshold_db;
        let w = p.knee_db;
        let slope = p.ratio - 1.0;
        let gr = if w > 0.0 && over.abs() * 2.0 <= w {
            let x = over - w * 0.5;
            -slope * x * x / (2.0 * w)
        } else if over < 0.0 {
            slope * over
        } else {
            0.0
        };
        gr.max(p.range_db)
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }
}

impl Processor for Expander {
    fn reset(&mut self) {
        self.env = 0.0;
        self.gr_db = 0.0;
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        for i in 0..left.len() {
            let x = frame_level(left, &right, i);
            let c = if x > self.env { self.env_attack } else { self.env_release };
            self.env = c * self.env + (1.0 - c) * x;
            let target = self.gain_reduction_for(gain_to_db(self.env));
            // Opening (less reduction) uses the attack time.
            let coeff = if target > self.gr_db { self.attack_coeff } else { self.release_coeff };
            self.gr_db = target + (self.gr_db - target) * coeff;
            apply_gain(left, &mut right, i, db_to_gain(self.gr_db));
        }
    }
}

// ---------------------------------------------------------------------------
// Compressor

/// Feed-forward compressor with soft knee and log-domain smoothing.
#[derive(Debug, Clone)]
pub struct Compressor {
    sample_rate: f32,
    params: CompressorParams,
    rms_state: f32,
    rms_coeff: f32,
    gr_db: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup: LinearSmoother,
}

impl Compressor {
    pub fn new(sample_rate: f32, params: CompressorParams) -> Self {
        let mut c = Self {
            sample_rate,
            params,
            rms_state: 0.0,
            rms_coeff: time_constant_coeff(10.0, sample_rate),
            gr_db: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            makeup: LinearSmoother::with_ramp(1.0, sample_rate, 30.0),
        };
        c.set_params(params);
        c.makeup.snap(db_to_gain(c.params.effective_makeup_db()));
        c
    }

    pub fn set_params(&mut self, params: CompressorParams) {
        let p = params.sanitized();
        self.params = p;
        self.attack_coeff = time_constant_coeff(p.attack_ms, self.sample_rate);
        self.release_coeff = time_constant_coeff(p.release_ms, self.sample_rate);
        self.makeup.set_target(db_to_gain(p.effective_makeup_db()));
    }

    /// Current gain reduction in dB (≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }
}

impl Processor for Compressor {
    fn reset(&mut self) {
        self.rms_state = 0.0;
        self.gr_db = 0.0;
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let rms_mode = self.params.detector == DetectorMode::Rms;
        for i in 0..left.len() {
            let x = frame_level(left, &right, i);
            let level_db = if rms_mode {
                self.rms_state = self.rms_coeff * self.rms_state + (1.0 - self.rms_coeff) * x * x;
                // +3 dB so a sine reads the same in peak and RMS modes.
                power_to_db(self.rms_state * 2.0)
            } else {
                gain_to_db(x)
            };
            let target = self.params.transfer_db(level_db) - level_db;
            let coeff = if target < self.gr_db { self.attack_coeff } else { self.release_coeff };
            self.gr_db = target + (self.gr_db - target) * coeff;
            let g = db_to_gain(self.gr_db) * self.makeup.next_value();
            apply_gain(left, &mut right, i, g);
        }
    }
}

// ---------------------------------------------------------------------------
// De-esser

/// Split-band de-esser. Sibilance is detected with a band-pass and removed
/// by subtracting a scaled copy of that band (`x - (1-g)·bp(x)`), which is
/// phase coherent because band-pass + notch sum to unity.
#[derive(Debug, Clone)]
pub struct DeEsser {
    sample_rate: f32,
    params: DeEsserParams,
    band: Biquad,
    env: f32,
    env_attack: f32,
    env_release: f32,
    gain: f32,
    threshold: f32,
}

impl DeEsser {
    pub fn new(sample_rate: f32, params: DeEsserParams) -> Self {
        let mut d = Self {
            sample_rate,
            params,
            band: Biquad::default(),
            env: 0.0,
            env_attack: time_constant_coeff(0.5, sample_rate),
            env_release: time_constant_coeff(60.0, sample_rate),
            gain: 1.0,
            threshold: 0.0,
        };
        d.set_params(params);
        d.band.set_coeffs_immediate(d.band.target_coeffs());
        d
    }

    pub fn set_params(&mut self, params: DeEsserParams) {
        let p = params.sanitized();
        self.params = p;
        self.band.set_coeffs(BiquadCoeffs::bandpass(self.sample_rate, p.frequency_hz, p.q));
        self.threshold = db_to_gain(p.threshold_db);
    }

    /// Current band attenuation in dB (≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        gain_to_db(self.gain)
    }
}

impl Processor for DeEsser {
    fn reset(&mut self) {
        self.band.reset();
        self.env = 0.0;
        self.gain = 1.0;
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let max_red = self.params.max_reduction_db;
        for i in 0..left.len() {
            self.band.tick();
            let bl = self.band.process_sample(left[i], 0);
            let br = match right.as_deref() {
                Some(r) => self.band.process_sample(r[i], 1),
                None => 0.0,
            };
            let level = bl.abs().max(br.abs());
            let c = if level > self.env { self.env_attack } else { self.env_release };
            self.env = c * self.env + (1.0 - c) * level;
            let over_db = if self.env > self.threshold { gain_to_db(self.env / self.threshold) } else { 0.0 };
            // Fixed 4:1 ratio above threshold, capped at the maximum reduction.
            let red_db = (over_db * 0.75).min(max_red);
            let target = db_to_gain(-red_db);
            let gc = if target < self.gain { 0.0 } else { self.env_release };
            self.gain = target + (self.gain - target) * gc;
            let k = 1.0 - self.gain;
            left[i] -= k * bl;
            if let Some(r) = right.as_deref_mut() {
                r[i] -= k * br;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AGC

/// Slow automatic gain control that never boosts below its noise floor.
#[derive(Debug, Clone)]
pub struct Agc {
    sample_rate: f32,
    params: AgcParams,
    power: f32,
    power_coeff: f32,
    gain_db: f32,
    rise_per_sample: f32,
    fall_per_sample: f32,
}

impl Agc {
    pub fn new(sample_rate: f32, params: AgcParams) -> Self {
        let mut a = Self {
            sample_rate,
            params,
            power: 0.0,
            power_coeff: time_constant_coeff(400.0, sample_rate),
            gain_db: 0.0,
            rise_per_sample: 0.0,
            fall_per_sample: 0.0,
        };
        a.set_params(params);
        a
    }

    pub fn set_params(&mut self, params: AgcParams) {
        let p = params.sanitized();
        self.params = p;
        let rise = p.speed.rise_db_per_sec();
        self.rise_per_sample = rise / self.sample_rate;
        // Lowering gain is allowed to be 3x faster than raising it.
        self.fall_per_sample = rise * 3.0 / self.sample_rate;
        self.gain_db = self.gain_db.clamp(-12.0, p.max_gain_db);
    }

    /// Gain currently applied, in dB.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }
}

impl Processor for Agc {
    fn reset(&mut self) {
        self.power = 0.0;
        self.gain_db = 0.0;
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let p = self.params;
        for i in 0..left.len() {
            let x = frame_level(left, &right, i);
            self.power = self.power_coeff * self.power + (1.0 - self.power_coeff) * x * x;
            let level_db = power_to_db(self.power);
            if level_db > p.noise_floor_db {
                let desired = (p.target_db - level_db).clamp(-12.0, p.max_gain_db);
                let diff = desired - self.gain_db;
                self.gain_db += diff.clamp(-self.fall_per_sample, self.rise_per_sample);
            }
            apply_gain(left, &mut right, i, db_to_gain(self.gain_db));
        }
    }
}

// ---------------------------------------------------------------------------
// Gain stage

/// Smoothed static gain (make-up / output gain).
#[derive(Debug, Clone)]
pub struct GainStage {
    gain: LinearSmoother,
}

impl GainStage {
    pub fn new(sample_rate: f32, params: OutputGainParams) -> Self {
        let mut gain = LinearSmoother::with_ramp(1.0, sample_rate, 20.0);
        gain.snap(db_to_gain(params.sanitized().gain_db));
        Self { gain }
    }

    pub fn set_params(&mut self, params: OutputGainParams) {
        let db = params.sanitized().gain_db;
        self.gain.set_target(if db <= MIN_DB { 0.0 } else { db_to_gain(db) });
    }
}

impl Processor for GainStage {
    fn reset(&mut self) {}

    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        match right {
            Some(r) => self.gain.apply_stereo(left, r),
            None => self.gain.apply(left),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, util};

    const SR: f32 = 48_000.0;

    fn level_db(buf: &[f32]) -> f32 {
        util::gain_to_db(util::rms(buf) * std::f32::consts::SQRT_2)
    }

    #[test]
    fn gate_attenuates_below_threshold_and_passes_above() {
        let p = GateParams { enabled: true, threshold_db: -40.0, range_db: -60.0, ..GateParams::default() };
        let mut g = NoiseGate::new(SR, p);
        let mut quiet = signal::sine(440.0, SR, util::db_to_gain(-55.0), 48_000);
        g.process(&mut quiet, None);
        assert!(level_db(&quiet[24_000..]) < -110.0, "{}", level_db(&quiet[24_000..]));
        let mut loud = signal::sine(440.0, SR, util::db_to_gain(-20.0), 48_000);
        g.process(&mut loud, None);
        assert!((level_db(&loud[4800..]) + 20.0).abs() < 0.2);
        assert!(g.is_open());
    }

    #[test]
    fn gate_hysteresis_keeps_gate_open_between_thresholds() {
        let p = GateParams { enabled: true, threshold_db: -40.0, hysteresis_db: 10.0, hold_ms: 0.0, ..GateParams::default() };
        let mut g = NoiseGate::new(SR, p);
        let mut loud = signal::sine(440.0, SR, util::db_to_gain(-30.0), 9600);
        g.process(&mut loud, None);
        assert!(g.is_open());
        // -45 dB: below open threshold, above close threshold (-50).
        let mut mid = signal::sine(440.0, SR, util::db_to_gain(-45.0), 24_000);
        g.process(&mut mid, None);
        assert!(g.is_open());
        let mut quiet = signal::sine(440.0, SR, util::db_to_gain(-60.0), 24_000);
        g.process(&mut quiet, None);
        assert!(!g.is_open());
    }

    #[test]
    fn gate_hold_delays_closing() {
        let p = GateParams { enabled: true, threshold_db: -40.0, hold_ms: 300.0, ..GateParams::default() };
        let mut g = NoiseGate::new(SR, p);
        let mut loud = signal::sine(440.0, SR, 0.3, 4800);
        g.process(&mut loud, None);
        let mut silence = vec![0.0; 4800]; // 100 ms
        g.process(&mut silence, None);
        assert!(g.is_open(), "still holding after 100 ms");
        let mut silence = vec![0.0; 24_000]; // +500 ms (envelope decay + hold)
        g.process(&mut silence, None);
        assert!(!g.is_open());
    }

    #[test]
    fn expander_reduces_quiet_signals_by_ratio() {
        let p = ExpanderParams { enabled: true, threshold_db: -30.0, ratio: 2.0, knee_db: 0.0, range_db: -60.0, ..ExpanderParams::default() };
        let mut e = Expander::new(SR, p);
        let mut x = signal::sine(1000.0, SR, util::db_to_gain(-40.0), 96_000);
        e.process(&mut x, None);
        // 10 dB below threshold at 1:2 → output 20 dB below threshold.
        let out = level_db(&x[48_000..]);
        assert!((out + 50.0).abs() < 1.5, "{out}");
        assert!((e.gain_reduction_for(-40.0) + 10.0).abs() < 1e-4);
        assert_eq!(e.gain_reduction_for(-10.0), 0.0);
    }

    #[test]
    fn compressor_static_gain_reduction() {
        let p = CompressorParams {
            enabled: true,
            threshold_db: -20.0,
            ratio: 4.0,
            knee_db: 0.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            detector: DetectorMode::Peak,
            ..CompressorParams::default()
        };
        let mut c = Compressor::new(SR, p);
        let mut x = signal::sine(1000.0, SR, util::db_to_gain(-8.0), 48_000);
        c.process(&mut x, None);
        // 12 dB over → 3 dB over → -17 dB out (peak detector follows the waveform, allow tolerance).
        let out = util::gain_to_db(util::peak(&x[24_000..]));
        assert!((out + 17.0).abs() < 1.0, "{out}");
        assert!(c.gain_reduction_db() < -8.0);
    }

    #[test]
    fn compressor_rms_mode_reads_sines_like_peak() {
        let p = CompressorParams { enabled: true, threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, ..CompressorParams::default() };
        let mut c = Compressor::new(SR, p);
        let mut x = signal::sine(1000.0, SR, util::db_to_gain(-8.0), 96_000);
        c.process(&mut x, None);
        let out = level_db(&x[48_000..]);
        assert!((out + 17.0).abs() < 0.7, "{out}");
    }

    #[test]
    fn compressor_attack_time_is_respected() {
        let p = CompressorParams {
            enabled: true,
            threshold_db: -30.0,
            ratio: 10.0,
            knee_db: 0.0,
            attack_ms: 20.0,
            detector: DetectorMode::Peak,
            ..CompressorParams::default()
        };
        let mut c = Compressor::new(SR, p);
        let mut x = vec![0.5; 4800];
        c.process(&mut x[..48], None); // 1 ms
        let early = c.gain_reduction_db();
        c.process(&mut x[48..], None);
        let late = c.gain_reduction_db();
        assert!(early > late * 0.2, "early {early} late {late}");
    }

    #[test]
    fn deesser_reduces_sibilant_band_only() {
        let p = DeEsserParams { enabled: true, frequency_hz: 7000.0, threshold_db: -40.0, max_reduction_db: 12.0, q: 1.5 };
        let mut d = DeEsser::new(SR, p);
        let mut ess = signal::sine(7000.0, SR, 0.3, 48_000);
        d.process(&mut ess, None);
        let red = level_db(&ess[24_000..]) - util::gain_to_db(0.3);
        assert!(red < -6.0, "sibilance reduced by {red}");

        let mut d = DeEsser::new(SR, p);
        let mut low = signal::sine(300.0, SR, 0.3, 48_000);
        d.process(&mut low, None);
        let red = level_db(&low[24_000..]) - util::gain_to_db(0.3);
        assert!(red.abs() < 0.5, "low frequencies untouched: {red}");
    }

    #[test]
    fn agc_moves_towards_target_slowly() {
        let p = AgcParams { enabled: true, target_db: -20.0, max_gain_db: 12.0, ..AgcParams::default() };
        let mut a = Agc::new(SR, p);
        let mut x = signal::sine(440.0, SR, util::db_to_gain(-27.0), 48_000 * 6);
        a.process(&mut x, None);
        // RMS of the input is -30 dBFS → wants +10 dB; Slow = 2 dB/s for 6 s.
        assert!(a.gain_db() > 8.0 && a.gain_db() <= 10.5, "{}", a.gain_db());
    }

    #[test]
    fn agc_freezes_below_noise_floor() {
        let p = AgcParams { enabled: true, noise_floor_db: -50.0, ..AgcParams::default() };
        let mut a = Agc::new(SR, p);
        let mut x = signal::sine(440.0, SR, util::db_to_gain(-70.0), 48_000 * 3);
        a.process(&mut x, None);
        assert_eq!(a.gain_db(), 0.0);
    }

    #[test]
    fn stereo_link_keeps_image() {
        let p = CompressorParams { enabled: true, threshold_db: -30.0, ratio: 8.0, ..CompressorParams::default() };
        let mut c = Compressor::new(SR, p);
        let mut l = signal::sine(1000.0, SR, 0.5, 24_000);
        let mut r = signal::sine(1000.0, SR, 0.05, 24_000);
        c.process(&mut l, Some(&mut r));
        let ratio = util::rms(&l[12_000..]) / util::rms(&r[12_000..]);
        assert!((ratio - 10.0).abs() < 0.1, "{ratio}");
    }
}
