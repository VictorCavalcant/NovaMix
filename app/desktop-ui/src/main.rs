//! NovaMix desktop application.
//!
//! Command line:
//! * `--minimized`  start hidden in the tray (used by autostart)
//! * `--safe-mode`  start with every effect bypassed and a minimal route
//! * `--no-audio`   run the UI without touching audio devices

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod hotkeys;
mod i18n;
mod icon;
mod theme;
mod tray;
mod views;
mod widgets;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use novamix_config::{ConfigStore, ResumeBehavior, Scenario, safe_mode_of, scenario_profile};
use novamix_control::{ControllerHandle, ControllerOptions};
use tracing_subscriber::EnvFilter;

struct Args {
    minimized: bool,
    safe_mode: bool,
    no_audio: bool,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| a.iter().any(|x| x == f);
    Args { minimized: has("--minimized"), safe_mode: has("--safe-mode"), no_audio: has("--no-audio") }
}

fn init_logging(logs: &Path, level: &str, keep_days: u32) -> tracing_appender::non_blocking::WorkerGuard {
    // Prune old log files.
    if let Ok(entries) = std::fs::read_dir(logs) {
        let limit = std::time::Duration::from_secs(keep_days as u64 * 86_400);
        for e in entries.flatten() {
            if let Ok(age) = e.metadata().and_then(|m| m.modified()).map(|t| t.elapsed().unwrap_or_default())
                && age > limit
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let appender = tracing_appender::rolling::daily(logs, "novamix.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("NOVAMIX_LOG").unwrap_or_else(|_| EnvFilter::new(format!("warn,novamix={level}")));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(writer).with_ansi(false).init();
    guard
}

/// Writes a local crash report (never uploaded) and keeps the default hook.
fn install_panic_hook(dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let path = dir.join(format!("crash-{}.txt", novamix_config::store::utc_timestamp()));
        let text = format!(
            "NovaMix {} crash report (local only, no audio)\nOS: {}\nThread: {:?}\n\n{info}\n\nBacktrace:\n{bt}\n",
            env!("CARGO_PKG_VERSION"),
            novamix_device_windows::os_description(),
            std::thread::current().name()
        );
        let _ = std::fs::write(&path, text);
        tracing::error!("panic: {info}");
        default(info);
    }));
}

/// Returns the newest unseen crash report and marks all as seen.
fn take_crash_notice(dir: &Path) -> Option<String> {
    let mut newest: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let name = p.file_name()?.to_str()?.to_string();
        if name.starts_with("crash-") && name.ends_with(".txt") {
            if newest.as_ref().map(|n| p > *n).unwrap_or(true) {
                newest = Some(p.clone());
            }
            let _ = std::fs::rename(&p, p.with_extension("seen.txt"));
        }
    }
    newest.map(|p| p.with_extension("seen.txt").display().to_string())
}

/// Single-instance guard: an exclusive lock on a file in the data folder.
fn acquire_instance_lock(store: &ConfigStore) -> Result<Option<File>> {
    let path = store.root().join("novamix.lock");
    let f = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

fn main() -> Result<()> {
    let args = parse_args();
    let store = ConfigStore::default_location()?;
    store.ensure_dirs()?;
    let (mut config, notes) = store.load_config();
    let _log_guard = init_logging(&store.logs_dir(), config.diagnostics.log_level.as_filter(), config.diagnostics.keep_log_days);
    install_panic_hook(store.diagnostics_dir());
    for n in &notes {
        tracing::warn!("config: {n}");
    }
    tracing::info!("NovaMix {} starting on {}", env!("CARGO_PKG_VERSION"), novamix_device_windows::os_description());

    let Some(_instance_lock) = acquire_instance_lock(&store)? else {
        let _ = rfd::MessageDialog::new()
            .set_title("NovaMix")
            .set_description("O NovaMix já está em execução (verifique a bandeja do sistema).\nNovaMix is already running (check the system tray).")
            .set_level(rfd::MessageLevel::Info)
            .show();
        return Ok(());
    };

    // First run: install the scenario templates as profiles.
    if store.list_profiles().is_empty() {
        for s in Scenario::ALL {
            let _ = store.save_profile(&scenario_profile(s));
        }
    }
    let (profile, note) = app::startup_profile(&store, &config);
    if let Some(n) = note {
        tracing::warn!("{n}");
    }
    config.active_profile = profile.name.clone();
    let safe_mode = args.safe_mode || config.general.resume == ResumeBehavior::SafeMode;
    let crash_notice = take_crash_notice(&store.diagnostics_dir());
    let initial = if safe_mode { safe_mode_of(&profile) } else { profile.clone() };
    let controller =
        ControllerHandle::spawn(config.audio.clone(), initial, ControllerOptions { null_backend: args.no_audio, device_watcher: !args.no_audio });

    let start_hidden = args.minimized || config.general.start_minimized;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NovaMix")
            .with_app_id("novamix")
            .with_inner_size([1280.0, 780.0])
            .with_min_inner_size([420.0, 260.0])
            .with_icon(Arc::new(egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 })),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "NovaMix",
        options,
        Box::new(move |cc| Ok(Box::new(app::NovaMixApp::new(cc, store, config, profile, controller, start_hidden, safe_mode, crash_notice)))),
    )
    .map_err(|e| anyhow::anyhow!("UI error: {e}"))?;
    Ok(())
}
