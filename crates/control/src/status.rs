//! Status snapshot published by the controller for the UI.

use novamix_device_api::{DeviceInfo, StreamInfo, VirtualDeviceStatus};
use novamix_diagnostics::StreamStatsSnapshot;
use novamix_mixer_graph::RouteAnalysis;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum EndpointState {
    /// Nothing configured.
    Off,
    Starting,
    Running,
    /// Loopback source with nothing playing.
    Idle,
    /// Synthetic test signal.
    TestTone,
    /// Configured device not present right now (waiting to reconnect).
    Unavailable(String),
    /// Opening or running the stream failed.
    Failed(String),
}

impl EndpointState {
    pub fn is_problem(&self) -> bool {
        matches!(self, EndpointState::Unavailable(_) | EndpointState::Failed(_))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndpointStatus {
    pub state: EndpointState,
    pub device_name: Option<String>,
    pub info: Option<StreamInfo>,
    pub stats: Option<StreamStatsSnapshot>,
    /// Latency added by device buffer + bridge, in milliseconds.
    pub latency_ms: f32,
    pub reconnects: u32,
}

impl Default for EndpointStatus {
    fn default() -> Self {
        Self { state: EndpointState::Off, device_name: None, info: None, stats: None, latency_ms: 0.0, reconnects: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "device", rename_all = "snake_case")]
pub enum ClockSource {
    None,
    Device(String),
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineStatus {
    pub running: bool,
    pub host: String,
    pub sample_rate: u32,
    pub block_frames: u32,
    pub clock: ClockSource,
    pub channels: Vec<EndpointStatus>,
    pub buses: Vec<EndpointStatus>,
    /// Estimated microphone → virtual microphone latency.
    pub voice_latency_ms: Option<f32>,
    pub route: RouteAnalysis,
    pub devices: Vec<DeviceInfo>,
    pub virtual_status: VirtualDeviceStatus,
    pub last_error: Option<String>,
    pub profile_name: String,
    pub engine_restarts: u32,
    /// Increments on every update (cheap change detection).
    pub revision: u64,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            running: false,
            host: String::new(),
            sample_rate: 0,
            block_frames: 0,
            clock: ClockSource::None,
            channels: Vec::new(),
            buses: Vec::new(),
            voice_latency_ms: None,
            route: RouteAnalysis::default(),
            devices: Vec::new(),
            virtual_status: VirtualDeviceStatus::default(),
            last_error: None,
            profile_name: String::new(),
            engine_restarts: 0,
            revision: 0,
        }
    }
}
