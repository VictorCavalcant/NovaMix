//! The controller thread: owns devices, streams and the engine; applies
//! profiles; supervises reconnection; publishes status for the UI.
//!
//! Nothing here runs on an audio thread. The audio threads are reached only
//! through the engine's lock-free command queue.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use novamix_audio_core::{
    BridgeConfig, CaptureSink, ChannelMap, Command, EngineConfig, EngineHandles, InternalClock, MAX_OUTPUTS, MasterDriver, MeterBank, PlaybackSource,
    SharedCore, TapPoint, bridge, create_engine,
};
use novamix_config::{AudioSettings, InputMap, Profile};
use novamix_device_api::{
    AudioBackend, DeviceError, DeviceInfo, Direction, NullBackend, StreamHandle, StreamRequest, VirtualDeviceAdapter, VirtualDeviceStatus,
    create_backend,
};
use novamix_device_windows::{DeviceWatcher, WindowsVirtualAdapter};
use novamix_diagnostics::{EventLog, Level, RtEvent, StreamStats};
use novamix_mixer_graph::{BusId, ChannelId, MAX_BUSES, MAX_CHANNELS, analyze};

use crate::reconcile::reconcile;
use crate::resolve::{ResolvedDevice, ResolvedOutput, ResolvedSource, graph_input, resolve_output, resolve_source};
use crate::status::{ClockSource, EndpointState, EndpointStatus, EngineStatus};

/// Which stream an error belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamRole {
    Input(u8),
    Output(u8),
    Master,
}

/// Messages accepted by the controller.
pub enum ControlMsg {
    ApplyProfile(Box<Profile>),
    ApplyAudioSettings(Box<AudioSettings>),
    AbListen { ch: u8, dry: bool, loudness_match: bool },
    SetTap(Option<TapPoint>),
    RescanDevices,
    RestartEngine,
    StreamError { role: StreamRole, generation: u64, error: DeviceError },
    DevicesChanged,
    Shutdown,
}

/// State shared with the UI thread.
pub struct Shared {
    pub status: RwLock<EngineStatus>,
    pub log: Mutex<EventLog>,
    meters: RwLock<Option<Arc<MeterBank>>>,
    tap: Mutex<Option<rtrb::Consumer<f32>>>,
    pub dropped_rt_events: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            status: RwLock::new(EngineStatus::default()),
            log: Mutex::new(EventLog::new(500)),
            meters: RwLock::new(None),
            tap: Mutex::new(None),
            dropped_rt_events: AtomicU64::new(0),
        }
    }

    /// Meters of the running engine (changes when the engine restarts).
    pub fn meters(&self) -> Option<Arc<MeterBank>> {
        self.meters.read().ok().and_then(|m| m.clone())
    }

    /// Runs `f` with the spectrum tap consumer, if available.
    pub fn with_tap<R>(&self, f: impl FnOnce(&mut rtrb::Consumer<f32>) -> R) -> Option<R> {
        self.tap.lock().ok().and_then(|mut t| t.as_mut().map(f))
    }

    pub fn status(&self) -> EngineStatus {
        self.status.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn log_snapshot(&self) -> Vec<novamix_diagnostics::LogEntry> {
        self.log.lock().map(|l| l.to_vec()).unwrap_or_default()
    }

    pub fn log_version(&self) -> u64 {
        self.log.lock().map(|l| l.version()).unwrap_or(0)
    }

    pub fn push_log(&self, level: Level, category: &str, message: impl Into<String>) {
        if let Ok(mut l) = self.log.lock() {
            l.push(level, category, message);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ControllerOptions {
    /// Run without touching audio hardware (tests, `--no-audio`).
    pub null_backend: bool,
    /// Register for OS device notifications.
    pub device_watcher: bool,
}

/// Handle owned by the UI.
pub struct ControllerHandle {
    tx: Sender<ControlMsg>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl ControllerHandle {
    pub fn spawn(audio: AudioSettings, profile: Profile, options: ControllerOptions) -> Self {
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared::new());
        let shared2 = shared.clone();
        let tx2 = tx.clone();
        let thread = std::thread::Builder::new()
            .name("novamix-control".into())
            .spawn(move || {
                let backend: Arc<dyn AudioBackend> = if options.null_backend {
                    Arc::new(NullBackend)
                } else {
                    match create_backend(audio.host.as_deref()) {
                        Ok(b) => b,
                        Err(e) => {
                            shared2.push_log(Level::Error, "engine", format!("audio host unavailable ({e}); running without audio"));
                            Arc::new(NullBackend)
                        }
                    }
                };
                let c = Controller::new(rx, tx2, shared2, backend, audio, profile);
                c.run(options.device_watcher);
            })
            .expect("failed to spawn controller thread");
        Self { tx, shared, thread: Some(thread) }
    }

    pub fn send(&self, msg: ControlMsg) {
        let _ = self.tx.send(msg);
    }

    pub fn apply_profile(&self, p: &Profile) {
        self.send(ControlMsg::ApplyProfile(Box::new(p.clone())));
    }

    pub fn apply_audio_settings(&self, a: &AudioSettings) {
        self.send(ControlMsg::ApplyAudioSettings(Box::new(a.clone())));
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn shutdown(&mut self) {
        let _ = self.tx.send(ControlMsg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ControllerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------

struct InputStream {
    generation: u64,
    device: ResolvedDevice,
    map: InputMap,
    _handle: Box<dyn StreamHandle>,
    info: novamix_device_api::StreamInfo,
    stats: Arc<StreamStats>,
    latency_frames: usize,
}

struct OutputStream {
    generation: u64,
    device: ResolvedDevice,
    _handle: Box<dyn StreamHandle>,
    info: novamix_device_api::StreamInfo,
    stats: Arc<StreamStats>,
    latency_frames: usize,
}

struct MasterStream {
    generation: u64,
    device: ResolvedDevice,
    handle: Box<dyn StreamHandle>,
}

struct Engine {
    /// Keeps the core alive independently of the stream/clock that drives it.
    _core: SharedCore,
    handles: EngineHandles,
    sample_rate: u32,
    block: usize,
    master: Option<MasterStream>,
    clock: Option<InternalClock>,
    inputs: Vec<Option<InputStream>>,
    outputs: Vec<Option<OutputStream>>,
    slot_devices: Vec<Option<String>>,
    master_slot: Option<u8>,
    backlog: VecDeque<Command>,
}

impl Engine {
    fn push(&mut self, cmd: Command) {
        if !self.backlog.is_empty() {
            self.backlog.push_back(cmd);
            return;
        }
        if let Err(rtrb::PushError::Full(cmd)) = self.handles.commands.push(cmd) {
            self.backlog.push_back(cmd);
        }
    }

    fn flush_backlog(&mut self) {
        while let Some(cmd) = self.backlog.pop_front() {
            if let Err(rtrb::PushError::Full(cmd)) = self.handles.commands.push(cmd) {
                self.backlog.push_front(cmd);
                break;
            }
        }
    }

    fn master_device_id(&self) -> Option<&str> {
        self.master.as_ref().map(|m| m.device.id.as_str())
    }
}

fn channel_map(m: InputMap) -> ChannelMap {
    match m {
        InputMap::Auto => ChannelMap::Auto,
        InputMap::Mono(c) => ChannelMap::Mono(c),
        InputMap::Stereo(c) => ChannelMap::Stereo(c),
        InputMap::MonoMix => ChannelMap::MonoMix,
    }
}

struct Controller {
    rx: Receiver<ControlMsg>,
    tx: Sender<ControlMsg>,
    shared: Arc<Shared>,
    backend: Arc<dyn AudioBackend>,
    virt_adapter: WindowsVirtualAdapter,
    devices: Vec<DeviceInfo>,
    virt: VirtualDeviceStatus,
    audio: AudioSettings,
    profile: Profile,
    applied: Option<Profile>,
    engine: Option<Engine>,
    generation: u64,
    sources: Vec<ResolvedSource>,
    outputs: Vec<ResolvedOutput>,
    channel_errors: Vec<Option<String>>,
    bus_errors: Vec<Option<String>>,
    reconnects_in: Vec<u32>,
    reconnects_out: Vec<u32>,
    route: novamix_mixer_graph::RouteAnalysis,
    rescan_at: Option<Instant>,
    restart_pending: bool,
    next_retry: Option<Instant>,
    retry_backoff: Duration,
    last_poll: Instant,
    last_publish: Instant,
    restarts: u32,
    last_error: Option<String>,
    tap_point: Option<TapPoint>,
    ab: [(bool, bool); MAX_CHANNELS],
    revision: u64,
}

impl Controller {
    fn new(
        rx: Receiver<ControlMsg>,
        tx: Sender<ControlMsg>,
        shared: Arc<Shared>,
        backend: Arc<dyn AudioBackend>,
        audio: AudioSettings,
        profile: Profile,
    ) -> Self {
        Self {
            rx,
            tx,
            shared,
            backend,
            virt_adapter: WindowsVirtualAdapter,
            devices: Vec::new(),
            virt: VirtualDeviceStatus::default(),
            audio,
            profile,
            applied: None,
            engine: None,
            generation: 0,
            sources: Vec::new(),
            outputs: Vec::new(),
            channel_errors: vec![None; MAX_CHANNELS],
            bus_errors: vec![None; MAX_BUSES],
            reconnects_in: vec![0; MAX_CHANNELS],
            reconnects_out: vec![0; MAX_BUSES],
            route: Default::default(),
            rescan_at: None,
            restart_pending: false,
            next_retry: None,
            retry_backoff: Duration::from_secs(2),
            last_poll: Instant::now(),
            last_publish: Instant::now() - Duration::from_secs(10),
            restarts: 0,
            last_error: None,
            tap_point: None,
            ab: [(false, true); MAX_CHANNELS],
            revision: 0,
        }
    }

    fn log(&self, level: Level, category: &str, msg: impl Into<String>) {
        let msg = msg.into();
        match level {
            Level::Error => tracing::error!(target: "novamix", "[{category}] {msg}"),
            Level::Warning => tracing::warn!(target: "novamix", "[{category}] {msg}"),
            Level::Info => tracing::info!(target: "novamix", "[{category}] {msg}"),
        }
        self.shared.push_log(level, category, msg);
    }

    fn run(mut self, watch_devices: bool) {
        let _watcher = if watch_devices {
            let tx = self.tx.clone();
            match DeviceWatcher::new(Arc::new(move |_| {
                let _ = tx.send(ControlMsg::DevicesChanged);
            })) {
                Ok(w) => Some(w),
                Err(e) => {
                    self.log(Level::Warning, "device", format!("device notifications unavailable ({e}); polling only"));
                    None
                }
            }
        } else {
            None
        };
        self.log(Level::Info, "engine", format!("audio host: {}", self.backend.host_name()));
        self.rescan_devices();
        self.sync(true);
        self.publish();
        loop {
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(msg) => {
                    let mut msgs = vec![msg];
                    msgs.extend(self.rx.try_iter());
                    // Coalesce profile updates: only the newest matters.
                    let last_profile = msgs.iter().rposition(|m| matches!(m, ControlMsg::ApplyProfile(_)));
                    let mut quit = false;
                    for (i, m) in msgs.into_iter().enumerate() {
                        if matches!(m, ControlMsg::ApplyProfile(_)) && Some(i) != last_profile {
                            continue;
                        }
                        if !self.handle(m) {
                            quit = true;
                        }
                    }
                    if quit {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
        self.stop_engine();
        self.publish();
    }

    fn handle(&mut self, msg: ControlMsg) -> bool {
        match msg {
            ControlMsg::ApplyProfile(p) => {
                let name_changed = p.name != self.profile.name;
                self.profile = *p;
                if name_changed {
                    self.log(Level::Info, "profile", format!("profile '{}' activated", self.profile.name));
                }
                self.sync(false);
            }
            ControlMsg::ApplyAudioSettings(a) => {
                let a = *a;
                let restart = a.preferred_sample_rate != self.audio.preferred_sample_rate
                    || a.latency_mode != self.audio.latency_mode
                    || a.buffer_frames != self.audio.buffer_frames
                    || a.clock_bus != self.audio.clock_bus
                    || a.host != self.audio.host;
                let host_changed = a.host != self.audio.host;
                let smoothing_changed = a.smoothing_ms != self.audio.smoothing_ms;
                self.audio = a;
                if host_changed {
                    self.stop_engine();
                    match create_backend(self.audio.host.as_deref()) {
                        Ok(b) => self.backend = b,
                        Err(e) => {
                            self.log(Level::Error, "engine", format!("audio host unavailable: {e}"));
                            self.backend = Arc::new(NullBackend);
                        }
                    }
                    self.rescan_devices();
                }
                if restart {
                    self.log(Level::Info, "engine", "audio settings changed: restarting engine");
                    self.sync(true);
                } else {
                    if smoothing_changed && let Some(e) = self.engine.as_mut() {
                        e.push(Command::SetSmoothing { ms: self.audio.smoothing_ms });
                    }
                    self.sync(false);
                }
            }
            ControlMsg::AbListen { ch, dry, loudness_match } => {
                if (ch as usize) < MAX_CHANNELS {
                    self.ab[ch as usize] = (dry, loudness_match);
                }
                if let Some(e) = self.engine.as_mut() {
                    e.push(Command::AbListen { ch, dry, loudness_match });
                }
            }
            ControlMsg::SetTap(point) => {
                self.tap_point = point;
                if let Some(e) = self.engine.as_mut() {
                    e.push(Command::SetTap { point });
                }
            }
            ControlMsg::RescanDevices => {
                self.rescan_devices();
                self.sync(false);
            }
            ControlMsg::RestartEngine => {
                self.log(Level::Info, "engine", "engine restart requested");
                self.rescan_devices();
                self.sync(true);
            }
            ControlMsg::StreamError { role, generation, error } => self.on_stream_error(role, generation, error),
            ControlMsg::DevicesChanged => {
                self.rescan_at = Some(Instant::now() + Duration::from_millis(300));
            }
            ControlMsg::Shutdown => return false,
        }
        self.publish();
        true
    }

    fn on_stream_error(&mut self, role: StreamRole, generation: u64, error: DeviceError) {
        let Some(engine) = self.engine.as_mut() else { return };
        let fatal = error.is_fatal();
        match role {
            StreamRole::Input(ch) => {
                let idx = ch as usize;
                let Some(s) = engine.inputs.get(idx).and_then(|s| s.as_ref()) else { return };
                if s.generation != generation {
                    return;
                }
                s.stats.errors.fetch_add(1, Ordering::Relaxed);
                if !fatal {
                    return;
                }
                let name = s.device.name.clone();
                engine.push(Command::DetachInput { ch });
                engine.inputs[idx] = None;
                self.channel_errors[idx] = Some(error.message.clone());
                self.log(Level::Warning, "device", format!("input '{name}' lost: {}", error.message));
            }
            StreamRole::Output(slot) => {
                let idx = slot as usize;
                let Some(s) = engine.outputs.get(idx).and_then(|s| s.as_ref()) else { return };
                if s.generation != generation {
                    return;
                }
                s.stats.errors.fetch_add(1, Ordering::Relaxed);
                if !fatal {
                    return;
                }
                let name = s.device.name.clone();
                engine.push(Command::DetachOutput { slot });
                engine.outputs[idx] = None;
                self.log(Level::Warning, "device", format!("output '{name}' lost: {}", error.message));
            }
            StreamRole::Master => {
                let Some(m) = engine.master.as_ref() else { return };
                if m.generation != generation || !fatal {
                    return;
                }
                let name = m.device.name.clone();
                self.log(Level::Warning, "device", format!("master output '{name}' lost: {}; switching clock", error.message));
                self.restart_pending = true;
            }
        }
        self.rescan_at = Some(Instant::now() + Duration::from_millis(300));
        self.schedule_retry();
    }

    fn schedule_retry(&mut self) {
        if self.audio.auto_reconnect && self.next_retry.is_none() {
            self.next_retry = Some(Instant::now() + self.retry_backoff);
        }
    }

    fn rescan_devices(&mut self) {
        match self.backend.devices() {
            Ok(list) => {
                let changed = list != self.devices;
                if changed && !self.devices.is_empty() {
                    let added: Vec<&str> = list
                        .iter()
                        .filter(|d| !self.devices.iter().any(|o| o.id == d.id && o.direction == d.direction))
                        .map(|d| d.name.as_str())
                        .collect();
                    let removed: Vec<&str> = self
                        .devices
                        .iter()
                        .filter(|d| !list.iter().any(|o| o.id == d.id && o.direction == d.direction))
                        .map(|d| d.name.as_str())
                        .collect();
                    for a in &added {
                        self.log(Level::Info, "device", format!("device connected: {a}"));
                    }
                    for r in &removed {
                        self.log(Level::Warning, "device", format!("device disconnected: {r}"));
                    }
                }
                self.devices = list;
                self.virt = self.virt_adapter.status(&self.devices);
            }
            Err(e) => self.log(Level::Error, "device", format!("device enumeration failed: {e}")),
        }
        self.last_poll = Instant::now();
    }

    // -----------------------------------------------------------------------
    // Engine lifecycle

    fn stop_engine(&mut self) {
        if let Some(mut e) = self.engine.take() {
            if let Some(mut c) = e.clock.take() {
                c.stop();
            }
            // Dropping the master stream stops the callback that owns the core.
            e.master = None;
            e.inputs.clear();
            e.outputs.clear();
            e.handles.meters.running.store(false, Ordering::Relaxed);
            while e.handles.garbage.pop().is_ok() {}
        }
        if let Ok(mut t) = self.shared.tap.lock() {
            *t = None;
        }
        self.applied = None;
    }

    fn next_gen(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    fn error_callback(&self, role: StreamRole, generation: u64, connected: Option<Arc<AtomicBool>>) -> novamix_device_api::ErrorCallback {
        let tx = self.tx.clone();
        Box::new(move |e: DeviceError| {
            if e.is_fatal()
                && let Some(c) = &connected
            {
                c.store(false, Ordering::Relaxed);
            }
            let _ = tx.send(ControlMsg::StreamError { role, generation, error: e });
        })
    }

    fn start_engine(&mut self, master: Option<ResolvedDevice>) {
        self.stop_engine();
        let mut rate = self.audio.preferred_sample_rate;
        let block = self.audio.effective_buffer_frames() as usize;
        for attempt in 0..2 {
            let mut cfg = EngineConfig::new(rate as f32);
            cfg.smoothing_ms = self.audio.smoothing_ms;
            cfg.command_capacity = 8192;
            let (core, handles) = create_engine(cfg);
            let core: SharedCore = Arc::new(Mutex::new(core));
            let mut engine = Engine {
                _core: core.clone(),
                handles,
                sample_rate: rate,
                block,
                master: None,
                clock: None,
                inputs: (0..MAX_CHANNELS).map(|_| None).collect(),
                outputs: (0..MAX_OUTPUTS).map(|_| None).collect(),
                slot_devices: vec![None; MAX_OUTPUTS],
                master_slot: None,
                backlog: VecDeque::new(),
            };
            if let Some(m) = &master {
                let generation = self.next_gen();
                let driver_failed = engine.handles.failed.clone();
                let mut driver = MasterDriver::new(core.clone(), driver_failed);
                let req = StreamRequest { device_id: m.id.clone(), loopback: false, sample_rate: Some(rate), buffer_frames: Some(block as u32) };
                let err = self.error_callback(StreamRole::Master, generation, None);
                match self.backend.open_output(&req, Box::new(move |out, ch| driver.on_output(out, ch)), err) {
                    Ok(handle) => {
                        let actual = handle.info().sample_rate;
                        if actual != rate && attempt == 0 {
                            // The device insisted on another rate: rebuild the
                            // engine at that rate so the master runs natively.
                            self.log(Level::Info, "engine", format!("'{}' runs at {actual} Hz; engine follows", m.name));
                            drop(handle);
                            rate = actual;
                            continue;
                        }
                        engine.block = handle.info().buffer_frames.map(|b| b as usize).unwrap_or(block);
                        self.log(
                            Level::Info,
                            "engine",
                            format!(
                                "engine started at {} Hz, clock: '{}' ({} ch, buffer {:?})",
                                actual,
                                m.name,
                                handle.info().channels,
                                handle.info().buffer_frames
                            ),
                        );
                        engine.master = Some(MasterStream { generation, device: m.clone(), handle });
                        self.bus_errors.iter_mut().for_each(|e| *e = None);
                    }
                    Err(e) => {
                        self.log(Level::Error, "device", format!("cannot open master output '{}': {e}; using internal clock", m.name));
                        self.last_error = Some(e.to_string());
                        self.schedule_retry();
                    }
                }
            }
            if engine.master.is_none() {
                let failed = engine.handles.failed.clone();
                match InternalClock::start(core.clone(), failed, rate as f32, block, || {
                    std::mem::forget(novamix_device_windows::boost_current_thread());
                }) {
                    Ok(c) => {
                        engine.clock = Some(c);
                        engine.handles.meters.internal_clock.store(true, Ordering::Relaxed);
                        self.log(Level::Info, "engine", format!("engine started at {rate} Hz on the internal clock"));
                    }
                    Err(e) => self.log(Level::Error, "engine", format!("cannot start internal clock: {e}")),
                }
            }
            engine.handles.meters.running.store(true, Ordering::Relaxed);
            if let Ok(mut m) = self.shared.meters.write() {
                *m = Some(engine.handles.meters.clone());
            }
            // Hand the spectrum tap to the UI side.
            let (_dummy_tx, dummy_rx) = rtrb::RingBuffer::<f32>::new(1);
            let tap = std::mem::replace(&mut engine.handles.tap, dummy_rx);
            if let Ok(mut t) = self.shared.tap.lock() {
                *t = Some(tap);
            }
            engine.push(Command::SetSmoothing { ms: self.audio.smoothing_ms });
            if let Some(p) = self.tap_point {
                engine.push(Command::SetTap { point: Some(p) });
            }
            for (ch, (dry, lm)) in self.ab.iter().enumerate() {
                if *dry {
                    engine.push(Command::AbListen { ch: ch as u8, dry: *dry, loudness_match: *lm });
                }
            }
            self.engine = Some(engine);
            self.applied = None;
            self.restarts += 1;
            return;
        }
    }

    fn bridge_margin_ms(&self) -> f64 {
        self.audio.latency_mode.bridge_margin_ms() as f64
    }

    fn open_input(&mut self, ch: usize, dev: &ResolvedDevice, map: InputMap) -> Result<InputStream, String> {
        let (engine_rate, engine_block) = {
            let e = self.engine.as_ref().ok_or("engine not running")?;
            (e.sample_rate, e.block)
        };
        let generation = self.next_gen();
        let stats = StreamStats::new_shared();
        let connected = Arc::new(AtomicBool::new(true));
        let margin = self.bridge_margin_ms();
        let mut device_rate = dev.sample_rate.max(8000);
        let req = StreamRequest {
            device_id: dev.id.clone(),
            loopback: dev.loopback,
            sample_rate: None,
            buffer_frames: Some(self.audio.effective_buffer_frames()),
        };
        // Backends that know their period up front (low-latency WASAPI) get
        // bridges sized for it; others are assumed to run at ≥ 10 ms.
        let mut period = self.backend.period_hint(&req, Direction::Input).map(|p| p as usize);
        for _ in 0..3 {
            let src_block = period.unwrap_or_else(|| (self.audio.effective_buffer_frames() as usize).max(device_rate as usize / 100));
            let mut cfg = BridgeConfig::for_blocks(device_rate as f64, engine_rate as f64, src_block, engine_block, margin);
            cfg.idle_is_silence = dev.loopback;
            let (writer, reader) = bridge(cfg, stats.clone(), connected.clone());
            let latency_frames = reader.latency_frames();
            let mut sink = CaptureSink::new(writer, channel_map(map));
            let err = self.error_callback(StreamRole::Input(ch as u8), generation, Some(connected.clone()));
            let handle = self.backend.open_input(&req, Box::new(move |data, chs| sink.on_input(data, chs)), err).map_err(|e| e.to_string())?;
            let info = handle.info().clone();
            if info.sample_rate != device_rate {
                // The real rate differs from what enumeration reported:
                // rebuild the bridge with the right ratio.
                device_rate = info.sample_rate;
                drop(handle);
                continue;
            }
            if let (Some(p), Some(actual)) = (period, info.buffer_frames)
                && p != actual as usize
            {
                // The hinted period was not granted (e.g. another client
                // fixed the engine period): rebuild the bridge for the real one.
                period = Some(actual as usize);
                drop(handle);
                continue;
            }
            stats.set_state(novamix_diagnostics::StreamState::Starting);
            let engine = self.engine.as_mut().ok_or("engine not running")?;
            engine.push(Command::AttachInput { ch: ch as u8, reader: Box::new(reader) });
            return Ok(InputStream { generation, device: dev.clone(), map, _handle: handle, info, stats, latency_frames });
        }
        Err("device sample rate keeps changing".into())
    }

    fn open_output(&mut self, slot: usize, dev: &ResolvedDevice) -> Result<OutputStream, String> {
        let (engine_rate, engine_block) = {
            let e = self.engine.as_ref().ok_or("engine not running")?;
            (e.sample_rate, e.block)
        };
        let generation = self.next_gen();
        let stats = StreamStats::new_shared();
        let connected = Arc::new(AtomicBool::new(true));
        let margin = self.bridge_margin_ms();
        let mut device_rate = engine_rate;
        let req = StreamRequest {
            device_id: dev.id.clone(),
            loopback: false,
            sample_rate: Some(engine_rate),
            buffer_frames: Some(self.audio.effective_buffer_frames()),
        };
        let mut period = self.backend.period_hint(&req, Direction::Output).map(|p| p as usize);
        for _ in 0..3 {
            let sink_block = period.unwrap_or_else(|| (self.audio.effective_buffer_frames() as usize).max(device_rate as usize / 100));
            let cfg = BridgeConfig::for_blocks(engine_rate as f64, device_rate as f64, engine_block, sink_block, margin);
            let (writer, reader) = bridge(cfg, stats.clone(), connected.clone());
            let latency_frames = reader.latency_frames();
            let mut playback = PlaybackSource::new(reader);
            let err = self.error_callback(StreamRole::Output(slot as u8), generation, Some(connected.clone()));
            let handle = self.backend.open_output(&req, Box::new(move |out, chs| playback.on_output(out, chs)), err).map_err(|e| e.to_string())?;
            let info = handle.info().clone();
            if info.sample_rate != device_rate {
                device_rate = info.sample_rate;
                drop(handle);
                continue;
            }
            if let (Some(p), Some(actual)) = (period, info.buffer_frames)
                && p != actual as usize
            {
                period = Some(actual as usize);
                drop(handle);
                continue;
            }
            let engine = self.engine.as_mut().ok_or("engine not running")?;
            engine.push(Command::AttachOutput { slot: slot as u8, writer: Box::new(writer) });
            return Ok(OutputStream { generation, device: dev.clone(), _handle: handle, info, stats, latency_frames });
        }
        Err("device sample rate keeps changing".into())
    }

    // -----------------------------------------------------------------------
    // Synchronization

    /// Chooses the bus whose device clocks the engine.
    fn clock_bus(&self) -> Option<usize> {
        let resolved = |i: usize| matches!(self.outputs.get(i), Some(ResolvedOutput::Device(_)));
        if let Some(b) = self.audio.clock_bus
            && resolved(b as usize)
        {
            return Some(b as usize);
        }
        if let Some(m) = self.profile.monitor_bus()
            && resolved(m)
        {
            return Some(m);
        }
        (0..self.profile.buses.len()).find(|&i| resolved(i))
    }

    /// Brings streams and engine parameters in line with `self.profile`.
    fn sync(&mut self, force_restart: bool) {
        let profile = self.profile.clone();
        self.sources = profile
            .channels
            .iter()
            .map(|c| if c.enabled { resolve_source(&c.source, &self.devices, &self.virt) } else { ResolvedSource::None })
            .collect();
        self.outputs = profile.buses.iter().map(|b| resolve_output(&b.output, b.role, &self.devices, &self.virt, &self.audio)).collect();

        // Feedback protection: sends that close a loop are muted until the
        // user fixes the route (the UI offers "Fix route").
        let graph = graph_input(&profile, &self.sources, &self.outputs, &self.devices, &self.virt);
        let analysis = analyze(&graph);
        let mut effective = profile.clone();
        if analysis.has_cycles() {
            let fixes = novamix_mixer_graph::suggest_fixes(&graph);
            for (ChannelId(c), BusId(b)) in &fixes {
                if let Some(s) = effective.channels.get_mut(*c as usize).and_then(|ch| ch.sends.get_mut(*b as usize)) {
                    s.enabled = false;
                }
            }
            if self.route.cycles != analysis.cycles {
                for c in &analysis.cycles {
                    self.log(Level::Warning, "route", format!("feedback loop blocked: {}", c.describe()));
                }
            }
        }
        self.route = analysis;

        let clock_bus = self.clock_bus();
        let master_dev = clock_bus.and_then(|b| match &self.outputs[b] {
            ResolvedOutput::Device(d) => Some(d.clone()),
            _ => None,
        });
        let need_restart = force_restart
            || self.restart_pending
            || self.engine.is_none()
            || self.engine.as_ref().map(|e| e.master_device_id() != master_dev.as_ref().map(|d| d.id.as_str())).unwrap_or(true)
            || self.engine.as_ref().map(|e| e.handles.failed.load(Ordering::Relaxed)).unwrap_or(false);
        if need_restart {
            self.restart_pending = false;
            self.start_engine(master_dev.clone());
        }
        if self.engine.is_none() {
            return;
        }

        // Output slots: one per distinct device.
        let mut wanted: Vec<String> = Vec::new();
        for o in &self.outputs {
            if let ResolvedOutput::Device(d) = o
                && !wanted.contains(&d.id)
            {
                wanted.push(d.id.clone());
            }
        }
        let master_id = self.engine.as_ref().and_then(|e| e.master_device_id().map(String::from));
        {
            let engine = self.engine.as_mut().unwrap();
            // Free slots whose device is no longer wanted.
            for slot in 0..MAX_OUTPUTS {
                let keep = engine.slot_devices[slot].as_ref().map(|d| wanted.contains(d)).unwrap_or(false);
                if !keep && engine.slot_devices[slot].is_some() {
                    engine.slot_devices[slot] = None;
                    if engine.outputs[slot].take().is_some() {
                        engine.push(Command::DetachOutput { slot: slot as u8 });
                    }
                }
            }
            for id in &wanted {
                if !engine.slot_devices.iter().any(|s| s.as_ref() == Some(id))
                    && let Some(free) = engine.slot_devices.iter().position(|s| s.is_none())
                {
                    engine.slot_devices[free] = Some(id.clone());
                }
            }
            engine.master_slot = master_id.as_ref().and_then(|m| engine.slot_devices.iter().position(|s| s.as_ref() == Some(m))).map(|s| s as u8);
        }

        // Open missing output streams (non-master slots).
        let slot_list: Vec<(usize, String)> = {
            let e = self.engine.as_ref().unwrap();
            e.slot_devices.iter().enumerate().filter_map(|(i, d)| d.clone().map(|d| (i, d))).collect()
        };
        for (slot, id) in slot_list {
            let is_master = self.engine.as_ref().unwrap().master_slot == Some(slot as u8);
            if is_master {
                continue;
            }
            let open_now = self.engine.as_ref().unwrap().outputs[slot].as_ref().map(|o| o.device.id != id).unwrap_or(true);
            if !open_now {
                continue;
            }
            let dev = self.outputs.iter().find_map(|o| match o {
                ResolvedOutput::Device(d) if d.id == id => Some(d.clone()),
                _ => None,
            });
            let Some(dev) = dev else { continue };
            match self.open_output(slot, &dev) {
                Ok(s) => {
                    self.log(Level::Info, "device", format!("output '{}' opened ({} Hz, {} ch)", dev.name, s.info.sample_rate, s.info.channels));
                    self.engine.as_mut().unwrap().outputs[slot] = Some(s);
                    for (bi, o) in self.outputs.iter().enumerate() {
                        if matches!(o, ResolvedOutput::Device(d) if d.id == id) {
                            self.bus_errors[bi] = None;
                        }
                    }
                }
                Err(e) => {
                    self.log(Level::Error, "device", format!("cannot open output '{}': {e}", dev.name));
                    for (bi, o) in self.outputs.iter().enumerate() {
                        if matches!(o, ResolvedOutput::Device(d) if d.id == id) {
                            self.bus_errors[bi] = Some(e.clone());
                        }
                    }
                    self.schedule_retry();
                }
            }
        }

        // Inputs.
        for ch in 0..MAX_CHANNELS {
            let want = match (self.sources.get(ch), profile.channels.get(ch)) {
                (Some(ResolvedSource::Device(d)), Some(c)) => Some((d.clone(), c.input_map)),
                _ => None,
            };
            let current_ok = {
                let e = self.engine.as_ref().unwrap();
                match (&e.inputs[ch], &want) {
                    (Some(s), Some((d, map))) => s.device == *d && s.map == *map && s.stats.state() != novamix_diagnostics::StreamState::Failed,
                    (None, None) => true,
                    _ => false,
                }
            };
            if current_ok {
                continue;
            }
            {
                let e = self.engine.as_mut().unwrap();
                if e.inputs[ch].take().is_some() {
                    e.push(Command::DetachInput { ch: ch as u8 });
                }
            }
            if let Some((dev, map)) = want {
                match self.open_input(ch, &dev, map) {
                    Ok(s) => {
                        self.log(
                            Level::Info,
                            "device",
                            format!(
                                "input '{}'{} opened ({} Hz, {} ch)",
                                dev.name,
                                if dev.loopback { " (loopback)" } else { "" },
                                s.info.sample_rate,
                                s.info.channels
                            ),
                        );
                        if self.channel_errors[ch].take().is_some() {
                            self.reconnects_in[ch] += 1;
                        }
                        self.engine.as_mut().unwrap().inputs[ch] = Some(s);
                    }
                    Err(e) => {
                        self.log(Level::Error, "device", format!("cannot open input '{}': {e}", dev.name));
                        self.channel_errors[ch] = Some(e);
                        self.schedule_retry();
                    }
                }
            } else {
                self.channel_errors[ch] = None;
            }
        }

        // Routing of buses to slots and engine parameters.
        let engine = self.engine.as_mut().unwrap();
        for bi in 0..MAX_BUSES {
            let slot = match self.outputs.get(bi) {
                Some(ResolvedOutput::Device(d)) => engine.slot_devices.iter().position(|s| s.as_ref() == Some(&d.id)).map(|s| s as u8),
                _ => None,
            };
            engine.push(Command::BusOutput { bus: bi as u8, slot });
        }
        engine.push(Command::MasterSlot { slot: engine.master_slot });
        let mut cmds = Vec::new();
        reconcile(self.applied.as_ref(), &effective, &mut cmds);
        for c in cmds {
            engine.push(c);
        }
        self.applied = Some(effective);

        let problems = self.sources.iter().any(|s| matches!(s, ResolvedSource::Unavailable(_)))
            || self.outputs.iter().any(|o| matches!(o, ResolvedOutput::Unavailable(_)))
            || self.channel_errors.iter().any(|e| e.is_some())
            || self.bus_errors.iter().any(|e| e.is_some());
        if problems {
            self.schedule_retry();
        } else {
            self.next_retry = None;
            self.retry_backoff = Duration::from_millis(self.audio.reconnect_interval_ms as u64);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let mut restart_reason = None;
        if let Some(engine) = self.engine.as_mut() {
            while engine.handles.garbage.pop().is_ok() {}
            engine.flush_backlog();
            let mut events = Vec::new();
            engine.handles.events.drain(|e| events.push(e));
            self.shared.dropped_rt_events.store(engine.handles.events.dropped(), Ordering::Relaxed);
            if engine.handles.failed.load(Ordering::Relaxed) {
                restart_reason = Some("audio processing fault detected");
            }
            for e in events {
                self.log_rt_event(e);
            }
        }
        if let Some(reason) = restart_reason {
            self.log(Level::Error, "engine", format!("{reason}: restarting engine"));
            self.last_error = Some(reason.to_string());
            self.sync(true);
        }
        if self.rescan_at.is_some_and(|t| now >= t) {
            self.rescan_at = None;
            self.rescan_devices();
            self.sync(false);
        }
        if now.duration_since(self.last_poll) > Duration::from_secs(5) {
            let before = self.devices.clone();
            self.rescan_devices();
            if before != self.devices {
                self.sync(false);
            }
        }
        if self.next_retry.is_some_and(|t| now >= t) {
            self.next_retry = None;
            self.retry_backoff = (self.retry_backoff * 2).min(Duration::from_secs(30));
            self.rescan_devices();
            self.sync(false);
        }
        if now.duration_since(self.last_publish) > Duration::from_millis(250) {
            self.publish();
        }
    }

    fn channel_name(&self, ch: u8) -> String {
        self.profile.channels.get(ch as usize).map(|c| c.name.clone()).unwrap_or_else(|| format!("#{}", ch + 1))
    }

    fn bus_name(&self, b: u8) -> String {
        self.profile.buses.get(b as usize).map(|c| c.name.clone()).unwrap_or_else(|| format!("#{}", b + 1))
    }

    fn log_rt_event(&self, e: RtEvent) {
        match e {
            RtEvent::InputUnderrun { channel } => self.log(Level::Warning, "audio", format!("underrun on channel '{}'", self.channel_name(channel))),
            RtEvent::InputOverrun { channel } => self.log(Level::Warning, "audio", format!("overrun on channel '{}'", self.channel_name(channel))),
            RtEvent::OutputUnderrun { output } => self.log(Level::Warning, "audio", format!("underrun on output {output}")),
            RtEvent::OutputOverrun { output } => self.log(Level::Warning, "audio", format!("output {output} buffer full (device not consuming)")),
            RtEvent::OverBudget { load_pct } => {
                self.log(Level::Warning, "audio", format!("audio callback over budget ({load_pct}% of the block time)"))
            }
            RtEvent::InputClipped { channel } => {
                self.log(Level::Warning, "audio", format!("source of '{}' arrives clipped: lower the device/input level", self.channel_name(channel)))
            }
            RtEvent::BusClipped { bus } => self.log(Level::Info, "audio", format!("bus '{}' reached 0 dBFS", self.bus_name(bus))),
            RtEvent::InputRecovered { channel } => self.log(Level::Info, "audio", format!("channel '{}' recovered", self.channel_name(channel))),
        }
    }

    fn publish(&mut self) {
        self.last_publish = Instant::now();
        self.revision += 1;
        let mut st = EngineStatus { host: self.backend.host_name(), ..Default::default() };
        let mut chain_latency = [0u32; MAX_CHANNELS];
        if let Some(e) = &self.engine {
            st.running = e.handles.meters.running.load(Ordering::Relaxed) && !e.handles.failed.load(Ordering::Relaxed);
            st.sample_rate = e.sample_rate;
            st.block_frames = e.block as u32;
            st.clock = match &e.master {
                Some(m) => ClockSource::Device(m.device.name.clone()),
                None if e.clock.is_some() => ClockSource::Internal,
                None => ClockSource::None,
            };
            for (i, l) in chain_latency.iter_mut().enumerate() {
                *l = e.handles.meters.channels[i].latency_samples.load(Ordering::Relaxed);
            }
        }
        let ms = |frames: usize, rate: u32| if rate > 0 { frames as f32 * 1000.0 / rate as f32 } else { 0.0 };
        for (i, c) in self.profile.channels.iter().enumerate() {
            let mut es = EndpointStatus { reconnects: self.reconnects_in.get(i).copied().unwrap_or(0), ..Default::default() };
            let stream = self.engine.as_ref().and_then(|e| e.inputs.get(i).and_then(|s| s.as_ref()));
            es.state = match (&self.sources.get(i), stream, self.channel_errors.get(i).cloned().flatten()) {
                _ if !c.enabled => EndpointState::Off,
                (_, _, Some(err)) => EndpointState::Failed(err),
                (Some(ResolvedSource::TestTone { .. }), _, _) => EndpointState::TestTone,
                (Some(ResolvedSource::Unavailable(r)), _, _) => EndpointState::Unavailable(r.clone()),
                (Some(ResolvedSource::Device(_)), Some(s), _) => match s.stats.state() {
                    novamix_diagnostics::StreamState::Running => EndpointState::Running,
                    novamix_diagnostics::StreamState::Idle => EndpointState::Idle,
                    novamix_diagnostics::StreamState::Failed => EndpointState::Failed("device error".into()),
                    _ => EndpointState::Starting,
                },
                (Some(ResolvedSource::Device(_)), None, _) => EndpointState::Starting,
                _ => EndpointState::Off,
            };
            if let Some(s) = stream {
                es.device_name = Some(s.device.name.clone());
                es.stats = Some(s.stats.snapshot());
                let dev_ms = ms(s.info.buffer_frames.unwrap_or(s.info.sample_rate / 100) as usize, s.info.sample_rate);
                es.latency_ms = dev_ms + ms(s.latency_frames, st.sample_rate);
                es.info = Some(s.info.clone());
            } else if let Some(ResolvedSource::Device(d)) = self.sources.get(i) {
                es.device_name = Some(d.name.clone());
            }
            st.channels.push(es);
        }
        for (i, b) in self.profile.buses.iter().enumerate() {
            let mut es = EndpointStatus { reconnects: self.reconnects_out.get(i).copied().unwrap_or(0), ..Default::default() };
            let _ = b;
            match self.outputs.get(i) {
                Some(ResolvedOutput::Device(d)) => {
                    es.device_name = Some(d.name.clone());
                    let e = self.engine.as_ref();
                    let is_master = e.and_then(|e| e.master.as_ref()).map(|m| m.device.id == d.id).unwrap_or(false);
                    let slot = e.and_then(|e| e.slot_devices.iter().position(|s| s.as_ref() == Some(&d.id)));
                    let stream = e.and_then(|e| slot.and_then(|s| e.outputs[s].as_ref()));
                    es.state = if let Some(err) = self.bus_errors[i].clone() {
                        EndpointState::Failed(err)
                    } else if is_master {
                        EndpointState::Running
                    } else if let Some(s) = stream {
                        es.stats = Some(s.stats.snapshot());
                        es.info = Some(s.info.clone());
                        es.latency_ms = ms(s.latency_frames, s.info.sample_rate)
                            + ms(s.info.buffer_frames.unwrap_or(s.info.sample_rate / 100) as usize, s.info.sample_rate);
                        match s.stats.state() {
                            novamix_diagnostics::StreamState::Failed => EndpointState::Failed("device error".into()),
                            _ => EndpointState::Running,
                        }
                    } else {
                        EndpointState::Starting
                    };
                    if is_master && let Some(m) = e.and_then(|e| e.master.as_ref()) {
                        es.info = Some(m.handle.info().clone());
                        es.latency_ms = ms(m.handle.info().buffer_frames.unwrap_or(st.sample_rate / 100) as usize, st.sample_rate);
                    }
                }
                Some(ResolvedOutput::Unavailable(r)) => es.state = EndpointState::Unavailable(r.clone()),
                _ => es.state = EndpointState::Off,
            }
            st.buses.push(es);
        }
        // Voice latency estimate: first microphone channel → voice bus.
        let mic = self.profile.channels.iter().position(|c| c.is_microphone && c.enabled);
        let voice = self.profile.buses.iter().position(|b| b.role == novamix_config::BusRole::Voice);
        if let (Some(m), Some(v)) = (mic, voice)
            && let (Some(ci), Some(bo)) = (st.channels.get(m), st.buses.get(v))
            && ci.state == EndpointState::Running
            && bo.state == EndpointState::Running
        {
            st.voice_latency_ms =
                Some(ci.latency_ms + ms(chain_latency[m] as usize, st.sample_rate) + ms(st.block_frames as usize, st.sample_rate) + bo.latency_ms);
        }
        st.route = self.route.clone();
        st.devices = self.devices.clone();
        st.virtual_status = self.virt.clone();
        st.last_error = self.last_error.clone();
        st.profile_name = self.profile.name.clone();
        st.engine_restarts = self.restarts;
        st.revision = self.revision;
        if let Ok(mut s) = self.shared.status.write() {
            *s = st;
        }
    }
}
