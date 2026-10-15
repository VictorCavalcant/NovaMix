//! Shared fixtures for the integration test suite.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use novamix_audio_core::{BridgeConfig, Command, EngineConfig, EngineHandles, MixerCore, PlaybackSource, bridge, create_engine};
use novamix_config::Profile;
use novamix_diagnostics::StreamStats;
use novamix_dsp::signal;

pub const SR: f32 = 48_000.0;

/// Speech-like test signal: 250 ms tone bursts (with harmonics) separated
/// by 250 ms gaps, over a constant noise floor. Returns (signal, mask) where
/// mask[i] is true inside bursts.
pub fn speech_like(seconds: f32, noise_amp: f32, seed: u32) -> (Vec<f32>, Vec<bool>) {
    let n = (seconds * SR) as usize;
    let burst = (SR * 0.25) as usize;
    let f0 = signal::sine(180.0, SR, 0.25, n);
    let f1 = signal::sine(360.0, SR, 0.12, n);
    let f2 = signal::sine(2400.0, SR, 0.05, n);
    let noise = signal::white_noise(n, noise_amp, seed);
    let mut x = vec![0.0; n];
    let mut mask = vec![false; n];
    for i in 0..n {
        let on = (i / burst).is_multiple_of(2);
        mask[i] = on;
        x[i] = noise[i] + if on { f0[i] + f1[i] + f2[i] } else { 0.0 };
    }
    (x, mask)
}

/// Engine driven offline: bus 0 → master slot 0; every other bus with an
/// output gets its own bridge consumed like a device would.
pub struct OfflineRig {
    pub core: MixerCore,
    pub handles: EngineHandles,
    pub outputs: Vec<(usize, PlaybackSource)>,
    pub block: usize,
}

impl OfflineRig {
    pub fn new(block: usize) -> Self {
        let mut cfg = EngineConfig::new(SR);
        cfg.max_block = block.max(64);
        let (core, handles) = create_engine(cfg);
        Self { core, handles, outputs: Vec::new(), block }
    }

    pub fn push(&mut self, c: Command) {
        self.handles.commands.push(c).expect("command queue full");
    }

    /// Applies a profile through the real reconciler and wires bus outputs:
    /// bus 0 is the master, buses listed in `bridged` get bridges.
    pub fn apply_profile(&mut self, old: Option<&Profile>, p: &Profile, bridged: &[usize]) {
        let mut cmds = Vec::new();
        novamix_control::reconcile(old, p, &mut cmds);
        for c in cmds {
            self.push(c);
        }
        self.push(Command::BusOutput { bus: 0, slot: Some(0) });
        self.push(Command::MasterSlot { slot: Some(0) });
        for &b in bridged {
            if self.outputs.iter().any(|(bb, _)| *bb == b) {
                continue;
            }
            let slot = b as u8;
            let cfg = BridgeConfig::for_blocks(SR as f64, SR as f64, self.block, self.block, 2.0);
            let (w, r) = bridge(cfg, StreamStats::new_shared(), Arc::new(AtomicBool::new(true)));
            self.push(Command::AttachOutput { slot, writer: Box::new(w) });
            self.push(Command::BusOutput { bus: b as u8, slot: Some(slot) });
            self.outputs.push((b, PlaybackSource::new(r)));
        }
    }

    /// Runs `frames` and returns (master left channel, per-bridged-bus left channel).
    pub fn run(&mut self, frames: usize) -> (Vec<f32>, Vec<(usize, Vec<f32>)>) {
        let mut master = Vec::with_capacity(frames);
        let mut bridged: Vec<(usize, Vec<f32>)> = self.outputs.iter().map(|(b, _)| (*b, Vec::new())).collect();
        let mut buf = vec![0.0; self.block * 2];
        let mut done = 0;
        while done < frames {
            self.core.process_master(&mut buf, 2);
            master.extend(buf.chunks(2).map(|f| f[0]));
            for (k, (_, pb)) in self.outputs.iter_mut().enumerate() {
                let mut dev = vec![0.0; self.block * 2];
                pb.on_output(&mut dev, 2);
                bridged[k].1.extend(dev.chunks(2).map(|f| f[0]));
            }
            done += self.block;
        }
        while self.handles.garbage.pop().is_ok() {}
        (master, bridged)
    }
}

/// Level of a frequency component (full-scale sine = 1.0).
pub fn tone_level(x: &[f32], freq: f32) -> f32 {
    signal::goertzel_magnitude(x, freq, SR)
}
