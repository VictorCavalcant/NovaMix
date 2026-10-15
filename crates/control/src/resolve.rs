//! Resolution of profile source/output specifications to concrete devices,
//! and construction of the routing graph used for feedback detection.

use std::collections::{BTreeSet, HashMap};

use novamix_config::{AudioSettings, BusRole, DeviceRef, OutputSpec, Profile, SourceSpec};
use novamix_device_api::{DeviceInfo, Direction, VirtualDeviceStatus};
use novamix_dsp::signal::ToneKind;
use novamix_mixer_graph::{BusId, BusNode, ChannelId, ChannelNode, Endpoint, GraphInput, LinkKind, SystemLink};

/// A concrete capture endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDevice {
    pub id: String,
    pub name: String,
    pub loopback: bool,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedSource {
    None,
    Device(ResolvedDevice),
    TestTone {
        kind: ToneKind,
        frequency_hz: f32,
        level_db: f32,
    },
    /// The source is configured but cannot be used right now.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedOutput {
    None,
    Device(ResolvedDevice),
    Unavailable(String),
}

fn find<'a>(devices: &'a [DeviceInfo], r: &DeviceRef, dir: Direction) -> Option<&'a DeviceInfo> {
    devices
        .iter()
        .find(|d| d.direction == dir && d.id == r.id)
        // The id can change (e.g. USB device on another port): fall back to
        // the display name.
        .or_else(|| devices.iter().find(|d| d.direction == dir && !r.name.is_empty() && d.name == r.name))
}

fn default_of(devices: &[DeviceInfo], dir: Direction) -> Option<&DeviceInfo> {
    devices.iter().find(|d| d.direction == dir && d.is_default)
}

fn dev(d: &DeviceInfo, loopback: bool) -> ResolvedDevice {
    ResolvedDevice { id: d.id.clone(), name: d.name.clone(), loopback, sample_rate: d.default_sample_rate }
}

pub fn resolve_source(spec: &SourceSpec, devices: &[DeviceInfo], virt: &VirtualDeviceStatus) -> ResolvedSource {
    match spec {
        SourceSpec::None => ResolvedSource::None,
        SourceSpec::TestTone { kind, frequency_hz, level_db } => {
            ResolvedSource::TestTone { kind: *kind, frequency_hz: *frequency_hz, level_db: *level_db }
        }
        SourceSpec::DefaultInput => match default_of(devices, Direction::Input) {
            Some(d) => ResolvedSource::Device(dev(d, false)),
            None => ResolvedSource::Unavailable("no default input device".into()),
        },
        SourceSpec::Input { device } => match find(devices, device, Direction::Input) {
            Some(d) => ResolvedSource::Device(dev(d, false)),
            None => ResolvedSource::Unavailable(format!("'{}' is not connected", device.name)),
        },
        SourceSpec::Loopback { device } => match find(devices, device, Direction::Output) {
            Some(d) => ResolvedSource::Device(dev(d, true)),
            None => ResolvedSource::Unavailable(format!("'{}' is not connected", device.name)),
        },
        SourceSpec::DefaultOutputLoopback => match default_of(devices, Direction::Output) {
            Some(d) => ResolvedSource::Device(dev(d, true)),
            None => ResolvedSource::Unavailable("no default output device".into()),
        },
        SourceSpec::VirtualPlayback => match &virt.playback {
            Some(render) => match novamix_device_windows::playback_capture_of(virt, render) {
                // Third-party cable: read its capture side (continuous stream).
                Some(cap) => ResolvedSource::Device(dev(cap, false)),
                // NovaMix driver: loopback-capture the playback endpoint.
                None => ResolvedSource::Device(dev(render, true)),
            },
            None => ResolvedSource::Unavailable("no virtual playback endpoint (install the NovaMix driver)".into()),
        },
    }
}

pub fn resolve_output(spec: &OutputSpec, role: BusRole, devices: &[DeviceInfo], virt: &VirtualDeviceStatus, audio: &AudioSettings) -> ResolvedOutput {
    let resolved = match spec {
        OutputSpec::None => return ResolvedOutput::None,
        OutputSpec::DefaultOutput => default_of(devices, Direction::Output).map(|d| dev(d, false)),
        OutputSpec::Device { device } => find(devices, device, Direction::Output).map(|d| dev(d, false)),
        OutputSpec::VirtualMic => virt.mic_feed.as_ref().map(|d| dev(d, false)),
    };
    match resolved {
        Some(d) => ResolvedOutput::Device(d),
        None if role == BusRole::Monitor => {
            // Recovery device for the monitor bus.
            let fallback =
                audio.fallback_output.as_ref().and_then(|r| find(devices, r, Direction::Output)).or_else(|| default_of(devices, Direction::Output));
            match fallback {
                Some(d) => ResolvedOutput::Device(dev(d, false)),
                None => ResolvedOutput::Unavailable("no output device available".into()),
            }
        }
        None => ResolvedOutput::Unavailable(match spec {
            OutputSpec::VirtualMic => "no virtual microphone endpoint (install the NovaMix driver)".into(),
            OutputSpec::Device { device } => format!("'{}' is not connected", device.name),
            _ => "device not available".into(),
        }),
    }
}

/// Builds the routing graph of a profile with its resolved endpoints.
pub fn graph_input(
    profile: &Profile,
    sources: &[ResolvedSource],
    outputs: &[ResolvedOutput],
    devices: &[DeviceInfo],
    virt: &VirtualDeviceStatus,
) -> GraphInput {
    let mut device_names: HashMap<String, String> = devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
    let channels = profile
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.enabled)
        .map(|(i, c)| ChannelNode {
            id: ChannelId(i as u8),
            name: c.name.clone(),
            source: match sources.get(i) {
                Some(ResolvedSource::Device(d)) => Some(if d.loopback { Endpoint::loopback(&d.id) } else { Endpoint::capture(&d.id) }),
                _ => None,
            },
            is_microphone: c.is_microphone,
        })
        .collect();
    let buses = profile
        .buses
        .iter()
        .enumerate()
        .map(|(i, b)| BusNode {
            id: BusId(i as u8),
            name: b.name.clone(),
            output: match outputs.get(i) {
                Some(ResolvedOutput::Device(d)) => Some(d.id.clone()),
                _ => None,
            },
            is_monitor: b.role == BusRole::Monitor,
        })
        .collect();
    let sends = crate::reconcile::effective_sends(profile);
    let mut active = Vec::new();
    for (ci, c) in profile.channels.iter().enumerate() {
        if !c.enabled {
            continue;
        }
        for (bi, (gain, _)) in sends[ci].iter().enumerate().take(profile.buses.len()) {
            if *gain > 0.0 {
                active.push((ChannelId(ci as u8), BusId(bi as u8)));
            }
        }
    }
    let links = virt
        .cables
        .iter()
        .filter_map(|c| {
            c.capture.as_ref().map(|cap| SystemLink {
                render_device: c.render.id.clone(),
                capture_device: cap.id.clone(),
                kind: LinkKind::VirtualCable,
            })
        })
        .collect();
    let speaker_devices: BTreeSet<String> = devices
        .iter()
        .filter(|d| d.direction == Direction::Output && d.form.is_loudspeaker() && !d.is_virtual && !virt.cables.iter().any(|c| c.render.id == d.id))
        .map(|d| d.id.clone())
        .collect();
    for c in &virt.cables {
        device_names.entry(c.render.id.clone()).or_insert_with(|| c.render.name.clone());
    }
    GraphInput { channels, buses, sends: active, links, speaker_devices, device_names }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novamix_config::default_profile;
    use novamix_device_api::DeviceForm;

    fn d(id: &str, name: &str, dir: Direction, default: bool) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
            direction: dir,
            is_default: default,
            channels: 2,
            default_sample_rate: 48_000,
            sample_rates: vec![48_000],
            buffer_range: None,
            sample_format: "f32".into(),
            form: DeviceForm::Unknown,
            interface: "usb".into(),
            is_virtual: false,
            manufacturer: None,
        }
    }

    fn devices() -> Vec<DeviceInfo> {
        vec![
            d("spk", "Speakers", Direction::Output, true),
            d("cin", "CABLE Input (VB-Audio Virtual Cable)", Direction::Output, false),
            d("hin", "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)", Direction::Output, false),
            d("mic", "USB Mic", Direction::Input, true),
            d("cout", "CABLE Output (VB-Audio Virtual Cable)", Direction::Input, false),
            d("hout", "Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)", Direction::Input, false),
        ]
    }

    #[test]
    fn resolves_default_profile_with_dev_cables() {
        let devs = devices();
        let virt = novamix_device_windows::classify(&devs);
        let p = default_profile();
        let src: Vec<ResolvedSource> = p.channels.iter().map(|c| resolve_source(&c.source, &devs, &virt)).collect();
        assert!(matches!(&src[0], ResolvedSource::Device(d) if d.id == "mic"));
        assert!(matches!(&src[1], ResolvedSource::Device(d) if d.id == "hout" && !d.loopback));
        assert_eq!(src[2], ResolvedSource::None);
        let audio = AudioSettings::default();
        let out: Vec<ResolvedOutput> = p.buses.iter().map(|b| resolve_output(&b.output, b.role, &devs, &virt, &audio)).collect();
        assert!(matches!(&out[0], ResolvedOutput::Device(d) if d.id == "spk"));
        assert!(matches!(&out[1], ResolvedOutput::Device(d) if d.id == "cin"));
        assert_eq!(out[2], ResolvedOutput::None);
        let g = graph_input(&p, &src, &out, &devs, &virt);
        assert!(novamix_mixer_graph::analyze(&g).cycles.is_empty());
    }

    #[test]
    fn name_fallback_when_id_changes() {
        let devs = devices();
        let virt = VirtualDeviceStatus::default();
        let spec = SourceSpec::Input { device: DeviceRef::new("old-id", "USB Mic") };
        assert!(matches!(resolve_source(&spec, &devs, &virt), ResolvedSource::Device(d) if d.id == "mic"));
        let spec = SourceSpec::Input { device: DeviceRef::new("gone", "Gone Mic") };
        assert!(matches!(resolve_source(&spec, &devs, &virt), ResolvedSource::Unavailable(_)));
    }

    #[test]
    fn loopback_of_monitor_creates_cycle() {
        let devs = devices();
        let virt = novamix_device_windows::classify(&devs);
        let mut p = default_profile();
        p.channels[1].source = SourceSpec::DefaultOutputLoopback;
        let src: Vec<ResolvedSource> = p.channels.iter().map(|c| resolve_source(&c.source, &devs, &virt)).collect();
        let audio = AudioSettings::default();
        let out: Vec<ResolvedOutput> = p.buses.iter().map(|b| resolve_output(&b.output, b.role, &devs, &virt, &audio)).collect();
        let g = graph_input(&p, &src, &out, &devs, &virt);
        let a = novamix_mixer_graph::analyze(&g);
        assert_eq!(a.cycles.len(), 1, "{a:?}");
    }

    #[test]
    fn monitor_falls_back_to_default_output() {
        let devs = devices();
        let virt = VirtualDeviceStatus::default();
        let spec = OutputSpec::Device { device: DeviceRef::new("x", "Unplugged Headset") };
        let out = resolve_output(&spec, BusRole::Monitor, &devs, &virt, &AudioSettings::default());
        assert!(matches!(out, ResolvedOutput::Device(d) if d.id == "spk"));
        let out = resolve_output(&spec, BusRole::Stream, &devs, &virt, &AudioSettings::default());
        assert!(matches!(out, ResolvedOutput::Unavailable(_)));
    }
}
