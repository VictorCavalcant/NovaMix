//! # novamix-device-windows
//!
//! Windows-specific integration:
//! * virtual endpoint detection (NovaMix driver, or third-party cables as a
//!   development fallback) — see [`virtual_endpoints`];
//! * endpoint change notifications (`IMMNotificationClient`) so device
//!   hot-plug is handled immediately instead of by polling only;
//! * MMCSS "Pro Audio" scheduling for engine threads we own;
//! * per-user autostart, OS description for diagnostics, shortcuts to the
//!   system sound settings.
//!
//! On other platforms the same API exists with no-op implementations.

use std::sync::Arc;

pub mod virtual_endpoints;

#[cfg(windows)]
mod win;

pub use virtual_endpoints::{WindowsVirtualAdapter, classify, find_cables, playback_capture_of};

/// Endpoint change reported by the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChange {
    Added(String),
    Removed(String),
    StateChanged(String),
    DefaultChanged { output: bool },
}

/// Keeps an OS notification registration alive.
pub struct DeviceWatcher {
    #[cfg(windows)]
    _inner: win::DeviceWatcher,
}

impl DeviceWatcher {
    /// Registers `callback` for endpoint changes. The callback runs on an
    /// OS thread: keep it short (e.g. send a message).
    pub fn new(callback: Arc<dyn Fn(DeviceChange) + Send + Sync>) -> Result<Self, String> {
        #[cfg(windows)]
        {
            Ok(Self { _inner: win::DeviceWatcher::new(callback)? })
        }
        #[cfg(not(windows))]
        {
            let _ = callback;
            Err("device notifications are not implemented on this platform".into())
        }
    }
}

/// Guard returned by [`boost_current_thread`]; reverts on drop.
pub struct ThreadBoost {
    #[cfg(windows)]
    _guard: Option<win::MmcssGuard>,
}

/// Requests MMCSS "Pro Audio" scheduling for the calling thread.
pub fn boost_current_thread() -> ThreadBoost {
    #[cfg(windows)]
    {
        ThreadBoost { _guard: win::boost_current_thread() }
    }
    #[cfg(not(windows))]
    {
        ThreadBoost {}
    }
}

/// Enables or disables starting NovaMix with the user session.
pub fn set_autostart(enabled: bool, command: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::set_autostart(enabled, command)
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, command);
        Err("autostart is not implemented on this platform".into())
    }
}

/// The registered autostart command, if any.
pub fn autostart_command() -> Option<String> {
    #[cfg(windows)]
    {
        win::autostart_command()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Human-readable OS name and build for diagnostic reports.
pub fn os_description() -> String {
    #[cfg(windows)]
    {
        win::os_description()
    }
    #[cfg(not(windows))]
    {
        std::env::consts::OS.to_string()
    }
}

/// Raw endpoint ids of the default render and capture devices.
pub fn default_endpoint_ids() -> (Option<String>, Option<String>) {
    #[cfg(windows)]
    {
        win::default_endpoint_ids()
    }
    #[cfg(not(windows))]
    {
        (None, None)
    }
}

/// Opens the system sound control panel.
pub fn open_sound_settings() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("control").arg("mmsys.cpl").spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("not supported".into())
    }
}

/// Opens a folder in the system file manager.
pub fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("not supported".into())
    }
}
