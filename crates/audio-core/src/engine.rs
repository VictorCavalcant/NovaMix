//! The mixer core: runs once per audio block on the master clock thread.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Instant;

use novamix_diagnostics::{RtEvent, RtEventReceiver, RtEventSender, rt_event_channel};
use novamix_dsp::util::{db_to_gain, enable_flush_to_zero};
use novamix_mixer_graph::{MAX_BUSES, MAX_CHANNELS};

use crate::bridge::{BridgeWriter, write_interleaved};
use crate::bus::BusStrip;
use crate::command::{Command, Garbage, TapPoint};
use crate::meters::MeterBank;
use crate::strip::ChannelStrip;

/// Maximum number of distinct output devices.
pub const MAX_OUTPUTS: usize = MAX_BUSES;
/// Commands applied per block at most (the rest wait for the next block).
const MAX_COMMANDS_PER_BLOCK: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConfig {
    pub sample_rate: f32,
    /// Internal processing block; device buffers of any size are split.
    pub max_block: usize,
    pub smoothing_ms: f32,
    pub command_capacity: usize,
}

impl EngineConfig {
    pub fn new(sample_rate: f32) -> Self {
        Self { sample_rate, max_block: 1024, smoothing_ms: 20.0, command_capacity: 4096 }
    }
}

/// Control-side handles of an engine.
pub struct EngineHandles {
    pub commands: rtrb::Producer<Command>,
    pub garbage: rtrb::Consumer<Garbage>,
    pub events: RtEventReceiver,
    pub meters: Arc<MeterBank>,
    pub tap: rtrb::Consumer<f32>,
    /// Set when the audio thread caught a panic; the engine must be rebuilt.
    pub failed: Arc<AtomicBool>,
}

pub struct MixerCore {
    sample_rate: f32,
    max_block: usize,
    commands: rtrb::Consumer<Command>,
    garbage: rtrb::Producer<Garbage>,
    events: RtEventSender,
    meters: Arc<MeterBank>,
    strips: Vec<ChannelStrip>,
    buses: Vec<BusStrip>,
    outputs: Vec<Option<Box<BridgeWriter>>>,
    master_slot: Option<u8>,
    monitor_bus: Option<u8>,
    cue_mask: u8,
    mix: [Vec<f32>; 2],
    interleave: Vec<f32>,
    tap: rtrb::Producer<f32>,
    tap_point: Option<TapPoint>,
    leaked: u64,
    bus_clip_hold: [u32; MAX_BUSES],
    /// Frames to wait before reporting another over-budget event.
    over_budget_cooldown: usize,
}

/// Builds an engine (all allocation happens here).
pub fn create_engine(cfg: EngineConfig) -> (MixerCore, EngineHandles) {
    let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(cfg.command_capacity.max(64));
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(256);
    let (ev_tx, ev_rx) = rt_event_channel(1024);
    let (tap_tx, tap_rx) = rtrb::RingBuffer::new(32_768);
    let meters = Arc::new(MeterBank::default());
    meters.sample_rate.store(cfg.sample_rate as u32, Ordering::Relaxed);
    let max_block = cfg.max_block.clamp(64, 8192);
    let core = MixerCore {
        sample_rate: cfg.sample_rate,
        max_block,
        commands: cmd_rx,
        garbage: garbage_tx,
        events: ev_tx,
        meters: meters.clone(),
        strips: (0..MAX_CHANNELS).map(|i| ChannelStrip::new(i as u8, cfg.sample_rate, max_block, cfg.smoothing_ms)).collect(),
        buses: (0..MAX_BUSES).map(|_| BusStrip::new(cfg.sample_rate, max_block, cfg.smoothing_ms)).collect(),
        outputs: (0..MAX_OUTPUTS).map(|_| None).collect(),
        master_slot: None,
        monitor_bus: None,
        cue_mask: 0,
        mix: [vec![0.0; max_block], vec![0.0; max_block]],
        interleave: vec![0.0; max_block * 2],
        tap: tap_tx,
        tap_point: None,
        leaked: 0,
        bus_clip_hold: [0; MAX_BUSES],
        over_budget_cooldown: 0,
    };
    let handles =
        EngineHandles { commands: cmd_tx, garbage: garbage_rx, events: ev_rx, meters, tap: tap_rx, failed: Arc::new(AtomicBool::new(false)) };
    (core, handles)
}

impl MixerCore {
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn meters(&self) -> &Arc<MeterBank> {
        &self.meters
    }

    /// Objects that could not be returned through the garbage queue.
    pub fn leaked(&self) -> u64 {
        self.leaked
    }

    fn discard(&mut self, g: Garbage) {
        if let Err(rtrb::PushError::Full(g)) = self.garbage.push(g) {
            // Never free on the audio thread: leak instead (should not
            // happen with a 256-slot queue drained every few ms).
            std::mem::forget(g);
            self.leaked += 1;
        }
    }

    fn update_cue(&mut self) {
        let active = self.cue_mask != 0;
        let monitor = self.monitor_bus;
        for (i, b) in self.buses.iter_mut().enumerate() {
            let cued = active && self.cue_mask & (1 << i) != 0 && Some(i as u8) != monitor;
            b.cue_contrib.set_target(if cued { 1.0 } else { 0.0 });
        }
        if let Some(m) = monitor {
            let monitor_cued = self.cue_mask & (1 << m) != 0;
            let keep = !active || monitor_cued;
            self.buses[m as usize].monitor_contrib.set_target(if keep { 1.0 } else { 0.0 });
        }
    }

    fn apply(&mut self, cmd: Command) {
        let nc = MAX_CHANNELS as u8;
        let nb = MAX_BUSES as u8;
        match cmd {
            Command::AttachInput { ch, reader } => {
                if ch < nc {
                    if let Some(old) = self.strips[ch as usize].input.replace(reader) {
                        self.discard(Garbage::Input(old));
                    }
                } else {
                    self.discard(Garbage::Input(reader));
                }
            }
            Command::DetachInput { ch } => {
                if ch < nc
                    && let Some(old) = self.strips[ch as usize].input.take()
                {
                    self.discard(Garbage::Input(old));
                }
            }
            Command::AttachOutput { slot, writer } => {
                if (slot as usize) < MAX_OUTPUTS {
                    if let Some(old) = self.outputs[slot as usize].replace(writer) {
                        self.discard(Garbage::Output(old));
                    }
                } else {
                    self.discard(Garbage::Output(writer));
                }
            }
            Command::DetachOutput { slot } => {
                if (slot as usize) < MAX_OUTPUTS
                    && let Some(old) = self.outputs[slot as usize].take()
                {
                    self.discard(Garbage::Output(old));
                }
            }
            Command::ChannelEnabled { ch, enabled } if ch < nc => {
                let s = &mut self.strips[ch as usize];
                if enabled && !s.enabled {
                    s.chain.reset();
                }
                s.enabled = enabled;
            }
            Command::InputGain { ch, db } if ch < nc => self.strips[ch as usize].input_gain.set_target(db_to_gain(db.clamp(-24.0, 24.0))),
            Command::Fader { ch, db } if ch < nc => self.strips[ch as usize].set_fader_db(db.clamp(-90.0, 12.0)),
            Command::ChannelMute { ch, mute } if ch < nc => self.strips[ch as usize].mute.set_target(if mute { 0.0 } else { 1.0 }),
            Command::Pan { ch, pan } if ch < nc => {
                let s = &mut self.strips[ch as usize];
                s.pan = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
                s.update_pan();
            }
            Command::Mono { ch, mono } if ch < nc => {
                let s = &mut self.strips[ch as usize];
                s.mono = mono;
                s.update_pan();
            }
            Command::Send { ch, bus, gain, pre_fader } if ch < nc && bus < nb => {
                let s = &mut self.strips[ch as usize];
                let g = if gain.is_finite() { gain.clamp(0.0, 4.0) } else { 0.0 };
                s.sends[bus as usize].set_target(g);
                s.send_pre[bus as usize] = pre_fader;
                s.has_pre_sends = s.send_pre.iter().zip(&s.sends).any(|(p, g)| *p && (g.target() > 0.0 || g.is_smoothing()));
            }
            Command::Stage { ch, params } if ch < nc => self.strips[ch as usize].chain.set_stage(params),
            Command::ChainOrder { ch, order } if ch < nc => self.strips[ch as usize].chain.set_order(order),
            Command::ChainBypass { ch, bypass } if ch < nc => self.strips[ch as usize].chain.set_bypass(bypass),
            Command::AbListen { ch, dry, loudness_match } if ch < nc => self.strips[ch as usize].chain.set_ab_listen(dry, loudness_match),
            Command::TestTone { ch, enabled, kind, frequency_hz, level_db } if ch < nc => {
                let s = &mut self.strips[ch as usize];
                s.tone_enabled = enabled;
                s.tone.set(kind, frequency_hz, level_db);
            }
            Command::BusFader { bus, db } if bus < nb => self.buses[bus as usize].set_fader_db(db.clamp(-90.0, 12.0)),
            Command::BusMute { bus, mute } if bus < nb => self.buses[bus as usize].mute.set_target(if mute { 0.0 } else { 1.0 }),
            Command::BusMono { bus, mono } if bus < nb => self.buses[bus as usize].mono = mono,
            Command::BusDelay { bus, ms } if bus < nb => self.buses[bus as usize].set_delay_ms(ms),
            Command::BusLimiter { bus, params } if bus < nb => self.buses[bus as usize].set_limiter(params),
            Command::BusEnabled { bus, enabled } if bus < nb => self.buses[bus as usize].enabled = enabled,
            Command::BusOutput { bus, slot } if bus < nb => {
                self.buses[bus as usize].output = slot.filter(|s| (*s as usize) < MAX_OUTPUTS);
            }
            Command::MonitorBus { bus } => {
                self.monitor_bus = bus.filter(|b| *b < nb);
                self.update_cue();
            }
            Command::CueMask { mask } => {
                self.cue_mask = mask;
                self.update_cue();
            }
            Command::MasterSlot { slot } => self.master_slot = slot.filter(|s| (*s as usize) < MAX_OUTPUTS),
            Command::SetTap { point } => self.tap_point = point,
            Command::SetSmoothing { ms } => {
                let ms = ms.clamp(1.0, 500.0);
                let sr = self.sample_rate;
                for s in &mut self.strips {
                    s.set_smoothing(sr, ms);
                }
                for b in &mut self.buses {
                    b.set_smoothing(ms);
                }
            }
            // Out-of-range indexes are ignored.
            _ => {}
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..MAX_COMMANDS_PER_BLOCK {
            match self.commands.pop() {
                Ok(cmd) => self.apply(cmd),
                Err(_) => break,
            }
        }
    }

    /// Mixes the buses assigned to `slot` into `self.mix`.
    fn mix_slot(&mut self, slot: u8, n: usize) {
        let [ml, mr] = &mut self.mix;
        ml[..n].fill(0.0);
        mr[..n].fill(0.0);
        let monitor = self.monitor_bus;
        let monitor_slot = monitor.and_then(|m| self.buses[m as usize].output);
        for (i, b) in self.buses.iter_mut().enumerate() {
            if !b.enabled || b.output != Some(slot) {
                continue;
            }
            let is_monitor = Some(i as u8) == monitor;
            if is_monitor && (b.monitor_contrib.is_smoothing() || b.monitor_contrib.current() < 1.0) {
                for k in 0..n {
                    let g = b.monitor_contrib.next_value();
                    ml[k] += b.buf[0][k] * g;
                    mr[k] += b.buf[1][k] * g;
                }
            } else {
                for k in 0..n {
                    ml[k] += b.buf[0][k];
                    mr[k] += b.buf[1][k];
                }
            }
        }
        if monitor_slot == Some(slot) {
            for b in self.buses.iter_mut() {
                if !b.enabled || (!b.cue_contrib.is_smoothing() && b.cue_contrib.current() == 0.0) {
                    continue;
                }
                for k in 0..n {
                    let g = b.cue_contrib.next_value();
                    ml[k] += b.buf[0][k] * g;
                    mr[k] += b.buf[1][k] * g;
                }
            }
        }
    }

    fn feed_tap(&mut self, n: usize) {
        let Some(point) = self.tap_point else { return };
        let (l, r) = match point {
            TapPoint::ChannelPost(c) if (c as usize) < MAX_CHANNELS => (&self.strips[c as usize].post[0], &self.strips[c as usize].post[1]),
            TapPoint::Bus(b) if (b as usize) < MAX_BUSES => (&self.buses[b as usize].buf[0], &self.buses[b as usize].buf[1]),
            _ => return,
        };
        let k = n.min(self.tap.slots());
        if k == 0 {
            return;
        }
        if let Ok(mut chunk) = self.tap.write_chunk(k) {
            let (a, b) = chunk.as_mut_slices();
            for (i, s) in a.iter_mut().chain(b.iter_mut()).enumerate() {
                *s = 0.5 * (l[i] + r[i]);
            }
            chunk.commit_all();
        }
    }

    /// Processes one block of `n <= max_block` frames. If `master` is
    /// given, the master slot is written into it (interleaved).
    fn run_block(&mut self, n: usize, master: Option<(&mut [f32], usize)>) {
        self.drain_commands();
        let budget_us = n as f32 / self.sample_rate * 1e6;

        for b in &mut self.buses {
            b.clear(n);
        }
        for si in 0..self.strips.len() {
            if self.strips[si].is_silent_and_settled() {
                let m = &self.meters.channels[si];
                for c in 0..2 {
                    m.input_rms[c].store(0.0);
                    m.output_rms[c].store(0.0);
                }
                m.source_active.store(false, Ordering::Relaxed);
                continue;
            }
            let meter = &self.meters.channels[si];
            self.strips[si].process(n, meter, &mut self.events, budget_us);
            let strip = &mut self.strips[si];
            for (bi, bus) in self.buses.iter_mut().enumerate() {
                if !bus.enabled {
                    continue;
                }
                let send = &mut strip.sends[bi];
                if !send.is_smoothing() && send.current() == 0.0 {
                    continue;
                }
                let src = if strip.send_pre[bi] { &strip.pre } else { &strip.post };
                bus.accumulate(&src[0][..n], &src[1][..n], send);
            }
        }
        for (bi, b) in self.buses.iter_mut().enumerate() {
            if !b.enabled {
                continue;
            }
            let meter = &self.meters.buses[bi];
            let clips_before = meter.clips.load(Ordering::Relaxed);
            b.process(n, meter);
            if meter.clips.load(Ordering::Relaxed) != clips_before {
                if self.bus_clip_hold[bi] == 0 {
                    self.events.send(RtEvent::BusClipped { bus: bi as u8 });
                }
                self.bus_clip_hold[bi] = 100;
            } else if self.bus_clip_hold[bi] > 0 {
                self.bus_clip_hold[bi] -= 1;
            }
        }

        let master_slot = self.master_slot;
        let mut master = master;
        for slot in 0..MAX_OUTPUTS as u8 {
            let is_master = master_slot == Some(slot) && master.is_some();
            if !is_master && self.outputs[slot as usize].is_none() {
                continue;
            }
            self.mix_slot(slot, n);
            if is_master {
                if let Some((buf, ch)) = master.as_mut() {
                    write_interleaved(buf, *ch, &self.mix[0][..n], &self.mix[1][..n]);
                }
            } else if let Some(w) = self.outputs[slot as usize].as_mut() {
                let inter = &mut self.interleave[..2 * n];
                for k in 0..n {
                    inter[2 * k] = self.mix[0][k];
                    inter[2 * k + 1] = self.mix[1][k];
                }
                if w.push_interleaved(inter) < n {
                    self.events.send(RtEvent::OutputOverrun { output: slot });
                }
            }
        }
        if let Some((buf, ch)) = master
            && master_slot.is_none()
        {
            buf[..n * ch].fill(0.0);
        }
        self.feed_tap(n);
    }

    fn record_timing(&mut self, t0: Instant, frames: usize) {
        let us = t0.elapsed().as_secs_f32() * 1e6;
        let budget = frames as f32 / self.sample_rate * 1e6;
        let load = self.meters.timing.record(us, budget);
        self.meters.block_frames.store(frames as u32, Ordering::Relaxed);
        // At most one event per second: an overloaded system must not also
        // flood the event log.
        self.over_budget_cooldown = self.over_budget_cooldown.saturating_sub(frames);
        if load > 100.0 && self.over_budget_cooldown == 0 {
            self.events.send(RtEvent::OverBudget { load_pct: load.min(65_535.0) as u16 });
            self.over_budget_cooldown = self.sample_rate as usize;
        }
    }

    /// Entry point for the master device callback (interleaved buffer).
    pub fn process_master(&mut self, out: &mut [f32], channels: usize) {
        enable_flush_to_zero();
        let t0 = Instant::now();
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let mut start = 0;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            let buf = &mut out[start * channels..(start + n) * channels];
            self.run_block(n, Some((buf, channels)));
            start += n;
        }
        self.record_timing(t0, frames);
    }

    /// Entry point for the internal clock (no master device).
    pub fn process_detached(&mut self, frames: usize) {
        enable_flush_to_zero();
        let t0 = Instant::now();
        let mut start = 0;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            self.run_block(n, None);
            start += n;
        }
        self.record_timing(t0, frames);
    }
}

/// Shared ownership of the core between the master callback and the
/// internal clock. Audio threads only ever `try_lock` (never block).
pub type SharedCore = Arc<Mutex<MixerCore>>;

/// Runs `f` on the core if it is available; catches panics so a bug in a
/// processor cannot kill the device thread (the engine is flagged failed
/// and rebuilt by the control thread).
#[inline]
pub fn with_core(core: &SharedCore, failed: &AtomicBool, f: impl FnOnce(&mut MixerCore)) -> bool {
    if failed.load(Ordering::Relaxed) {
        return false;
    }
    match core.try_lock() {
        Ok(mut guard) => {
            let r = std::panic::catch_unwind(AssertUnwindSafe(|| f(&mut guard)));
            if r.is_err() {
                failed.store(true, Ordering::Relaxed);
                return false;
            }
            true
        }
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Poisoned(_)) => {
            failed.store(true, Ordering::Relaxed);
            false
        }
    }
}

/// Lives inside the master output device callback.
pub struct MasterDriver {
    core: SharedCore,
    failed: Arc<AtomicBool>,
}

impl MasterDriver {
    pub fn new(core: SharedCore, failed: Arc<AtomicBool>) -> Self {
        Self { core, failed }
    }

    pub fn on_output(&mut self, out: &mut [f32], channels: usize) {
        let ok = with_core(&self.core, &self.failed, |c| c.process_master(out, channels));
        if !ok {
            out.fill(0.0);
        }
    }
}
