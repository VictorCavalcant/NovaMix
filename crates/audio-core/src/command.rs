//! Messages from the control thread to the audio thread, and objects the
//! audio thread hands back for deallocation.

use novamix_dsp::params::{LimiterParams, STAGE_COUNT, StageKind, StageParams};
use novamix_dsp::signal::ToneKind;

use crate::bridge::{BridgeReader, BridgeWriter};

/// What the spectrum tap listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapPoint {
    ChannelPost(u8),
    Bus(u8),
}

/// Commands are applied at the start of the next audio block. Boxed
/// payloads (device bridges) are allocated by the control thread; whatever
/// they replace is sent back through the garbage queue so the audio thread
/// never frees memory.
pub enum Command {
    ChannelEnabled {
        ch: u8,
        enabled: bool,
    },
    InputGain {
        ch: u8,
        db: f32,
    },
    Fader {
        ch: u8,
        db: f32,
    },
    ChannelMute {
        ch: u8,
        mute: bool,
    },
    Pan {
        ch: u8,
        pan: f32,
    },
    Mono {
        ch: u8,
        mono: bool,
    },
    /// `gain` is linear and already includes solo resolution.
    Send {
        ch: u8,
        bus: u8,
        gain: f32,
        pre_fader: bool,
    },
    Stage {
        ch: u8,
        params: StageParams,
    },
    ChainOrder {
        ch: u8,
        order: [StageKind; STAGE_COUNT],
    },
    ChainBypass {
        ch: u8,
        bypass: bool,
    },
    AbListen {
        ch: u8,
        dry: bool,
        loudness_match: bool,
    },
    TestTone {
        ch: u8,
        enabled: bool,
        kind: ToneKind,
        frequency_hz: f32,
        level_db: f32,
    },
    AttachInput {
        ch: u8,
        reader: Box<BridgeReader>,
    },
    DetachInput {
        ch: u8,
    },

    BusFader {
        bus: u8,
        db: f32,
    },
    BusMute {
        bus: u8,
        mute: bool,
    },
    BusMono {
        bus: u8,
        mono: bool,
    },
    BusDelay {
        bus: u8,
        ms: f32,
    },
    BusLimiter {
        bus: u8,
        params: LimiterParams,
    },
    BusEnabled {
        bus: u8,
        enabled: bool,
    },
    /// Output slot fed by the bus (`None` = nowhere).
    BusOutput {
        bus: u8,
        slot: Option<u8>,
    },
    /// Monitor bus whose output plays the cued buses while any cue is set.
    MonitorBus {
        bus: Option<u8>,
    },
    CueMask {
        mask: u8,
    },

    AttachOutput {
        slot: u8,
        writer: Box<BridgeWriter>,
    },
    DetachOutput {
        slot: u8,
    },
    /// Output slot written directly into the master device callback.
    MasterSlot {
        slot: Option<u8>,
    },

    SetTap {
        point: Option<TapPoint>,
    },
    SetSmoothing {
        ms: f32,
    },
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Bridges are not Debug; print the variant name only.
        let name = match self {
            Command::ChannelEnabled { .. } => "ChannelEnabled",
            Command::InputGain { .. } => "InputGain",
            Command::Fader { .. } => "Fader",
            Command::ChannelMute { .. } => "ChannelMute",
            Command::Pan { .. } => "Pan",
            Command::Mono { .. } => "Mono",
            Command::Send { .. } => "Send",
            Command::Stage { .. } => "Stage",
            Command::ChainOrder { .. } => "ChainOrder",
            Command::ChainBypass { .. } => "ChainBypass",
            Command::AbListen { .. } => "AbListen",
            Command::TestTone { .. } => "TestTone",
            Command::AttachInput { .. } => "AttachInput",
            Command::DetachInput { .. } => "DetachInput",
            Command::BusFader { .. } => "BusFader",
            Command::BusMute { .. } => "BusMute",
            Command::BusMono { .. } => "BusMono",
            Command::BusDelay { .. } => "BusDelay",
            Command::BusLimiter { .. } => "BusLimiter",
            Command::BusEnabled { .. } => "BusEnabled",
            Command::BusOutput { .. } => "BusOutput",
            Command::MonitorBus { .. } => "MonitorBus",
            Command::CueMask { .. } => "CueMask",
            Command::AttachOutput { .. } => "AttachOutput",
            Command::DetachOutput { .. } => "DetachOutput",
            Command::MasterSlot { .. } => "MasterSlot",
            Command::SetTap { .. } => "SetTap",
            Command::SetSmoothing { .. } => "SetSmoothing",
        };
        f.write_str(name)
    }
}

/// Objects returned by the audio thread for deallocation elsewhere.
pub enum Garbage {
    Input(Box<BridgeReader>),
    Output(Box<BridgeWriter>),
}
