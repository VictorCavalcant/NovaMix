//! Offline engine tests: the core is driven directly, without devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use novamix_diagnostics::{RtEvent, StreamStats};
use novamix_dsp::params::{CompressorParams, StageParams, VoicePreset};
use novamix_dsp::signal::{ToneKind, goertzel_magnitude};

use super::*;

const SR: f32 = 48_000.0;

struct Rig {
    core: MixerCore,
    h: EngineHandles,
}

impl Rig {
    fn new() -> Self {
        let (core, h) = create_engine(EngineConfig::new(SR));
        let mut r = Self { core, h };
        // Bus 0 → slot 0 (master), bus 1 → slot 1 (bridge).
        r.cmd(Command::BusEnabled { bus: 0, enabled: true });
        r.cmd(Command::BusEnabled { bus: 1, enabled: true });
        r.cmd(Command::BusOutput { bus: 0, slot: Some(0) });
        r.cmd(Command::BusOutput { bus: 1, slot: Some(1) });
        r.cmd(Command::MasterSlot { slot: Some(0) });
        r.cmd(Command::MonitorBus { bus: Some(0) });
        r
    }

    fn cmd(&mut self, c: Command) {
        self.h.commands.push(c).expect("queue full");
    }

    fn tone(&mut self, ch: u8, freq: f32, db: f32) {
        self.cmd(Command::ChannelEnabled { ch, enabled: true });
        self.cmd(Command::TestTone { ch, enabled: true, kind: ToneKind::Sine, frequency_hz: freq, level_db: db });
    }

    /// Runs `frames` through the master callback and returns the left channel.
    fn run(&mut self, frames: usize, block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let mut buf = vec![0.0; block * 2];
        let mut done = 0;
        while done < frames {
            self.core.process_master(&mut buf, 2);
            out.extend(buf.chunks(2).map(|f| f[0]));
            done += block;
        }
        out
    }
}

fn level(x: &[f32], f: f32) -> f32 {
    goertzel_magnitude(x, f, SR)
}

#[test]
fn tone_reaches_master_only_when_sent() {
    let mut r = Rig::new();
    r.tone(0, 1000.0, -6.0);
    let out = r.run(9600, 480);
    assert!(level(&out[4800..], 1000.0) < 1e-4, "no send yet");
    r.cmd(Command::Send { ch: 0, bus: 0, gain: 1.0, pre_fader: false });
    let out = r.run(9600, 480);
    let m = level(&out[4800..], 1000.0);
    assert!((m - 0.5).abs() < 0.01, "{m}");
}

#[test]
fn fader_mute_and_pan_are_smooth() {
    let mut r = Rig::new();
    r.tone(0, 200.0, -6.0);
    r.cmd(Command::Send { ch: 0, bus: 0, gain: 1.0, pre_fader: false });
    let mut out = r.run(4800, 256);
    r.cmd(Command::ChannelMute { ch: 0, mute: true });
    out.extend(r.run(4800, 256));
    r.cmd(Command::ChannelMute { ch: 0, mute: false });
    r.cmd(Command::Fader { ch: 0, db: -12.0 });
    out.extend(r.run(4800, 256));
    let max_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    // A 200 Hz sine at 0.5 moves at most 0.013 per sample.
    assert!(max_step < 0.02, "click: {max_step}");
    assert!(out[6000..9600].iter().all(|v| v.abs() < 1e-6), "muted");
    let tail = level(&out[12_000..], 200.0);
    assert!((tail - 0.5 * 0.2512).abs() < 0.01, "{tail}");
}

#[test]
fn bus_output_goes_through_bridge() {
    let mut r = Rig::new();
    r.tone(1, 500.0, -12.0);
    r.cmd(Command::Send { ch: 1, bus: 1, gain: 1.0, pre_fader: false });
    let stats = StreamStats::new_shared();
    let cfg = BridgeConfig::for_blocks(48_000.0, 48_000.0, 480, 480, 5.0);
    let (w, rd) = bridge(cfg, stats, Arc::new(AtomicBool::new(true)));
    r.cmd(Command::AttachOutput { slot: 1, writer: Box::new(w) });
    let mut playback = PlaybackSource::new(rd);
    let master = r.run(4800, 480);
    assert!(level(&master[2400..], 500.0) < 1e-4, "not on master");
    // Consume the bridge like an output device would.
    let mut collected = Vec::new();
    for _ in 0..20 {
        r.run(480, 480);
        let mut dev = vec![0.0; 480 * 2];
        playback.on_output(&mut dev, 2);
        collected.extend(dev.chunks(2).map(|f| f[0]));
    }
    let m = level(&collected[4800..], 500.0);
    assert!((m - 0.2512).abs() < 0.01, "{m}");
}

#[test]
fn input_bridge_feeds_channel_and_replaced_reader_is_returned() {
    let mut r = Rig::new();
    r.cmd(Command::ChannelEnabled { ch: 2, enabled: true });
    r.cmd(Command::Send { ch: 2, bus: 0, gain: 1.0, pre_fader: false });
    let make = || {
        let cfg = BridgeConfig::for_blocks(44_100.0, 48_000.0, 441, 480, 5.0);
        bridge(cfg, StreamStats::new_shared(), Arc::new(AtomicBool::new(true)))
    };
    let (w1, rd1) = make();
    let cap_stats = rd1.stats().clone();
    r.cmd(Command::AttachInput { ch: 2, reader: Box::new(rd1) });
    let mut cap = CaptureSink::new(w1, ChannelMap::Auto);
    let mut out = Vec::new();
    let mut phase = 0.0f64;
    for _ in 0..100 {
        let mut dev = vec![0.0; 441];
        for s in dev.iter_mut() {
            *s = 0.5 * phase.sin() as f32;
            phase = (phase + 2.0 * std::f64::consts::PI * 1000.0 / 44_100.0) % (2.0 * std::f64::consts::PI);
        }
        cap.on_input(&dev, 1);
        out.extend(r.run(480, 480));
    }
    let m = level(&out[24_000..], 1000.0);
    assert_eq!(cap_stats.snapshot().underruns, 0);
    assert!((m - 0.5).abs() < 0.02, "{m}");

    let (_w2, rd2) = make();
    r.cmd(Command::AttachInput { ch: 2, reader: Box::new(rd2) });
    r.run(480, 480);
    let mut returned = 0;
    while let Ok(g) = r.h.garbage.pop() {
        if matches!(g, Garbage::Input(_)) {
            returned += 1;
        }
    }
    assert_eq!(returned, 1);
    assert_eq!(r.core.leaked(), 0);
}

#[test]
fn cue_replaces_monitor_mix() {
    let mut r = Rig::new();
    r.tone(0, 300.0, -6.0);
    r.tone(1, 3000.0, -6.0);
    r.cmd(Command::Send { ch: 0, bus: 0, gain: 1.0, pre_fader: false });
    r.cmd(Command::Send { ch: 1, bus: 1, gain: 1.0, pre_fader: false });
    let out = r.run(9600, 480);
    assert!(level(&out[4800..], 300.0) > 0.4);
    assert!(level(&out[4800..], 3000.0) < 1e-3);
    r.cmd(Command::CueMask { mask: 0b10 });
    let out = r.run(9600, 480);
    assert!(level(&out[4800..], 300.0) < 1e-3, "monitor mix replaced");
    assert!(level(&out[4800..], 3000.0) > 0.4, "cued bus audible");
}

#[test]
fn pre_fader_send_ignores_fader() {
    let mut r = Rig::new();
    r.tone(0, 1000.0, -6.0);
    r.cmd(Command::Fader { ch: 0, db: -90.0 });
    r.cmd(Command::Send { ch: 0, bus: 0, gain: 1.0, pre_fader: true });
    let out = r.run(9600, 480);
    assert!((level(&out[4800..], 1000.0) - 0.5).abs() < 0.01);
}

#[test]
fn dsp_stage_commands_apply() {
    let mut r = Rig::new();
    r.tone(0, 1000.0, -6.0);
    r.cmd(Command::Send { ch: 0, bus: 0, gain: 1.0, pre_fader: false });
    r.cmd(Command::Stage {
        ch: 0,
        params: StageParams::Compressor(CompressorParams { enabled: true, threshold_db: -30.0, ratio: 10.0, knee_db: 0.0, ..Default::default() }),
    });
    let out = r.run(48_000, 480);
    let m = level(&out[24_000..], 1000.0);
    assert!(m < 0.1, "compressed: {m}");
    let gr = r.h.meters.channels[0].compressor_gr_db.load();
    assert!(gr < -15.0, "{gr}");
}

#[test]
fn every_preset_runs_on_all_channels_within_budget() {
    let mut r = Rig::new();
    for ch in 0..8u8 {
        r.tone(ch, 200.0 + ch as f32 * 100.0, -20.0);
        let preset = VoicePreset::ALL[ch as usize % VoicePreset::ALL.len()];
        let chain = preset.chain();
        for k in novamix_dsp::params::StageKind::DEFAULT_ORDER {
            r.cmd(Command::Stage { ch, params: chain.stage(k) });
        }
        for bus in 0..2u8 {
            r.cmd(Command::Send { ch, bus, gain: 0.5, pre_fader: false });
        }
    }
    let out = r.run(48_000, 256);
    assert!(out.iter().all(|v| v.is_finite()));
    let t = r.h.meters.timing.snapshot();
    assert!(t.callbacks > 100);
    // Generous bound: debug builds of dependencies are slow; release runs
    // far below this (see tests/performance).
    assert!(t.p50_load_pct < 100.0, "p50 load {}", t.p50_load_pct);
}

#[test]
fn invalid_indexes_are_ignored_and_boxes_returned() {
    let mut r = Rig::new();
    r.cmd(Command::Fader { ch: 200, db: 0.0 });
    r.cmd(Command::Send { ch: 0, bus: 99, gain: 1.0, pre_fader: false });
    let (w, rd) = bridge(BridgeConfig::for_blocks(48e3, 48e3, 64, 64, 1.0), StreamStats::new_shared(), Arc::new(AtomicBool::new(true)));
    r.cmd(Command::AttachInput { ch: 77, reader: Box::new(rd) });
    r.cmd(Command::AttachOutput { slot: 99, writer: Box::new(w) });
    r.run(480, 480);
    let mut n = 0;
    while r.h.garbage.pop().is_ok() {
        n += 1;
    }
    assert_eq!(n, 2);
}

#[test]
fn source_clipping_is_reported() {
    let mut r = Rig::new();
    r.tone(0, 100.0, 0.0);
    r.run(4800, 480);
    let mut clipped = false;
    r.h.events.drain(|e| clipped |= matches!(e, RtEvent::InputClipped { channel: 0 }));
    assert!(clipped);
    assert!(r.h.meters.channels[0].source_clips.load(Ordering::Relaxed) > 0);
}

#[test]
fn panics_in_audio_thread_flag_failure() {
    let (core, h) = create_engine(EngineConfig::new(SR));
    let shared = Arc::new(Mutex::new(core));
    let ok = with_core(&shared, &h.failed, |_| panic!("boom"));
    assert!(!ok);
    assert!(h.failed.load(Ordering::Relaxed));
    let mut drv = MasterDriver::new(shared, h.failed.clone());
    let mut buf = vec![1.0; 64];
    drv.on_output(&mut buf, 2);
    assert!(buf.iter().all(|v| *v == 0.0));
}

#[test]
fn spectrum_tap_receives_samples() {
    let mut r = Rig::new();
    r.tone(0, 1000.0, -6.0);
    r.cmd(Command::SetTap { point: Some(TapPoint::ChannelPost(0)) });
    r.run(4800, 480);
    assert!(r.h.tap.slots() >= 4800);
}
