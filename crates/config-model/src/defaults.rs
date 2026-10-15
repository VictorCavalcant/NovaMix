//! Built-in profiles: the default route and scenario templates.

use novamix_dsp::params::{ChainParams, LimiterParams, VoicePreset};

use crate::model::*;

pub const DEFAULT_PROFILE_NAME: &str = "Padrão";

/// Scenario templates offered by the profile manager and setup assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Default,
    Meeting,
    Gaming,
    Streaming,
}

impl Scenario {
    pub const ALL: [Scenario; 4] = [Scenario::Default, Scenario::Meeting, Scenario::Gaming, Scenario::Streaming];

    pub fn key(self) -> &'static str {
        match self {
            Scenario::Default => "scenario_default",
            Scenario::Meeting => "scenario_meeting",
            Scenario::Gaming => "scenario_gaming",
            Scenario::Streaming => "scenario_streaming",
        }
    }

    pub fn profile_name(self) -> &'static str {
        match self {
            Scenario::Default => DEFAULT_PROFILE_NAME,
            Scenario::Meeting => "Reunião",
            Scenario::Gaming => "Jogo",
            Scenario::Streaming => "Stream",
        }
    }
}

fn bus(name: &str, role: BusRole, output: OutputSpec) -> BusConfig {
    BusConfig { name: name.into(), role, output, ..BusConfig::default() }
}

fn channel(name: &str, source: SourceSpec, mic: bool, chain: ChainParams, sends: [bool; 3]) -> ChannelConfig {
    ChannelConfig {
        name: name.into(),
        source,
        is_microphone: mic,
        mono: mic,
        chain,
        sends: sends.iter().map(|&on| if on { SendConfig::on() } else { SendConfig::default() }).collect(),
        ..ChannelConfig::default()
    }
}

/// Default route:
///
/// ```text
/// Microfone ─ DSP de voz ─┬─> Voz (microfone virtual)
///                         └─> Stream
/// Desktop (reprodução virtual) ─┬─> Monitor (fones)
///                               └─> Stream
/// ```
///
/// The microphone is **not** sent to the monitor bus by default, to avoid
/// acoustic echo.
pub fn default_profile() -> Profile {
    scenario_profile(Scenario::Default)
}

/// Builds the profile for a scenario.
pub fn scenario_profile(s: Scenario) -> Profile {
    // Bus order: 0 Monitor, 1 Voz, 2 Stream.
    let buses = vec![
        bus("Monitor", BusRole::Monitor, OutputSpec::DefaultOutput),
        BusConfig {
            limiter: LimiterParams { enabled: true, ceiling_db: -1.0, lookahead_ms: 2.0, release_ms: 80.0 },
            ..bus("Voz", BusRole::Voice, OutputSpec::VirtualMic)
        },
        bus("Stream", BusRole::Stream, OutputSpec::None),
    ];
    let mic_preset = match s {
        Scenario::Default => VoicePreset::Natural,
        Scenario::Meeting => VoicePreset::Call,
        Scenario::Gaming => VoicePreset::NoisyRoom,
        Scenario::Streaming => VoicePreset::Stream,
    };
    let stream_on = matches!(s, Scenario::Default | Scenario::Streaming);
    let channels = vec![
        channel("Microfone", SourceSpec::DefaultInput, true, mic_preset.chain(), [false, true, stream_on]),
        channel("Desktop", SourceSpec::VirtualPlayback, false, VoicePreset::Raw.chain(), [true, false, stream_on]),
        channel("Aux 1", SourceSpec::None, false, VoicePreset::Raw.chain(), [true, false, stream_on]),
        channel("Aux 2", SourceSpec::None, false, VoicePreset::Raw.chain(), [true, false, stream_on]),
    ];
    let description = match s {
        Scenario::Default => "Microfone tratado no microfone virtual; áudio do desktop nos fones.",
        Scenario::Meeting => "Voz clara para chamadas: filtro, redução de ruído moderada, compressor e limiter.",
        Scenario::Gaming => "Ambiente ruidoso: redução de ruído forte e gate; jogo e conversa nos fones.",
        Scenario::Streaming => "Voz encorpada para transmissão; mixagem própria no bus Stream.",
    };
    Profile {
        schema_version: SCHEMA_VERSION,
        name: s.profile_name().into(),
        description: description.into(),
        solo_mode: Default::default(),
        channels,
        buses,
    }
}

/// Safe-mode profile derived from `p`: every effect bypassed, monitoring of
/// microphones disabled, all bus cues cleared. Devices are kept.
pub fn safe_mode_of(p: &Profile) -> Profile {
    let mut s = p.clone();
    let monitor = s.monitor_bus();
    for ch in &mut s.channels {
        ch.chain.bypass = true;
        ch.solo = false;
        if ch.is_microphone
            && let Some(m) = monitor
            && let Some(send) = ch.sends.get_mut(m)
        {
            send.enabled = false;
        }
    }
    for b in &mut s.buses {
        b.cue = false;
        b.delay_ms = 0.0;
        b.limiter.enabled = true;
    }
    s
}
