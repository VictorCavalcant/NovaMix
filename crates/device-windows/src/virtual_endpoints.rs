//! Detection of virtual audio endpoints.
//!
//! Priority:
//! 1. NovaMix driver endpoints (`NovaMix Virtual Playback`, `NovaMix Mic
//!    Feed` → `NovaMix Virtual Mic`).
//! 2. Development fallback: third-party virtual cables already installed
//!    (VB-Audio, VoiceMeeter, Virtual Audio Cable…). This is the "proof of
//!    concept with an existing virtual endpoint" path recorded in
//!    `docs/decisions.md`.
//!
//! The logic is name based and pure, so it is unit-tested on any platform.

use novamix_device_api::{CablePair, DeviceInfo, Direction, VirtualDeviceAdapter, VirtualDeviceStatus, VirtualProvider};

/// Endpoint names exposed by the NovaMix driver (see platform/windows-driver).
pub const NOVAMIX_PLAYBACK: &str = "NovaMix Virtual Playback";
pub const NOVAMIX_MIC_FEED: &str = "NovaMix Mic Feed";
pub const NOVAMIX_MIC: &str = "NovaMix Virtual Mic";

/// Substrings identifying well-known virtual cable products.
const VENDOR_HINTS: &[(&str, &str)] = &[
    ("VB-Audio Virtual Cable", "VB-CABLE"),
    ("VB-Audio Hi-Fi Cable", "VB-Audio Hi-Fi Cable"),
    ("VB-Audio Cable A", "VB-CABLE A"),
    ("VB-Audio Cable B", "VB-CABLE B"),
    ("VB-Audio Point", "VB-Audio Point"),
    ("VoiceMeeter", "VoiceMeeter"),
    ("Voicemeeter", "VoiceMeeter"),
    ("Virtual Audio Cable", "Virtual Audio Cable"),
    ("VB-Audio", "VB-Audio"),
    ("BlackHole", "BlackHole"),
    ("Virtual Cable", "Virtual Cable"),
];

fn vendor_of(name: &str) -> Option<&'static str> {
    VENDOR_HINTS.iter().find(|(k, _)| name.contains(k)).map(|(_, v)| *v)
}

fn is_novamix(name: &str) -> bool {
    name.starts_with("NovaMix")
}

/// Pairs render endpoints with their capture counterparts.
pub fn find_cables(devices: &[DeviceInfo]) -> Vec<CablePair> {
    let captures: Vec<&DeviceInfo> = devices.iter().filter(|d| d.direction == Direction::Input).collect();
    let mut pairs = Vec::new();
    for r in devices.iter().filter(|d| d.direction == Direction::Output) {
        let provider = if is_novamix(&r.name) {
            VirtualProvider::NovaMix
        } else if let Some(v) = vendor_of(&r.name) {
            VirtualProvider::ThirdParty(v.to_string())
        } else if r.is_virtual {
            VirtualProvider::ThirdParty("virtual".into())
        } else {
            continue;
        };
        let capture = if r.name.starts_with(NOVAMIX_MIC_FEED) {
            captures.iter().find(|c| c.name.starts_with(NOVAMIX_MIC)).copied()
        } else {
            // "X Input (…)" ↔ "X Output (…)", or identical names (VAC "Line 1").
            let swapped = r.name.replacen("Input", "Output", 1);
            captures.iter().find(|c| c.name == swapped && swapped != r.name).or_else(|| captures.iter().find(|c| c.name == r.name)).copied()
        };
        let label = r.name.split(" (").next().unwrap_or(&r.name).to_string();
        pairs.push(CablePair { provider, label, render: r.clone(), capture: capture.cloned() });
    }
    pairs
}

/// Windows virtual endpoint adapter.
#[derive(Debug, Default)]
pub struct WindowsVirtualAdapter;

impl VirtualDeviceAdapter for WindowsVirtualAdapter {
    fn status(&self, devices: &[DeviceInfo]) -> VirtualDeviceStatus {
        classify(devices)
    }
}

/// Builds the virtual endpoint status from a device list.
pub fn classify(devices: &[DeviceInfo]) -> VirtualDeviceStatus {
    let cables = find_cables(devices);
    let mut st = VirtualDeviceStatus { cables: cables.clone(), ..Default::default() };

    let nm_play = devices.iter().find(|d| d.direction == Direction::Output && d.name.starts_with(NOVAMIX_PLAYBACK));
    let nm_feed = devices.iter().find(|d| d.direction == Direction::Output && d.name.starts_with(NOVAMIX_MIC_FEED));
    let nm_mic = devices.iter().find(|d| d.direction == Direction::Input && d.name.starts_with(NOVAMIX_MIC));
    if nm_play.is_some() || nm_feed.is_some() {
        st.driver_installed = true;
        st.playback = nm_play.cloned();
        st.mic_feed = nm_feed.cloned();
        st.mic_capture = nm_mic.cloned();
        if st.mic_capture.is_none() && st.mic_feed.is_some() {
            st.notes.push("NovaMix Mic Feed found without NovaMix Virtual Mic: driver installation looks incomplete".into());
        }
        return st;
    }

    // Development fallback: third-party cables with both sides present.
    let mut usable: Vec<&CablePair> = cables.iter().filter(|c| c.capture.is_some()).collect();
    // Prefer the classic VB-CABLE for the microphone path.
    usable.sort_by_key(|c| match &c.provider {
        VirtualProvider::ThirdParty(p) if p == "VB-CABLE" => 0,
        VirtualProvider::ThirdParty(p) if p.starts_with("VB") => 1,
        _ => 2,
    });
    if let Some(mic) = usable.first() {
        st.mic_feed = Some(mic.render.clone());
        st.mic_capture = mic.capture.clone();
        st.notes.push(format!(
            "NovaMix driver not installed: using '{}' as development virtual microphone (apps must select '{}')",
            mic.render.name,
            mic.capture.as_ref().map(|c| c.name.as_str()).unwrap_or("?")
        ));
    }
    if let Some(play) = usable.get(1) {
        st.playback = Some(play.render.clone());
        st.notes.push(format!("Using '{}' as development virtual playback device (route apps to it)", play.render.name));
    } else {
        st.notes
            .push("No second virtual cable found: capture desktop audio with loopback of the default output, or install the NovaMix driver".into());
    }
    if usable.is_empty() {
        st.notes.push("No virtual endpoints found: install the NovaMix driver (or a virtual cable for development)".into());
    }
    st
}

/// For a playback endpoint chosen as "virtual playback", the capture side
/// to read it from (cable output) when it exists; otherwise the mixer uses
/// loopback capture of the render endpoint.
pub fn playback_capture_of<'a>(status: &'a VirtualDeviceStatus, render: &DeviceInfo) -> Option<&'a DeviceInfo> {
    status.cables.iter().find(|c| c.render.id == render.id).and_then(|c| c.capture.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use novamix_device_api::DeviceForm;

    fn dev(name: &str, dir: Direction) -> DeviceInfo {
        DeviceInfo {
            id: format!("id:{name}:{dir:?}"),
            name: name.into(),
            direction: dir,
            is_default: false,
            channels: 2,
            default_sample_rate: 48_000,
            sample_rates: vec![48_000],
            buffer_range: None,
            sample_format: "f32".into(),
            form: DeviceForm::Unknown,
            interface: "unknown".into(),
            is_virtual: false,
            manufacturer: None,
        }
    }

    fn machine() -> Vec<DeviceInfo> {
        vec![
            dev("Alto-falantes (High Definition Audio Device)", Direction::Output),
            dev("CABLE Input (VB-Audio Virtual Cable)", Direction::Output),
            dev("Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)", Direction::Output),
            dev("Microphone (High Definition Audio Device)", Direction::Input),
            dev("CABLE Output (VB-Audio Virtual Cable)", Direction::Input),
            dev("Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)", Direction::Input),
        ]
    }

    #[test]
    fn pairs_vb_audio_cables() {
        let cables = find_cables(&machine());
        assert_eq!(cables.len(), 2);
        assert!(cables.iter().all(|c| c.capture.is_some()));
        assert_eq!(cables[0].label, "CABLE Input");
    }

    #[test]
    fn fallback_assigns_mic_and_playback() {
        let st = classify(&machine());
        assert!(!st.driver_installed);
        assert_eq!(st.mic_feed.as_ref().unwrap().name, "CABLE Input (VB-Audio Virtual Cable)");
        assert_eq!(st.mic_capture.as_ref().unwrap().name, "CABLE Output (VB-Audio Virtual Cable)");
        let play = st.playback.as_ref().unwrap();
        assert_eq!(play.name, "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)");
        assert_eq!(playback_capture_of(&st, play).unwrap().name, "Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)");
    }

    #[test]
    fn novamix_driver_takes_priority() {
        let mut m = machine();
        m.push(dev("NovaMix Virtual Playback (NovaMix Audio)", Direction::Output));
        m.push(dev("NovaMix Mic Feed (NovaMix Audio)", Direction::Output));
        m.push(dev("NovaMix Virtual Mic (NovaMix Audio)", Direction::Input));
        let st = classify(&m);
        assert!(st.driver_installed);
        assert!(st.playback.unwrap().name.starts_with(NOVAMIX_PLAYBACK));
        assert!(st.mic_capture.unwrap().name.starts_with(NOVAMIX_MIC));
    }

    #[test]
    fn no_cables_means_notes() {
        let m = vec![dev("Speakers (Realtek)", Direction::Output)];
        let st = classify(&m);
        assert!(st.mic_feed.is_none() && st.playback.is_none());
        assert!(!st.notes.is_empty());
    }

    #[test]
    fn vac_style_identical_names() {
        let m = vec![dev("Line 1 (Virtual Audio Cable)", Direction::Output), dev("Line 1 (Virtual Audio Cable)", Direction::Input)];
        let c = find_cables(&m);
        assert_eq!(c.len(), 1);
        assert!(c[0].capture.is_some());
    }
}
