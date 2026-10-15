//! Lock-free bridges between independently clocked audio threads.
//!
//! Every device runs on its own clock. A bridge is a single-producer /
//! single-consumer ring of interleaved stereo frames plus, on the consumer
//! side, a variable-ratio resampler driven by a PI controller that keeps the
//! ring fill level at its target. This absorbs both sample-rate differences
//! (44.1 vs 48 kHz) and slow clock drift (±100 ppm typical) without
//! dropouts and without unbounded latency growth.
//!
//! Real-time rules: no allocation, no locks, no syscalls in `push`/`read`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use novamix_diagnostics::{StreamState, StreamStats};
use novamix_dsp::Resampler;

/// Parameters of a bridge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BridgeConfig {
    /// Sample rate of the producer side.
    pub source_rate: f64,
    /// Sample rate of the consumer side.
    pub sink_rate: f64,
    /// Fill level (in source frames) the controller aims for.
    pub target_frames: usize,
    /// Ring capacity in source frames.
    pub capacity_frames: usize,
    /// Starvation means "nothing is playing" (loopback capture), not an
    /// error: it is reported as idle instead of underrun.
    pub idle_is_silence: bool,
}

impl BridgeConfig {
    /// Derives a sensible target/capacity from the block sizes of both
    /// sides plus a safety margin.
    pub fn for_blocks(source_rate: f64, sink_rate: f64, source_block: usize, sink_block: usize, margin_ms: f64) -> Self {
        let sink_in_source = (sink_block as f64 * source_rate / sink_rate).ceil() as usize;
        let margin = (margin_ms * 0.001 * source_rate) as usize;
        let target = source_block.max(sink_in_source) + source_block.min(sink_in_source) / 2 + margin;
        let target = target.max(64);
        Self { source_rate, sink_rate, target_frames: target, capacity_frames: target * 4 + 8192, idle_is_silence: false }
    }
}

/// Producer half.
pub struct BridgeWriter {
    prod: rtrb::Producer<f32>,
    stats: Arc<StreamStats>,
}

/// Outcome of a [`BridgeReader::read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    /// The whole block was real audio.
    Full,
    /// Waiting for the buffer to fill (silence was produced).
    Priming,
    /// Ran dry in the middle of the block (rest is silence).
    Underrun,
    /// Starved, but the source is a loopback with nothing playing.
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Priming,
    Running,
}

/// Consumer half with drift-compensating resampler.
pub struct BridgeReader {
    cons: rtrb::Consumer<f32>,
    rs: Resampler,
    stats: Arc<StreamStats>,
    cfg: BridgeConfig,
    phase: Phase,
    fill_avg: f64,
    integral: f64,
    connected: Arc<AtomicBool>,
    primed_once: bool,
    /// Sink frames left in the start-up grace period, during which a dry
    /// buffer re-primes silently (device start-up jitter is not an error).
    warmup_frames: usize,
}

/// PI controller gains (relative ratio per unit relative fill error).
const KP: f64 = 0.002;
const KI: f64 = 0.000_4;
const MAX_CORRECTION: f64 = 0.005;
const FILL_SMOOTHING: f64 = 0.02;

/// Creates a bridge. `connected` is shared with the device error callback;
/// when it goes false the reader produces silence without counting
/// underruns.
pub fn bridge(cfg: BridgeConfig, stats: Arc<StreamStats>, connected: Arc<AtomicBool>) -> (BridgeWriter, BridgeReader) {
    let cap = cfg.capacity_frames.max(cfg.target_frames * 2 + 64) * 2;
    let (prod, cons) = rtrb::RingBuffer::new(cap);
    stats.target_frames.store(cfg.target_frames as u32, Ordering::Relaxed);
    stats.set_state(StreamState::Starting);
    (
        BridgeWriter { prod, stats: stats.clone() },
        BridgeReader {
            cons,
            rs: Resampler::new(cfg.source_rate, cfg.sink_rate, 2),
            stats,
            cfg,
            phase: Phase::Priming,
            fill_avg: cfg.target_frames as f64,
            integral: 0.0,
            connected,
            primed_once: false,
            warmup_frames: (cfg.sink_rate * 0.5) as usize,
        },
    )
}

impl BridgeWriter {
    /// Free space in frames.
    pub fn free_frames(&self) -> usize {
        self.prod.slots() / 2
    }

    /// Pushes interleaved stereo frames. Frames that do not fit are dropped
    /// and counted as an overrun. Returns the number of frames written.
    pub fn push_interleaved(&mut self, frames: &[f32]) -> usize {
        let want = frames.len() / 2;
        let n = want.min(self.free_frames());
        if n > 0
            && let Ok(mut chunk) = self.prod.write_chunk(n * 2)
        {
            let (a, b) = chunk.as_mut_slices();
            let split = a.len();
            a.copy_from_slice(&frames[..split]);
            b.copy_from_slice(&frames[split..n * 2]);
            chunk.commit_all();
        }
        if n < want {
            self.stats.add_overrun();
        }
        n
    }

    /// Pushes planar stereo.
    pub fn push_planar(&mut self, left: &[f32], right: &[f32]) -> usize {
        let want = left.len().min(right.len());
        let n = want.min(self.free_frames());
        if n > 0
            && let Ok(mut chunk) = self.prod.write_chunk(n * 2)
        {
            let (a, b) = chunk.as_mut_slices();
            let pairs = a.as_chunks_mut::<2>().0.iter_mut().chain(b.as_chunks_mut::<2>().0.iter_mut());
            for (i, pair) in pairs.enumerate() {
                pair[0] = left[i];
                pair[1] = right[i];
            }
            chunk.commit_all();
        }
        if n < want {
            self.stats.add_overrun();
        }
        n
    }

    pub fn stats(&self) -> &Arc<StreamStats> {
        &self.stats
    }
}

impl BridgeReader {
    pub fn stats(&self) -> &Arc<StreamStats> {
        &self.stats
    }

    pub fn config(&self) -> &BridgeConfig {
        &self.cfg
    }

    /// Frames currently buffered.
    pub fn fill_frames(&self) -> usize {
        self.cons.slots() / 2
    }

    /// Latency contributed by this bridge (target fill + resampler), in
    /// sink frames.
    pub fn latency_frames(&self) -> usize {
        ((self.cfg.target_frames + Resampler::latency()) as f64 * self.cfg.sink_rate / self.cfg.source_rate) as usize
    }

    /// Discards everything buffered and re-primes.
    pub fn flush(&mut self) {
        let n = self.cons.slots();
        if let Ok(c) = self.cons.read_chunk(n) {
            c.commit_all();
        }
        self.phase = Phase::Priming;
        self.integral = 0.0;
        self.fill_avg = self.cfg.target_frames as f64;
        self.rs.reset();
        self.warmup_frames = (self.cfg.sink_rate * 0.5) as usize;
    }

    /// Fills `out_l`/`out_r` at the sink rate.
    pub fn read(&mut self, out_l: &mut [f32], out_r: &mut [f32]) -> ReadStatus {
        let n = out_l.len().min(out_r.len());
        let target = self.cfg.target_frames;
        let mut avail = self.cons.slots() / 2;
        self.stats.fill_frames.store(avail as u32, Ordering::Relaxed);

        if self.phase == Phase::Priming {
            if avail >= target {
                // Start exactly at the target: priming overshoot is just
                // start-up latency, drop it before any audio is heard.
                if avail > target {
                    if let Ok(c) = self.cons.read_chunk((avail - target) * 2) {
                        c.commit_all();
                    }
                    avail = target;
                }
                self.phase = Phase::Running;
                self.fill_avg = avail as f64;
                self.integral = 0.0;
                self.primed_once = true;
                self.stats.set_state(StreamState::Running);
            } else {
                out_l[..n].fill(0.0);
                out_r[..n].fill(0.0);
                return ReadStatus::Priming;
            }
        }

        // Re-sync when far too much is buffered (e.g. after a stall), to
        // keep latency bounded.
        if avail > target * 3 + 1024 {
            let drop = avail - target;
            if let Ok(c) = self.cons.read_chunk(drop * 2) {
                c.commit_all();
            }
            avail -= drop;
            self.fill_avg = avail as f64;
            self.integral = 0.0;
            self.stats.add_overrun();
        }

        // Drift control.
        self.fill_avg += FILL_SMOOTHING * (avail as f64 - self.fill_avg);
        let err = (self.fill_avg - target as f64) / target as f64;
        let dt = n as f64 / self.cfg.sink_rate;
        self.integral = (self.integral + err * dt).clamp(-MAX_CORRECTION / KI, MAX_CORRECTION / KI);
        let correction = (KP * err + KI * self.integral).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        self.rs.set_relative_ratio(1.0 + correction);
        self.stats.correction_ppm.store((correction * 1e6) as i32, Ordering::Relaxed);

        let mut produced = 0;
        while produced < n && avail > 0 {
            let want = self.rs.input_needed(n - produced).min(avail);
            let Ok(chunk) = self.cons.read_chunk(want * 2) else { break };
            let (a, b) = chunk.as_slices();
            let (c1, p1) = self.rs.process_interleaved(a, &mut out_l[produced..n], Some(&mut out_r[produced..n]));
            produced += p1;
            let mut consumed = c1;
            if c1 * 2 == a.len() && produced < n && !b.is_empty() {
                let (c2, p2) = self.rs.process_interleaved(b, &mut out_l[produced..n], Some(&mut out_r[produced..n]));
                produced += p2;
                consumed += c2;
            }
            chunk.commit(consumed * 2);
            avail -= consumed;
            if consumed == 0 && p1 == 0 {
                break;
            }
        }

        let in_warmup = self.warmup_frames > 0;
        self.warmup_frames = self.warmup_frames.saturating_sub(n);
        if produced < n {
            out_l[produced..n].fill(0.0);
            out_r[produced..n].fill(0.0);
            self.phase = Phase::Priming;
            self.integral = 0.0;
            if in_warmup {
                self.stats.set_state(StreamState::Priming);
                return ReadStatus::Priming;
            }
            let connected = self.connected.load(Ordering::Relaxed);
            if self.cfg.idle_is_silence || !connected {
                self.stats.set_state(if connected { StreamState::Idle } else { StreamState::Failed });
                return ReadStatus::Idle;
            }
            self.stats.add_underrun();
            self.stats.set_state(StreamState::Priming);
            return ReadStatus::Underrun;
        }
        ReadStatus::Full
    }
}

// ---------------------------------------------------------------------------
// Device-side adapters

/// How device channels are folded into the stereo bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMap {
    /// First two channels, or the only channel duplicated.
    #[default]
    Auto,
    Mono(u16),
    Stereo(u16),
    MonoMix,
}

/// Folds one interleaved device frame into a stereo pair.
#[inline]
pub fn map_frame(f: &[f32], map: ChannelMap) -> (f32, f32) {
    let channels = f.len();
    if channels == 0 {
        return (0.0, 0.0);
    }
    match map {
        ChannelMap::Auto => {
            if channels == 1 {
                (f[0], f[0])
            } else {
                (f[0], f[1])
            }
        }
        ChannelMap::Mono(c) => {
            let v = f[(c as usize).min(channels - 1)];
            (v, v)
        }
        ChannelMap::Stereo(c) => {
            let c = (c as usize).min(channels - 1);
            (f[c], f[(c + 1).min(channels - 1)])
        }
        ChannelMap::MonoMix => {
            let v = f.iter().sum::<f32>() / channels as f32;
            (v, v)
        }
    }
}

/// Lives inside an input device callback: converts the device's
/// interleaved channels to stereo and pushes them into the bridge.
pub struct CaptureSink {
    writer: BridgeWriter,
    map: ChannelMap,
    scratch: Vec<f32>,
}

impl CaptureSink {
    pub fn new(writer: BridgeWriter, map: ChannelMap) -> Self {
        Self { writer, map, scratch: vec![0.0; 2 * 4096] }
    }

    /// Called from the device callback with interleaved samples.
    pub fn on_input(&mut self, data: &[f32], channels: usize) {
        novamix_dsp::util::enable_flush_to_zero();
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let chunk_frames = self.scratch.len() / 2;
        let mut start = 0;
        while start < frames {
            let end = (start + chunk_frames).min(frames);
            let n = end - start;
            for i in 0..n {
                let f = &data[(start + i) * channels..(start + i + 1) * channels];
                let (l, r) = map_frame(f, self.map);
                self.scratch[2 * i] = novamix_dsp::util::sanitize(l);
                self.scratch[2 * i + 1] = novamix_dsp::util::sanitize(r);
            }
            self.writer.push_interleaved(&self.scratch[..2 * n]);
            start = end;
        }
        self.writer.stats().add_callback(frames);
    }
}

/// Lives inside a (non-master) output device callback: pulls engine audio
/// from the bridge, resampled to the device clock, and spreads it over the
/// device channels.
pub struct PlaybackSource {
    reader: BridgeReader,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl PlaybackSource {
    pub fn new(reader: BridgeReader) -> Self {
        Self { reader, l: vec![0.0; 4096], r: vec![0.0; 4096] }
    }

    pub fn reader(&self) -> &BridgeReader {
        &self.reader
    }

    /// Called from the device callback with the interleaved output buffer.
    pub fn on_output(&mut self, out: &mut [f32], channels: usize) {
        novamix_dsp::util::enable_flush_to_zero();
        let channels = channels.max(1);
        let frames = out.len() / channels;
        self.reader.stats().add_callback(frames);
        let chunk = self.l.len();
        let mut start = 0;
        while start < frames {
            let n = (frames - start).min(chunk);
            let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
            self.reader.read(l, r);
            write_interleaved(&mut out[start * channels..(start + n) * channels], channels, l, r);
            start += n;
        }
    }
}

/// Writes stereo into an interleaved device buffer: mono devices get the
/// average, extra channels are silenced.
#[inline]
pub fn write_interleaved(out: &mut [f32], channels: usize, l: &[f32], r: &[f32]) {
    let n = l.len().min(r.len()).min(out.len() / channels.max(1));
    match channels {
        1 => {
            for i in 0..n {
                out[i] = 0.5 * (l[i] + r[i]);
            }
        }
        2 => {
            for i in 0..n {
                out[2 * i] = l[i];
                out[2 * i + 1] = r[i];
            }
        }
        c => {
            for i in 0..n {
                let f = &mut out[i * c..(i + 1) * c];
                f[0] = l[i];
                f[1] = r[i];
                f[2..].fill(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(cfg: BridgeConfig) -> (BridgeWriter, BridgeReader) {
        bridge(cfg, StreamStats::new_shared(), Arc::new(AtomicBool::new(true)))
    }

    #[test]
    fn primes_then_passes_audio() {
        let cfg = BridgeConfig { source_rate: 48_000.0, sink_rate: 48_000.0, target_frames: 256, capacity_frames: 4096, idle_is_silence: false };
        let (mut w, mut r) = make(cfg);
        let mut l = vec![0.0; 128];
        let mut rr = vec![0.0; 128];
        assert_eq!(r.read(&mut l, &mut rr), ReadStatus::Priming);
        let frames: Vec<f32> = (0..512).flat_map(|i| [i as f32 * 1e-3, -(i as f32) * 1e-3]).collect();
        assert_eq!(w.push_interleaved(&frames), 512);
        assert_eq!(r.read(&mut l, &mut rr), ReadStatus::Full);
        assert!(l.iter().any(|v| *v != 0.0));
        assert!(l.iter().zip(&rr).all(|(a, b)| (a + b).abs() < 1e-6), "channels kept apart");
    }

    #[test]
    fn underrun_is_counted_and_idle_is_not() {
        for idle in [false, true] {
            let cfg = BridgeConfig { source_rate: 48_000.0, sink_rate: 48_000.0, target_frames: 512, capacity_frames: 4096, idle_is_silence: idle };
            let (mut w, mut r) = make(cfg);
            // Run past the start-up grace period first.
            let mut l = vec![0.0; 256];
            let mut rr = vec![0.0; 256];
            for _ in 0..200 {
                w.push_interleaved(&vec![0.1; 2 * 256]);
                r.read(&mut l, &mut rr);
            }
            // Producer stops: the reader drains and then starves.
            let mut st = ReadStatus::Full;
            for _ in 0..10 {
                st = r.read(&mut l, &mut rr);
                if st != ReadStatus::Full {
                    break;
                }
            }
            if idle {
                assert_eq!(st, ReadStatus::Idle);
                assert_eq!(r.stats().snapshot().underruns, 0);
            } else {
                assert_eq!(st, ReadStatus::Underrun);
                assert_eq!(r.stats().snapshot().underruns, 1);
            }
            let _ = &mut w;
        }
    }

    #[test]
    fn overrun_counted_when_full() {
        let cfg = BridgeConfig { source_rate: 48_000.0, sink_rate: 48_000.0, target_frames: 64, capacity_frames: 128, idle_is_silence: false };
        let (mut w, _r) = make(cfg);
        let written = w.push_interleaved(&vec![0.0; 2 * 10_000]);
        assert!(written < 10_000);
        assert_eq!(w.stats().snapshot().overruns, 1);
    }

    /// Simulates a producer whose clock runs 300 ppm fast and a consumer at
    /// nominal speed, with different block sizes, for 120 simulated
    /// seconds. The controller must keep the fill near its target with no
    /// underruns or overruns once settled.
    #[test]
    fn drift_controller_converges_without_glitches() {
        let src_rate = 48_000.0 * (1.0 + 300e-6);
        let cfg = BridgeConfig::for_blocks(48_000.0, 48_000.0, 480, 256, 10.0);
        let (mut w, mut r) = make(cfg);
        let mut l = vec![0.0; 256];
        let mut rr = vec![0.0; 256];
        let block_in = vec![0.25f32; 480 * 2];
        let mut t_in = 0.0f64;
        let mut t_out = 0.0f64;
        let mut fills = Vec::new();
        let total = 120.0;
        while t_out < total {
            // Advance whichever side is due next.
            if t_in <= t_out {
                w.push_interleaved(&block_in);
                t_in += 480.0 / src_rate;
            } else {
                r.read(&mut l, &mut rr);
                t_out += 256.0 / 48_000.0;
                if t_out > 60.0 {
                    fills.push(r.fill_frames() as f64);
                }
            }
        }
        let s = r.stats().snapshot();
        let mean = fills.iter().sum::<f64>() / fills.len() as f64;
        assert!(s.underruns <= 1, "underruns {}", s.underruns);
        assert_eq!(s.overruns, 0);
        assert!((mean - cfg.target_frames as f64).abs() < cfg.target_frames as f64 * 0.5, "mean fill {mean} target {}", cfg.target_frames);
        assert!((s.correction_ppm - 300).abs() < 120, "correction {} ppm", s.correction_ppm);
    }

    #[test]
    fn sample_rate_conversion_through_bridge() {
        let cfg = BridgeConfig::for_blocks(44_100.0, 48_000.0, 441, 480, 10.0);
        let (mut w, mut r) = make(cfg);
        let tone: Vec<f32> = (0..44_100 * 2)
            .flat_map(|i| {
                let v = (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin()) as f32;
                [v, v]
            })
            .collect();
        let mut out = Vec::new();
        let mut l = vec![0.0; 480];
        let mut rr = vec![0.0; 480];
        let mut pos = 0;
        let mut t_in = 0.0;
        let mut t_out = 0.0;
        while pos + 882 <= tone.len() {
            if t_in <= t_out {
                w.push_interleaved(&tone[pos..pos + 882]);
                pos += 882;
                t_in += 0.01;
            } else {
                r.read(&mut l, &mut rr);
                out.extend_from_slice(&l);
                t_out += 0.01;
            }
        }
        let seg = &out[48_000..90_000];
        let mag = novamix_dsp::signal::goertzel_magnitude(seg, 1000.0, 48_000.0);
        assert!((mag - 0.5).abs() < 0.01, "{mag}");
    }

    #[test]
    fn channel_maps() {
        let f = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(map_frame(&f, ChannelMap::Auto), (0.1, 0.2));
        assert_eq!(map_frame(&f[..1], ChannelMap::Auto), (0.1, 0.1));
        assert_eq!(map_frame(&f, ChannelMap::Mono(3)), (0.4, 0.4));
        assert_eq!(map_frame(&f, ChannelMap::Stereo(2)), (0.3, 0.4));
        assert_eq!(map_frame(&f, ChannelMap::Stereo(9)), (0.4, 0.4));
        let (m, _) = map_frame(&f, ChannelMap::MonoMix);
        assert!((m - 0.25).abs() < 1e-6);
    }

    #[test]
    fn capture_sink_pushes_all_frames() {
        let cfg = BridgeConfig { source_rate: 48_000.0, sink_rate: 48_000.0, target_frames: 2, capacity_frames: 64, idle_is_silence: false };
        let (w, r) = make(cfg);
        let mut sink = CaptureSink::new(w, ChannelMap::Stereo(2));
        let data: Vec<f32> = (0..16).map(|i| (i % 4) as f32 * 0.1).collect();
        sink.on_input(&data, 4);
        assert_eq!(r.fill_frames(), 4);
        assert_eq!(r.stats().snapshot().callbacks, 1);
    }
}
