//! Win32 integrations (compiled on Windows only).

use std::sync::Arc;

use windows::Win32::Foundation::{HANDLE, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, eCapture, eConsole,
    eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW};
use windows::core::{PCWSTR, implement};

use crate::DeviceChange;

fn pcwstr_to_string(p: &PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the OS passes a valid NUL-terminated UTF-16 string.
    unsafe { p.to_string().unwrap_or_default() }
}

fn ensure_com() {
    // SAFETY: initializing COM for the calling thread; repeated calls are
    // harmless (S_FALSE / RPC_E_CHANGED_MODE are ignored).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[implement(IMMNotificationClient)]
struct Notifier {
    callback: Arc<dyn Fn(DeviceChange) + Send + Sync>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        (self.callback)(DeviceChange::StateChanged(pcwstr_to_string(id)));
        Ok(())
    }
    fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
        (self.callback)(DeviceChange::Added(pcwstr_to_string(id)));
        Ok(())
    }
    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        (self.callback)(DeviceChange::Removed(pcwstr_to_string(id)));
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if role == eConsole {
            (self.callback)(DeviceChange::DefaultChanged { output: flow == eRender });
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Registers for endpoint notifications for as long as it lives.
pub struct DeviceWatcher {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

// SAFETY: the enumerator and client are free-threaded COM objects created
// in the multithreaded apartment.
unsafe impl Send for DeviceWatcher {}

impl DeviceWatcher {
    pub fn new(callback: Arc<dyn Fn(DeviceChange) + Send + Sync>) -> Result<Self, String> {
        ensure_com();
        // SAFETY: standard COM activation and registration calls.
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
            let client: IMMNotificationClient = Notifier { callback }.into();
            enumerator.RegisterEndpointNotificationCallback(&client).map_err(|e| e.to_string())?;
            Ok(Self { enumerator, client })
        }
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        // SAFETY: unregistering the client registered in `new`.
        unsafe {
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

/// Endpoint id strings of the current default devices (render, capture).
pub fn default_endpoint_ids() -> (Option<String>, Option<String>) {
    ensure_com();
    // SAFETY: COM calls on a freshly created enumerator.
    unsafe {
        let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return (None, None);
        };
        let get = |flow| enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| d.GetId().ok()).and_then(|p| p.to_string().ok());
        (get(eRender), get(eCapture))
    }
}

/// MMCSS registration of the current thread ("Pro Audio" task).
pub struct MmcssGuard(HANDLE);

// SAFETY: the handle is only used to revert the characteristics.
unsafe impl Send for MmcssGuard {}

impl Drop for MmcssGuard {
    fn drop(&mut self) {
        // SAFETY: reverting a handle returned by AvSetMmThreadCharacteristicsW.
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

pub fn boost_current_thread() -> Option<MmcssGuard> {
    let mut index = 0u32;
    let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    // SAFETY: task name is a valid NUL-terminated UTF-16 string.
    unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(task.as_ptr()), &mut index).ok().map(MmcssGuard) }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "NovaMix";

pub fn set_autostart(enabled: bool, command: &str) -> Result<(), String> {
    let key = windows_registry::CURRENT_USER.create(RUN_KEY).map_err(|e| e.to_string())?;
    if enabled {
        key.set_string(RUN_VALUE, command).map_err(|e| e.to_string())
    } else {
        match key.remove_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(_) => Ok(()), // already absent
        }
    }
}

pub fn autostart_command() -> Option<String> {
    windows_registry::CURRENT_USER.open(RUN_KEY).ok()?.get_string(RUN_VALUE).ok()
}

pub fn os_description() -> String {
    let Ok(key) = windows_registry::LOCAL_MACHINE.open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else {
        return "Windows".into();
    };
    let product = key.get_string("ProductName").unwrap_or_else(|_| "Windows".into());
    let display = key.get_string("DisplayVersion").unwrap_or_default();
    let build = key.get_string("CurrentBuildNumber").unwrap_or_default();
    let ubr = key.get_u32("UBR").ok();
    // ProductName still says "Windows 10" on Windows 11 builds.
    let product = if build.parse::<u32>().map(|b| b >= 22_000).unwrap_or(false) { product.replacen("Windows 10", "Windows 11", 1) } else { product };
    match ubr {
        Some(u) => format!("{product} {display} (build {build}.{u})"),
        None => format!("{product} {display} (build {build})"),
    }
}
