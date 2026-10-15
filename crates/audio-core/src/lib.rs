//! # novamix-audio-core
//!
//! The real-time mixing engine.
//!
//! ```text
//!  input device ──CaptureSink──▶ bridge ──▶ ┐
//!  input device ──CaptureSink──▶ bridge ──▶ ├─ MixerCore (master clock thread)
//!  test tone ─────────────────────────────▶ ┘   strips → sends → buses
//!                                                │
//!          master output device ◀── MasterDriver ┤ (written in place)
//!          other output device ◀── PlaybackSource ◀── bridge ◀┘
//! ```
//!
//! * The engine runs inside the callback of one output device (the clock
//!   master) or on an internal timer thread when there is none.
//! * Every other device is connected through a lock-free bridge with a
//!   drift-compensating resampler.
//! * The control thread talks to the engine only through a bounded SPSC
//!   command queue; replaced objects come back through a garbage queue, so
//!   the audio thread never allocates, frees, locks or does I/O.

pub mod bridge;
pub mod bus;
pub mod clock;
pub mod command;
pub mod engine;
pub mod meters;
pub mod strip;

pub use bridge::{BridgeConfig, BridgeReader, BridgeWriter, CaptureSink, ChannelMap, PlaybackSource, ReadStatus, bridge};
pub use clock::InternalClock;
pub use command::{Command, Garbage, TapPoint};
pub use engine::{EngineConfig, EngineHandles, MAX_OUTPUTS, MasterDriver, MixerCore, SharedCore, create_engine, with_core};
pub use meters::{BusMeter, MeterBank, StripMeter};

#[cfg(test)]
mod tests;
