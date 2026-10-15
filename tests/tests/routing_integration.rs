//! End-to-end routing: profile → reconciler → engine → outputs.

use novamix_config::{SoloMode, SourceSpec, default_profile};
use novamix_dsp::params::VoicePreset;
use novamix_dsp::signal::ToneKind;
use novamix_tests::{OfflineRig, tone_level};

/// Default profile with test tones instead of devices:
/// mic (ch0) = 1 kHz, desktop (ch1) = 3 kHz.
fn tones_profile() -> novamix_config::Profile {
    let mut p = default_profile();
    p.channels[0].source = SourceSpec::TestTone { kind: ToneKind::Sine, frequency_hz: 1000.0, level_db: -12.0 };
    p.channels[0].chain = VoicePreset::Raw.chain();
    p.channels[0].mono = false;
    p.channels[1].source = SourceSpec::TestTone { kind: ToneKind::Sine, frequency_hz: 3000.0, level_db: -12.0 };
    p
}

#[test]
fn default_route_sends_mic_to_voice_and_desktop_to_monitor() {
    let p = tones_profile();
    let mut rig = OfflineRig::new(480);
    rig.apply_profile(None, &p, &[1, 2]);
    let (monitor, bridged) = rig.run(48_000);
    let voice = &bridged.iter().find(|(b, _)| *b == 1).unwrap().1;
    let stream = &bridged.iter().find(|(b, _)| *b == 2).unwrap().1;
    let a = 0.2512; // -12 dBFS
    // Monitor: desktop only (mic monitoring is off by default).
    assert!((tone_level(&monitor[24_000..], 3000.0) - a).abs() < 0.01);
    assert!(tone_level(&monitor[24_000..], 1000.0) < 1e-3, "mic must not reach the monitor by default");
    // Voice bus: mic only (limited at -1 dBFS, tone is below).
    assert!((tone_level(&voice[24_000..], 1000.0) - a).abs() < 0.01);
    assert!(tone_level(&voice[24_000..], 3000.0) < 1e-3);
    // Stream bus: both.
    assert!(tone_level(&stream[24_000..], 1000.0) > 0.2);
    assert!(tone_level(&stream[24_000..], 3000.0) > 0.2);
}

#[test]
fn incremental_profile_changes_apply_live() {
    let p = tones_profile();
    let mut rig = OfflineRig::new(256);
    rig.apply_profile(None, &p, &[1]);
    rig.run(9600);
    // Mute desktop, enable mic monitoring.
    let mut q = p.clone();
    q.channels[1].mute = true;
    q.channels[0].sends[0].enabled = true;
    rig.apply_profile(Some(&p), &q, &[1]);
    let (monitor, _) = rig.run(24_000);
    assert!(tone_level(&monitor[9600..], 3000.0) < 1e-3, "desktop muted");
    assert!(tone_level(&monitor[9600..], 1000.0) > 0.2, "mic monitoring on");
}

#[test]
fn solo_monitor_only_keeps_voice_bus_intact() {
    let p = tones_profile();
    let mut q = p.clone();
    q.solo_mode = SoloMode::MonitorOnly;
    q.channels[1].solo = true; // solo desktop
    q.channels[0].sends[0].enabled = true; // mic also on monitor
    let mut rig = OfflineRig::new(480);
    rig.apply_profile(None, &q, &[1]);
    let (monitor, bridged) = rig.run(48_000);
    let voice = &bridged[0].1;
    assert!(tone_level(&monitor[24_000..], 1000.0) < 1e-3, "non-soloed mic silenced on monitor");
    assert!(tone_level(&voice[24_000..], 1000.0) > 0.2, "voice bus unaffected by solo");

    let mut all = q.clone();
    all.solo_mode = SoloMode::AllBuses;
    rig.apply_profile(Some(&q), &all, &[1]);
    let (_, bridged) = rig.run(48_000);
    assert!(tone_level(&bridged[0].1[24_000..], 1000.0) < 1e-3, "solo-in-place silences everywhere");
}

#[test]
fn bus_cue_pre_listens_stream_mix_on_monitor() {
    let mut p = tones_profile();
    p.channels[1].sends[0].enabled = false; // nothing on monitor
    p.buses[2].cue = true; // cue the stream bus
    let mut rig = OfflineRig::new(480);
    rig.apply_profile(None, &p, &[1, 2]);
    let (monitor, _) = rig.run(48_000);
    assert!(tone_level(&monitor[24_000..], 1000.0) > 0.2);
    assert!(tone_level(&monitor[24_000..], 3000.0) > 0.2);
}

#[test]
fn feedback_loop_is_detected_and_fixed() {
    use novamix_config::DeviceRef;
    use novamix_control::{graph_input, resolve_output, resolve_source};
    use novamix_device_api::{DeviceForm, DeviceInfo, Direction};
    let dev = |id: &str, name: &str, dir, default| DeviceInfo {
        id: id.into(),
        name: name.into(),
        direction: dir,
        is_default: default,
        channels: 2,
        default_sample_rate: 48_000,
        sample_rates: vec![48_000],
        buffer_range: None,
        sample_format: "f32".into(),
        form: DeviceForm::Speakers,
        interface: "builtin".into(),
        is_virtual: false,
        manufacturer: None,
    };
    let devices = vec![dev("spk", "Speakers", Direction::Output, true), dev("mic", "Mic", Direction::Input, true)];
    let virt = Default::default();
    let mut p = default_profile();
    // Desktop captures the loopback of the speakers it is monitored on.
    p.channels[1].source = SourceSpec::Loopback { device: DeviceRef::new("spk", "Speakers") };
    let audio = novamix_config::AudioSettings::default();
    let src: Vec<_> = p.channels.iter().map(|c| resolve_source(&c.source, &devices, &virt)).collect();
    let out: Vec<_> = p.buses.iter().map(|b| resolve_output(&b.output, b.role, &devices, &virt, &audio)).collect();
    let g = graph_input(&p, &src, &out, &devices, &virt);
    let a = novamix_mixer_graph::analyze(&g);
    assert_eq!(a.cycles.len(), 1);
    let fixes = novamix_mixer_graph::suggest_fixes(&g);
    assert_eq!(fixes.len(), 1);
    for (c, b) in &fixes {
        p.channels[c.index()].sends[b.index()].enabled = false;
    }
    let g = graph_input(&p, &src, &out, &devices, &virt);
    assert!(novamix_mixer_graph::analyze(&g).cycles.is_empty());
}
