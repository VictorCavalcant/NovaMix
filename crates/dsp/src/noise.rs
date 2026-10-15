//! Noise reduction.
//!
//! * [`SpectralDenoiser`]: STFT (sqrt-Hann, 75% overlap) with
//!   minimum-statistics noise tracking and a decision-directed Wiener gain.
//!   Gains are smoothed across frequency and time to keep "musical noise"
//!   low. All buffers and FFT plans for the three frame sizes are allocated
//!   up front, so switching quality never allocates on the audio thread.
//! * `NeuralDenoiser` (feature `rnnoise`): RNNoise via `nnnoiseless`,
//!   running locally, 48 kHz only.
//!
//! [`NoiseReducer`] wraps both and exposes a single processor.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::Processor;
use crate::params::{NoiseReductionMode, NoiseReductionParams, NrQuality};
use crate::util::db_to_gain;

/// Number of minimum-statistics sub-windows.
const MS_SUBWINDOWS: usize = 8;
/// Duration of one sub-window in seconds (total window ≈ 1.6 s).
const MS_SUBWINDOW_SECS: f32 = 0.2;
/// Bias compensation for the minimum of a smoothed periodogram.
const MS_BIAS: f32 = 1.6;

struct StftCore {
    n: usize,
    hop: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    norm: f32,
    // Per channel streaming buffers.
    frame: [Vec<f32>; 2],
    in_fifo: [Vec<f32>; 2],
    out_fifo: [Vec<f32>; 2],
    accum: [Vec<f32>; 2],
    pos: usize,
    // Work buffers.
    time: Vec<f32>,
    spec: [Vec<Complex32>; 2],
    scratch_fwd: Vec<Complex32>,
    scratch_inv: Vec<Complex32>,
    // Spectral state.
    psd_smooth: Vec<f32>,
    cur_min: Vec<f32>,
    sub_min: Vec<f32>, // MS_SUBWINDOWS * bins
    sub_index: usize,
    sub_frames: usize,
    frames_per_sub: usize,
    noise: Vec<f32>,
    prev_clean: Vec<f32>,
    gain: Vec<f32>,
    gain_tmp: Vec<f32>,
    psd_alpha: f32,
    gain_release: f32,
    frames_seen: usize,
}

impl StftCore {
    fn new(planner: &mut RealFftPlanner<f32>, n: usize, sample_rate: f32) -> Self {
        let hop = n / 4;
        let bins = n / 2 + 1;
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        // Periodic sqrt-Hann for analysis and synthesis.
        let window: Vec<f32> = (0..n)
            .map(|i| {
                let h = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
                h.sqrt()
            })
            .collect();
        // Overlap-add normalization: sum of w² over overlapping frames, and
        // 1/n for the unnormalized inverse FFT.
        let mut ola = 0.0;
        for k in 0..(n / hop) {
            let w = window[k * hop];
            ola += w * w;
        }
        let norm = 1.0 / (ola * n as f32);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        let hop_secs = hop as f32 / sample_rate;
        let frames_per_sub = ((MS_SUBWINDOW_SECS / hop_secs).round() as usize).max(1);
        Self {
            n,
            hop,
            fft,
            ifft,
            window,
            norm,
            frame: [vec![0.0; n], vec![0.0; n]],
            in_fifo: [vec![0.0; hop], vec![0.0; hop]],
            out_fifo: [vec![0.0; hop], vec![0.0; hop]],
            accum: [vec![0.0; n], vec![0.0; n]],
            pos: 0,
            time: vec![0.0; n],
            spec: [vec![Complex32::new(0.0, 0.0); bins], vec![Complex32::new(0.0, 0.0); bins]],
            scratch_fwd,
            scratch_inv,
            psd_smooth: vec![0.0; bins],
            cur_min: vec![f32::MAX; bins],
            sub_min: vec![f32::MAX; bins * MS_SUBWINDOWS],
            sub_index: 0,
            sub_frames: 0,
            frames_per_sub,
            noise: vec![0.0; bins],
            prev_clean: vec![0.0; bins],
            gain: vec![1.0; bins],
            gain_tmp: vec![1.0; bins],
            psd_alpha: (-hop_secs / 0.03).exp(),
            gain_release: (-hop_secs / 0.06).exp(),
            frames_seen: 0,
        }
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.frame[ch].fill(0.0);
            self.in_fifo[ch].fill(0.0);
            self.out_fifo[ch].fill(0.0);
            self.accum[ch].fill(0.0);
        }
        self.pos = 0;
        self.psd_smooth.fill(0.0);
        self.cur_min.fill(f32::MAX);
        self.sub_min.fill(f32::MAX);
        self.sub_index = 0;
        self.sub_frames = 0;
        self.noise.fill(0.0);
        self.prev_clean.fill(0.0);
        self.gain.fill(1.0);
        self.frames_seen = 0;
    }

    fn latency(&self) -> usize {
        self.n
    }

    /// Updates the noise estimate and computes the suppression gains for
    /// the current spectra (power = mean of the active channels).
    fn compute_gains(&mut self, channels: usize, strength: f32, floor: f32) {
        let bins = self.psd_smooth.len();
        let a = self.psd_alpha;
        let over = 0.6 + 1.6 * strength; // over-subtraction factor
        const DD_ALPHA: f32 = 0.96;
        self.frames_seen += 1;

        for k in 0..bins {
            let mut p = self.spec[0][k].norm_sqr();
            if channels == 2 {
                p = 0.5 * (p + self.spec[1][k].norm_sqr());
            }
            let s = if self.frames_seen == 1 { p } else { a * self.psd_smooth[k] + (1.0 - a) * p };
            self.psd_smooth[k] = s;
            if s < self.cur_min[k] {
                self.cur_min[k] = s;
            }
        }

        self.sub_frames += 1;
        if self.sub_frames >= self.frames_per_sub {
            let base = self.sub_index * bins;
            self.sub_min[base..base + bins].copy_from_slice(&self.cur_min);
            self.sub_index = (self.sub_index + 1) % MS_SUBWINDOWS;
            self.sub_frames = 0;
            self.cur_min.copy_from_slice(&self.psd_smooth);
        }

        // Until a couple of sub-windows have been observed the minimum is not
        // trustworthy (it may be speech); fade the suppression in.
        let warm = (self.frames_seen as f32 / (2 * self.frames_per_sub) as f32).min(1.0);
        for k in 0..bins {
            let mut m = self.cur_min[k];
            for w in 0..MS_SUBWINDOWS {
                m = m.min(self.sub_min[w * bins + k]);
            }
            let noise = (m * MS_BIAS * warm).max(1e-12);
            self.noise[k] = noise;

            let mut p = self.spec[0][k].norm_sqr();
            if channels == 2 {
                p = 0.5 * (p + self.spec[1][k].norm_sqr());
            }
            let n_eff = noise * over;
            let post = (p / n_eff).min(1.0e4);
            let prio = DD_ALPHA * (self.prev_clean[k] / n_eff) + (1.0 - DD_ALPHA) * (post - 1.0).max(0.0);
            let g = (prio / (1.0 + prio)).max(floor);
            self.gain_tmp[k] = g;
        }

        // Smooth across frequency (3-tap) to reduce isolated "musical" bins.
        for k in 0..bins {
            let lo = if k > 0 { self.gain_tmp[k - 1] } else { self.gain_tmp[k] };
            let hi = if k + 1 < bins { self.gain_tmp[k + 1] } else { self.gain_tmp[k] };
            let g = 0.25 * lo + 0.5 * self.gain_tmp[k] + 0.25 * hi;
            // Fast attack (speech onsets), slower release.
            let prev = self.gain[k];
            let g = if g > prev { g } else { g + (prev - g) * self.gain_release };
            self.gain[k] = g.clamp(floor, 1.0);

            let mut p = self.spec[0][k].norm_sqr();
            if channels == 2 {
                p = 0.5 * (p + self.spec[1][k].norm_sqr());
            }
            self.prev_clean[k] = self.gain[k] * self.gain[k] * p;
        }
    }

    fn process_frame(&mut self, channels: usize, strength: f32, floor: f32) {
        let n = self.n;
        let hop = self.hop;
        for ch in 0..channels {
            // Slide the analysis frame and append the new hop.
            self.frame[ch].copy_within(hop.., 0);
            self.frame[ch][n - hop..].copy_from_slice(&self.in_fifo[ch]);
            for i in 0..n {
                self.time[i] = self.frame[ch][i] * self.window[i];
            }
            // The FFT cannot fail with correctly sized buffers.
            let _ = self.fft.process_with_scratch(&mut self.time, &mut self.spec[ch], &mut self.scratch_fwd);
        }

        self.compute_gains(channels, strength, floor);

        for ch in 0..channels {
            let bins = self.gain.len();
            for k in 0..bins {
                self.spec[ch][k] *= self.gain[k];
            }
            // DC and Nyquist bins must be purely real for the inverse FFT.
            self.spec[ch][0].im = 0.0;
            self.spec[ch][bins - 1].im = 0.0;
            let _ = self.ifft.process_with_scratch(&mut self.spec[ch], &mut self.time, &mut self.scratch_inv);
            let acc = &mut self.accum[ch];
            let norm = self.norm;
            for ((a, t), w) in acc.iter_mut().zip(&self.time).zip(&self.window) {
                *a += t * w * norm;
            }
            self.out_fifo[ch].copy_from_slice(&acc[..hop]);
            acc.copy_within(hop.., 0);
            acc[n - hop..].fill(0.0);
        }
    }

    fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>, strength: f32, floor: f32) {
        let channels = if right.is_some() { 2 } else { 1 };
        for i in 0..left.len() {
            let p = self.pos;
            self.in_fifo[0][p] = left[i];
            left[i] = self.out_fifo[0][p];
            if let Some(r) = right.as_deref_mut() {
                self.in_fifo[1][p] = r[i];
                r[i] = self.out_fifo[1][p];
            }
            self.pos += 1;
            if self.pos == self.hop {
                self.pos = 0;
                self.process_frame(channels, strength, floor);
            }
        }
    }

    /// Average suppression currently applied, in dB (for the UI).
    fn average_gain_db(&self) -> f32 {
        let sum: f32 = self.gain.iter().sum();
        crate::util::gain_to_db(sum / self.gain.len() as f32)
    }
}

/// STFT spectral noise suppressor with pre-allocated frame sizes.
pub struct SpectralDenoiser {
    cores: [StftCore; 3],
    active: usize,
    strength: f32,
    floor: f32,
}

impl SpectralDenoiser {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            cores: [
                StftCore::new(&mut planner, NrQuality::LowLatency.fft_size(), sample_rate),
                StftCore::new(&mut planner, NrQuality::Balanced.fft_size(), sample_rate),
                StftCore::new(&mut planner, NrQuality::HighQuality.fft_size(), sample_rate),
            ],
            active: 1,
            strength: 0.5,
            floor: db_to_gain(-18.0),
        }
    }

    fn quality_index(q: NrQuality) -> usize {
        match q {
            NrQuality::LowLatency => 0,
            NrQuality::Balanced => 1,
            NrQuality::HighQuality => 2,
        }
    }

    pub fn set_params(&mut self, p: &NoiseReductionParams) {
        let idx = Self::quality_index(p.quality);
        if idx != self.active {
            self.active = idx;
            self.cores[idx].reset();
        }
        self.strength = p.strength;
        self.floor = db_to_gain(-p.max_reduction_db);
    }

    pub fn latency_for(&self, quality: NrQuality) -> usize {
        self.cores[Self::quality_index(quality)].latency()
    }

    pub fn reduction_db(&self) -> f32 {
        self.cores[self.active].average_gain_db()
    }
}

impl Processor for SpectralDenoiser {
    fn reset(&mut self) {
        self.cores[self.active].reset();
    }

    fn latency(&self) -> usize {
        self.cores[self.active].latency()
    }

    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        let (s, f) = (self.strength, self.floor);
        self.cores[self.active].process(left, right, s, f);
    }
}

#[cfg(feature = "rnnoise")]
mod neural {
    use nnnoiseless::DenoiseState;

    pub const FRAME: usize = DenoiseState::FRAME_SIZE;

    /// Total latency: one frame of FIFO buffering plus RNNoise's own
    /// one-frame overlap-add delay.
    pub const LATENCY: usize = 2 * FRAME;

    /// RNNoise wrapper with FIFO framing. Stereo inputs are processed per
    /// channel with two independent states.
    pub struct NeuralDenoiser {
        states: [Box<DenoiseState<'static>>; 2],
        in_fifo: [Vec<f32>; 2],
        prev_in: [Vec<f32>; 2],
        out_fifo: [Vec<f32>; 2],
        dry_fifo: [Vec<f32>; 2],
        scaled: Vec<f32>,
        frame_out: Vec<f32>,
        pos: usize,
        pub vad: f32,
        mix: f32,
    }

    impl NeuralDenoiser {
        pub fn new() -> Self {
            Self {
                states: [DenoiseState::new(), DenoiseState::new()],
                in_fifo: [vec![0.0; FRAME], vec![0.0; FRAME]],
                prev_in: [vec![0.0; FRAME], vec![0.0; FRAME]],
                out_fifo: [vec![0.0; FRAME], vec![0.0; FRAME]],
                dry_fifo: [vec![0.0; FRAME], vec![0.0; FRAME]],
                scaled: vec![0.0; FRAME],
                frame_out: vec![0.0; FRAME],
                pos: 0,
                vad: 0.0,
                mix: 1.0,
            }
        }

        /// Clears the streaming buffers. The network state is kept (it
        /// re-converges within a few frames and re-creating it would
        /// allocate on the audio thread).
        pub fn reset(&mut self) {
            for ch in 0..2 {
                self.in_fifo[ch].fill(0.0);
                self.prev_in[ch].fill(0.0);
                self.out_fifo[ch].fill(0.0);
                self.dry_fifo[ch].fill(0.0);
            }
            self.pos = 0;
            self.vad = 0.0;
        }

        /// `mix` blends the denoised signal with the (aligned) original, used
        /// to implement the strength / maximum reduction controls.
        pub fn set_mix(&mut self, mix: f32) {
            self.mix = mix.clamp(0.0, 1.0);
        }

        pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
            let channels = if right.is_some() { 2 } else { 1 };
            let mix = self.mix;
            for i in 0..left.len() {
                let p = self.pos;
                let out_l = self.out_fifo[0][p] * mix + self.dry_fifo[0][p] * (1.0 - mix);
                self.in_fifo[0][p] = left[i];
                left[i] = out_l;
                if let Some(r) = right.as_deref_mut() {
                    let out_r = self.out_fifo[1][p] * mix + self.dry_fifo[1][p] * (1.0 - mix);
                    self.in_fifo[1][p] = r[i];
                    r[i] = out_r;
                }
                self.pos += 1;
                if self.pos == FRAME {
                    self.pos = 0;
                    for ch in 0..channels {
                        for (d, s) in self.scaled.iter_mut().zip(self.in_fifo[ch].iter()) {
                            *d = s * 32_767.0;
                        }
                        let vad = self.states[ch].process_frame(&mut self.frame_out, &self.scaled);
                        if ch == 0 {
                            self.vad = vad;
                        }
                        for (o, s) in self.out_fifo[ch].iter_mut().zip(self.frame_out.iter()) {
                            *o = s / 32_767.0;
                        }
                        // The network output corresponds to the previous
                        // input frame; keep that frame as the dry reference.
                        self.dry_fifo[ch].copy_from_slice(&self.prev_in[ch]);
                        self.prev_in[ch].copy_from_slice(&self.in_fifo[ch]);
                    }
                }
            }
        }
    }
}

/// Unified noise reduction stage (spectral or neural).
pub struct NoiseReducer {
    sample_rate: f32,
    params: NoiseReductionParams,
    spectral: SpectralDenoiser,
    #[cfg(feature = "rnnoise")]
    neural: Option<neural::NeuralDenoiser>,
}

impl NoiseReducer {
    pub fn new(sample_rate: f32, params: NoiseReductionParams) -> Self {
        let mut n = Self {
            sample_rate,
            params,
            spectral: SpectralDenoiser::new(sample_rate),
            #[cfg(feature = "rnnoise")]
            neural: if Self::neural_supported_at(sample_rate) { Some(neural::NeuralDenoiser::new()) } else { None },
        };
        n.set_params(params);
        n
    }

    /// Whether the neural mode was compiled in.
    pub const fn neural_compiled() -> bool {
        cfg!(feature = "rnnoise")
    }

    /// Whether the neural mode can run at the given sample rate.
    pub fn neural_supported_at(sample_rate: f32) -> bool {
        Self::neural_compiled() && (sample_rate - 48_000.0).abs() < 1.0
    }

    /// Mode that will actually run (neural falls back to spectral when
    /// unavailable).
    pub fn effective_mode(&self) -> NoiseReductionMode {
        match self.params.mode {
            NoiseReductionMode::Neural if Self::neural_supported_at(self.sample_rate) => NoiseReductionMode::Neural,
            _ => NoiseReductionMode::Spectral,
        }
    }

    pub fn set_params(&mut self, params: NoiseReductionParams) {
        self.params = params.sanitized();
        self.spectral.set_params(&self.params);
        #[cfg(feature = "rnnoise")]
        if let Some(n) = self.neural.as_mut() {
            // Strength and maximum reduction map to a wet/dry blend: at full
            // strength the network output is used as-is, the maximum
            // reduction caps how much of the original may be removed.
            let floor = db_to_gain(-self.params.max_reduction_db);
            let mix = (self.params.strength * (1.0 - floor)).clamp(0.0, 1.0);
            n.set_mix(mix);
        }
    }

    /// Latency for a parameter set (used by the chain to pre-compute
    /// compensation before switching).
    pub fn latency_for(&self, params: &NoiseReductionParams) -> usize {
        let p = params.sanitized();
        match p.mode {
            #[cfg(feature = "rnnoise")]
            NoiseReductionMode::Neural if self.neural.is_some() => neural::LATENCY,
            _ => self.spectral.latency_for(p.quality),
        }
    }

    /// Voice activity probability from the neural model (0 when spectral).
    pub fn vad(&self) -> f32 {
        #[cfg(feature = "rnnoise")]
        if self.effective_mode() == NoiseReductionMode::Neural {
            return self.neural.as_ref().map_or(0.0, |n| n.vad);
        }
        0.0
    }

    /// Approximate average attenuation (spectral mode only).
    pub fn reduction_db(&self) -> f32 {
        match self.effective_mode() {
            NoiseReductionMode::Spectral => self.spectral.reduction_db(),
            NoiseReductionMode::Neural => 0.0,
        }
    }
}

impl Processor for NoiseReducer {
    fn reset(&mut self) {
        self.spectral.reset();
        #[cfg(feature = "rnnoise")]
        if let Some(n) = self.neural.as_mut() {
            n.reset();
        }
    }

    fn latency(&self) -> usize {
        self.latency_for(&self.params)
    }

    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        match self.effective_mode() {
            NoiseReductionMode::Spectral => self.spectral.process(left, right),
            NoiseReductionMode::Neural =>
            {
                #[cfg(feature = "rnnoise")]
                if let Some(n) = self.neural.as_mut() {
                    n.process(left, right);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, util};

    const SR: f32 = 48_000.0;

    #[test]
    fn spectral_is_transparent_for_reconstruction_with_unity_gain() {
        // With a floor of 0 dB the gains are forced to 1: the STFT must then
        // reconstruct the input exactly, delayed by its latency.
        let mut d = SpectralDenoiser::new(SR);
        d.set_params(&NoiseReductionParams { max_reduction_db: 0.0, ..NoiseReductionParams::default() });
        let lat = d.latency();
        let x = signal::sine(440.0, SR, 0.5, 8192);
        let mut y = x.clone();
        d.process(&mut y, None);
        for i in (lat + 1024)..x.len() {
            assert!((y[i] - x[i - lat]).abs() < 1e-4, "i={i} y={} x={}", y[i], x[i - lat]);
        }
    }

    #[test]
    fn spectral_reduces_stationary_noise() {
        let mut d = SpectralDenoiser::new(SR);
        d.set_params(&NoiseReductionParams { enabled: true, strength: 0.8, max_reduction_db: 30.0, ..Default::default() });
        let mut noise = signal::white_noise(SR as usize * 4, 0.05, 3);
        let before = util::rms(&noise[SR as usize * 3..]);
        d.process(&mut noise, None);
        let after = util::rms(&noise[SR as usize * 3..]);
        let red = util::gain_to_db(after / before);
        assert!(red < -10.0, "noise reduced by {red} dB");
    }

    #[test]
    fn spectral_preserves_tone_over_noise() {
        let mut d = SpectralDenoiser::new(SR);
        d.set_params(&NoiseReductionParams { enabled: true, strength: 0.5, max_reduction_db: 24.0, ..Default::default() });
        // Speech-like: 250 ms tone bursts separated by 250 ms of noise only.
        // (A perfectly stationary tone is indistinguishable from hum and is
        // expected to be suppressed by a stationary-noise reducer.)
        let n = SR as usize * 4;
        let burst = (SR * 0.25) as usize;
        let mut tone = signal::sine(1000.0, SR, 0.3, n);
        for (i, s) in tone.iter_mut().enumerate() {
            if (i / burst) % 2 == 1 {
                *s = 0.0;
            }
        }
        let noise = signal::white_noise(n, 0.01, 11);
        let mut x: Vec<f32> = tone.iter().zip(&noise).map(|(a, b)| a + b).collect();
        d.process(&mut x, None);
        let lat = d.latency();
        // Compare the 1 kHz component of a late burst (skip its onset).
        let start = 12 * burst + burst / 4;
        let seg = &x[start + lat..start + lat + burst / 2];
        let ref_seg = &tone[start..start + burst / 2];
        let g_out = signal::goertzel_magnitude(seg, 1000.0, SR);
        let g_ref = signal::goertzel_magnitude(ref_seg, 1000.0, SR);
        let diff = util::gain_to_db(g_out / g_ref);
        assert!(diff.abs() < 1.5, "tone changed by {diff} dB");
    }

    #[test]
    fn quality_switch_changes_latency() {
        let d = SpectralDenoiser::new(SR);
        assert_eq!(d.latency_for(NrQuality::LowLatency), 256);
        assert_eq!(d.latency_for(NrQuality::Balanced), 512);
        assert_eq!(d.latency_for(NrQuality::HighQuality), 1024);
    }

    #[cfg(feature = "rnnoise")]
    #[test]
    fn neural_runs_and_reduces_noise() {
        let mut nr = NoiseReducer::new(
            SR,
            NoiseReductionParams { enabled: true, mode: NoiseReductionMode::Neural, strength: 1.0, max_reduction_db: 40.0, ..Default::default() },
        );
        assert_eq!(nr.effective_mode(), NoiseReductionMode::Neural);
        let mut noise = signal::white_noise(SR as usize * 3, 0.05, 5);
        let before = util::rms(&noise[SR as usize * 2..]);
        nr.process(&mut noise, None);
        let after = util::rms(&noise[SR as usize * 2..]);
        assert!(util::gain_to_db(after / before) < -6.0);
    }

    #[test]
    fn neural_falls_back_when_rate_unsupported() {
        let nr = NoiseReducer::new(44_100.0, NoiseReductionParams { enabled: true, mode: NoiseReductionMode::Neural, ..Default::default() });
        assert_eq!(nr.effective_mode(), NoiseReductionMode::Spectral);
    }
}
