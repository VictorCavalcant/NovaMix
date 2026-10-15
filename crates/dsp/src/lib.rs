//! # novamix-dsp
//!
//! Real-time safe DSP building blocks for NovaMix.
//!
//! Every processor follows the same contract:
//! * construction (and `set_params`) may compute coefficients but never
//!   allocates after `new()`;
//! * `process` works in place on a left buffer and an optional right buffer
//!   (`None` = mono), for any block length;
//! * parameter changes are smoothed internally so they never click.
//!
//! The default voice chain is **HPF → noise reduction → gate/expander →
//! AGC → EQ → de-esser → compressor → make-up gain → limiter**
//! (see [`chain::VoiceChain`]).

pub mod biquad;
pub mod chain;
pub mod delay;
pub mod dynamics;
pub mod filters;
pub mod limiter;
pub mod meter;
pub mod noise;
pub mod pan;
pub mod params;
pub mod resampler;
pub mod signal;
pub mod smoother;
pub mod spectrum;
pub mod util;

pub use chain::{ChainMeters, VoiceChain};
pub use delay::DelayLine;
pub use meter::{LevelMeter, MeterReading};
pub use params::*;
pub use resampler::Resampler;
pub use smoother::LinearSmoother;

/// Common interface of in-place stereo/mono processors.
pub trait Processor {
    /// Clears internal state (filter memories, envelopes, delay lines).
    fn reset(&mut self);
    /// Processes `left` and, if present, `right` in place.
    fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>);
    /// Latency introduced by the processor, in samples.
    fn latency(&self) -> usize {
        0
    }
}
