//! Streaming, variable-ratio windowed-sinc resampler.
//!
//! Used both for sample-rate conversion between devices (44.1 ↔ 48 kHz…)
//! and for clock-drift compensation between independent devices, where the
//! ratio is nudged by a fraction of a percent at run time. The polyphase
//! table (Kaiser-windowed sinc, linearly interpolated between phases) is
//! built at construction; processing never allocates.

/// Taps per output sample (even).
const TAPS: usize = 48;
/// Number of fractional phases in the table.
const PHASES: usize = 256;
/// Kaiser window β.
const KAISER_BETA: f64 = 7.5;
/// Cutoff relative to the lower of the two Nyquist frequencies.
const CUTOFF: f64 = 0.90;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        let t2 = term * term;
        sum += t2;
        if t2 < sum * 1e-16 {
            break;
        }
    }
    sum
}

#[derive(Debug, Clone)]
pub struct Resampler {
    channels: usize,
    table: Vec<f32>,
    hist: [Vec<f32>; 2],
    hist_pos: usize,
    frac: f64,
    need: usize,
    ratio: f64,
    nominal_ratio: f64,
    max_deviation: f64,
}

impl Resampler {
    /// Creates a resampler converting `input_rate` to `output_rate`.
    /// `channels` is 1 or 2.
    pub fn new(input_rate: f64, output_rate: f64, channels: usize) -> Self {
        let ratio = input_rate / output_rate;
        // When downsampling, the cutoff must follow the output Nyquist.
        let cutoff = CUTOFF * (1.0 / ratio).min(1.0);
        let half = (TAPS / 2) as f64;
        let i0b = bessel_i0(KAISER_BETA);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let mut sum = 0.0;
            for i in 0..TAPS {
                let t = i as f64 - (half - 1.0) - frac;
                let x = std::f64::consts::PI * cutoff * t;
                let sinc = if t.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                let u = (t / half).clamp(-1.0, 1.0);
                let w = bessel_i0(KAISER_BETA * (1.0 - u * u).max(0.0).sqrt()) / i0b;
                let h = cutoff * sinc * w;
                table[p * TAPS + i] = h as f32;
                sum += h;
            }
            // Normalize each phase to unity DC gain.
            for i in 0..TAPS {
                table[p * TAPS + i] = (table[p * TAPS + i] as f64 / sum) as f32;
            }
        }
        Self {
            channels: channels.clamp(1, 2),
            table,
            hist: [vec![0.0; TAPS * 2], vec![0.0; TAPS * 2]],
            hist_pos: 0,
            frac: 0.0,
            need: 1,
            ratio,
            nominal_ratio: ratio,
            max_deviation: 0.02,
        }
    }

    /// Algorithmic latency in input samples.
    pub const fn latency() -> usize {
        TAPS / 2
    }

    pub fn nominal_ratio(&self) -> f64 {
        self.nominal_ratio
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Adjusts the ratio relative to nominal (e.g. `1.0005` = +0.05 %).
    /// Clamped to ±2 % so the anti-aliasing design stays valid.
    pub fn set_relative_ratio(&mut self, rel: f64) {
        let rel = rel.clamp(1.0 - self.max_deviation, 1.0 + self.max_deviation);
        self.ratio = self.nominal_ratio * rel;
    }

    pub fn reset(&mut self) {
        self.hist[0].fill(0.0);
        self.hist[1].fill(0.0);
        self.hist_pos = 0;
        self.frac = 0.0;
        self.need = 1;
        self.ratio = self.nominal_ratio;
    }

    /// Estimated number of input frames required to produce `out` frames.
    pub fn input_needed(&self, out: usize) -> usize {
        (out as f64 * self.ratio + self.frac).ceil() as usize + 1
    }

    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        let p = self.hist_pos;
        self.hist[0][p] = l;
        self.hist[0][p + TAPS] = l;
        if self.channels == 2 {
            self.hist[1][p] = r;
            self.hist[1][p + TAPS] = r;
        }
        self.hist_pos = (p + 1) % TAPS;
    }

    #[inline]
    fn interpolate(&self, ch: usize) -> f32 {
        let pf = self.frac * PHASES as f64;
        let p0 = (pf as usize).min(PHASES - 1);
        let t = (pf - p0 as f64) as f32;
        let row0 = &self.table[p0 * TAPS..(p0 + 1) * TAPS];
        let row1 = &self.table[(p0 + 1) * TAPS..(p0 + 2) * TAPS];
        let win = &self.hist[ch][self.hist_pos..self.hist_pos + TAPS];
        let mut a0 = 0.0f32;
        let mut a1 = 0.0f32;
        for i in 0..TAPS {
            a0 += row0[i] * win[i];
            a1 += row1[i] * win[i];
        }
        a0 + (a1 - a0) * t
    }

    /// Resamples interleaved stereo (or mono when `channels == 1`, where
    /// `input` is still read as interleaved pairs and only the left value
    /// is used). Stops when either the input is exhausted or the output is
    /// full. Returns `(frames_consumed, frames_produced)`.
    pub fn process_interleaved(&mut self, input: &[f32], out_l: &mut [f32], mut out_r: Option<&mut [f32]>) -> (usize, usize) {
        let in_frames = input.len() / 2;
        let mut ci = 0;
        let mut po = 0;
        let out_len = out_l.len();
        while po < out_len {
            while self.need > 0 {
                if ci >= in_frames {
                    return (ci, po);
                }
                self.push(input[2 * ci], input[2 * ci + 1]);
                ci += 1;
                self.need -= 1;
            }
            out_l[po] = self.interpolate(0);
            if let Some(r) = out_r.as_deref_mut() {
                r[po] = if self.channels == 2 { self.interpolate(1) } else { out_l[po] };
            }
            po += 1;
            self.frac += self.ratio;
            let adv = self.frac.floor();
            self.frac -= adv;
            self.need = adv as usize;
        }
        (ci, po)
    }

    /// Same as [`process_interleaved`](Self::process_interleaved) but writes
    /// interleaved stereo output.
    pub fn process_interleaved_to_interleaved(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        let in_frames = input.len() / 2;
        let out_frames = output.len() / 2;
        let mut ci = 0;
        let mut po = 0;
        while po < out_frames {
            while self.need > 0 {
                if ci >= in_frames {
                    return (ci, po);
                }
                self.push(input[2 * ci], input[2 * ci + 1]);
                ci += 1;
                self.need -= 1;
            }
            let l = self.interpolate(0);
            let r = if self.channels == 2 { self.interpolate(1) } else { l };
            output[2 * po] = l;
            output[2 * po + 1] = r;
            po += 1;
            self.frac += self.ratio;
            let adv = self.frac.floor();
            self.frac -= adv;
            self.need = adv as usize;
        }
        (ci, po)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{signal, util};

    fn interleave(x: &[f32]) -> Vec<f32> {
        x.iter().flat_map(|&s| [s, s]).collect()
    }

    fn convert(input_rate: f64, output_rate: f64, x: &[f32]) -> Vec<f32> {
        let mut rs = Resampler::new(input_rate, output_rate, 2);
        let inter = interleave(x);
        let out_len = (x.len() as f64 / rs.nominal_ratio()) as usize + 64;
        let mut l = vec![0.0; out_len];
        let mut r = vec![0.0; out_len];
        let (c, p) = rs.process_interleaved(&inter, &mut l, Some(&mut r));
        assert!(c <= x.len());
        l.truncate(p);
        l
    }

    #[test]
    fn unity_ratio_is_a_pure_delay_in_the_passband() {
        let a = signal::sine(1000.0, 48_000.0, 0.5, 4096);
        let b = signal::sine(5000.0, 48_000.0, 0.3, 4096);
        let x: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
        let y = convert(48_000.0, 48_000.0, &x);
        let lat = Resampler::latency();
        for i in 200..3000 {
            assert!((y[i + lat] - x[i]).abs() < 2e-3, "i={i} {} vs {}", y[i + lat], x[i]);
        }
    }

    #[test]
    fn converts_44k1_to_48k_preserving_tone() {
        let x = signal::sine(1000.0, 44_100.0, 0.5, 44_100);
        let y = convert(44_100.0, 48_000.0, &x);
        assert!((y.len() as i64 - 48_000).abs() < 40, "{}", y.len());
        let seg = &y[4800..43_200];
        let mag = signal::goertzel_magnitude(seg, 1000.0, 48_000.0);
        assert!((util::gain_to_db(mag / 0.5)).abs() < 0.1, "{mag}");
        // Residual (THD+N) must be small.
        let mut resid = seg.to_vec();
        let ref_sig = signal::sine(1000.0, 48_000.0, 1.0, resid.len());
        // Estimate phase by correlating with sin/cos.
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (i, v) in seg.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0;
            s += *v as f64 * ph.sin();
            c += *v as f64 * ph.cos();
        }
        let n = seg.len() as f64;
        let (a, b) = (2.0 * s / n, 2.0 * c / n);
        for (i, v) in resid.iter_mut().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0;
            *v -= (a * ph.sin() + b * ph.cos()) as f32;
        }
        let _ = ref_sig;
        let snr = util::gain_to_db(util::rms(seg) / util::rms(&resid).max(1e-9));
        assert!(snr > 70.0, "SNR {snr} dB");
    }

    #[test]
    fn downsampling_rejects_aliases() {
        // 30 kHz tone at 96 kHz must not alias into the 48 kHz output.
        let x = signal::sine(30_000.0, 96_000.0, 0.5, 96_000);
        let y = convert(96_000.0, 48_000.0, &x);
        let alias = signal::goertzel_magnitude(&y[2000..40_000], 18_000.0, 48_000.0);
        assert!(util::gain_to_db(alias / 0.5) < -50.0, "alias {}", util::gain_to_db(alias / 0.5));
    }

    #[test]
    fn drift_adjustment_changes_consumption_rate() {
        let mut rs = Resampler::new(48_000.0, 48_000.0, 2);
        rs.set_relative_ratio(1.001);
        let input = vec![0.0f32; 2 * 20_000];
        let mut out = vec![0.0f32; 10_000];
        let (c, p) = rs.process_interleaved(&input, &mut out, None);
        assert_eq!(p, 10_000);
        assert!((c as i64 - 10_010).abs() <= 2, "consumed {c}");
    }

    #[test]
    fn stops_when_input_runs_out() {
        let mut rs = Resampler::new(48_000.0, 44_100.0, 2);
        let input = vec![0.1f32; 2 * 100];
        let mut out = vec![0.0f32; 1000];
        let (c, p) = rs.process_interleaved(&input, &mut out, None);
        assert_eq!(c, 100);
        assert!(p > 80 && p < 100);
    }
}
