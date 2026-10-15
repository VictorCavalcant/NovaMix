//! NovaMix's own WASAPI implementation of [`AudioBackend`], for low latency.
//!
//! Two hosts are exposed next to CPAL's `wasapi`:
//!
//! * **`wasapi-lowlatency`** — shared mode through
//!   `IAudioClient3::InitializeSharedAudioStream`. The engine period is the
//!   requested buffer size rounded to the driver's fundamental period and
//!   clamped to the range reported by `GetSharedModeEnginePeriod` (the minimum
//!   when nothing is requested). Drivers without small-period support stay at
//!   their default period (usually 10 ms), exactly like CPAL.
//! * **`wasapi-exclusive`** — exclusive mode (the device is reserved for
//!   NovaMix) at the device's minimum period or the requested buffer. A device
//!   that refuses exclusive access (busy, policy, no native format) falls back
//!   to the shared low-latency path. Loopback capture is always shared.
//!
//! Every stream runs on its own event-driven thread registered with MMCSS
//! ("Pro Audio"). COM objects are created and used on that thread only; the
//! opener waits for the initialization result, so errors are synchronous.
//! Samples are converted to/from interleaved `f32` with buffers allocated
//! before the stream starts.
//!
//! Device ids are the same as CPAL's WASAPI host (`wasapi:{endpoint id}`), so
//! profiles keep their devices when the host is switched.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Devices::FunctionDiscovery::{PKEY_Device_DeviceDesc, PKEY_Device_EnumeratorName, PKEY_Device_FriendlyName};
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, PROPERTYKEY, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_ENDPOINT_OFFLOAD_NOT_CAPABLE, AUDCLNT_E_ENGINE_FORMAT_LOCKED, AUDCLNT_E_ENGINE_PERIODICITY_LOCKED,
    AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_INVALID_DEVICE_PERIOD, AUDCLNT_E_RESOURCES_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING,
    AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE, AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, EDataFlow, IAudioCaptureClient, IAudioClient, IAudioClient3, IAudioRenderClient, IMMDevice,
    IMMDeviceEnumerator, IMMEndpoint, MMDeviceEnumerator, PKEY_AudioEndpoint_FormFactor, PKEY_AudioEndpoint_JackSubType, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eAll, eCapture, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_FRONT_CENTER, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent, WaitForMultipleObjects,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::{HRESULT, Interface, PCWSTR, w};

use crate::*;

/// Shared mode through `IAudioClient3` (small engine periods).
pub const HOST_LOW_LATENCY: &str = "wasapi-lowlatency";
/// Exclusive mode (device reserved for NovaMix).
pub const HOST_EXCLUSIVE: &str = "wasapi-exclusive";

/// Same prefix as CPAL's WASAPI device ids.
const ID_PREFIX: &str = "wasapi:";
/// Without an event for this long the stream is probed for device loss.
const PROBE_TIMEOUT_MS: u32 = 500;
/// Upper end of the buffer range reported in exclusive mode.
const EXCLUSIVE_MAX_FRAMES: u32 = 4096;
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`, returned by `GetDevice`.
const E_NOT_FOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);
/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`.
const E_FILE_NOT_FOUND: HRESULT = HRESULT(0x8007_0002_u32 as i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareMode {
    Shared,
    Exclusive,
}

/// The `wasapi-lowlatency` / `wasapi-exclusive` hosts.
pub struct WasapiBackend {
    mode: ShareMode,
    /// Exclusive-mode capabilities per device id (probing is slow on some
    /// drivers and enumeration is repeated every few seconds).
    exclusive_caps: Mutex<HashMap<String, ExclusiveCaps>>,
    /// Devices whose exclusive mode failed the pace check: opened shared.
    exclusive_refused: Arc<Mutex<HashSet<String>>>,
}

#[derive(Debug, Clone)]
struct ExclusiveCaps {
    rates: Vec<u32>,
    format: String,
    min_frames: u32,
}

impl WasapiBackend {
    pub fn new(mode: ShareMode) -> Result<Self, DeviceError> {
        let _com = ComGuard::init();
        enumerator()?;
        Ok(Self { mode, exclusive_caps: Mutex::new(HashMap::new()), exclusive_refused: Arc::new(Mutex::new(HashSet::new())) })
    }

    /// Backend for a host name handled here, if it is one.
    pub fn for_host(name: &str) -> Option<Result<Self, DeviceError>> {
        if name.eq_ignore_ascii_case(HOST_LOW_LATENCY) {
            Some(Self::new(ShareMode::Shared))
        } else if name.eq_ignore_ascii_case(HOST_EXCLUSIVE) {
            Some(Self::new(ShareMode::Exclusive))
        } else {
            None
        }
    }

    fn describe(&self, dev: &IMMDevice, default_in: Option<&str>, default_out: Option<&str>) -> Option<DeviceInfo> {
        let raw_id = endpoint_id(dev)?;
        let direction = data_flow(dev).ok()?;
        // SAFETY: COM calls on a live endpoint; returned values are owned.
        let store = unsafe { dev.OpenPropertyStore(STGM_READ) }.ok()?;
        let name = prop_string(&store, &PKEY_Device_FriendlyName).or_else(|| prop_string(&store, &PKEY_Device_DeviceDesc))?;
        let (form, interface) = classify(&store);
        let client: IAudioClient = activate(dev).ok()?;
        let mix = mix_format(&client).ok()?;
        let id = format!("{ID_PREFIX}{raw_id}");
        let (sample_rates, buffer_range, sample_format) = match self.mode {
            ShareMode::Shared => (vec![mix.rate], shared_period_range(&client, &mix), SampleKind::F32.name().to_string()),
            ShareMode::Exclusive => {
                let caps = self.exclusive_caps(&id, &client, &mix);
                let rates = if caps.rates.is_empty() { vec![mix.rate] } else { caps.rates };
                (rates, Some((caps.min_frames, EXCLUSIVE_MAX_FRAMES.max(caps.min_frames))), caps.format)
            }
        };
        let is_default = match direction {
            Direction::Input => default_in == Some(raw_id.as_str()),
            Direction::Output => default_out == Some(raw_id.as_str()),
        };
        Some(DeviceInfo {
            id,
            name,
            direction,
            is_default,
            channels: mix.channels,
            default_sample_rate: mix.rate,
            sample_rates,
            buffer_range,
            sample_format,
            form,
            is_virtual: interface == "virtual" || form == DeviceForm::Virtual,
            interface: interface.to_string(),
            manufacturer: None,
        })
    }

    fn exclusive_caps(&self, id: &str, client: &IAudioClient, mix: &Format) -> ExclusiveCaps {
        if let Some(c) = self.exclusive_caps.lock().ok().and_then(|m| m.get(id).cloned()) {
            return c;
        }
        let mut rates = Vec::new();
        let mut format = None;
        for r in STANDARD_RATES {
            if let Some(f) = exclusive_format(client, mix, r) {
                rates.push(r);
                if r == mix.rate || format.is_none() {
                    format = Some(f.kind);
                }
            }
        }
        let caps = ExclusiveCaps {
            rates,
            format: format.unwrap_or(mix.kind).name().to_string(),
            min_frames: device_periods(client).map(|(_, min)| hns_to_frames(min, mix.rate)).unwrap_or(mix.rate / 100),
        };
        if let Ok(mut m) = self.exclusive_caps.lock() {
            m.insert(id.to_string(), caps.clone());
        }
        caps
    }

    fn open(&self, req: &StreamRequest, direction: Direction, io: Callback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        let stop = Event::new(true)?;
        let (ready_tx, ready_rx) = sync_channel(1);
        let refused = self.exclusive_refused.clone();
        let job = Job { mode: self.mode, req: req.clone(), direction, io, error, stop: stop.0.0 as usize, ready: ready_tx, refused };
        let thread = std::thread::Builder::new()
            .name(if direction == Direction::Input { "novamix-wasapi-in" } else { "novamix-wasapi-out" }.into())
            .spawn(move || job.run())
            .map_err(|e| DeviceError::new(DeviceErrorKind::Backend, format!("cannot spawn stream thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(info)) => Ok(Box::new(WasapiStream { info, stop, thread: Some(thread) })),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(DeviceError::new(DeviceErrorKind::Backend, "stream thread exited during initialization"))
            }
        }
    }
}

impl AudioBackend for WasapiBackend {
    fn host_name(&self) -> String {
        match self.mode {
            ShareMode::Shared => HOST_LOW_LATENCY.into(),
            ShareMode::Exclusive => HOST_EXCLUSIVE.into(),
        }
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let _com = ComGuard::init();
        let en = enumerator()?;
        let default_in = default_endpoint(&en, eCapture);
        let default_out = default_endpoint(&en, eRender);
        // SAFETY: COM calls on a live enumerator; each item is an owned reference.
        let list = unsafe { en.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE) }.map_err(|e| hr_error(&e, "enumerating endpoints"))?;
        let count = unsafe { list.GetCount() }.map_err(|e| hr_error(&e, "enumerating endpoints"))?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Ok(dev) = unsafe { list.Item(i) }
                && let Some(info) = self.describe(&dev, default_in.as_deref(), default_out.as_deref())
            {
                out.push(info);
            }
        }
        Ok(out)
    }

    fn open_input(&self, req: &StreamRequest, data: InputCallback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        self.open(req, Direction::Input, Callback::Input(data), error)
    }

    fn open_output(&self, req: &StreamRequest, data: OutputCallback, error: ErrorCallback) -> Result<Box<dyn StreamHandle>, DeviceError> {
        self.open(req, Direction::Output, Callback::Output(data), error)
    }

    fn period_hint(&self, req: &StreamRequest, _direction: Direction) -> Option<u32> {
        let _com = ComGuard::init();
        let en = enumerator().ok()?;
        let dev = find_device(&en, &req.device_id).ok()?;
        let client: IAudioClient = activate(&dev).ok()?;
        let mix = mix_format(&client).ok()?;
        let refused = self.exclusive_refused.lock().map(|r| r.contains(&req.device_id)).unwrap_or(false);
        if self.mode == ShareMode::Exclusive && !req.loopback && !refused {
            let (_, min) = device_periods(&client).ok()?;
            let min = hns_to_frames(min, mix.rate);
            return Some(req.buffer_frames.unwrap_or(min).max(min));
        }
        let client3: IAudioClient3 = client.cast().ok()?;
        if req.loopback {
            return current_period(&client3);
        }
        let periods = shared_periods(&client3, &Format { kind: SampleKind::F32, valid_bits: 32, ..mix }).ok()?;
        Some(periods.choose(req.buffer_frames))
    }
}

// ---------------------------------------------------------------------------
// Stream thread

enum Callback {
    Input(InputCallback),
    Output(OutputCallback),
}

struct Job {
    mode: ShareMode,
    req: StreamRequest,
    direction: Direction,
    io: Callback,
    error: ErrorCallback,
    /// Raw stop event (owned by the [`WasapiStream`], which outlives the thread).
    stop: usize,
    ready: SyncSender<Result<StreamInfo, DeviceError>>,
    refused: Arc<Mutex<HashSet<String>>>,
}

impl Job {
    fn run(self) {
        let Job { mode, req, direction, io, mut error, stop, ready, refused } = self;
        let _com = ComGuard::init();
        let mode = if refused.lock().map(|r| r.contains(&req.device_id)).unwrap_or(false) { ShareMode::Shared } else { mode };
        let mut stream = match Stream::open(mode, &req, direction, io) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let _mmcss = Mmcss::register();
        if let Err(e) = stream.start() {
            let _ = ready.send(Err(e));
            return;
        }
        tracing::info!(
            "{}: '{}' {} {} Hz {} ch {}, period {} frames, buffer {} frames",
            if stream.exclusive { HOST_EXCLUSIVE } else { HOST_LOW_LATENCY },
            stream.name,
            if req.loopback {
                "loopback"
            } else if direction == Direction::Input {
                "capture"
            } else {
                "render"
            },
            stream.format.rate,
            stream.format.channels,
            stream.format.kind.name(),
            stream.period_frames,
            stream.buffer_frames
        );
        let info = StreamInfo {
            device_id: req.device_id.clone(),
            device_name: stream.name.clone(),
            direction,
            loopback: req.loopback,
            sample_rate: stream.format.rate,
            channels: stream.format.channels,
            buffer_frames: Some(stream.period_frames),
            sample_format: stream.format.kind.name().to_string(),
        };
        if ready.send(Ok(info)).is_err() {
            return;
        }
        if let Err(e) = stream.run(HANDLE(stop as *mut _), &mut error) {
            if stream.exclusive && e.kind == DeviceErrorKind::Unsupported {
                tracing::warn!("{HOST_EXCLUSIVE}: '{}': {e}", stream.name);
                if let Ok(mut r) = refused.lock() {
                    r.insert(req.device_id.clone());
                }
            }
            error(e);
        }
        // SAFETY: stopping a started client on its own thread.
        unsafe {
            let _ = stream.client.Stop();
        }
    }
}

enum Io {
    Capture { client: IAudioCaptureClient, cb: InputCallback },
    Render { client: IAudioRenderClient, cb: OutputCallback },
}

struct Stream {
    client: IAudioClient,
    io: Io,
    event: Event,
    format: Format,
    period_frames: u32,
    buffer_frames: u32,
    exclusive: bool,
    name: String,
    /// Conversion space for one full buffer (allocated before starting).
    scratch: Vec<f32>,
    first_packet: bool,
    /// Exclusive streams only, until their pace is verified.
    pace: Option<PaceCheck>,
}

impl Stream {
    fn open(mode: ShareMode, req: &StreamRequest, direction: Direction, io: Callback) -> Result<Self, DeviceError> {
        let en = enumerator()?;
        let dev = find_device(&en, &req.device_id)?;
        let flow = data_flow(&dev)?;
        let expected = if direction == Direction::Output || req.loopback { Direction::Output } else { Direction::Input };
        if flow != expected {
            let msg = if req.loopback { "loopback needs an output device" } else { "device has the wrong direction" };
            return Err(DeviceError::new(DeviceErrorKind::Unsupported, msg));
        }
        // SAFETY: property store of a live endpoint.
        let name = unsafe { dev.OpenPropertyStore(STGM_READ) }
            .ok()
            .and_then(|s| prop_string(&s, &PKEY_Device_FriendlyName))
            .unwrap_or_else(|| req.device_id.clone());

        let mut init = None;
        if mode == ShareMode::Exclusive && !req.loopback {
            match init_exclusive(&dev, req) {
                Ok(i) => init = Some(i),
                Err(e) if matches!(e.kind, DeviceErrorKind::Busy | DeviceErrorKind::Unsupported | DeviceErrorKind::PermissionDenied) => {
                    tracing::warn!("{HOST_EXCLUSIVE}: '{name}' refused exclusive mode ({e}); using shared low-latency mode");
                }
                Err(e) => return Err(e),
            }
        }
        let init = match init {
            Some(i) => i,
            None => init_shared(&dev, req)?,
        };

        let event = Event::new(false)?;
        // SAFETY: COM calls on an initialized client.
        unsafe {
            init.client.SetEventHandle(event.0).map_err(|e| hr_error(&e, "SetEventHandle"))?;
        }
        let buffer_frames = unsafe { init.client.GetBufferSize() }.map_err(|e| hr_error(&e, "GetBufferSize"))?;
        let io = match io {
            Callback::Input(cb) => Io::Capture { client: unsafe { init.client.GetService() }.map_err(|e| hr_error(&e, "capture client"))?, cb },
            Callback::Output(cb) => Io::Render { client: unsafe { init.client.GetService() }.map_err(|e| hr_error(&e, "render client"))?, cb },
        };
        let scratch = vec![0.0f32; buffer_frames.max(init.period_frames).max(64) as usize * init.format.channels as usize];
        Ok(Self {
            client: init.client,
            io,
            event,
            format: init.format,
            period_frames: init.period_frames,
            buffer_frames,
            exclusive: init.exclusive,
            name,
            scratch,
            first_packet: true,
            pace: init.exclusive.then(PaceCheck::new),
        })
    }

    /// Frames kept queued in a shared render buffer: two periods, so one
    /// late wake-up is absorbed without adding the whole buffer as latency.
    fn render_target(&self) -> u32 {
        if self.exclusive {
            self.buffer_frames
        } else {
            (self.period_frames * 2).clamp(self.period_frames.min(self.buffer_frames), self.buffer_frames)
        }
    }

    fn start(&mut self) -> Result<(), DeviceError> {
        if let Io::Render { client, .. } = &self.io {
            // Pre-roll one period (exclusive: one buffer) of silence.
            let frames = if self.exclusive { self.buffer_frames } else { self.period_frames.min(self.buffer_frames) };
            // SAFETY: requesting no more than the buffer size before Start.
            unsafe {
                client.GetBuffer(frames).map_err(|e| hr_error(&e, "render pre-roll"))?;
                client.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32).map_err(|e| hr_error(&e, "render pre-roll"))?;
            }
        }
        // SAFETY: starting an initialized client.
        unsafe { self.client.Start() }.map_err(|e| hr_error(&e, "Start"))
    }

    fn run(&mut self, stop: HANDLE, error: &mut ErrorCallback) -> Result<(), DeviceError> {
        let handles = [stop, self.event.0];
        loop {
            // SAFETY: both handles stay valid while the thread runs.
            let w = unsafe { WaitForMultipleObjects(&handles, false, PROBE_TIMEOUT_MS) };
            if w == WAIT_OBJECT_0 {
                return Ok(());
            } else if w == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                self.service(error)?;
            } else if w == WAIT_TIMEOUT {
                // Loopback of an idle output and stalled devices end up here;
                // an invalidated device shows up as an error on any call.
                // SAFETY: query on an initialized client.
                unsafe { self.client.GetCurrentPadding() }.map_err(|e| hr_error(&e, "stream"))?;
            } else if w == WAIT_FAILED {
                return Err(DeviceError::new(DeviceErrorKind::Backend, format!("wait failed: {}", windows::core::Error::from_thread())));
            }
        }
    }

    fn service(&mut self, error: &mut ErrorCallback) -> Result<(), DeviceError> {
        let channels = self.format.channels as usize;
        let kind = self.format.kind;
        let frame_bytes = self.format.frame_bytes();
        let chunk_frames = self.scratch.len() / channels;
        let target = self.render_target();
        let exclusive = self.exclusive;
        match &mut self.io {
            Io::Capture { client, cb } => loop {
                // SAFETY: standard capture packet loop; the buffer returned by
                // GetBuffer is valid for `frames` frames until ReleaseBuffer.
                // Exclusive event-driven streams deliver exactly one packet per
                // event (GetNextPacketSize would hand out the half-filled one).
                if !exclusive && unsafe { client.GetNextPacketSize() }.map_err(|e| hr_error(&e, "capture"))? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                unsafe { client.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.map_err(|e| hr_error(&e, "capture"))?;
                if frames == 0 {
                    // AUDCLNT_S_BUFFER_EMPTY: nothing acquired, nothing to release.
                    return Ok(());
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                let mut done = 0usize;
                while done < frames as usize {
                    let n = (frames as usize - done).min(chunk_frames);
                    let dst = &mut self.scratch[..n * channels];
                    if silent {
                        dst.fill(0.0);
                    } else {
                        let src = unsafe { std::slice::from_raw_parts(data.add(done * frame_bytes), n * frame_bytes) };
                        read_samples(kind, src, dst);
                    }
                    cb(dst, channels);
                    done += n;
                }
                unsafe { client.ReleaseBuffer(frames) }.map_err(|e| hr_error(&e, "capture"))?;
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 && !self.first_packet {
                    error(DeviceError::new(DeviceErrorKind::Xrun, "capture discontinuity"));
                }
                self.first_packet = false;
                if let Some(p) = &mut self.pace
                    && p.check(frames, self.format.rate, self.period_frames)?
                {
                    self.pace = None;
                }
                if exclusive {
                    return Ok(());
                }
            },
            Io::Render { client, cb } => {
                let padding = if self.exclusive {
                    0
                } else {
                    // SAFETY: query on an initialized client.
                    unsafe { self.client.GetCurrentPadding() }.map_err(|e| hr_error(&e, "render"))?
                };
                let frames = target.saturating_sub(padding);
                if frames == 0 {
                    return Ok(());
                }
                // SAFETY: `frames` never exceeds the free space of the buffer;
                // the pointer is valid for `frames` frames until ReleaseBuffer.
                let data = unsafe { client.GetBuffer(frames) }.map_err(|e| hr_error(&e, "render"))?;
                let mut done = 0usize;
                while done < frames as usize {
                    let n = (frames as usize - done).min(chunk_frames);
                    let src = &mut self.scratch[..n * channels];
                    cb(src, channels);
                    let dst = unsafe { std::slice::from_raw_parts_mut(data.add(done * frame_bytes), n * frame_bytes) };
                    write_samples(kind, src, dst);
                    done += n;
                }
                unsafe { client.ReleaseBuffer(frames, 0) }.map_err(|e| hr_error(&e, "render"))?;
                if let Some(p) = &mut self.pace
                    && p.check(frames, self.format.rate, self.period_frames)?
                {
                    self.pace = None;
                }
                Ok(())
            }
        }
    }
}

/// Verifies that an exclusive-mode stream moves `rate` frames per second,
/// one period per event. Some drivers (seen with virtual cables) signal
/// exclusive-mode events on the system timer instead, so data comes in bursts
/// or too slowly and the bridges, sized for one period, starve. Such devices
/// are reopened in shared mode.
struct PaceCheck {
    start: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
    frames: u64,
    events: u32,
    late: u32,
}

impl PaceCheck {
    /// Events ignored while the stream settles.
    const SETTLE_EVENTS: u32 = 50;
    const WINDOW_SECS: f64 = 1.0;
    /// Allowed deviation of the frame rate.
    const RATE_TOLERANCE: f64 = 0.1;
    /// An event is late when it comes more than this many periods after the previous one…
    const LATE_PERIODS: f64 = 2.0;
    /// …and the stream is bursty when more than this fraction of events is late.
    const MAX_LATE_FRACTION: f64 = 0.05;

    fn new() -> Self {
        Self { start: None, last: None, frames: 0, events: 0, late: 0 }
    }

    /// `Ok(true)` once the stream has proved its pace.
    fn check(&mut self, frames: u32, rate: u32, period: u32) -> Result<bool, DeviceError> {
        let now = std::time::Instant::now();
        let last = self.last.replace(now);
        self.events += 1;
        let Some(start) = self.start else {
            if self.events >= Self::SETTLE_EVENTS {
                self.start = Some(now);
                self.events = 0;
            }
            return Ok(false);
        };
        self.frames += frames as u64;
        if let Some(last) = last
            && now.duration_since(last).as_secs_f64() > Self::LATE_PERIODS * period as f64 / rate as f64
        {
            self.late += 1;
        }
        let secs = now.duration_since(start).as_secs_f64();
        if secs < Self::WINDOW_SECS {
            return Ok(false);
        }
        let actual = self.frames as f64 / secs;
        let late = self.late as f64 / self.events.max(1) as f64;
        if (actual / rate as f64 - 1.0).abs() > Self::RATE_TOLERANCE || late > Self::MAX_LATE_FRACTION {
            return Err(DeviceError::new(
                DeviceErrorKind::Unsupported,
                format!(
                    "exclusive mode is not paced by the period ({actual:.0} frames/s for {rate} Hz, {:.0}% late events); reopening in shared mode",
                    late * 100.0
                ),
            ));
        }
        Ok(true)
    }
}

/// A running stream; dropping it stops and joins the thread.
struct WasapiStream {
    info: StreamInfo,
    stop: Event,
    thread: Option<JoinHandle<()>>,
}

impl StreamHandle for WasapiStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        // SAFETY: the event is owned by `self` and still open.
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization

struct Init {
    client: IAudioClient,
    format: Format,
    period_frames: u32,
    exclusive: bool,
}

/// Shared-mode engine periods of an endpoint, in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SharedPeriods {
    default: u32,
    fundamental: u32,
    min: u32,
    max: u32,
}

impl SharedPeriods {
    /// The requested size rounded up to the fundamental period and clamped
    /// to the supported range; the minimum when nothing is requested.
    fn choose(&self, requested: Option<u32>) -> u32 {
        let f = self.fundamental.max(1);
        let want = requested.unwrap_or(self.min);
        (want.div_ceil(f) * f).clamp(self.min, self.max.max(self.min))
    }
}

fn shared_periods(client: &IAudioClient3, format: &Format) -> Result<SharedPeriods, DeviceError> {
    let wave = format.to_wave();
    let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
    // SAFETY: `wave` outlives the call; outputs are plain integers.
    unsafe { client.GetSharedModeEnginePeriod(&wave.Format, &mut default, &mut fundamental, &mut min, &mut max) }
        .map_err(|e| hr_error(&e, "GetSharedModeEnginePeriod"))?;
    Ok(SharedPeriods { default, fundamental, min, max })
}

fn shared_period_range(client: &IAudioClient, mix: &Format) -> Option<(u32, u32)> {
    let f32_format = Format { kind: SampleKind::F32, valid_bits: 32, ..*mix };
    match client.cast::<IAudioClient3>().ok().and_then(|c| shared_periods(&c, &f32_format).ok()) {
        Some(p) => Some((p.min, p.max.max(p.min))),
        None => device_periods(client).ok().map(|(d, _)| hns_to_frames(d, mix.rate)).map(|d| (d, d)),
    }
}

/// Period the shared engine of this endpoint is running at right now.
fn current_period(client: &IAudioClient3) -> Option<u32> {
    let mut fmt: *mut WAVEFORMATEX = std::ptr::null_mut();
    let mut frames = 0u32;
    // SAFETY: the returned format is CoTaskMem-allocated and freed here.
    unsafe {
        let r = client.GetCurrentSharedModeEnginePeriod(&mut fmt, &mut frames);
        if !fmt.is_null() {
            CoTaskMemFree(Some(fmt as *const _));
        }
        r.ok()?;
    }
    (frames > 0).then_some(frames)
}

fn init_shared(dev: &IMMDevice, req: &StreamRequest) -> Result<Init, DeviceError> {
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | if req.loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    let client: IAudioClient = activate(dev)?;
    let mix = mix_format(&client)?;
    // The shared engine mixes in float: ask for f32 at the mix rate/layout.
    let f32_format = Format { kind: SampleKind::F32, valid_bits: 32, ..mix };
    let format = if is_supported(&client, AUDCLNT_SHAREMODE_SHARED, &f32_format) { f32_format } else { mix };
    let wave = format.to_wave();

    if let Ok(client3) = client.cast::<IAudioClient3>() {
        let periods = shared_periods(&client3, &format);
        let wanted = match (&periods, req.loopback) {
            // Loopback packets follow the render engine: join its period.
            (_, true) => current_period(&client3),
            (Ok(p), false) => Some(p.choose(req.buffer_frames)),
            (Err(_), false) => None,
        };
        if let Some(period) = wanted {
            // SAFETY: `wave` outlives the call.
            match unsafe { client3.InitializeSharedAudioStream(flags, period, &wave.Format, None) } {
                Ok(()) => {
                    let actual = current_period(&client3).unwrap_or(period);
                    return Ok(Init { client, format, period_frames: actual, exclusive: false });
                }
                Err(e) if e.code() == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED => {
                    // Another low-latency client fixed the engine period: join it.
                    if let Some(cur) = current_period(&client3)
                        && let Ok(c3) = activate::<IAudioClient3>(dev)
                        && unsafe { c3.InitializeSharedAudioStream(flags, cur, &wave.Format, None) }.is_ok()
                    {
                        return Ok(Init {
                            client: c3.cast().map_err(|e| hr_error(&e, "IAudioClient"))?,
                            format,
                            period_frames: cur,
                            exclusive: false,
                        });
                    }
                }
                Err(e) => tracing::debug!("InitializeSharedAudioStream({period}) failed: {e}; using the default period"),
            }
        }
    }

    // Classic shared stream at the default engine period.
    let client: IAudioClient = activate(dev)?;
    let (default_hns, _) = device_periods(&client)?;
    // Capture may hold a few periods without adding latency (it is drained
    // on every event); render keeps the minimum buffer.
    let buffer_hns = if req.loopback || !matches!(data_flow(dev), Ok(Direction::Output)) { default_hns * 4 } else { 0 };
    // SAFETY: `wave` outlives the call.
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer_hns, 0, &wave.Format, None) }
        .map_err(|e| hr_error(&e, "Initialize (shared)"))?;
    let period_frames =
        client.cast::<IAudioClient3>().ok().and_then(|c| current_period(&c)).unwrap_or_else(|| hns_to_frames(default_hns, format.rate));
    Ok(Init { client, format, period_frames, exclusive: false })
}

fn init_exclusive(dev: &IMMDevice, req: &StreamRequest) -> Result<Init, DeviceError> {
    let client: IAudioClient = activate(dev)?;
    let mix = mix_format(&client)?;
    let format = req
        .sample_rate
        .filter(|r| *r != mix.rate)
        .and_then(|r| exclusive_format(&client, &mix, r))
        .or_else(|| exclusive_format(&client, &mix, mix.rate))
        .ok_or_else(|| DeviceError::new(DeviceErrorKind::Unsupported, "no exclusive-mode format accepted by the device"))?;
    let wave = format.to_wave();
    let (_, min_hns) = device_periods(&client)?;
    let wanted = req.buffer_frames.map(|f| frames_to_hns(f, format.rate)).unwrap_or(min_hns).max(min_hns);
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    // SAFETY: `wave` outlives the calls.
    let client = match unsafe { client.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, flags, wanted, wanted, &wave.Format, None) } {
        Ok(()) => client,
        Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            // Retry with the aligned size the driver proposes (needs a new client).
            let aligned = unsafe { client.GetBufferSize() }.map_err(|e| hr_error(&e, "GetBufferSize"))?;
            let hns = frames_to_hns(aligned, format.rate);
            let client: IAudioClient = activate(dev)?;
            unsafe { client.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, flags, hns, hns, &wave.Format, None) }
                .map_err(|e| hr_error(&e, "Initialize (exclusive, aligned)"))?;
            client
        }
        Err(e) => return Err(hr_error(&e, "Initialize (exclusive)")),
    };
    let period_frames = unsafe { client.GetBufferSize() }.map_err(|e| hr_error(&e, "GetBufferSize"))?;
    Ok(Init { client, format, period_frames, exclusive: true })
}

/// First native exclusive-mode format at `rate`, best quality first.
fn exclusive_format(client: &IAudioClient, mix: &Format, rate: u32) -> Option<Format> {
    [(SampleKind::F32, 32), (SampleKind::I32, 32), (SampleKind::I32, 24), (SampleKind::I24, 24), (SampleKind::I16, 16)]
        .into_iter()
        .map(|(kind, valid_bits)| Format { kind, valid_bits, rate, ..*mix })
        .find(|f| is_supported(client, AUDCLNT_SHAREMODE_EXCLUSIVE, f))
}

fn is_supported(client: &IAudioClient, mode: AUDCLNT_SHAREMODE, format: &Format) -> bool {
    let wave = format.to_wave();
    let mut closest: *mut WAVEFORMATEX = std::ptr::null_mut();
    let closest_arg = if mode == AUDCLNT_SHAREMODE_SHARED { Some(&mut closest as *mut _) } else { None };
    // SAFETY: `wave` outlives the call; a proposed closest match is freed.
    unsafe {
        let hr = client.IsFormatSupported(mode, &wave.Format, closest_arg);
        if !closest.is_null() {
            CoTaskMemFree(Some(closest as *const _));
        }
        hr == HRESULT(0)
    }
}

/// (default, minimum) device periods in 100 ns units.
fn device_periods(client: &IAudioClient) -> Result<(i64, i64), DeviceError> {
    let (mut default, mut min) = (0i64, 0i64);
    // SAFETY: outputs are plain integers.
    unsafe { client.GetDevicePeriod(Some(&mut default), Some(&mut min)) }.map_err(|e| hr_error(&e, "GetDevicePeriod"))?;
    Ok((default, min))
}

fn hns_to_frames(hns: i64, rate: u32) -> u32 {
    ((hns as f64 * rate as f64 / 10_000_000.0) + 0.5) as u32
}

fn frames_to_hns(frames: u32, rate: u32) -> i64 {
    (10_000_000.0 * frames as f64 / rate as f64 + 0.5) as i64
}

// ---------------------------------------------------------------------------
// Formats

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleKind {
    F32,
    I16,
    /// Packed 24-bit.
    I24,
    /// 32-bit container (also 24 valid bits, left-justified).
    I32,
}

impl SampleKind {
    fn bytes(self) -> usize {
        match self {
            SampleKind::I16 => 2,
            SampleKind::I24 => 3,
            SampleKind::F32 | SampleKind::I32 => 4,
        }
    }

    fn name(self) -> &'static str {
        match self {
            SampleKind::F32 => "f32",
            SampleKind::I16 => "i16",
            SampleKind::I24 => "i24",
            SampleKind::I32 => "i32",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Format {
    kind: SampleKind,
    valid_bits: u16,
    channels: u16,
    rate: u32,
    mask: u32,
}

impl Format {
    fn frame_bytes(&self) -> usize {
        self.kind.bytes() * self.channels as usize
    }

    fn to_wave(self) -> WAVEFORMATEXTENSIBLE {
        let block = self.frame_bytes() as u16;
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: self.channels,
                nSamplesPerSec: self.rate,
                nAvgBytesPerSec: self.rate * block as u32,
                nBlockAlign: block,
                wBitsPerSample: (self.kind.bytes() * 8) as u16,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: self.valid_bits },
            dwChannelMask: self.mask,
            SubFormat: if self.kind == SampleKind::F32 { KSDATAFORMAT_SUBTYPE_IEEE_FLOAT } else { KSDATAFORMAT_SUBTYPE_PCM },
        }
    }

    /// Parses a format returned by WASAPI.
    ///
    /// # Safety
    /// `p` must point to a valid `WAVEFORMATEX` (followed by the extensible
    /// part when `cbSize` says so).
    unsafe fn from_wave(p: *const WAVEFORMATEX) -> Option<Format> {
        // The structs are packed: read them unaligned, by value.
        let w = unsafe { p.read_unaligned() };
        let (sub, valid, mask) = if w.wFormatTag as u32 == WAVE_FORMAT_EXTENSIBLE && w.cbSize >= 22 {
            let x = unsafe { (p as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
            (x.SubFormat, unsafe { x.Samples.wValidBitsPerSample }, x.dwChannelMask)
        } else if w.wFormatTag as u32 == WAVE_FORMAT_IEEE_FLOAT {
            (KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, w.wBitsPerSample, 0)
        } else {
            (KSDATAFORMAT_SUBTYPE_PCM, w.wBitsPerSample, 0)
        };
        let kind = match (sub, w.wBitsPerSample) {
            (s, 32) if s == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT => SampleKind::F32,
            (s, 16) if s == KSDATAFORMAT_SUBTYPE_PCM => SampleKind::I16,
            (s, 24) if s == KSDATAFORMAT_SUBTYPE_PCM => SampleKind::I24,
            (s, 32) if s == KSDATAFORMAT_SUBTYPE_PCM => SampleKind::I32,
            _ => return None,
        };
        let channels = w.nChannels;
        let mask = if mask != 0 { mask } else { default_mask(channels) };
        Some(Format { kind, valid_bits: if valid == 0 { w.wBitsPerSample } else { valid }, channels, rate: w.nSamplesPerSec, mask })
    }
}

fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => SPEAKER_FRONT_CENTER,
        n if n < 32 => (1u32 << n) - 1,
        _ => 0,
    }
}

fn mix_format(client: &IAudioClient) -> Result<Format, DeviceError> {
    // SAFETY: the mix format is CoTaskMem-allocated and freed after parsing.
    unsafe {
        let p = client.GetMixFormat().map_err(|e| hr_error(&e, "GetMixFormat"))?;
        let f = Format::from_wave(p);
        CoTaskMemFree(Some(p as *const _));
        f.ok_or_else(|| DeviceError::new(DeviceErrorKind::Unsupported, "unsupported mix format"))
    }
}

/// Device bytes → interleaved f32 (`dst.len()` samples).
fn read_samples(kind: SampleKind, src: &[u8], dst: &mut [f32]) {
    match kind {
        SampleKind::F32 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
        }
        SampleKind::I16 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<2>().0) {
                *d = i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0;
            }
        }
        SampleKind::I24 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<3>().0) {
                *d = (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0;
            }
        }
        SampleKind::I32 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = (i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64 / 2_147_483_648.0) as f32;
            }
        }
    }
}

/// Interleaved f32 → device bytes (clamped to full scale).
fn write_samples(kind: SampleKind, src: &[f32], dst: &mut [u8]) {
    match kind {
        SampleKind::F32 => {
            for (s, c) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                c.copy_from_slice(&s.to_le_bytes());
            }
        }
        SampleKind::I16 => {
            for (s, c) in src.iter().zip(dst.as_chunks_mut::<2>().0) {
                c.copy_from_slice(&((s.clamp(-1.0, 1.0) * 32_767.0).round() as i16).to_le_bytes());
            }
        }
        SampleKind::I24 => {
            for (s, c) in src.iter().zip(dst.as_chunks_mut::<3>().0) {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                c.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        SampleKind::I32 => {
            for (s, c) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0).round() as i32;
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoints and properties

fn enumerator() -> Result<IMMDeviceEnumerator, DeviceError> {
    // SAFETY: standard COM activation on a COM-initialized thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| hr_error(&e, "MMDeviceEnumerator"))
}

fn activate<T: Interface>(dev: &IMMDevice) -> Result<T, DeviceError> {
    // SAFETY: activation of an audio interface on a live endpoint.
    unsafe { dev.Activate(CLSCTX_ALL, None) }.map_err(|e| hr_error(&e, "Activate"))
}

fn endpoint_id(dev: &IMMDevice) -> Option<String> {
    // SAFETY: the id string is CoTaskMem-allocated and freed after copying.
    unsafe {
        let p = dev.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn default_endpoint(en: &IMMDeviceEnumerator, flow: EDataFlow) -> Option<String> {
    // SAFETY: COM call on a live enumerator.
    unsafe { en.GetDefaultAudioEndpoint(flow, eConsole) }.ok().and_then(|d| endpoint_id(&d))
}

fn find_device(en: &IMMDeviceEnumerator, id: &str) -> Result<IMMDevice, DeviceError> {
    let raw = id.strip_prefix(ID_PREFIX).ok_or_else(|| DeviceError::new(DeviceErrorKind::NotFound, format!("not a WASAPI device id: {id}")))?;
    let wide: Vec<u16> = raw.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let dev = unsafe { en.GetDevice(PCWSTR(wide.as_ptr())) }.map_err(|e| hr_error(&e, "device"))?;
    let state = unsafe { dev.GetState() }.map_err(|e| hr_error(&e, "device state"))?;
    if state != DEVICE_STATE_ACTIVE {
        return Err(DeviceError::new(DeviceErrorKind::Disconnected, format!("device not active: {id}")));
    }
    Ok(dev)
}

fn data_flow(dev: &IMMDevice) -> Result<Direction, DeviceError> {
    let ep: IMMEndpoint = dev.cast().map_err(|e| hr_error(&e, "IMMEndpoint"))?;
    // SAFETY: COM call on a live endpoint.
    let flow = unsafe { ep.GetDataFlow() }.map_err(|e| hr_error(&e, "GetDataFlow"))?;
    Ok(if flow == eCapture { Direction::Input } else { Direction::Output })
}

fn prop_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    // SAFETY: the PROPVARIANT is owned and cleared on drop.
    let v = unsafe { store.GetValue(key) }.ok()?;
    if v.is_empty() {
        return None;
    }
    let s = v.to_string();
    (!s.is_empty()).then_some(s)
}

fn prop_u32(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<u32> {
    // SAFETY: the PROPVARIANT is owned and cleared on drop.
    let v = unsafe { store.GetValue(key) }.ok()?;
    u32::try_from(&v).ok()
}

/// Form and interface, mapped the same way as CPAL's WASAPI host.
fn classify(store: &IPropertyStore) -> (DeviceForm, &'static str) {
    let (form, mut interface) = match prop_u32(store, &PKEY_AudioEndpoint_FormFactor) {
        Some(0) => (DeviceForm::Unknown, "network"),
        Some(1) => (DeviceForm::Speakers, "unknown"),
        Some(2) => (DeviceForm::Unknown, "line"),
        Some(3) => (DeviceForm::Headphones, "unknown"),
        Some(4) => (DeviceForm::Microphone, "unknown"),
        Some(5 | 6) => (DeviceForm::Headset, "unknown"),
        Some(8) => (DeviceForm::Unknown, "spdif"),
        Some(9) => (DeviceForm::Unknown, "hdmi"),
        _ => (DeviceForm::Unknown, "unknown"),
    };
    if let Some(e) = prop_string(store, &PKEY_Device_EnumeratorName) {
        interface = match e.to_ascii_uppercase().as_str() {
            "HDAUDIO" => "builtin",
            "USB" => "usb",
            "BTHENUM" => "bluetooth",
            "MMDEVAPI" | "SW" => "virtual",
            _ => interface,
        };
    }
    if let Some(j) = prop_string(store, &PKEY_AudioEndpoint_JackSubType) {
        interface = match j.to_ascii_uppercase().as_str() {
            "{D9E55EA0-0C89-4692-84FF-EB3C4B0D172F}" => "hdmi",
            "{E47E4031-3EA6-418D-8F9B-B73843CCB2AD}" => "displayport",
            "{DFF21CE1-F70F-11D0-B917-00A0C9223196}" => "spdif",
            _ => interface,
        };
    }
    (form, interface)
}

fn hr_error(e: &windows::core::Error, context: &str) -> DeviceError {
    let code = e.code();
    let kind = if code == AUDCLNT_E_DEVICE_INVALIDATED || code == AUDCLNT_E_SERVICE_NOT_RUNNING || code == AUDCLNT_E_RESOURCES_INVALIDATED {
        DeviceErrorKind::Disconnected
    } else if code == AUDCLNT_E_DEVICE_IN_USE
        || code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED
        || code == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED
        || code == AUDCLNT_E_ENGINE_FORMAT_LOCKED
    {
        DeviceErrorKind::Busy
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT
        || code == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED
        || code == AUDCLNT_E_INVALID_DEVICE_PERIOD
        || code == AUDCLNT_E_ENDPOINT_OFFLOAD_NOT_CAPABLE
    {
        DeviceErrorKind::Unsupported
    } else if code == E_ACCESSDENIED {
        DeviceErrorKind::PermissionDenied
    } else if code == E_NOT_FOUND || code == E_FILE_NOT_FOUND {
        DeviceErrorKind::NotFound
    } else {
        DeviceErrorKind::Backend
    };
    DeviceError::new(kind, format!("{context}: {e}"))
}

// ---------------------------------------------------------------------------
// OS resources

/// COM for the current thread; balanced on drop when this call initialized it.
struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        // SAFETY: per-thread COM initialization; an existing apartment of
        // another type (RPC_E_CHANGED_MODE) is used as is and not released.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

/// Win32 event handle closed on drop.
struct Event(HANDLE);

// SAFETY: event handles may be signaled and waited on from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    fn new(manual_reset: bool) -> Result<Self, DeviceError> {
        // SAFETY: anonymous event with default security.
        unsafe { CreateEventW(None, manual_reset, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|e| DeviceError::new(DeviceErrorKind::Backend, format!("CreateEvent: {e}")))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closing a handle owned by this value.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// MMCSS "Pro Audio" registration of the current thread.
struct Mmcss(Option<HANDLE>);

impl Mmcss {
    fn register() -> Self {
        let mut index = 0u32;
        // SAFETY: valid NUL-terminated task name.
        let h = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut index) };
        if let Err(e) = &h {
            tracing::warn!("MMCSS registration failed: {e}");
        }
        Self(h.ok())
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            // SAFETY: reverting a handle returned by AvSetMmThreadCharacteristicsW.
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_choice_rounds_to_fundamental_and_clamps() {
        let p = SharedPeriods { default: 480, fundamental: 48, min: 96, max: 480 };
        assert_eq!(p.choose(None), 96);
        assert_eq!(p.choose(Some(128)), 144);
        assert_eq!(p.choose(Some(256)), 288);
        assert_eq!(p.choose(Some(32)), 96);
        assert_eq!(p.choose(Some(4096)), 480);
        // Drivers without small-period support: min = max = default.
        let fixed = SharedPeriods { default: 480, fundamental: 480, min: 480, max: 480 };
        assert_eq!(fixed.choose(Some(128)), 480);
    }

    #[test]
    fn sample_conversions_round_trip() {
        let src = [-1.0f32, -0.5, 0.0, 0.25, 0.999];
        for kind in [SampleKind::F32, SampleKind::I16, SampleKind::I24, SampleKind::I32] {
            let mut bytes = vec![0u8; src.len() * kind.bytes()];
            write_samples(kind, &src, &mut bytes);
            let mut back = [0.0f32; 5];
            read_samples(kind, &bytes, &mut back);
            let tol = if kind == SampleKind::I16 { 1e-4 } else { 1e-6 };
            for (a, b) in src.iter().zip(&back) {
                assert!((a - b).abs() <= tol, "{kind:?}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn out_of_range_samples_are_clamped() {
        let mut bytes = [0u8; 4];
        write_samples(SampleKind::I16, &[2.0, -2.0], &mut bytes);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), -i16::MAX);
    }

    #[test]
    fn wave_format_round_trips() {
        let f = Format { kind: SampleKind::I32, valid_bits: 24, channels: 2, rate: 48_000, mask: 3 };
        let w = f.to_wave();
        assert_eq!({ w.Format.nBlockAlign }, 8);
        assert_eq!({ w.Format.cbSize }, 22);
        // SAFETY: `w` is a complete WAVEFORMATEXTENSIBLE.
        let back = unsafe { Format::from_wave(&w.Format) }.unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn frame_time_conversions() {
        assert_eq!(hns_to_frames(100_000, 48_000), 480);
        assert_eq!(frames_to_hns(480, 48_000), 100_000);
        assert_eq!(hns_to_frames(frames_to_hns(128, 44_100), 44_100), 128);
    }

    /// Opens real devices. Ignored by default (CI machines may have no
    /// audio hardware); run with `cargo test -p novamix-device-api -- --ignored`.
    #[test]
    #[ignore]
    fn enumerate_and_stream_real_devices() {
        let b = WasapiBackend::new(ShareMode::Shared).unwrap();
        let devs = b.devices().unwrap();
        for d in &devs {
            println!("{:?} {} [{}] {} Hz {} ch periods {:?}", d.direction, d.name, d.id, d.default_sample_rate, d.channels, d.buffer_range);
        }
        if let Some(out) = devs.iter().find(|d| d.direction == Direction::Output && d.is_default) {
            let req = StreamRequest { device_id: out.id.clone(), loopback: false, sample_rate: None, buffer_frames: Some(128) };
            println!("hint {:?}", b.period_hint(&req, Direction::Output));
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let c2 = calls.clone();
            let s = b
                .open_output(
                    &req,
                    Box::new(move |buf, _| {
                        buf.fill(0.0);
                        c2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }),
                    Box::new(|e| println!("error {e}")),
                )
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            println!("{:?} callbacks {}", s.info(), calls.load(std::sync::atomic::Ordering::Relaxed));
            assert!(calls.load(std::sync::atomic::Ordering::Relaxed) > 0);
        }
    }

    /// Frames per second actually delivered by every active endpoint in both
    /// modes (should match the sample rate). Ignored: needs real devices and
    /// briefly takes them in exclusive mode.
    #[test]
    #[ignore]
    fn measure_real_stream_rates() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        for mode in [ShareMode::Shared, ShareMode::Exclusive] {
            let b = WasapiBackend::new(mode).unwrap();
            for d in b.devices().unwrap() {
                let req = StreamRequest { device_id: d.id.clone(), loopback: false, sample_rate: None, buffer_frames: Some(128) };
                let frames = Arc::new(AtomicUsize::new(0));
                let (f, f2) = (frames.clone(), frames.clone());
                let s = match d.direction {
                    Direction::Input => {
                        b.open_input(&req, Box::new(move |buf, ch| _ = f.fetch_add(buf.len() / ch, Ordering::Relaxed)), Box::new(|_| {}))
                    }
                    Direction::Output => b.open_output(
                        &req,
                        Box::new(move |buf, ch| {
                            buf.fill(0.0);
                            f2.fetch_add(buf.len() / ch, Ordering::Relaxed);
                        }),
                        Box::new(|_| {}),
                    ),
                };
                let s = match s {
                    Ok(s) => s,
                    Err(e) => {
                        println!("{mode:?} {:?} {}: {e}", d.direction, d.name);
                        continue;
                    }
                };
                std::thread::sleep(std::time::Duration::from_millis(500));
                let start = frames.load(Ordering::Relaxed);
                let t = std::time::Instant::now();
                std::thread::sleep(std::time::Duration::from_secs(2));
                let rate = (frames.load(Ordering::Relaxed) - start) as f64 / t.elapsed().as_secs_f64();
                println!("{mode:?} {:?} {} {} period {:?}: {rate:.0} frames/s", d.direction, d.name, s.info().sample_format, s.info().buffer_frames);
            }
        }
    }
}
