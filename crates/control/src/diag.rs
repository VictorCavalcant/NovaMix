//! Builds the privacy-aware diagnostic report from the controller state.

use std::sync::atomic::Ordering;

use novamix_config::{AppConfig, Profile};
use novamix_diagnostics::{DeviceDiag, DiagnosticReport, StreamDiag, time};

use crate::controller::Shared;

/// Collects a diagnostic report. Contains no audio. With `anonymize`,
/// device names/ids and user/system identifiers are replaced.
pub fn build_report(shared: &Shared, config: &AppConfig, profile: &Profile, app_version: &str, anonymize: bool) -> DiagnosticReport {
    let status = shared.status();
    let timing = shared.meters().map(|m| m.timing.snapshot()).unwrap_or_default();
    let devices = status
        .devices
        .iter()
        .map(|d| DeviceDiag {
            id: d.id.clone(),
            name: d.name.clone(),
            direction: format!("{:?}", d.direction).to_lowercase(),
            is_default: d.is_default,
            is_virtual: d.is_virtual
                || status.virtual_status.cables.iter().any(|c| c.render.id == d.id || c.capture.as_ref().map(|x| x.id == d.id).unwrap_or(false)),
            channels: d.channels,
            sample_rate: d.default_sample_rate,
        })
        .collect();
    let mut streams = Vec::new();
    for (i, c) in status.channels.iter().enumerate() {
        if let (Some(info), Some(stats)) = (&c.info, c.stats) {
            streams.push(StreamDiag {
                role: format!("channel {} ({})", i + 1, profile.channels.get(i).map(|c| c.name.as_str()).unwrap_or("?")),
                device: info.device_name.clone(),
                sample_rate: info.sample_rate,
                channels: info.channels,
                buffer_frames: info.buffer_frames,
                sample_format: info.sample_format.clone(),
                stats,
            });
        }
    }
    for (i, b) in status.buses.iter().enumerate() {
        if let Some(info) = &b.info {
            streams.push(StreamDiag {
                role: format!("bus {} ({})", i + 1, profile.buses.get(i).map(|b| b.name.as_str()).unwrap_or("?")),
                device: info.device_name.clone(),
                sample_rate: info.sample_rate,
                channels: info.channels,
                buffer_frames: info.buffer_frames,
                sample_format: info.sample_format.clone(),
                stats: b.stats.unwrap_or(novamix_diagnostics::StreamStatsSnapshot {
                    state: novamix_diagnostics::StreamState::Running,
                    underruns: 0,
                    overruns: 0,
                    callbacks: timing.callbacks,
                    frames: 0,
                    errors: 0,
                    fill_frames: 0,
                    target_frames: 0,
                    correction_ppm: 0,
                    last_callback_frames: status.block_frames,
                }),
            });
        }
    }
    let engine = serde_json::json!({
        "running": status.running,
        "host": status.host,
        "sample_rate": status.sample_rate,
        "block_frames": status.block_frames,
        "clock": status.clock,
        "voice_latency_ms": status.voice_latency_ms,
        "engine_restarts": status.engine_restarts,
        "last_error": status.last_error,
        "virtual_driver_installed": status.virtual_status.driver_installed,
        "virtual_notes": status.virtual_status.notes,
        "channel_states": status.channels.iter().map(|c| &c.state).collect::<Vec<_>>(),
        "bus_states": status.buses.iter().map(|b| &b.state).collect::<Vec<_>>(),
    });
    let route = serde_json::json!({
        "cycles": status.route.cycles,
        "warnings": status.route.warnings,
    });
    let config_summary = serde_json::json!({
        "profile": profile.name,
        "channels": profile.channels.iter().map(|c| serde_json::json!({
            "name": c.name,
            "enabled": c.enabled,
            "source": c.source.key(),
            "mic": c.is_microphone,
            "effects_enabled": c.chain.enabled_count(),
            "bypass": c.chain.bypass,
        })).collect::<Vec<_>>(),
        "buses": profile.buses.iter().map(|b| serde_json::json!({
            "name": b.name,
            "role": b.role,
            "output": b.output.key(),
        })).collect::<Vec<_>>(),
        "audio": {
            "preferred_sample_rate": config.audio.preferred_sample_rate,
            "latency_mode": config.audio.latency_mode,
            "buffer_frames": config.audio.effective_buffer_frames(),
            "auto_reconnect": config.audio.auto_reconnect,
        },
    });
    let mut report = DiagnosticReport {
        generated_at: time::now_iso8601(),
        app_version: app_version.to_string(),
        os: novamix_device_windows::os_description(),
        anonymized: false,
        engine,
        timing,
        devices,
        streams,
        route,
        config_summary,
        events: shared.log_snapshot(),
        dropped_rt_events: shared.dropped_rt_events.load(Ordering::Relaxed),
    };
    if anonymize {
        report.anonymize();
    }
    report
}
