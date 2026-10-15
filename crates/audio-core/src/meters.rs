//! Shared meter values written by the audio thread and read by the UI.
//!
//! Peaks accumulate with `fetch_max` and are cleared by the reader with
//! [`take_peak`](StripMeter::take_peak), so the UI never misses a transient
//! even when it refreshes less often than the audio callback runs.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use novamix_diagnostics::{AtomicF32, TimingStats};
use novamix_mixer_graph::{MAX_BUSES, MAX_CHANNELS};

#[derive(Debug, Default)]
pub struct StripMeter {
    pub input_peak: [AtomicF32; 2],
    pub input_rms: [AtomicF32; 2],
    pub output_peak: [AtomicF32; 2],
    pub output_rms: [AtomicF32; 2],
    /// Incremented whenever the *source* arrives clipped (before input gain).
    pub source_clips: AtomicU32,
    /// Incremented whenever the signal clips after input gain / DSP.
    pub output_clips: AtomicU32,
    pub gate_open: AtomicBool,
    pub gate_gain_db: AtomicF32,
    pub expander_gr_db: AtomicF32,
    pub compressor_gr_db: AtomicF32,
    pub deesser_gr_db: AtomicF32,
    pub limiter_gr_db: AtomicF32,
    pub agc_gain_db: AtomicF32,
    pub nr_reduction_db: AtomicF32,
    pub vad: AtomicF32,
    pub neural_active: AtomicBool,
    pub latency_samples: AtomicU32,
    /// DSP time of this strip as a percentage of the block budget.
    pub dsp_load_pct: AtomicF32,
    /// True while the channel receives real audio from its source.
    pub source_active: AtomicBool,
}

impl StripMeter {
    /// Reads and clears an accumulated peak atomically.
    pub fn take_peak(p: &AtomicF32) -> f32 {
        p.swap(0.0)
    }
}

#[derive(Debug, Default)]
pub struct BusMeter {
    pub peak: [AtomicF32; 2],
    pub rms: [AtomicF32; 2],
    pub limiter_gr_db: AtomicF32,
    pub clips: AtomicU32,
}

/// All meters of the engine.
#[derive(Debug)]
pub struct MeterBank {
    pub channels: [StripMeter; MAX_CHANNELS],
    pub buses: [BusMeter; MAX_BUSES],
    pub timing: TimingStats,
    pub sample_rate: AtomicU32,
    pub block_frames: AtomicU32,
    /// True when the engine is clocked by its internal timer (no master
    /// output device).
    pub internal_clock: AtomicBool,
    pub running: AtomicBool,
}

impl Default for MeterBank {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| StripMeter::default()),
            buses: std::array::from_fn(|_| BusMeter::default()),
            timing: TimingStats::default(),
            sample_rate: AtomicU32::new(0),
            block_frames: AtomicU32::new(0),
            internal_clock: AtomicBool::new(false),
            running: AtomicBool::new(false),
        }
    }
}

impl MeterBank {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
}
