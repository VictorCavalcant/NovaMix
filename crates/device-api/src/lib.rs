//! # novamix-device-api
//!
//! Platform-neutral audio device abstraction:
//!
//! * [`AudioBackend`]: enumerate devices, open input/output/loopback
//!   streams with simple `f32` interleaved callbacks;
//! * [`VirtualDeviceStatus`]: what the platform adapter knows about virtual
//!   endpoints (the NovaMix driver or, in development, third-party virtual
//!   cables);
//! * a CPAL implementation (WASAPI on Windows, optional ASIO), NovaMix's own
//!   low-latency WASAPI backend on Windows (hosts `wasapi-lowlatency` and
//!   `wasapi-exclusive`) and a [`NullBackend`] for headless tests.
//!
//! CPAL covers physical devices and loopback capture; it does **not** create
//! virtual endpoints. Those come from a driver (see `platform/windows-driver`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(feature = "cpal-backend")]
pub mod cpal_backend;
#[cfg(all(windows, feature = "wasapi-lowlatency"))]
pub mod wasapi_lowlat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

/// Physical form of a device (used for feedback warnings and icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceForm {
    Speakers,
    Headphones,
    Headset,
    Microphone,
    LineLevel,
    Digital,
    Virtual,
    #[default]
    Unknown,
}

impl DeviceForm {
    /// Devices that radiate sound into the room (acoustic feedback risk).
    pub fn is_loudspeaker(self) -> bool {
        matches!(self, DeviceForm::Speakers)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable backend identifier (persisted in profiles).
    pub id: String,
    pub name: String,
    pub direction: Direction,
    pub is_default: bool,
    pub channels: u16,
    pub default_sample_rate: u32,
    /// Standard rates the device reports support for.
    pub sample_rates: Vec<u32>,
    /// Supported buffer size range in frames, when known.
    pub buffer_range: Option<(u32, u32)>,
    pub sample_format: String,
    pub form: DeviceForm,
    pub interface: String,
    pub is_virtual: bool,
    pub manufacturer: Option<String>,
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRequest {
    pub device_id: String,
    /// For inputs: capture what an *output* device is playing.
    pub loopback: bool,
    /// Requested rate (outputs may be resampled by the OS; capture streams
    /// generally use the device's native rate).
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
}

/// What was actually negotiated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub device_id: String,
    pub device_name: String,
    pub direction: Direction,
    pub loopback: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: Option<u32>,
    pub sample_format: String,
}

/// A running stream; dropping it stops the stream.
pub trait StreamHandle: Send {
    fn info(&self) -> &StreamInfo;
}

pub type InputCallback = Box<dyn FnMut(&[f32], usize) + Send + 'static>;
pub type OutputCallback = Box<dyn FnMut(&mut [f32], usize) + Send + 'static>;
pub type ErrorCallback = Box<dyn FnMut(DeviceError) + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceErrorKind {
    NotFound,
    Disconnected,
    Busy,
    Unsupported,
    PermissionDenied,
    Xrun,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("{kind:?}: {message}")]
pub struct DeviceError {
    pub kind: DeviceErrorKind,
    pub message: String,
}

impl DeviceError {
    pub fn new(kind: DeviceErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    /// Errors after which the stream must be reopened.
    pub fn is_fatal(&self) -> bool {
        !matches!(self.kind, DeviceErrorKind::Xrun)
    }
}

/// An audio host (WASAPI, ASIO, PipeWire, Core Audio…).
pub trait AudioBackend: Send + Sync {
    fn host_name(&self) -> String;
    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError>;
    fn open_input(&self, req: &StreamRequest, data: InputCallback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError>;
    fn open_output(&self, req: &StreamRequest, data: OutputCallback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError>;

    /// Callback period (frames) a stream opened with `req` is expected to
    /// use, when the backend can tell before opening it. Callers size their
    /// buffering from it; `None` means "unknown" (assume the 10 ms WASAPI
    /// default). [`StreamInfo::buffer_frames`] has the actual value.
    fn period_hint(&self, req: &StreamRequest, direction: Direction) -> Option<u32> {
        let _ = (req, direction);
        None
    }
}

/// Backend without devices (tests, `--no-audio`).
#[derive(Debug, Default)]
pub struct NullBackend;

impl AudioBackend for NullBackend {
    fn host_name(&self) -> String {
        "null".into()
    }
    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        Ok(Vec::new())
    }
    fn open_input(&self, req: &StreamRequest, _: InputCallback, _: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        Err(DeviceError::new(DeviceErrorKind::NotFound, format!("no device {}", req.device_id)))
    }
    fn open_output(&self, req: &StreamRequest, _: OutputCallback, _: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        Err(DeviceError::new(DeviceErrorKind::NotFound, format!("no device {}", req.device_id)))
    }
}

/// Names of the hosts compiled in and available on this machine.
pub fn available_hosts() -> Vec<String> {
    #[allow(unused_mut)]
    let mut hosts: Vec<String> = Vec::new();
    #[cfg(feature = "cpal-backend")]
    hosts.extend(cpal_backend::available_hosts());
    #[cfg(all(windows, feature = "wasapi-lowlatency"))]
    hosts.extend([wasapi_lowlat::HOST_LOW_LATENCY.to_string(), wasapi_lowlat::HOST_EXCLUSIVE.to_string()]);
    hosts
}

/// Creates the backend for `host` (`None` = platform default, i.e. CPAL).
pub fn create_backend(host: Option<&str>) -> Result<Arc<dyn AudioBackend>, DeviceError> {
    #[cfg(all(windows, feature = "wasapi-lowlatency"))]
    if let Some(b) = host.and_then(wasapi_lowlat::WasapiBackend::for_host) {
        return Ok(Arc::new(b?));
    }
    #[cfg(feature = "cpal-backend")]
    {
        Ok(Arc::new(cpal_backend::CpalBackend::new(host)?))
    }
    #[cfg(not(feature = "cpal-backend"))]
    {
        let _ = host;
        Ok(Arc::new(NullBackend))
    }
}

// ---------------------------------------------------------------------------
// Virtual endpoints

/// Where virtual endpoints come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualProvider {
    /// NovaMix's own driver.
    NovaMix,
    /// A third-party virtual cable used as a development fallback.
    ThirdParty(String),
}

/// A virtual cable: what is played into `render` comes out of `capture`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CablePair {
    pub provider: VirtualProvider,
    pub label: String,
    pub render: DeviceInfo,
    pub capture: Option<DeviceInfo>,
}

/// Virtual endpoint availability as seen by the platform adapter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualDeviceStatus {
    /// True when the NovaMix driver endpoints are present.
    pub driver_installed: bool,
    /// Render endpoint that applications play into; the mixer captures it
    /// via loopback ("Desktop/Virtual In" channel).
    pub playback: Option<DeviceInfo>,
    /// Render endpoint the mixer writes the voice bus into…
    pub mic_feed: Option<DeviceInfo>,
    /// …which applications record from as a microphone.
    pub mic_capture: Option<DeviceInfo>,
    /// All detected cables (for routing choices and feedback detection).
    pub cables: Vec<CablePair>,
    /// Human-readable remarks (e.g. "using VB-CABLE as development fallback").
    pub notes: Vec<String>,
}

impl VirtualDeviceStatus {
    pub fn has_virtual_mic(&self) -> bool {
        self.mic_feed.is_some()
    }
}

/// Platform adapter for virtual endpoints.
pub trait VirtualDeviceAdapter: Send + Sync {
    fn status(&self, devices: &[DeviceInfo]) -> VirtualDeviceStatus;
}

/// Sample rates probed when building [`DeviceInfo::sample_rates`].
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
