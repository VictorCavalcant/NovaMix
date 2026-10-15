//! Declarative profile → engine command reconciliation.
//!
//! The UI edits a [`Profile`]; the controller diffs the previously applied
//! profile against the new one and emits only the commands that changed.
//! With `old == None` (engine just started) everything is sent.

use novamix_audio_core::Command;
use novamix_config::{BusRole, Profile, SourceSpec};
use novamix_dsp::params::{STAGE_COUNT, StageKind};
use novamix_dsp::util::db_to_gain;
use novamix_mixer_graph::{MAX_BUSES, MAX_CHANNELS, solo_allows};

/// Effective linear gain of every send after enable/level/solo resolution.
pub fn effective_sends(p: &Profile) -> [[(f32, bool); MAX_BUSES]; MAX_CHANNELS] {
    let mut out = [[(0.0f32, false); MAX_BUSES]; MAX_CHANNELS];
    let any_solo = p.any_solo();
    for (ci, ch) in p.channels.iter().enumerate().take(MAX_CHANNELS) {
        for (bi, bus) in p.buses.iter().enumerate().take(MAX_BUSES) {
            let send = ch.sends.get(bi).copied().unwrap_or_default();
            let audible = solo_allows(p.solo_mode, any_solo, ch.solo, bus.role == BusRole::Monitor);
            let gain = if send.enabled && audible && send.level_db > -90.0 { db_to_gain(send.level_db) } else { 0.0 };
            out[ci][bi] = (gain, send.pre_fader);
        }
    }
    out
}

fn cue_mask(p: &Profile) -> u8 {
    p.buses.iter().take(MAX_BUSES).enumerate().fold(0u8, |m, (i, b)| if b.cue { m | (1 << i) } else { m })
}

/// Appends the commands needed to go from `old` to `new`.
pub fn reconcile(old: Option<&Profile>, new: &Profile, out: &mut Vec<Command>) {
    let full = old.is_none();
    for ci in 0..MAX_CHANNELS {
        let ch = ci as u8;
        let n = new.channels.get(ci);
        let o = old.and_then(|p| p.channels.get(ci));
        let n_enabled = n.map(|c| c.enabled).unwrap_or(false);
        let o_enabled = o.map(|c| c.enabled).unwrap_or(false);
        if full || n_enabled != o_enabled {
            out.push(Command::ChannelEnabled { ch, enabled: n_enabled });
        }
        let Some(n) = n else { continue };
        let changed = |f: &dyn Fn(&novamix_config::ChannelConfig) -> bool| full || o.map(f).unwrap_or(true);
        if changed(&|o| o.input_gain_db != n.input_gain_db) {
            out.push(Command::InputGain { ch, db: n.input_gain_db });
        }
        if changed(&|o| o.fader_db != n.fader_db) {
            out.push(Command::Fader { ch, db: n.fader_db });
        }
        if changed(&|o| o.mute != n.mute) {
            out.push(Command::ChannelMute { ch, mute: n.mute });
        }
        if changed(&|o| o.pan != n.pan) {
            out.push(Command::Pan { ch, pan: n.pan });
        }
        if changed(&|o| o.mono != n.mono) {
            out.push(Command::Mono { ch, mono: n.mono });
        }
        let tone_changed = changed(&|o| o.source != n.source);
        if tone_changed {
            match &n.source {
                SourceSpec::TestTone { kind, frequency_hz, level_db } => {
                    out.push(Command::TestTone { ch, enabled: true, kind: *kind, frequency_hz: *frequency_hz, level_db: *level_db })
                }
                _ => out.push(Command::TestTone { ch, enabled: false, kind: Default::default(), frequency_hz: 440.0, level_db: -20.0 }),
            }
        }
        // Chain: order and bypass, then each stage.
        let oc = o.map(|o| &o.chain);
        if full || oc.map(|c| c.order != n.chain.order).unwrap_or(true) {
            out.push(Command::ChainOrder { ch, order: n.chain.order });
        }
        if full || oc.map(|c| c.bypass != n.chain.bypass).unwrap_or(true) {
            out.push(Command::ChainBypass { ch, bypass: n.chain.bypass });
        }
        for k in StageKind::DEFAULT_ORDER.iter().take(STAGE_COUNT) {
            let ns = n.chain.stage(*k);
            if full || oc.map(|c| c.stage(*k) != ns).unwrap_or(true) {
                out.push(Command::Stage { ch, params: ns });
            }
        }
    }

    // Sends (solo affects many at once, so compare effective values).
    let ns = effective_sends(new);
    let os = old.map(effective_sends);
    for ci in 0..MAX_CHANNELS {
        for bi in 0..MAX_BUSES {
            let n = ns[ci][bi];
            if full || os.map(|o| o[ci][bi] != n).unwrap_or(true) {
                out.push(Command::Send { ch: ci as u8, bus: bi as u8, gain: n.0, pre_fader: n.1 });
            }
        }
    }

    for bi in 0..MAX_BUSES {
        let bus = bi as u8;
        let n = new.buses.get(bi);
        let o = old.and_then(|p| p.buses.get(bi));
        if full || n.is_some() != o.is_some() {
            out.push(Command::BusEnabled { bus, enabled: n.is_some() });
        }
        let Some(n) = n else { continue };
        let changed = |f: &dyn Fn(&novamix_config::BusConfig) -> bool| full || o.map(f).unwrap_or(true);
        if changed(&|o| o.fader_db != n.fader_db) {
            out.push(Command::BusFader { bus, db: n.fader_db });
        }
        if changed(&|o| o.mute != n.mute) {
            out.push(Command::BusMute { bus, mute: n.mute });
        }
        if changed(&|o| o.mono != n.mono) {
            out.push(Command::BusMono { bus, mono: n.mono });
        }
        if changed(&|o| o.delay_ms != n.delay_ms) {
            out.push(Command::BusDelay { bus, ms: n.delay_ms });
        }
        if changed(&|o| o.limiter != n.limiter) {
            out.push(Command::BusLimiter { bus, params: n.limiter });
        }
    }

    let monitor = new.monitor_bus().map(|b| b as u8);
    if full || old.map(|o| o.monitor_bus().map(|b| b as u8) != monitor).unwrap_or(true) {
        out.push(Command::MonitorBus { bus: monitor });
    }
    let mask = cue_mask(new);
    if full || old.map(|o| cue_mask(o) != mask).unwrap_or(true) {
        out.push(Command::CueMask { mask });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novamix_config::default_profile;
    use novamix_mixer_graph::SoloMode;

    fn count(cmds: &[Command], pred: impl Fn(&Command) -> bool) -> usize {
        cmds.iter().filter(|c| pred(c)).count()
    }

    #[test]
    fn full_sync_sends_everything() {
        let p = default_profile();
        let mut out = Vec::new();
        reconcile(None, &p, &mut out);
        assert_eq!(count(&out, |c| matches!(c, Command::ChannelEnabled { .. })), MAX_CHANNELS);
        assert_eq!(count(&out, |c| matches!(c, Command::Send { .. })), MAX_CHANNELS * MAX_BUSES);
        assert_eq!(count(&out, |c| matches!(c, Command::Stage { .. })), p.channels.len() * STAGE_COUNT);
    }

    #[test]
    fn unchanged_profile_sends_nothing() {
        let p = default_profile();
        let mut out = Vec::new();
        reconcile(Some(&p), &p, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn fader_change_sends_one_command() {
        let a = default_profile();
        let mut b = a.clone();
        b.channels[1].fader_db = -6.0;
        let mut out = Vec::new();
        reconcile(Some(&a), &b, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Command::Fader { ch: 1, db } if db == -6.0));
    }

    #[test]
    fn solo_in_monitor_only_mode_mutes_others_on_monitor() {
        let a = default_profile();
        let mut b = a.clone();
        b.solo_mode = SoloMode::MonitorOnly;
        b.channels[0].solo = true;
        let s = effective_sends(&b);
        // Desktop (1) → Monitor (0) muted by solo, Desktop → Stream (2) kept.
        assert_eq!(s[1][0].0, 0.0);
        assert!(s[1][2].0 > 0.0);
        let mut out = Vec::new();
        reconcile(Some(&a), &b, &mut out);
        assert!(out.iter().any(|c| matches!(c, Command::Send { ch: 1, bus: 0, gain, .. } if *gain == 0.0)));
    }

    #[test]
    fn stage_change_sends_only_that_stage() {
        let a = default_profile();
        let mut b = a.clone();
        b.channels[0].chain.compressor.ratio = 6.0;
        let mut out = Vec::new();
        reconcile(Some(&a), &b, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Command::Stage { ch: 0, .. }));
    }
}
