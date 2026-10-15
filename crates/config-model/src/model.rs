//! Configuration and profile data model (serialized as TOML or JSON).

use novamix_dsp::params::{ChainParams, LimiterParams};
use novamix_dsp::signal::ToneKind;
use novamix_mixer_graph::SoloMode;
use serde::{Deserialize, Serialize};

/// Current schema version of both `config.toml` and profile files.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Application configuration

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub general: GeneralSettings,
    pub audio: AudioSettings,
    pub hotkeys: HotkeySettings,
    pub diagnostics: DiagnosticsSettings,
    /// Name of the profile loaded at start-up.
    pub active_profile: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            audio: AudioSettings::default(),
            hotkeys: HotkeySettings::default(),
            diagnostics: DiagnosticsSettings::default(),
            active_profile: crate::defaults::DEFAULT_PROFILE_NAME.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    PtBr,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    #[default]
    Dark,
    Light,
    System,
}

/// What to do when the application (or the computer) restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeBehavior {
    /// Reload the last active profile and open its devices.
    #[default]
    RestoreProfile,
    /// Start with the default profile.
    DefaultProfile,
    /// Start in safe mode (no effects, minimal known route).
    SafeMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralSettings {
    pub language: Language,
    pub theme: ThemePref,
    pub ui_scale: f32,
    pub start_with_system: bool,
    pub start_minimized: bool,
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub confirm_destructive: bool,
    pub resume: ResumeBehavior,
    pub compact_mode: bool,
    /// "Modo economia": lower meter refresh, no spectrum analysis.
    pub power_saving: bool,
    pub meter_fps: u32,
    pub show_spectrum: bool,
    /// Advanced DSP controls instead of preset/intensity only.
    pub advanced_mode: bool,
    pub first_run_completed: bool,
    pub show_tooltips: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: Language::PtBr,
            theme: ThemePref::Dark,
            ui_scale: 1.0,
            start_with_system: false,
            start_minimized: false,
            minimize_to_tray: true,
            close_to_tray: false,
            confirm_destructive: true,
            resume: ResumeBehavior::RestoreProfile,
            compact_mode: false,
            power_saving: false,
            meter_fps: 60,
            show_spectrum: false,
            advanced_mode: false,
            first_run_completed: false,
            show_tooltips: true,
        }
    }
}

/// Latency/stability trade-off presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyMode {
    LowLatency,
    #[default]
    Balanced,
    Stability,
}

impl LatencyMode {
    pub const ALL: [LatencyMode; 3] = [LatencyMode::LowLatency, LatencyMode::Balanced, LatencyMode::Stability];

    /// Requested device buffer size in frames.
    pub fn buffer_frames(self) -> u32 {
        match self {
            LatencyMode::LowLatency => 128,
            LatencyMode::Balanced => 256,
            LatencyMode::Stability => 512,
        }
    }

    /// Safety margin kept in the bridges between devices, in milliseconds.
    pub fn bridge_margin_ms(self) -> f32 {
        match self {
            LatencyMode::LowLatency => 2.0,
            LatencyMode::Balanced => 5.0,
            LatencyMode::Stability => 20.0,
        }
    }
}

/// Sample rates offered to the user (only those supported are shown).
pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
/// Buffer sizes offered to the user (only those supported are shown).
pub const BUFFER_SIZES: [u32; 5] = [64, 128, 256, 512, 1024];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioSettings {
    /// Audio host/backend ("wasapi", "asio"...). `None` = platform default.
    pub host: Option<String>,
    pub preferred_sample_rate: u32,
    pub latency_mode: LatencyMode,
    /// Explicit buffer size; `None` derives it from `latency_mode`.
    pub buffer_frames: Option<u32>,
    pub auto_reconnect: bool,
    pub reconnect_interval_ms: u32,
    /// Output used when the monitor device disappears.
    pub fallback_output: Option<DeviceRef>,
    /// Ramp time for faders, mutes and sends.
    pub smoothing_ms: f32,
    /// Bus whose output device drives the engine clock; `None` = automatic
    /// (the monitor bus, otherwise the first bus with an output).
    pub clock_bus: Option<u8>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            host: None,
            preferred_sample_rate: 48_000,
            latency_mode: LatencyMode::Balanced,
            buffer_frames: None,
            auto_reconnect: true,
            reconnect_interval_ms: 2000,
            fallback_output: None,
            smoothing_ms: 20.0,
            clock_bus: None,
        }
    }
}

impl AudioSettings {
    pub fn effective_buffer_frames(&self) -> u32 {
        self.buffer_frames.unwrap_or_else(|| self.latency_mode.buffer_frames())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    ToggleMicMute,
    PushToTalk,
    PushToMute,
    NextProfile,
    PreviousProfile,
    ActivateProfile,
    ToggleBusMute,
    SafeMode,
    ToggleWindow,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 9] = [
        HotkeyAction::ToggleMicMute,
        HotkeyAction::PushToTalk,
        HotkeyAction::PushToMute,
        HotkeyAction::NextProfile,
        HotkeyAction::PreviousProfile,
        HotkeyAction::ActivateProfile,
        HotkeyAction::ToggleBusMute,
        HotkeyAction::SafeMode,
        HotkeyAction::ToggleWindow,
    ];

    pub fn key(self) -> &'static str {
        match self {
            HotkeyAction::ToggleMicMute => "hk_toggle_mic_mute",
            HotkeyAction::PushToTalk => "hk_push_to_talk",
            HotkeyAction::PushToMute => "hk_push_to_mute",
            HotkeyAction::NextProfile => "hk_next_profile",
            HotkeyAction::PreviousProfile => "hk_previous_profile",
            HotkeyAction::ActivateProfile => "hk_activate_profile",
            HotkeyAction::ToggleBusMute => "hk_toggle_bus_mute",
            HotkeyAction::SafeMode => "hk_safe_mode",
            HotkeyAction::ToggleWindow => "hk_toggle_window",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeyBinding {
    pub enabled: bool,
    pub action: HotkeyAction,
    /// Accelerator such as `Ctrl+Alt+M`.
    pub accelerator: String,
    /// Target bus index for `ToggleBusMute`.
    pub bus: Option<u8>,
    /// Target profile for `ActivateProfile`.
    pub profile: Option<String>,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self { enabled: true, action: HotkeyAction::ToggleMicMute, accelerator: String::new(), bus: None, profile: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeySettings {
    /// Global hotkeys are opt-in.
    pub enabled: bool,
    pub bindings: Vec<HotkeyBinding>,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        let b = |action, accel: &str, enabled| HotkeyBinding { enabled, action, accelerator: accel.into(), bus: None, profile: None };
        Self {
            enabled: false,
            bindings: vec![
                b(HotkeyAction::ToggleMicMute, "Ctrl+Alt+M", true),
                b(HotkeyAction::PushToTalk, "Ctrl+Alt+T", false),
                b(HotkeyAction::NextProfile, "Ctrl+Alt+PageDown", true),
                b(HotkeyAction::PreviousProfile, "Ctrl+Alt+PageUp", true),
                b(HotkeyAction::SafeMode, "Ctrl+Alt+Shift+S", true),
                b(HotkeyAction::ToggleWindow, "Ctrl+Alt+N", true),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_filter(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsSettings {
    /// Hide device names and system details in exported reports by default.
    pub anonymize_reports: bool,
    pub log_level: LogLevel,
    pub keep_log_days: u32,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self { anonymize_reports: true, log_level: LogLevel::Info, keep_log_days: 7 }
    }
}

// ---------------------------------------------------------------------------
// Profiles

/// Reference to a device: the stable backend id plus the display name, used
/// as a fallback when the id changed (e.g. USB device moved to another port).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
}

impl DeviceRef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into() }
    }
}

/// Where a channel gets its audio from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSpec {
    #[default]
    None,
    /// The system default recording device (follows changes).
    DefaultInput,
    /// A specific recording device.
    Input { device: DeviceRef },
    /// Loopback capture of a specific playback device.
    Loopback { device: DeviceRef },
    /// Loopback capture of the system default playback device.
    DefaultOutputLoopback,
    /// The NovaMix virtual playback endpoint (apps play into the mixer).
    /// Falls back to a detected third-party virtual cable in development.
    VirtualPlayback,
    /// Local synthetic signal; captures and sends no real audio.
    TestTone { kind: ToneKind, frequency_hz: f32, level_db: f32 },
}

impl SourceSpec {
    pub fn is_none(&self) -> bool {
        matches!(self, SourceSpec::None)
    }

    pub fn key(&self) -> &'static str {
        match self {
            SourceSpec::None => "src_none",
            SourceSpec::DefaultInput => "src_default_input",
            SourceSpec::Input { .. } => "src_input",
            SourceSpec::Loopback { .. } => "src_loopback",
            SourceSpec::DefaultOutputLoopback => "src_default_loopback",
            SourceSpec::VirtualPlayback => "src_virtual_playback",
            SourceSpec::TestTone { .. } => "src_test_tone",
        }
    }
}

/// How device channels map onto the (stereo) channel strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", content = "channel", rename_all = "snake_case")]
pub enum InputMap {
    /// First two channels (or the single channel duplicated).
    #[default]
    Auto,
    /// One device channel (0-based) as mono.
    Mono(u16),
    /// Two consecutive device channels starting at the given index.
    Stereo(u16),
    /// Average of all device channels.
    MonoMix,
}

/// Where a bus sends its mix.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputSpec {
    #[default]
    None,
    /// The system default playback device (follows changes).
    DefaultOutput,
    Device {
        device: DeviceRef,
    },
    /// The render side of the NovaMix virtual microphone (apps record the
    /// bus from "NovaMix Virtual Mic"). Falls back to a detected virtual
    /// cable in development.
    VirtualMic,
}

impl OutputSpec {
    pub fn is_none(&self) -> bool {
        matches!(self, OutputSpec::None)
    }

    pub fn key(&self) -> &'static str {
        match self {
            OutputSpec::None => "out_none",
            OutputSpec::DefaultOutput => "out_default",
            OutputSpec::Device { .. } => "out_device",
            OutputSpec::VirtualMic => "out_virtual_mic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SendConfig {
    pub enabled: bool,
    pub level_db: f32,
    pub pre_fader: bool,
}

impl Default for SendConfig {
    fn default() -> Self {
        Self { enabled: false, level_db: 0.0, pre_fader: false }
    }
}

impl SendConfig {
    pub fn on() -> Self {
        Self { enabled: true, ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChannelConfig {
    pub name: String,
    pub enabled: bool,
    pub source: SourceSpec,
    pub input_map: InputMap,
    /// Marks microphone channels (feedback warnings, mic-mute hotkey).
    pub is_microphone: bool,
    pub input_gain_db: f32,
    pub fader_db: f32,
    pub mute: bool,
    pub solo: bool,
    pub pan: f32,
    /// Process and send as mono.
    pub mono: bool,
    pub auto_reconnect: bool,
    pub chain: ChainParams,
    /// One entry per bus, index aligned with `Profile::buses`.
    pub sends: Vec<SendConfig>,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            name: "Canal".into(),
            enabled: true,
            source: SourceSpec::None,
            input_map: InputMap::Auto,
            is_microphone: false,
            input_gain_db: 0.0,
            fader_db: 0.0,
            mute: false,
            solo: false,
            pan: 0.0,
            mono: false,
            auto_reconnect: true,
            chain: ChainParams::default(),
            sends: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusRole {
    /// Local listening (headphones/speakers); solo and cue apply here.
    Monitor,
    /// Processed voice, usually feeding the virtual microphone.
    Voice,
    /// Independent mix for streaming/recording.
    Stream,
    #[default]
    Generic,
}

impl BusRole {
    pub const ALL: [BusRole; 4] = [BusRole::Monitor, BusRole::Voice, BusRole::Stream, BusRole::Generic];

    pub fn key(self) -> &'static str {
        match self {
            BusRole::Monitor => "role_monitor",
            BusRole::Voice => "role_voice",
            BusRole::Stream => "role_stream",
            BusRole::Generic => "role_generic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BusConfig {
    pub name: String,
    pub role: BusRole,
    pub output: OutputSpec,
    pub fader_db: f32,
    pub mute: bool,
    /// Cue: replace the monitor mix with this bus to pre-listen to it.
    pub cue: bool,
    pub mono: bool,
    /// Output delay (sync with video, or delayed self-monitoring).
    pub delay_ms: f32,
    /// Safety limiter on the bus output.
    pub limiter: LimiterParams,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            name: "Bus".into(),
            role: BusRole::Generic,
            output: OutputSpec::None,
            fader_db: 0.0,
            mute: false,
            cue: false,
            mono: false,
            delay_ms: 0.0,
            limiter: LimiterParams { enabled: true, ceiling_db: -0.3, lookahead_ms: 1.0, release_ms: 60.0 },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub name: String,
    pub description: String,
    pub solo_mode: SoloMode,
    pub channels: Vec<ChannelConfig>,
    pub buses: Vec<BusConfig>,
}

impl Default for Profile {
    fn default() -> Self {
        crate::defaults::default_profile()
    }
}

impl Profile {
    /// Index of the first monitor bus.
    pub fn monitor_bus(&self) -> Option<usize> {
        self.buses.iter().position(|b| b.role == BusRole::Monitor)
    }

    /// Indexes of microphone channels.
    pub fn microphone_channels(&self) -> Vec<usize> {
        self.channels.iter().enumerate().filter(|(_, c)| c.is_microphone).map(|(i, _)| i).collect()
    }

    pub fn any_solo(&self) -> bool {
        self.channels.iter().any(|c| c.enabled && c.solo)
    }

    /// Copies the effect chain of channel `from` to channel `to`.
    pub fn copy_chain(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.channels.len() || to >= self.channels.len() {
            return false;
        }
        self.channels[to].chain = self.channels[from].chain;
        true
    }

    /// Adds a channel (up to the engine maximum).
    pub fn add_channel(&mut self, name: &str) -> Option<usize> {
        if self.channels.len() >= novamix_mixer_graph::MAX_CHANNELS {
            return None;
        }
        let mut ch = ChannelConfig { name: name.into(), ..ChannelConfig::default() };
        ch.sends = vec![SendConfig::default(); self.buses.len()];
        if let Some(m) = self.monitor_bus() {
            ch.sends[m] = SendConfig::on();
        }
        self.channels.push(ch);
        Some(self.channels.len() - 1)
    }

    pub fn remove_channel(&mut self, index: usize) -> bool {
        if self.channels.len() <= 1 || index >= self.channels.len() {
            return false;
        }
        self.channels.remove(index);
        true
    }

    /// Adds a bus (up to the engine maximum) and extends every send list.
    pub fn add_bus(&mut self, name: &str, role: BusRole) -> Option<usize> {
        if self.buses.len() >= novamix_mixer_graph::MAX_BUSES {
            return None;
        }
        self.buses.push(BusConfig { name: name.into(), role, ..BusConfig::default() });
        for ch in &mut self.channels {
            ch.sends.push(SendConfig::default());
        }
        Some(self.buses.len() - 1)
    }

    pub fn remove_bus(&mut self, index: usize) -> bool {
        if self.buses.len() <= 1 || index >= self.buses.len() {
            return false;
        }
        self.buses.remove(index);
        for ch in &mut self.channels {
            if index < ch.sends.len() {
                ch.sends.remove(index);
            }
        }
        true
    }
}
