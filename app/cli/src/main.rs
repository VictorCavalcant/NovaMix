//! `novamix-cli` — headless tools for NovaMix.
//!
//! * `devices`   list audio devices and virtual endpoint status
//! * `bench`     measure engine processing time per block (percentiles)
//! * `tone`      play a synthetic test signal on an output (no capture)
//! * `run`       run a profile headless and print counters (soak tests)
//! * `validate`  validate a profile file (TOML/JSON)
//! * `diag`      export a diagnostic report
//! * `paths`     show configuration locations

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use novamix_audio_core::{Command, EngineConfig, create_engine};
use novamix_config::{AppConfig, BusRole, ConfigStore, DeviceRef, Format, LatencyMode, OutputSpec, Profile, SourceSpec, default_profile};
use novamix_control::{ControllerHandle, ControllerOptions, EndpointState, build_report};
use novamix_device_api::{VirtualDeviceAdapter, create_backend};
use novamix_dsp::params::{StageKind, VoicePreset};
use novamix_dsp::signal::ToneKind;

#[derive(Parser)]
#[command(name = "novamix-cli", version, about = "NovaMix headless tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum PresetArg {
    Natural,
    NoisyRoom,
    Call,
    Stream,
    Raw,
}

impl From<PresetArg> for VoicePreset {
    fn from(p: PresetArg) -> Self {
        match p {
            PresetArg::Natural => VoicePreset::Natural,
            PresetArg::NoisyRoom => VoicePreset::NoisyRoom,
            PresetArg::Call => VoicePreset::Call,
            PresetArg::Stream => VoicePreset::Stream,
            PresetArg::Raw => VoicePreset::Raw,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum LatencyArg {
    Low,
    Balanced,
    Stability,
}

impl From<LatencyArg> for LatencyMode {
    fn from(l: LatencyArg) -> Self {
        match l {
            LatencyArg::Low => LatencyMode::LowLatency,
            LatencyArg::Balanced => LatencyMode::Balanced,
            LatencyArg::Stability => LatencyMode::Stability,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ToneArg {
    Sine,
    Pink,
    White,
    Beep,
}

#[derive(Subcommand)]
enum Cmd {
    /// List audio devices and virtual endpoints.
    Devices {
        #[arg(long)]
        json: bool,
        /// Audio host (e.g. wasapi, wasapi-lowlatency, wasapi-exclusive, asio).
        #[arg(long)]
        host: Option<String>,
    },
    /// Benchmark the engine offline (no devices).
    Bench {
        #[arg(long, default_value_t = 48_000)]
        rate: u32,
        #[arg(long, default_value_t = 128)]
        block: usize,
        #[arg(long, default_value_t = 4)]
        channels: usize,
        #[arg(long, value_enum, default_value_t = PresetArg::Stream)]
        preset: PresetArg,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
    },
    /// Play a synthetic signal on an output device (test mode: nothing is captured).
    Tone {
        /// Device id, part of the name, or "default".
        #[arg(long, default_value = "default")]
        device: String,
        #[arg(long, default_value_t = 440.0)]
        freq: f32,
        #[arg(long, default_value_t = -20.0)]
        level: f32,
        #[arg(long, value_enum, default_value_t = ToneArg::Beep)]
        kind: ToneArg,
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
    },
    /// Run a profile headless, printing status every second.
    Run {
        /// Profile file (TOML/JSON). Defaults to the active profile.
        #[arg(long)]
        profile: Option<PathBuf>,
        #[arg(long, default_value_t = 30.0)]
        seconds: f64,
        /// Audio host for this run (e.g. wasapi, wasapi-lowlatency, wasapi-exclusive);
        /// overrides the configuration without changing it.
        #[arg(long)]
        host: Option<String>,
        /// Latency mode for this run (overrides the configuration).
        #[arg(long, value_enum)]
        latency: Option<LatencyArg>,
    },
    /// Validate a profile file.
    Validate { file: PathBuf },
    /// Export a diagnostic report (runs the engine briefly).
    Diag {
        #[arg(long)]
        out: Option<PathBuf>,
        /// Keep device names and system details (anonymized by default).
        #[arg(long)]
        no_anonymize: bool,
    },
    /// Show configuration locations.
    Paths,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(std::env::var("NOVAMIX_LOG").unwrap_or_else(|_| "warn".into())).init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Devices { json, host } => devices(json, host),
        Cmd::Bench { rate, block, channels, preset, seconds } => bench(rate, block, channels, preset.into(), seconds),
        Cmd::Tone { device, freq, level, kind, seconds } => tone(&device, freq, level, kind, seconds),
        Cmd::Run { profile, seconds, host, latency } => run(profile, seconds, host, latency.map(Into::into)),
        Cmd::Validate { file } => validate(&file),
        Cmd::Diag { out, no_anonymize } => diag(out, !no_anonymize),
        Cmd::Paths => paths(),
    }
}

fn devices(json: bool, host: Option<String>) -> Result<()> {
    let backend = create_backend(host.as_deref()).map_err(|e| anyhow::anyhow!(e))?;
    let devices = backend.devices().map_err(|e| anyhow::anyhow!(e))?;
    let virt = novamix_device_windows::WindowsVirtualAdapter.status(&devices);
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "host": backend.host_name(), "devices": devices, "virtual": virt }))?);
        return Ok(());
    }
    println!("Host: {}   (available: {})", backend.host_name(), novamix_device_api::available_hosts().join(", "));
    for dir in [novamix_device_api::Direction::Input, novamix_device_api::Direction::Output] {
        println!("\n{}", if dir == novamix_device_api::Direction::Input { "INPUTS" } else { "OUTPUTS" });
        for d in devices.iter().filter(|d| d.direction == dir) {
            println!(
                "  {} {:<55} {:>6} Hz {:>2} ch  {:<9} {:<10} {}",
                if d.is_default { "*" } else { " " },
                d.name,
                d.default_sample_rate,
                d.channels,
                format!("{:?}", d.form).to_lowercase(),
                d.interface,
                d.id
            );
        }
    }
    println!("\nVIRTUAL ENDPOINTS (NovaMix driver installed: {})", virt.driver_installed);
    if let Some(p) = &virt.playback {
        println!("  playback (apps → mixer): {}", p.name);
    }
    if let Some(m) = &virt.mic_feed {
        println!("  virtual mic feed (mixer → apps): {} → {}", m.name, virt.mic_capture.as_ref().map(|c| c.name.as_str()).unwrap_or("?"));
    }
    for n in &virt.notes {
        println!("  note: {n}");
    }
    Ok(())
}

fn bench(rate: u32, block: usize, channels: usize, preset: VoicePreset, seconds: f64) -> Result<()> {
    let channels = channels.clamp(1, novamix_mixer_graph::MAX_CHANNELS);
    let mut cfg = EngineConfig::new(rate as f32);
    cfg.max_block = block.max(64);
    let (mut core, mut h) = create_engine(cfg);
    let push = |h: &mut novamix_audio_core::EngineHandles, c: Command| h.commands.push(c).map_err(|_| anyhow::anyhow!("command queue full"));
    for bus in 0..3u8 {
        push(&mut h, Command::BusEnabled { bus, enabled: true })?;
        push(&mut h, Command::BusOutput { bus, slot: Some(0) })?;
        push(&mut h, Command::BusLimiter { bus, params: novamix_dsp::params::LimiterParams::default() })?;
    }
    push(&mut h, Command::MasterSlot { slot: Some(0) })?;
    let chain = preset.chain();
    for ch in 0..channels as u8 {
        push(&mut h, Command::ChannelEnabled { ch, enabled: true })?;
        push(&mut h, Command::TestTone { ch, enabled: true, kind: ToneKind::PinkNoise, frequency_hz: 440.0, level_db: -18.0 })?;
        for k in StageKind::DEFAULT_ORDER {
            push(&mut h, Command::Stage { ch, params: chain.stage(k) })?;
        }
        for bus in 0..3u8 {
            push(&mut h, Command::Send { ch, bus, gain: 0.5, pre_fader: false })?;
        }
    }
    let blocks = (seconds * rate as f64 / block as f64) as usize;
    let mut buf = vec![0.0f32; block * 2];
    // Warm-up (caches, branch predictors, NR minimum tracking).
    for _ in 0..200 {
        core.process_master(&mut buf, 2);
    }
    let mut times = Vec::with_capacity(blocks);
    let start = Instant::now();
    for _ in 0..blocks {
        let t = Instant::now();
        core.process_master(&mut buf, 2);
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let wall = start.elapsed().as_secs_f64();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let budget = block as f64 / rate as f64 * 1e6;
    let pct = |p: f64| times[((p / 100.0) * (times.len() - 1) as f64) as usize];
    println!("NovaMix engine benchmark");
    println!("  {rate} Hz, block {block} frames (budget {budget:.1} µs), {channels} channels, preset {preset:?}, 3 buses with limiter");
    println!("  processed {:.1} s of audio in {:.2} s ({:.0}x real time)", seconds, wall, seconds / wall);
    for p in [50.0, 90.0, 99.0, 99.9] {
        let t = pct(p);
        println!("  p{p:<5} {t:>8.1} µs  ({:>5.1}% of budget)", t / budget * 100.0);
    }
    let max = *times.last().unwrap();
    println!("  max    {max:>8.1} µs  ({:>5.1}% of budget)", max / budget * 100.0);
    if cfg!(debug_assertions) {
        println!("  note: debug build; use `cargo run --release -p novamix-cli -- bench` for representative numbers");
    }
    if pct(99.0) > budget {
        bail!("p99 exceeds the block budget");
    }
    Ok(())
}

fn wait_running(h: &ControllerHandle, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if h.shared().status().running {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn tone(device: &str, freq: f32, level: f32, kind: ToneArg, seconds: f64) -> Result<()> {
    let backend = create_backend(None).map_err(|e| anyhow::anyhow!(e))?;
    let devices = backend.devices().map_err(|e| anyhow::anyhow!(e))?;
    let output = if device == "default" {
        OutputSpec::DefaultOutput
    } else {
        let d = devices
            .iter()
            .filter(|d| d.direction == novamix_device_api::Direction::Output)
            .find(|d| d.id == device || d.name.to_lowercase().contains(&device.to_lowercase()))
            .with_context(|| format!("output device '{device}' not found (see `novamix-cli devices`)"))?;
        OutputSpec::Device { device: DeviceRef::new(&d.id, &d.name) }
    };
    let mut p = default_profile();
    p.name = "cli-tone".into();
    for c in &mut p.channels {
        c.source = SourceSpec::None;
    }
    let kind = match kind {
        ToneArg::Sine => ToneKind::Sine,
        ToneArg::Pink => ToneKind::PinkNoise,
        ToneArg::White => ToneKind::WhiteNoise,
        ToneArg::Beep => ToneKind::Beep,
    };
    p.channels[0].source = SourceSpec::TestTone { kind, frequency_hz: freq, level_db: level };
    p.channels[0].chain = VoicePreset::Raw.chain();
    p.channels[0].sends = vec![Default::default(); p.buses.len()];
    p.channels[0].sends[0].enabled = true;
    p.buses[0].output = output;
    for b in p.buses.iter_mut().skip(1) {
        b.output = OutputSpec::None;
    }
    let mut h = ControllerHandle::spawn(AppConfig::default().audio, p, ControllerOptions { null_backend: false, device_watcher: false });
    if !wait_running(&h, Duration::from_secs(5)) {
        bail!("engine did not start");
    }
    let st = h.shared().status();
    println!("Playing {kind:?} {freq} Hz at {level} dBFS on {:?} for {seconds} s…", st.buses.first().and_then(|b| b.device_name.clone()));
    std::thread::sleep(Duration::from_secs_f64(seconds));
    h.shutdown();
    Ok(())
}

fn load_profile_file(path: &PathBuf) -> Result<Profile> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let loaded = novamix_config::migrate::parse_profile(&text, Format::from_path(path))?;
    for i in &loaded.issues {
        eprintln!("warning: {i}");
    }
    Ok(loaded.value)
}

fn run(profile: Option<PathBuf>, seconds: f64, host: Option<String>, latency: Option<LatencyMode>) -> Result<()> {
    let store = ConfigStore::default_location()?;
    let (mut config, notes) = store.load_config();
    for n in notes {
        eprintln!("config: {n}");
    }
    if host.is_some() {
        config.audio.host = host;
    }
    if let Some(l) = latency {
        config.audio.latency_mode = l;
    }
    let profile = match profile {
        Some(p) => load_profile_file(&p)?,
        None => store.load_profile(&config.active_profile).map(|l| l.value).unwrap_or_else(|_| default_profile()),
    };
    println!("Running profile '{}' for {seconds} s (Ctrl+C to stop)…", profile.name);
    let mut h = ControllerHandle::spawn(config.audio.clone(), profile.clone(), ControllerOptions { null_backend: false, device_watcher: true });
    let t0 = Instant::now();
    let mut last_log = 0;
    while t0.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_secs(1));
        let st = h.shared().status();
        let timing = h.shared().meters().map(|m| m.timing.snapshot()).unwrap_or_default();
        let mut line = format!(
            "[{:>5.0}s] {} {} Hz blk {} | load avg {:.1}% p99 {:.0}% max {:.0}% over {}",
            t0.elapsed().as_secs_f64(),
            if st.running { "RUN" } else { "STOP" },
            st.sample_rate,
            st.block_frames,
            timing.avg_load_pct,
            timing.p99_load_pct,
            timing.max_load_pct,
            timing.over_budget
        );
        for (i, c) in st.channels.iter().enumerate() {
            if c.state == EndpointState::Off {
                continue;
            }
            let (u, o) = c.stats.map(|s| (s.underruns, s.overruns)).unwrap_or((0, 0));
            line.push_str(&format!(" | {}: {} u{u} o{o}", profile.channels[i].name, short_state(&c.state)));
        }
        if let Some(l) = st.voice_latency_ms {
            line.push_str(&format!(" | voice {l:.1} ms"));
        }
        println!("{line}");
        let log = h.shared().log_snapshot();
        for e in log.iter().skip(last_log) {
            println!("    {:?} [{}] {}{}", e.level, e.category, e.message, if e.repeat > 1 { format!(" (x{})", e.repeat) } else { String::new() });
        }
        last_log = log.len();
    }
    h.shutdown();
    Ok(())
}

fn short_state(s: &EndpointState) -> String {
    match s {
        EndpointState::Off => "off".into(),
        EndpointState::Starting => "starting".into(),
        EndpointState::Running => "ok".into(),
        EndpointState::Idle => "idle".into(),
        EndpointState::TestTone => "tone".into(),
        EndpointState::Unavailable(r) => format!("unavailable ({r})"),
        EndpointState::Failed(r) => format!("FAILED ({r})"),
    }
}

fn validate(file: &PathBuf) -> Result<()> {
    let meta = std::fs::metadata(file)?;
    if meta.len() > novamix_config::store::MAX_IMPORT_BYTES {
        bail!("file too large ({} bytes)", meta.len());
    }
    let text = std::fs::read_to_string(file)?;
    match novamix_config::migrate::parse_profile(&text, Format::from_path(file)) {
        Ok(l) => {
            println!("OK: profile '{}' ({} channels, {} buses)", l.value.name, l.value.channels.len(), l.value.buses.len());
            if let Some(v) = l.migrated_from {
                println!("  migrated from schema v{v}");
            }
            for i in &l.issues {
                println!("  corrected: {i}");
            }
            let monitor = l.value.buses.iter().filter(|b| b.role == BusRole::Monitor).count();
            if monitor == 0 {
                println!("  note: no monitor bus");
            }
            Ok(())
        }
        Err(e) => bail!("invalid profile: {e}"),
    }
}

fn diag(out: Option<PathBuf>, anonymize: bool) -> Result<()> {
    let store = ConfigStore::default_location()?;
    let (config, _) = store.load_config();
    let profile = store.load_profile(&config.active_profile).map(|l| l.value).unwrap_or_else(|_| default_profile());
    let mut h = ControllerHandle::spawn(config.audio.clone(), profile.clone(), ControllerOptions { null_backend: false, device_watcher: false });
    wait_running(&h, Duration::from_secs(5));
    std::thread::sleep(Duration::from_secs(2));
    let report = build_report(h.shared(), &config, &profile, env!("CARGO_PKG_VERSION"), anonymize);
    h.shutdown();
    let path = out.unwrap_or_else(|| store.diagnostics_dir().join(format!("novamix-diag-{}.json", novamix_config::store::utc_timestamp())));
    novamix_config::store::write_atomic(&path, report.to_json().as_bytes())?;
    print!("{}", report.summary());
    println!("Report written to {}", path.display());
    Ok(())
}

fn paths() -> Result<()> {
    let s = ConfigStore::default_location()?;
    println!("root:        {}", s.root().display());
    println!("config:      {}", s.config_path().display());
    println!("profiles:    {}", s.profiles_dir().display());
    println!("backups:     {}", s.backups_dir().display());
    println!("logs:        {}", s.logs_dir().display());
    println!("diagnostics: {}", s.diagnostics_dir().display());
    Ok(())
}
