//! User-facing parameter structures for every processing stage.
//!
//! All structures are `Copy` so they can travel through the lock-free
//! command queue into the audio thread without allocating. Each one knows
//! its valid range (`sanitized()`), which is applied both when a profile is
//! loaded from disk and again inside the engine as a last line of defense.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Clamps `v` into `[lo, hi]`, replacing NaN/inf with `default`.
#[inline]
pub fn clampf(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

macro_rules! serde_struct {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
        pub struct $name { $($body)* }
    };
}

macro_rules! serde_enum {
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
        pub enum $name { $($body)* }
    };
}

// ---------------------------------------------------------------------------
// High-pass filter

serde_enum! {
    /// Roll-off slope of the high-pass filter.
    pub enum HpfSlope {
        Db6,
        #[default]
        Db12,
        Db18,
        Db24,
    }
}

impl HpfSlope {
    pub const ALL: [HpfSlope; 4] = [HpfSlope::Db6, HpfSlope::Db12, HpfSlope::Db18, HpfSlope::Db24];

    pub fn db_per_octave(self) -> u32 {
        match self {
            HpfSlope::Db6 => 6,
            HpfSlope::Db12 => 12,
            HpfSlope::Db18 => 18,
            HpfSlope::Db24 => 24,
        }
    }
}

serde_struct! {
    /// High-pass filter: removes rumble, handling noise and plosive energy.
    pub struct HpfParams {
        pub enabled: bool,
        pub frequency_hz: f32,
        pub slope: HpfSlope,
    }
}

impl Default for HpfParams {
    fn default() -> Self {
        Self { enabled: true, frequency_hz: 80.0, slope: HpfSlope::Db12 }
    }
}

impl HpfParams {
    pub const FREQ_RANGE: (f32, f32) = (20.0, 500.0);
    pub fn sanitized(mut self) -> Self {
        self.frequency_hz = clampf(self.frequency_hz, Self::FREQ_RANGE.0, Self::FREQ_RANGE.1, 80.0);
        self
    }
}

// ---------------------------------------------------------------------------
// Noise reduction

serde_enum! {
    /// Noise reduction algorithm.
    pub enum NoiseReductionMode {
        /// Classic STFT spectral suppression with minimum-statistics noise
        /// tracking. Light on CPU, works at any sample rate.
        #[default]
        Spectral,
        /// RNNoise-based neural suppression (local, no network). Handles
        /// non-stationary noise better; requires 48 kHz.
        Neural,
    }
}

serde_enum! {
    /// Trade-off between latency/CPU and frequency resolution for the
    /// spectral algorithm.
    pub enum NrQuality {
        /// 256-point frames (~5 ms latency at 48 kHz).
        LowLatency,
        /// 512-point frames (~11 ms latency at 48 kHz).
        #[default]
        Balanced,
        /// 1024-point frames (~21 ms latency at 48 kHz).
        HighQuality,
    }
}

impl NrQuality {
    pub const ALL: [NrQuality; 3] = [NrQuality::LowLatency, NrQuality::Balanced, NrQuality::HighQuality];

    pub fn fft_size(self) -> usize {
        match self {
            NrQuality::LowLatency => 256,
            NrQuality::Balanced => 512,
            NrQuality::HighQuality => 1024,
        }
    }
}

serde_struct! {
    /// Noise reduction parameters.
    pub struct NoiseReductionParams {
        pub enabled: bool,
        pub mode: NoiseReductionMode,
        /// Aggressiveness, 0..1.
        pub strength: f32,
        /// Maximum attenuation applied to noise-only regions.
        pub max_reduction_db: f32,
        pub quality: NrQuality,
    }
}

impl Default for NoiseReductionParams {
    fn default() -> Self {
        Self { enabled: false, mode: NoiseReductionMode::Spectral, strength: 0.5, max_reduction_db: 18.0, quality: NrQuality::Balanced }
    }
}

impl NoiseReductionParams {
    pub fn sanitized(mut self) -> Self {
        self.strength = clampf(self.strength, 0.0, 1.0, 0.5);
        self.max_reduction_db = clampf(self.max_reduction_db, 0.0, 40.0, 18.0);
        self
    }
}

// ---------------------------------------------------------------------------
// Gate / expander

serde_struct! {
    /// Noise gate with hysteresis and hold.
    pub struct GateParams {
        pub enabled: bool,
        pub threshold_db: f32,
        /// The gate closes at `threshold_db - hysteresis_db`.
        pub hysteresis_db: f32,
        pub attack_ms: f32,
        pub hold_ms: f32,
        pub release_ms: f32,
        /// Attenuation while closed (negative dB; -90 = silence).
        pub range_db: f32,
    }
}

impl Default for GateParams {
    fn default() -> Self {
        Self { enabled: false, threshold_db: -50.0, hysteresis_db: 6.0, attack_ms: 1.0, hold_ms: 150.0, release_ms: 200.0, range_db: -40.0 }
    }
}

impl GateParams {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.threshold_db = clampf(self.threshold_db, -90.0, 0.0, d.threshold_db);
        self.hysteresis_db = clampf(self.hysteresis_db, 0.0, 20.0, d.hysteresis_db);
        self.attack_ms = clampf(self.attack_ms, 0.05, 50.0, d.attack_ms);
        self.hold_ms = clampf(self.hold_ms, 0.0, 1000.0, d.hold_ms);
        self.release_ms = clampf(self.release_ms, 5.0, 2000.0, d.release_ms);
        self.range_db = clampf(self.range_db, -90.0, 0.0, d.range_db);
        self
    }
}

serde_struct! {
    /// Downward expander: a gentler alternative to the gate.
    pub struct ExpanderParams {
        pub enabled: bool,
        pub threshold_db: f32,
        pub ratio: f32,
        pub attack_ms: f32,
        pub release_ms: f32,
        /// Maximum attenuation (negative dB).
        pub range_db: f32,
        pub knee_db: f32,
    }
}

impl Default for ExpanderParams {
    fn default() -> Self {
        Self { enabled: false, threshold_db: -45.0, ratio: 2.0, attack_ms: 2.0, release_ms: 150.0, range_db: -24.0, knee_db: 6.0 }
    }
}

impl ExpanderParams {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.threshold_db = clampf(self.threshold_db, -90.0, 0.0, d.threshold_db);
        self.ratio = clampf(self.ratio, 1.0, 10.0, d.ratio);
        self.attack_ms = clampf(self.attack_ms, 0.1, 100.0, d.attack_ms);
        self.release_ms = clampf(self.release_ms, 5.0, 2000.0, d.release_ms);
        self.range_db = clampf(self.range_db, -60.0, 0.0, d.range_db);
        self.knee_db = clampf(self.knee_db, 0.0, 12.0, d.knee_db);
        self
    }
}

// ---------------------------------------------------------------------------
// AGC

serde_enum! {
    pub enum AgcSpeed {
        #[default]
        Slow,
        Normal,
        Fast,
    }
}

impl AgcSpeed {
    pub const ALL: [AgcSpeed; 3] = [AgcSpeed::Slow, AgcSpeed::Normal, AgcSpeed::Fast];

    /// Maximum gain change rate when raising the gain, in dB per second.
    pub fn rise_db_per_sec(self) -> f32 {
        match self {
            AgcSpeed::Slow => 2.0,
            AgcSpeed::Normal => 5.0,
            AgcSpeed::Fast => 12.0,
        }
    }
}

serde_struct! {
    /// Slow automatic gain control. Off by default to avoid pumping.
    pub struct AgcParams {
        pub enabled: bool,
        pub target_db: f32,
        pub max_gain_db: f32,
        pub speed: AgcSpeed,
        /// Below this RMS level the AGC freezes (does not boost noise).
        pub noise_floor_db: f32,
    }
}

impl Default for AgcParams {
    fn default() -> Self {
        Self { enabled: false, target_db: -20.0, max_gain_db: 12.0, speed: AgcSpeed::Slow, noise_floor_db: -50.0 }
    }
}

impl AgcParams {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.target_db = clampf(self.target_db, -36.0, -6.0, d.target_db);
        self.max_gain_db = clampf(self.max_gain_db, 0.0, 30.0, d.max_gain_db);
        self.noise_floor_db = clampf(self.noise_floor_db, -80.0, -30.0, d.noise_floor_db);
        self
    }
}

// ---------------------------------------------------------------------------
// Equalizer

serde_enum! {
    pub enum EqBandKind {
        #[default]
        Peak,
        LowShelf,
        HighShelf,
        LowPass,
        HighPass,
        Notch,
    }
}

impl EqBandKind {
    pub const ALL: [EqBandKind; 6] =
        [EqBandKind::Peak, EqBandKind::LowShelf, EqBandKind::HighShelf, EqBandKind::LowPass, EqBandKind::HighPass, EqBandKind::Notch];

    /// Whether the gain parameter has any effect for this band type.
    pub fn uses_gain(self) -> bool {
        matches!(self, EqBandKind::Peak | EqBandKind::LowShelf | EqBandKind::HighShelf)
    }
}

serde_struct! {
    pub struct EqBand {
        pub enabled: bool,
        pub kind: EqBandKind,
        pub frequency_hz: f32,
        pub gain_db: f32,
        pub q: f32,
    }
}

impl Default for EqBand {
    fn default() -> Self {
        Self { enabled: true, kind: EqBandKind::Peak, frequency_hz: 1000.0, gain_db: 0.0, q: 1.0 }
    }
}

impl EqBand {
    pub const fn new(kind: EqBandKind, frequency_hz: f32, gain_db: f32, q: f32) -> Self {
        Self { enabled: true, kind, frequency_hz, gain_db, q }
    }

    pub fn sanitized(mut self) -> Self {
        self.frequency_hz = clampf(self.frequency_hz, 20.0, 20_000.0, 1000.0);
        self.gain_db = clampf(self.gain_db, -18.0, 18.0, 0.0);
        self.q = clampf(self.q, 0.1, 18.0, 1.0);
        self
    }

    /// Biquad coefficients for this band.
    pub fn coeffs(&self, sample_rate: f32) -> crate::biquad::BiquadCoeffs {
        use crate::biquad::BiquadCoeffs as C;
        let b = self.sanitized();
        if !b.enabled {
            return C::IDENTITY;
        }
        match b.kind {
            EqBandKind::Peak => C::peaking(sample_rate, b.frequency_hz, b.q, b.gain_db),
            EqBandKind::LowShelf => C::low_shelf(sample_rate, b.frequency_hz, b.q, b.gain_db),
            EqBandKind::HighShelf => C::high_shelf(sample_rate, b.frequency_hz, b.q, b.gain_db),
            EqBandKind::LowPass => C::lowpass(sample_rate, b.frequency_hz, b.q),
            EqBandKind::HighPass => C::highpass(sample_rate, b.frequency_hz, b.q),
            EqBandKind::Notch => C::notch(sample_rate, b.frequency_hz, b.q),
        }
    }
}

/// Maximum number of parametric EQ bands per channel.
pub const MAX_EQ_BANDS: usize = 10;

serde_struct! {
    /// Parametric equalizer (up to [`MAX_EQ_BANDS`] bands).
    pub struct EqParams {
        pub enabled: bool,
        pub band_count: u8,
        pub bands: [EqBand; MAX_EQ_BANDS],
    }
}

impl Default for EqParams {
    fn default() -> Self {
        let mut bands = [EqBand::default(); MAX_EQ_BANDS];
        bands[0] = EqBand::new(EqBandKind::LowShelf, 100.0, 0.0, 0.707);
        bands[1] = EqBand::new(EqBandKind::Peak, 300.0, 0.0, 1.0);
        bands[2] = EqBand::new(EqBandKind::Peak, 1200.0, 0.0, 1.0);
        bands[3] = EqBand::new(EqBandKind::Peak, 3500.0, 0.0, 1.0);
        bands[4] = EqBand::new(EqBandKind::HighShelf, 9000.0, 0.0, 0.707);
        let extra = [60.0, 150.0, 600.0, 2400.0, 6000.0];
        for (i, f) in extra.iter().enumerate() {
            bands[5 + i] = EqBand::new(EqBandKind::Peak, *f, 0.0, 1.0);
        }
        Self { enabled: false, band_count: 5, bands }
    }
}

impl EqParams {
    pub fn sanitized(mut self) -> Self {
        self.band_count = self.band_count.clamp(1, MAX_EQ_BANDS as u8);
        for b in self.bands.iter_mut() {
            *b = b.sanitized();
        }
        self
    }

    pub fn active_bands(&self) -> &[EqBand] {
        &self.bands[..self.band_count.clamp(1, MAX_EQ_BANDS as u8) as usize]
    }

    /// Combined magnitude response (dB) at `freq`, for the curve display.
    pub fn magnitude_db(&self, freq: f32, sample_rate: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        self.active_bands().iter().filter(|b| b.enabled).map(|b| b.coeffs(sample_rate).magnitude_db(freq, sample_rate)).sum()
    }

    /// Switches between the 5-band "simple" and 10-band layouts.
    pub fn set_band_count(&mut self, count: u8) {
        self.band_count = count.clamp(1, MAX_EQ_BANDS as u8);
    }
}

// ---------------------------------------------------------------------------
// De-esser

serde_struct! {
    /// Dynamic band attenuation of sibilance.
    pub struct DeEsserParams {
        pub enabled: bool,
        pub frequency_hz: f32,
        pub q: f32,
        pub threshold_db: f32,
        pub max_reduction_db: f32,
    }
}

impl Default for DeEsserParams {
    fn default() -> Self {
        Self { enabled: false, frequency_hz: 6500.0, q: 1.5, threshold_db: -30.0, max_reduction_db: 8.0 }
    }
}

impl DeEsserParams {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.frequency_hz = clampf(self.frequency_hz, 2000.0, 12_000.0, d.frequency_hz);
        self.q = clampf(self.q, 0.5, 6.0, d.q);
        self.threshold_db = clampf(self.threshold_db, -60.0, 0.0, d.threshold_db);
        self.max_reduction_db = clampf(self.max_reduction_db, 0.0, 24.0, d.max_reduction_db);
        self
    }
}

// ---------------------------------------------------------------------------
// Compressor

serde_enum! {
    pub enum DetectorMode {
        Peak,
        #[default]
        Rms,
    }
}

serde_struct! {
    pub struct CompressorParams {
        pub enabled: bool,
        pub threshold_db: f32,
        pub ratio: f32,
        pub attack_ms: f32,
        pub release_ms: f32,
        pub knee_db: f32,
        pub makeup_db: f32,
        pub auto_makeup: bool,
        pub detector: DetectorMode,
    }
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            knee_db: 6.0,
            makeup_db: 0.0,
            auto_makeup: false,
            detector: DetectorMode::Rms,
        }
    }
}

impl CompressorParams {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.threshold_db = clampf(self.threshold_db, -60.0, 0.0, d.threshold_db);
        self.ratio = clampf(self.ratio, 1.0, 20.0, d.ratio);
        self.attack_ms = clampf(self.attack_ms, 0.1, 200.0, d.attack_ms);
        self.release_ms = clampf(self.release_ms, 5.0, 2000.0, d.release_ms);
        self.knee_db = clampf(self.knee_db, 0.0, 24.0, d.knee_db);
        self.makeup_db = clampf(self.makeup_db, -12.0, 24.0, d.makeup_db);
        self
    }

    /// Static gain computer (soft knee), returns the output level for an
    /// input level, both in dB. Shared by the processor and the UI curve.
    pub fn transfer_db(&self, input_db: f32) -> f32 {
        let t = self.threshold_db;
        let r = self.ratio.max(1.0);
        let w = self.knee_db.max(0.0);
        let over = input_db - t;
        if w > 0.0 && 2.0 * over.abs() <= w {
            let x = over + w * 0.5;
            input_db + (1.0 / r - 1.0) * x * x / (2.0 * w)
        } else if over > 0.0 {
            t + over / r
        } else {
            input_db
        }
    }

    /// Estimated makeup gain when `auto_makeup` is on.
    pub fn auto_makeup_db(&self) -> f32 {
        (-(self.threshold_db * (1.0 - 1.0 / self.ratio.max(1.0))) * 0.5).clamp(0.0, 24.0)
    }

    pub fn effective_makeup_db(&self) -> f32 {
        if self.auto_makeup { self.auto_makeup_db() } else { self.makeup_db }
    }
}

// ---------------------------------------------------------------------------
// Output gain and limiter

serde_struct! {
    /// Make-up / output gain stage.
    pub struct OutputGainParams {
        pub enabled: bool,
        pub gain_db: f32,
    }
}

impl Default for OutputGainParams {
    fn default() -> Self {
        Self { enabled: true, gain_db: 0.0 }
    }
}

impl OutputGainParams {
    pub fn sanitized(mut self) -> Self {
        self.gain_db = clampf(self.gain_db, -24.0, 24.0, 0.0);
        self
    }
}

serde_struct! {
    /// Safety brick-wall limiter with optional look-ahead.
    pub struct LimiterParams {
        pub enabled: bool,
        pub ceiling_db: f32,
        pub lookahead_ms: f32,
        pub release_ms: f32,
    }
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self { enabled: true, ceiling_db: -1.0, lookahead_ms: 2.0, release_ms: 80.0 }
    }
}

impl LimiterParams {
    pub const MAX_LOOKAHEAD_MS: f32 = 10.0;

    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.ceiling_db = clampf(self.ceiling_db, -24.0, 0.0, d.ceiling_db);
        self.lookahead_ms = clampf(self.lookahead_ms, 0.0, Self::MAX_LOOKAHEAD_MS, d.lookahead_ms);
        self.release_ms = clampf(self.release_ms, 1.0, 1000.0, d.release_ms);
        self
    }
}

// ---------------------------------------------------------------------------
// Chain

serde_enum! {
    /// Identifies a processing stage inside a [`ChainParams`].
    pub enum StageKind {
        #[default]
        Hpf,
        NoiseReduction,
        Gate,
        Expander,
        Agc,
        Eq,
        DeEsser,
        Compressor,
        OutputGain,
        Limiter,
    }
}

/// Number of stages in a voice chain.
pub const STAGE_COUNT: usize = 10;

impl StageKind {
    /// Default processing order: HPF → noise reduction → gate/expander →
    /// AGC → EQ → de-esser → compressor → make-up gain → limiter.
    pub const DEFAULT_ORDER: [StageKind; STAGE_COUNT] = [
        StageKind::Hpf,
        StageKind::NoiseReduction,
        StageKind::Gate,
        StageKind::Expander,
        StageKind::Agc,
        StageKind::Eq,
        StageKind::DeEsser,
        StageKind::Compressor,
        StageKind::OutputGain,
        StageKind::Limiter,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Stages whose position is fixed (the safety limiter is always last).
    pub fn is_pinned(self) -> bool {
        matches!(self, StageKind::Limiter)
    }

    /// Stages that add latency; toggling them briefly ducks the output
    /// instead of crossfading (see `VoiceChain`).
    pub fn has_latency(self) -> bool {
        matches!(self, StageKind::NoiseReduction | StageKind::Limiter)
    }

    /// Stable machine-readable key (used for i18n lookup and config).
    pub fn key(self) -> &'static str {
        match self {
            StageKind::Hpf => "hpf",
            StageKind::NoiseReduction => "noise_reduction",
            StageKind::Gate => "gate",
            StageKind::Expander => "expander",
            StageKind::Agc => "agc",
            StageKind::Eq => "eq",
            StageKind::DeEsser => "deesser",
            StageKind::Compressor => "compressor",
            StageKind::OutputGain => "output_gain",
            StageKind::Limiter => "limiter",
        }
    }
}

/// Parameters of a single stage; the unit of change sent to the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageParams {
    Hpf(HpfParams),
    NoiseReduction(NoiseReductionParams),
    Gate(GateParams),
    Expander(ExpanderParams),
    Agc(AgcParams),
    Eq(EqParams),
    DeEsser(DeEsserParams),
    Compressor(CompressorParams),
    OutputGain(OutputGainParams),
    Limiter(LimiterParams),
}

impl StageParams {
    pub fn kind(&self) -> StageKind {
        match self {
            StageParams::Hpf(_) => StageKind::Hpf,
            StageParams::NoiseReduction(_) => StageKind::NoiseReduction,
            StageParams::Gate(_) => StageKind::Gate,
            StageParams::Expander(_) => StageKind::Expander,
            StageParams::Agc(_) => StageKind::Agc,
            StageParams::Eq(_) => StageKind::Eq,
            StageParams::DeEsser(_) => StageKind::DeEsser,
            StageParams::Compressor(_) => StageKind::Compressor,
            StageParams::OutputGain(_) => StageKind::OutputGain,
            StageParams::Limiter(_) => StageKind::Limiter,
        }
    }

    pub fn sanitized(self) -> Self {
        match self {
            StageParams::Hpf(p) => StageParams::Hpf(p.sanitized()),
            StageParams::NoiseReduction(p) => StageParams::NoiseReduction(p.sanitized()),
            StageParams::Gate(p) => StageParams::Gate(p.sanitized()),
            StageParams::Expander(p) => StageParams::Expander(p.sanitized()),
            StageParams::Agc(p) => StageParams::Agc(p.sanitized()),
            StageParams::Eq(p) => StageParams::Eq(p.sanitized()),
            StageParams::DeEsser(p) => StageParams::DeEsser(p.sanitized()),
            StageParams::Compressor(p) => StageParams::Compressor(p.sanitized()),
            StageParams::OutputGain(p) => StageParams::OutputGain(p.sanitized()),
            StageParams::Limiter(p) => StageParams::Limiter(p.sanitized()),
        }
    }
}

serde_struct! {
    /// Complete voice/effects chain of one channel.
    pub struct ChainParams {
        /// Global bypass ("A/B" dry signal, latency compensated).
        pub bypass: bool,
        pub order: [StageKind; STAGE_COUNT],
        pub hpf: HpfParams,
        pub noise_reduction: NoiseReductionParams,
        pub gate: GateParams,
        pub expander: ExpanderParams,
        pub agc: AgcParams,
        pub eq: EqParams,
        pub deesser: DeEsserParams,
        pub compressor: CompressorParams,
        pub output_gain: OutputGainParams,
        pub limiter: LimiterParams,
    }
}

impl Default for ChainParams {
    fn default() -> Self {
        VoicePreset::Raw.chain()
    }
}

impl ChainParams {
    /// Returns a copy with every value in range and a valid stage order.
    pub fn sanitized(mut self) -> Self {
        self.order = sanitize_order(self.order);
        self.hpf = self.hpf.sanitized();
        self.noise_reduction = self.noise_reduction.sanitized();
        self.gate = self.gate.sanitized();
        self.expander = self.expander.sanitized();
        self.agc = self.agc.sanitized();
        self.eq = self.eq.sanitized();
        self.deesser = self.deesser.sanitized();
        self.compressor = self.compressor.sanitized();
        self.output_gain = self.output_gain.sanitized();
        self.limiter = self.limiter.sanitized();
        self
    }

    pub fn stage(&self, kind: StageKind) -> StageParams {
        match kind {
            StageKind::Hpf => StageParams::Hpf(self.hpf),
            StageKind::NoiseReduction => StageParams::NoiseReduction(self.noise_reduction),
            StageKind::Gate => StageParams::Gate(self.gate),
            StageKind::Expander => StageParams::Expander(self.expander),
            StageKind::Agc => StageParams::Agc(self.agc),
            StageKind::Eq => StageParams::Eq(self.eq),
            StageKind::DeEsser => StageParams::DeEsser(self.deesser),
            StageKind::Compressor => StageParams::Compressor(self.compressor),
            StageKind::OutputGain => StageParams::OutputGain(self.output_gain),
            StageKind::Limiter => StageParams::Limiter(self.limiter),
        }
    }

    pub fn set_stage(&mut self, params: StageParams) {
        match params {
            StageParams::Hpf(p) => self.hpf = p,
            StageParams::NoiseReduction(p) => self.noise_reduction = p,
            StageParams::Gate(p) => self.gate = p,
            StageParams::Expander(p) => self.expander = p,
            StageParams::Agc(p) => self.agc = p,
            StageParams::Eq(p) => self.eq = p,
            StageParams::DeEsser(p) => self.deesser = p,
            StageParams::Compressor(p) => self.compressor = p,
            StageParams::OutputGain(p) => self.output_gain = p,
            StageParams::Limiter(p) => self.limiter = p,
        }
    }

    pub fn is_enabled(&self, kind: StageKind) -> bool {
        match kind {
            StageKind::Hpf => self.hpf.enabled,
            StageKind::NoiseReduction => self.noise_reduction.enabled,
            StageKind::Gate => self.gate.enabled,
            StageKind::Expander => self.expander.enabled,
            StageKind::Agc => self.agc.enabled,
            StageKind::Eq => self.eq.enabled,
            StageKind::DeEsser => self.deesser.enabled,
            StageKind::Compressor => self.compressor.enabled,
            StageKind::OutputGain => self.output_gain.enabled,
            StageKind::Limiter => self.limiter.enabled,
        }
    }

    pub fn set_enabled(&mut self, kind: StageKind, enabled: bool) {
        match kind {
            StageKind::Hpf => self.hpf.enabled = enabled,
            StageKind::NoiseReduction => self.noise_reduction.enabled = enabled,
            StageKind::Gate => self.gate.enabled = enabled,
            StageKind::Expander => self.expander.enabled = enabled,
            StageKind::Agc => self.agc.enabled = enabled,
            StageKind::Eq => self.eq.enabled = enabled,
            StageKind::DeEsser => self.deesser.enabled = enabled,
            StageKind::Compressor => self.compressor.enabled = enabled,
            StageKind::OutputGain => self.output_gain.enabled = enabled,
            StageKind::Limiter => self.limiter.enabled = enabled,
        }
    }

    /// Moves `kind` one position earlier (`-1`) or later (`+1`) in the
    /// order, respecting pinned stages. Returns true if the order changed.
    pub fn move_stage(&mut self, kind: StageKind, delta: i32) -> bool {
        if kind.is_pinned() {
            return false;
        }
        let Some(pos) = self.order.iter().position(|k| *k == kind) else {
            return false;
        };
        let new_pos = pos as i32 + delta;
        if new_pos < 0 || new_pos as usize >= STAGE_COUNT {
            return false;
        }
        let new_pos = new_pos as usize;
        if self.order[new_pos].is_pinned() {
            return false;
        }
        self.order.swap(pos, new_pos);
        true
    }

    /// Number of enabled stages (for compact UI summaries).
    pub fn enabled_count(&self) -> usize {
        StageKind::DEFAULT_ORDER.iter().filter(|k| self.is_enabled(**k)).count()
    }
}

/// Ensures `order` is a permutation of all stages with pinned stages at the
/// end. Invalid orders fall back to the default order.
pub fn sanitize_order(order: [StageKind; STAGE_COUNT]) -> [StageKind; STAGE_COUNT] {
    let mut seen = [false; STAGE_COUNT];
    for k in order {
        if seen[k.index()] {
            return StageKind::DEFAULT_ORDER;
        }
        seen[k.index()] = true;
    }
    if order[STAGE_COUNT - 1] != StageKind::Limiter {
        // Keep the user's relative order but force the limiter last.
        let mut fixed = [StageKind::Limiter; STAGE_COUNT];
        let mut i = 0;
        for k in order {
            if k != StageKind::Limiter {
                fixed[i] = k;
                i += 1;
            }
        }
        return fixed;
    }
    order
}

// ---------------------------------------------------------------------------
// Presets

serde_enum! {
    /// Starting-point presets for voice processing. They are points of
    /// departure, not universal settings.
    pub enum VoicePreset {
        Natural,
        NoisyRoom,
        Call,
        Stream,
        #[default]
        Raw,
    }
}

impl VoicePreset {
    pub const ALL: [VoicePreset; 5] = [VoicePreset::Natural, VoicePreset::NoisyRoom, VoicePreset::Call, VoicePreset::Stream, VoicePreset::Raw];

    pub fn key(self) -> &'static str {
        match self {
            VoicePreset::Natural => "preset_natural",
            VoicePreset::NoisyRoom => "preset_noisy_room",
            VoicePreset::Call => "preset_call",
            VoicePreset::Stream => "preset_stream",
            VoicePreset::Raw => "preset_raw",
        }
    }

    /// Builds the chain for this preset.
    pub fn chain(self) -> ChainParams {
        let mut c = ChainParams {
            bypass: false,
            order: StageKind::DEFAULT_ORDER,
            hpf: HpfParams { enabled: false, ..HpfParams::default() },
            noise_reduction: NoiseReductionParams::default(),
            gate: GateParams::default(),
            expander: ExpanderParams::default(),
            agc: AgcParams::default(),
            eq: EqParams::default(),
            deesser: DeEsserParams::default(),
            compressor: CompressorParams::default(),
            output_gain: OutputGainParams::default(),
            limiter: LimiterParams { enabled: false, ..LimiterParams::default() },
        };
        match self {
            VoicePreset::Raw => {}
            VoicePreset::Natural => {
                c.hpf = HpfParams { enabled: true, frequency_hz: 80.0, slope: HpfSlope::Db12 };
                c.noise_reduction = NoiseReductionParams { enabled: true, strength: 0.35, max_reduction_db: 12.0, ..NoiseReductionParams::default() };
                c.expander = ExpanderParams { enabled: true, threshold_db: -50.0, ratio: 1.8, range_db: -15.0, ..ExpanderParams::default() };
                c.compressor = CompressorParams {
                    enabled: true,
                    threshold_db: -22.0,
                    ratio: 2.5,
                    attack_ms: 15.0,
                    release_ms: 150.0,
                    knee_db: 8.0,
                    makeup_db: 2.0,
                    ..CompressorParams::default()
                };
                c.limiter = LimiterParams { enabled: true, ..LimiterParams::default() };
            }
            VoicePreset::NoisyRoom => {
                c.hpf = HpfParams { enabled: true, frequency_hz: 100.0, slope: HpfSlope::Db18 };
                c.noise_reduction = NoiseReductionParams {
                    enabled: true,
                    mode: NoiseReductionMode::Neural,
                    strength: 0.8,
                    max_reduction_db: 30.0,
                    quality: NrQuality::Balanced,
                };
                c.gate = GateParams {
                    enabled: true,
                    threshold_db: -45.0,
                    hysteresis_db: 6.0,
                    attack_ms: 1.0,
                    hold_ms: 200.0,
                    release_ms: 250.0,
                    range_db: -30.0,
                };
                c.eq.enabled = true;
                c.eq.bands[1].gain_db = -2.0;
                c.eq.bands[3].gain_db = 2.0;
                c.deesser.enabled = true;
                c.compressor = CompressorParams { enabled: true, threshold_db: -24.0, ratio: 3.0, makeup_db: 3.0, ..CompressorParams::default() };
                c.limiter = LimiterParams { enabled: true, ..LimiterParams::default() };
            }
            VoicePreset::Call => {
                c.hpf = HpfParams { enabled: true, frequency_hz: 120.0, slope: HpfSlope::Db24 };
                c.noise_reduction = NoiseReductionParams { enabled: true, strength: 0.6, max_reduction_db: 20.0, ..NoiseReductionParams::default() };
                c.expander = ExpanderParams { enabled: true, threshold_db: -48.0, ratio: 2.0, ..ExpanderParams::default() };
                c.eq.enabled = true;
                c.eq.bands[0] = EqBand::new(EqBandKind::LowShelf, 150.0, -3.0, 0.707);
                c.eq.bands[3] = EqBand::new(EqBandKind::Peak, 2500.0, 3.0, 1.0);
                c.compressor = CompressorParams {
                    enabled: true,
                    threshold_db: -26.0,
                    ratio: 4.0,
                    attack_ms: 5.0,
                    release_ms: 100.0,
                    knee_db: 6.0,
                    makeup_db: 4.0,
                    ..CompressorParams::default()
                };
                c.limiter = LimiterParams { enabled: true, ceiling_db: -2.0, ..LimiterParams::default() };
            }
            VoicePreset::Stream => {
                c.hpf = HpfParams { enabled: true, frequency_hz: 70.0, slope: HpfSlope::Db12 };
                c.noise_reduction = NoiseReductionParams { enabled: true, strength: 0.4, max_reduction_db: 15.0, ..NoiseReductionParams::default() };
                c.expander = ExpanderParams { enabled: true, threshold_db: -50.0, ratio: 2.0, ..ExpanderParams::default() };
                c.eq.enabled = true;
                c.eq.bands[0] = EqBand::new(EqBandKind::LowShelf, 120.0, 2.0, 0.707);
                c.eq.bands[1] = EqBand::new(EqBandKind::Peak, 350.0, -2.0, 1.2);
                c.eq.bands[3] = EqBand::new(EqBandKind::Peak, 4000.0, 2.5, 1.0);
                c.eq.bands[4] = EqBand::new(EqBandKind::HighShelf, 10_000.0, 2.0, 0.707);
                c.deesser = DeEsserParams { enabled: true, threshold_db: -28.0, ..DeEsserParams::default() };
                c.compressor = CompressorParams {
                    enabled: true,
                    threshold_db: -20.0,
                    ratio: 3.5,
                    attack_ms: 8.0,
                    release_ms: 120.0,
                    knee_db: 6.0,
                    makeup_db: 3.0,
                    ..CompressorParams::default()
                };
                c.limiter = LimiterParams { enabled: true, lookahead_ms: 3.0, ..LimiterParams::default() };
            }
        }
        c
    }

    /// Returns the preset whose chain equals `chain`, if any.
    pub fn detect(chain: &ChainParams) -> Option<VoicePreset> {
        Self::ALL.into_iter().find(|p| p.chain() == *chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_and_replaces_nan() {
        let g = GateParams { threshold_db: f32::NAN, attack_ms: 1e9, ..GateParams::default() }.sanitized();
        assert_eq!(g.threshold_db, GateParams::default().threshold_db);
        assert_eq!(g.attack_ms, 50.0);
    }

    #[test]
    fn order_must_be_permutation_with_limiter_last() {
        let mut bad = StageKind::DEFAULT_ORDER;
        bad[1] = StageKind::Hpf;
        assert_eq!(sanitize_order(bad), StageKind::DEFAULT_ORDER);

        let mut moved = StageKind::DEFAULT_ORDER;
        moved.swap(0, 9);
        let fixed = sanitize_order(moved);
        assert_eq!(fixed[9], StageKind::Limiter);
        assert_eq!(fixed[0], StageKind::NoiseReduction);
    }

    #[test]
    fn move_stage_respects_pinned_limiter() {
        let mut c = ChainParams::default();
        assert!(!c.move_stage(StageKind::Limiter, -1));
        assert!(!c.move_stage(StageKind::OutputGain, 1));
        assert!(c.move_stage(StageKind::Eq, -1));
        assert_eq!(c.order[4], StageKind::Eq);
    }

    #[test]
    fn presets_are_valid_and_detectable() {
        for p in VoicePreset::ALL {
            let c = p.chain();
            assert_eq!(c, c.sanitized(), "{p:?} must already be in range");
            assert_eq!(VoicePreset::detect(&c), Some(p));
        }
    }

    #[test]
    fn compressor_transfer_curve() {
        let c = CompressorParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, ..Default::default() };
        assert_eq!(c.transfer_db(-30.0), -30.0);
        assert!((c.transfer_db(-10.0) - (-17.5)).abs() < 1e-4);
        let soft = CompressorParams { knee_db: 10.0, ..c };
        // Continuous at the knee edges.
        assert!((soft.transfer_db(-25.0) - (-25.0)).abs() < 1e-3);
        assert!((soft.transfer_db(-15.0) - c.transfer_db(-15.0)).abs() < 1e-3);
    }

    #[test]
    fn eq_magnitude_of_flat_bands_is_zero() {
        let eq = EqParams { enabled: true, ..Default::default() };
        for f in [50.0, 500.0, 5000.0] {
            assert!(eq.magnitude_db(f, 48_000.0).abs() < 1e-3);
        }
    }
}
