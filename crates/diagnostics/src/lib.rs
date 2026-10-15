//! # novamix-diagnostics
//!
//! * Lock-free counters and timing histograms that the audio thread can
//!   update without blocking or allocating ([`StreamStats`], [`TimingStats`]).
//! * A bounded, wait-free event queue from the audio thread to the control
//!   thread ([`rt_event_channel`]); events that do not fit are counted and
//!   dropped instead of blocking.
//! * A bounded event log for the UI ([`EventLog`]).
//! * Privacy-aware diagnostic reports ([`DiagnosticReport`]) that never
//!   contain audio and can hide device names and user/system details.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

pub mod time;

// ---------------------------------------------------------------------------
// Atomic float

/// `f32` stored in an `AtomicU32` (relaxed ordering; values are telemetry).
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub const fn zero() -> Self {
        Self(AtomicU32::new(0))
    }
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    #[inline]
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    #[inline]
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    /// Stores `v` and returns the previous value.
    #[inline]
    pub fn swap(&self, v: f32) -> f32 {
        f32::from_bits(self.0.swap(v.to_bits(), Ordering::Relaxed))
    }
    /// Stores `max(current, v)`.
    #[inline]
    pub fn fetch_max(&self, v: f32) {
        let mut cur = self.0.load(Ordering::Relaxed);
        while f32::from_bits(cur) < v {
            match self.0.compare_exchange_weak(cur, v.to_bits(), Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stream statistics

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum StreamState {
    Stopped = 0,
    Starting = 1,
    Running = 2,
    /// Waiting for the bridge buffer to refill after an underrun.
    Priming = 3,
    /// The device reported an error or disappeared.
    Failed = 4,
    /// Loopback source with nothing playing (silence, not an error).
    Idle = 5,
}

impl StreamState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => StreamState::Starting,
            2 => StreamState::Running,
            3 => StreamState::Priming,
            4 => StreamState::Failed,
            5 => StreamState::Idle,
            _ => StreamState::Stopped,
        }
    }
}

/// Counters for one device stream / bridge. All writes are relaxed atomics.
#[derive(Debug)]
pub struct StreamStats {
    pub underruns: AtomicU64,
    pub overruns: AtomicU64,
    pub callbacks: AtomicU64,
    pub frames: AtomicU64,
    pub errors: AtomicU64,
    pub fill_frames: AtomicU32,
    pub target_frames: AtomicU32,
    /// Drift correction applied by the bridge, in parts per million.
    pub correction_ppm: AtomicI32,
    pub last_callback_frames: AtomicU32,
    state: AtomicU8,
}

impl Default for StreamStats {
    fn default() -> Self {
        Self {
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            fill_frames: AtomicU32::new(0),
            target_frames: AtomicU32::new(0),
            correction_ppm: AtomicI32::new(0),
            last_callback_frames: AtomicU32::new(0),
            state: AtomicU8::new(StreamState::Stopped as u8),
        }
    }
}

impl StreamStats {
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
    #[inline]
    pub fn set_state(&self, s: StreamState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }
    #[inline]
    pub fn state(&self) -> StreamState {
        StreamState::from_u8(self.state.load(Ordering::Relaxed))
    }
    #[inline]
    pub fn add_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_overrun(&self) {
        self.overruns.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_callback(&self, frames: usize) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
        self.last_callback_frames.store(frames as u32, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StreamStatsSnapshot {
        StreamStatsSnapshot {
            state: self.state(),
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            fill_frames: self.fill_frames.load(Ordering::Relaxed),
            target_frames: self.target_frames.load(Ordering::Relaxed),
            correction_ppm: self.correction_ppm.load(Ordering::Relaxed),
            last_callback_frames: self.last_callback_frames.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStatsSnapshot {
    pub state: StreamState,
    pub underruns: u64,
    pub overruns: u64,
    pub callbacks: u64,
    pub frames: u64,
    pub errors: u64,
    pub fill_frames: u32,
    pub target_frames: u32,
    pub correction_ppm: i32,
    pub last_callback_frames: u32,
}

// ---------------------------------------------------------------------------
// Timing

/// Number of histogram buckets; each covers 5 % of the block budget, the
/// last one collects everything above 155 %.
pub const TIMING_BUCKETS: usize = 32;
const BUCKET_PCT: f32 = 5.0;

/// Processing-time statistics of the audio callback, expressed as a load
/// percentage of the time budget (block duration).
#[derive(Debug)]
pub struct TimingStats {
    buckets: [AtomicU32; TIMING_BUCKETS],
    count: AtomicU64,
    over_budget: AtomicU64,
    max_load: AtomicF32,
    last_load: AtomicF32,
    avg_load: AtomicF32,
    last_us: AtomicF32,
}

impl Default for TimingStats {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU32::new(0)),
            count: AtomicU64::new(0),
            over_budget: AtomicU64::new(0),
            max_load: AtomicF32::zero(),
            last_load: AtomicF32::zero(),
            avg_load: AtomicF32::zero(),
            last_us: AtomicF32::zero(),
        }
    }
}

impl TimingStats {
    /// Records one callback. Returns the load percentage.
    #[inline]
    pub fn record(&self, elapsed_us: f32, budget_us: f32) -> f32 {
        let load = if budget_us > 0.0 { elapsed_us / budget_us * 100.0 } else { 0.0 };
        let b = ((load / BUCKET_PCT) as usize).min(TIMING_BUCKETS - 1);
        self.buckets[b].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        if load > 100.0 {
            self.over_budget.fetch_add(1, Ordering::Relaxed);
        }
        self.max_load.fetch_max(load);
        self.last_load.store(load);
        self.last_us.store(elapsed_us);
        let avg = self.avg_load.load();
        self.avg_load.store(avg * 0.99 + load * 0.01);
        load
    }

    /// Load percentage at percentile `p` (0..100), estimated from the
    /// histogram (upper edge of the bucket).
    pub fn percentile(&self, p: f32) -> f32 {
        let total: u64 = self.buckets.iter().map(|b| b.load(Ordering::Relaxed) as u64).sum();
        if total == 0 {
            return 0.0;
        }
        let want = ((p / 100.0).clamp(0.0, 1.0) * total as f32).ceil() as u64;
        let mut acc = 0u64;
        for (i, b) in self.buckets.iter().enumerate() {
            acc += b.load(Ordering::Relaxed) as u64;
            if acc >= want.max(1) {
                return (i + 1) as f32 * BUCKET_PCT;
            }
        }
        TIMING_BUCKETS as f32 * BUCKET_PCT
    }

    pub fn reset(&self) {
        for b in &self.buckets {
            b.store(0, Ordering::Relaxed);
        }
        self.count.store(0, Ordering::Relaxed);
        self.over_budget.store(0, Ordering::Relaxed);
        self.max_load.store(0.0);
    }

    pub fn snapshot(&self) -> TimingSnapshot {
        TimingSnapshot {
            callbacks: self.count.load(Ordering::Relaxed),
            over_budget: self.over_budget.load(Ordering::Relaxed),
            last_load_pct: self.last_load.load(),
            avg_load_pct: self.avg_load.load(),
            max_load_pct: self.max_load.load(),
            p50_load_pct: self.percentile(50.0),
            p95_load_pct: self.percentile(95.0),
            p99_load_pct: self.percentile(99.0),
            last_us: self.last_us.load(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TimingSnapshot {
    pub callbacks: u64,
    pub over_budget: u64,
    pub last_load_pct: f32,
    pub avg_load_pct: f32,
    pub max_load_pct: f32,
    pub p50_load_pct: f32,
    pub p95_load_pct: f32,
    pub p99_load_pct: f32,
    pub last_us: f32,
}

// ---------------------------------------------------------------------------
// Real-time event queue

/// Small `Copy` events produced on the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RtEvent {
    InputUnderrun { channel: u8 },
    InputOverrun { channel: u8 },
    OutputUnderrun { output: u8 },
    OutputOverrun { output: u8 },
    OverBudget { load_pct: u16 },
    InputClipped { channel: u8 },
    BusClipped { bus: u8 },
    InputRecovered { channel: u8 },
}

/// Producer side (audio thread). Never blocks; full queue drops the event.
pub struct RtEventSender {
    tx: rtrb::Producer<RtEvent>,
    dropped: Arc<AtomicU64>,
}

impl RtEventSender {
    #[inline]
    pub fn send(&mut self, e: RtEvent) {
        if self.tx.push(e).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Consumer side (control thread).
pub struct RtEventReceiver {
    rx: rtrb::Consumer<RtEvent>,
    dropped: Arc<AtomicU64>,
}

impl RtEventReceiver {
    pub fn drain(&mut self, mut f: impl FnMut(RtEvent)) {
        while let Ok(e) = self.rx.pop() {
            f(e);
        }
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub fn rt_event_channel(capacity: usize) -> (RtEventSender, RtEventReceiver) {
    let (tx, rx) = rtrb::RingBuffer::new(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    (RtEventSender { tx, dropped: dropped.clone() }, RtEventReceiver { rx, dropped })
}

// ---------------------------------------------------------------------------
// Event log

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: String,
    pub unix_ms: u64,
    pub level: Level,
    pub category: String,
    pub message: String,
    /// How many identical consecutive messages were folded into this one.
    pub repeat: u32,
}

/// Bounded log of notable events (device changes, errors, reconnects).
#[derive(Debug, Clone)]
pub struct EventLog {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    version: u64,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity), capacity: capacity.max(1), version: 0 }
    }

    pub fn push(&mut self, level: Level, category: &str, message: impl Into<String>) {
        let message = message.into();
        self.version += 1;
        if let Some(last) = self.entries.back_mut()
            && last.level == level
            && last.category == category
            && last.message == message
        {
            last.repeat += 1;
            last.time = time::now_iso8601();
            last.unix_ms = time::unix_ms();
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(LogEntry {
            time: time::now_iso8601(),
            unix_ms: time::unix_ms(),
            level,
            category: category.to_string(),
            message,
            repeat: 1,
        });
    }

    pub fn info(&mut self, category: &str, message: impl Into<String>) {
        self.push(Level::Info, category, message);
    }
    pub fn warn(&mut self, category: &str, message: impl Into<String>) {
        self.push(Level::Warning, category, message);
    }
    pub fn error(&mut self, category: &str, message: impl Into<String>) {
        self.push(Level::Error, category, message);
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn to_vec(&self) -> Vec<LogEntry> {
        self.entries.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.version += 1;
    }

    /// Increments on every change (cheap change detection for the UI).
    pub fn version(&self) -> u64 {
        self.version
    }
}

// ---------------------------------------------------------------------------
// Diagnostic report

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDiag {
    pub id: String,
    pub name: String,
    pub direction: String,
    pub is_default: bool,
    pub is_virtual: bool,
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamDiag {
    pub role: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: Option<u32>,
    pub sample_format: String,
    pub stats: StreamStatsSnapshot,
}

/// Everything exported by "Export diagnostics". Contains no audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub generated_at: String,
    pub app_version: String,
    pub os: String,
    pub anonymized: bool,
    pub engine: serde_json::Value,
    pub timing: TimingSnapshot,
    pub devices: Vec<DeviceDiag>,
    pub streams: Vec<StreamDiag>,
    pub route: serde_json::Value,
    pub config_summary: serde_json::Value,
    pub events: Vec<LogEntry>,
    pub dropped_rt_events: u64,
}

impl DiagnosticReport {
    /// Replaces device names/ids and user/system identifiers with neutral
    /// placeholders throughout the report.
    pub fn anonymize(&mut self) {
        let mut secrets: Vec<(String, String)> = Vec::new();
        for (i, d) in self.devices.iter().enumerate() {
            let label = format!("Device {}", i + 1);
            if !d.name.is_empty() {
                secrets.push((d.name.clone(), label.clone()));
            }
            if !d.id.is_empty() {
                secrets.push((d.id.clone(), format!("device-{}", i + 1)));
            }
        }
        for s in &self.streams {
            if !s.device.is_empty() && !secrets.iter().any(|(k, _)| k == &s.device) {
                let label = format!("Device {}", secrets.len() + 1);
                secrets.push((s.device.clone(), label));
            }
        }
        for var in ["USERNAME", "COMPUTERNAME", "USERDOMAIN", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "HOME", "USER"] {
            if let Ok(v) = std::env::var(var)
                && v.len() >= 3
            {
                secrets.push((v, format!("%{var}%")));
            }
        }
        // Longest first so that substrings do not break longer matches.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.0.len()));
        let Ok(mut text) = serde_json::to_string(self) else { return };
        for (secret, label) in &secrets {
            let Ok(escaped) = serde_json::to_string(secret) else { continue };
            let escaped = &escaped[1..escaped.len() - 1];
            if escaped.is_empty() {
                continue;
            }
            text = text.replace(escaped, label);
        }
        if let Ok(mut r) = serde_json::from_str::<DiagnosticReport>(&text) {
            r.anonymized = true;
            *self = r;
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Short human-readable summary (shown before exporting).
    pub fn summary(&self) -> String {
        let mut s = String::new();
        use std::fmt::Write;
        let _ = writeln!(s, "NovaMix {} — {}", self.app_version, self.generated_at);
        let _ = writeln!(s, "OS: {}", self.os);
        let _ = writeln!(
            s,
            "Callback load: avg {:.1}% p95 {:.1}% p99 {:.1}% max {:.1}% (over budget: {})",
            self.timing.avg_load_pct, self.timing.p95_load_pct, self.timing.p99_load_pct, self.timing.max_load_pct, self.timing.over_budget
        );
        for st in &self.streams {
            let _ = writeln!(
                s,
                "[{}] {} {} Hz {} ch — underruns {} overruns {} errors {}",
                st.role, st.device, st.sample_rate, st.channels, st.stats.underruns, st.stats.overruns, st.stats.errors
            );
        }
        let _ = writeln!(s, "Events: {} (dropped RT events: {})", self.events.len(), self.dropped_rt_events);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_f32_max() {
        let a = AtomicF32::new(1.0);
        a.fetch_max(0.5);
        assert_eq!(a.load(), 1.0);
        a.fetch_max(2.5);
        assert_eq!(a.load(), 2.5);
    }

    #[test]
    fn timing_percentiles() {
        let t = TimingStats::default();
        for _ in 0..90 {
            t.record(100.0, 1000.0); // 10 %
        }
        for _ in 0..10 {
            t.record(900.0, 1000.0); // 90 %
        }
        let s = t.snapshot();
        assert_eq!(s.callbacks, 100);
        assert_eq!(s.p50_load_pct, 15.0);
        assert_eq!(s.p99_load_pct, 95.0);
        assert_eq!(s.over_budget, 0);
        t.record(2000.0, 1000.0);
        assert_eq!(t.snapshot().over_budget, 1);
    }

    #[test]
    fn rt_queue_drops_when_full() {
        let (mut tx, mut rx) = rt_event_channel(2);
        for _ in 0..5 {
            tx.send(RtEvent::OverBudget { load_pct: 120 });
        }
        let mut n = 0;
        rx.drain(|_| n += 1);
        assert_eq!(n, 2);
        assert_eq!(rx.dropped(), 3);
    }

    #[test]
    fn event_log_is_bounded_and_folds_repeats() {
        let mut log = EventLog::new(3);
        log.info("a", "x");
        log.info("a", "x");
        assert_eq!(log.len(), 1);
        assert_eq!(log.entries().next().unwrap().repeat, 2);
        for i in 0..5 {
            log.warn("b", format!("m{i}"));
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.entries().last().unwrap().message, "m4");
    }

    #[test]
    fn anonymize_removes_device_names() {
        let mut r = DiagnosticReport {
            generated_at: time::now_iso8601(),
            app_version: "0.1.0".into(),
            os: "Windows".into(),
            anonymized: false,
            engine: serde_json::json!({ "master": "Fone USB Gamer" }),
            timing: TimingSnapshot::default(),
            devices: vec![DeviceDiag {
                id: "{0.0.0.00000000}.{abc}".into(),
                name: "Fone USB Gamer".into(),
                direction: "output".into(),
                is_default: true,
                is_virtual: false,
                channels: 2,
                sample_rate: 48_000,
            }],
            streams: vec![],
            route: serde_json::json!(["Mic → Voz → Fone USB Gamer"]),
            config_summary: serde_json::Value::Null,
            events: vec![LogEntry {
                time: "t".into(),
                unix_ms: 0,
                level: Level::Warning,
                category: "device".into(),
                message: "Fone USB Gamer disconnected ({0.0.0.00000000}.{abc})".into(),
                repeat: 1,
            }],
            dropped_rt_events: 0,
        };
        r.anonymize();
        let text = r.to_json();
        assert!(!text.contains("Fone USB Gamer"), "{text}");
        assert!(!text.contains("{abc}"));
        assert!(text.contains("Device 1"));
        assert!(r.anonymized);
    }
}
