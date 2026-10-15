//! Processing-time budget tests.
//!
//! The quick test always runs; the soak test (`--ignored`) processes 30
//! minutes of audio offline through a fully loaded engine and checks that
//! nothing drifts, glitches or produces non-finite samples.

use std::time::Instant;

use novamix_audio_core::Command;
use novamix_dsp::params::{LimiterParams, StageKind, VoicePreset};
use novamix_dsp::signal::ToneKind;
use novamix_tests::{OfflineRig, SR};

fn loaded_rig(block: usize, channels: u8) -> OfflineRig {
    let mut rig = OfflineRig::new(block);
    for bus in 0..3u8 {
        rig.push(Command::BusEnabled { bus, enabled: true });
        rig.push(Command::BusOutput { bus, slot: Some(0) });
        rig.push(Command::BusLimiter { bus, params: LimiterParams::default() });
    }
    rig.push(Command::MasterSlot { slot: Some(0) });
    for ch in 0..channels {
        let chain = VoicePreset::ALL[ch as usize % 4].chain(); // every preset except Raw
        rig.push(Command::ChannelEnabled { ch, enabled: true });
        rig.push(Command::TestTone { ch, enabled: true, kind: ToneKind::PinkNoise, frequency_hz: 440.0, level_db: -18.0 });
        for k in StageKind::DEFAULT_ORDER {
            rig.push(Command::Stage { ch, params: chain.stage(k) });
        }
        for bus in 0..3u8 {
            rig.push(Command::Send { ch, bus, gain: 0.4, pre_fader: false });
        }
    }
    rig
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p / 100.0) * (v.len() - 1) as f64) as usize]
}

/// 8 fully processed channels at 48 kHz / 128 frames must fit the block
/// budget (2.67 ms) at the 99th percentile. DSP crates are optimized even in
/// dev builds (see the workspace profile), so this holds in `cargo test`.
#[test]
fn eight_channels_fit_the_128_frame_budget() {
    let block = 128;
    let mut rig = loaded_rig(block, 8);
    let budget_us = block as f64 / SR as f64 * 1e6;
    let mut buf = vec![0.0f32; block * 2];
    for _ in 0..200 {
        rig.core.process_master(&mut buf, 2);
    }
    let mut times = Vec::with_capacity(4000);
    for _ in 0..4000 {
        let t = Instant::now();
        rig.core.process_master(&mut buf, 2);
        times.push(t.elapsed().as_secs_f64() * 1e6);
        assert!(buf.iter().all(|v| v.is_finite()));
    }
    let p50 = percentile(&mut times, 50.0);
    let p99 = percentile(&mut times, 99.0);
    println!("8 ch @128: p50 {p50:.0} µs, p99 {p99:.0} µs (budget {budget_us:.0} µs)");
    assert!(p99 < budget_us, "p99 {p99:.0} µs exceeds budget {budget_us:.0} µs");
}

#[test]
#[ignore = "long-running soak test: cargo test -p novamix-tests --release -- --ignored"]
fn soak_thirty_minutes_offline() {
    let block = 480;
    let mut rig = loaded_rig(block, 4);
    let blocks = (30.0 * 60.0 * SR as f64 / block as f64) as usize;
    let mut buf = vec![0.0f32; block * 2];
    let t0 = Instant::now();
    for i in 0..blocks {
        rig.core.process_master(&mut buf, 2);
        if i % 1000 == 0 {
            assert!(buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "block {i}");
        }
    }
    let t = rig.handles.meters.timing.snapshot();
    println!(
        "30 min of audio in {:.1} s; load avg {:.1}% p99 {:.0}% max {:.0}%",
        t0.elapsed().as_secs_f64(),
        t.avg_load_pct,
        t.p99_load_pct,
        t.max_load_pct
    );
    assert_eq!(rig.core.leaked(), 0);
    assert!(t.p99_load_pct < 100.0);
}
