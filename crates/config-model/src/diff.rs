//! "Auditable profiles": a human-readable list of what switching from one
//! profile to another will change (channels, devices, effects, routing).

use novamix_dsp::params::{StageKind, VoicePreset};

use crate::model::*;

/// What part of the profile a change belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeScope {
    Profile,
    Channel(usize, String),
    Bus(usize, String),
}

/// One difference between two profiles. `field` is a stable key that the UI
/// translates; `from`/`to` are already formatted values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChange {
    pub scope: ChangeScope,
    pub field: &'static str,
    pub from: String,
    pub to: String,
    /// True for changes that reopen devices (may briefly interrupt audio).
    pub affects_devices: bool,
}

fn fmt_db(v: f32) -> String {
    if v <= crate::validate::FADER_MIN_DB { "-∞ dB".into() } else { format!("{v:+.1} dB") }
}

fn fmt_bool(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

pub fn describe_source(s: &SourceSpec) -> String {
    match s {
        SourceSpec::None => "—".into(),
        SourceSpec::DefaultInput => "default input".into(),
        SourceSpec::Input { device } => device.name.clone(),
        SourceSpec::Loopback { device } => format!("{} (loopback)", device.name),
        SourceSpec::DefaultOutputLoopback => "default output (loopback)".into(),
        SourceSpec::VirtualPlayback => "NovaMix virtual playback".into(),
        SourceSpec::TestTone { kind, frequency_hz, level_db } => format!("test {kind:?} {frequency_hz:.0} Hz {level_db:.0} dB"),
    }
}

pub fn describe_output(o: &OutputSpec) -> String {
    match o {
        OutputSpec::None => "—".into(),
        OutputSpec::DefaultOutput => "default output".into(),
        OutputSpec::Device { device } => device.name.clone(),
        OutputSpec::VirtualMic => "NovaMix virtual mic".into(),
    }
}

fn describe_chain(c: &novamix_dsp::params::ChainParams) -> String {
    if let Some(p) = VoicePreset::detect(c) {
        return format!("preset {:?}", p);
    }
    let on: Vec<&str> = StageKind::DEFAULT_ORDER.iter().filter(|k| c.is_enabled(**k)).map(|k| k.key()).collect();
    if on.is_empty() { "no effects".into() } else { on.join(", ") }
}

/// Lists the changes from `old` to `new`.
pub fn diff_profiles(old: &Profile, new: &Profile) -> Vec<ProfileChange> {
    let mut out = Vec::new();
    let mut push = |scope: ChangeScope, field: &'static str, from: String, to: String, dev: bool| {
        if from != to {
            out.push(ProfileChange { scope, field, from, to, affects_devices: dev });
        }
    };
    push(ChangeScope::Profile, "channel_count", old.channels.len().to_string(), new.channels.len().to_string(), true);
    push(ChangeScope::Profile, "bus_count", old.buses.len().to_string(), new.buses.len().to_string(), true);
    push(ChangeScope::Profile, "solo_mode", format!("{:?}", old.solo_mode), format!("{:?}", new.solo_mode), false);

    for (i, n) in new.channels.iter().enumerate() {
        let scope = || ChangeScope::Channel(i, n.name.clone());
        let Some(o) = old.channels.get(i) else {
            push(scope(), "added", "—".into(), describe_source(&n.source), true);
            continue;
        };
        push(scope(), "name", o.name.clone(), n.name.clone(), false);
        push(scope(), "enabled", fmt_bool(o.enabled), fmt_bool(n.enabled), false);
        push(scope(), "source", describe_source(&o.source), describe_source(&n.source), true);
        push(scope(), "input_map", format!("{:?}", o.input_map), format!("{:?}", n.input_map), true);
        push(scope(), "input_gain", fmt_db(o.input_gain_db), fmt_db(n.input_gain_db), false);
        push(scope(), "fader", fmt_db(o.fader_db), fmt_db(n.fader_db), false);
        push(scope(), "mute", fmt_bool(o.mute), fmt_bool(n.mute), false);
        push(scope(), "solo", fmt_bool(o.solo), fmt_bool(n.solo), false);
        push(scope(), "pan", format!("{:+.2}", o.pan), format!("{:+.2}", n.pan), false);
        push(scope(), "mono", fmt_bool(o.mono), fmt_bool(n.mono), false);
        if o.chain != n.chain {
            push(scope(), "effects", describe_chain(&o.chain), describe_chain(&n.chain), false);
        }
        for (j, ns) in n.sends.iter().enumerate() {
            let os = o.sends.get(j).copied().unwrap_or_default();
            let bus = new.buses.get(j).map(|b| b.name.clone()).unwrap_or_else(|| format!("{j}"));
            let f = |s: &SendConfig| if s.enabled { format!("{bus}: {}", fmt_db(s.level_db)) } else { format!("{bus}: off") };
            push(scope(), "send", f(&os), f(ns), false);
        }
    }
    for i in new.channels.len()..old.channels.len() {
        push(ChangeScope::Channel(i, old.channels[i].name.clone()), "removed", describe_source(&old.channels[i].source), "—".into(), true);
    }

    for (i, n) in new.buses.iter().enumerate() {
        let scope = || ChangeScope::Bus(i, n.name.clone());
        let Some(o) = old.buses.get(i) else {
            push(scope(), "added", "—".into(), describe_output(&n.output), true);
            continue;
        };
        push(scope(), "name", o.name.clone(), n.name.clone(), false);
        push(scope(), "role", format!("{:?}", o.role), format!("{:?}", n.role), false);
        push(scope(), "output", describe_output(&o.output), describe_output(&n.output), true);
        push(scope(), "fader", fmt_db(o.fader_db), fmt_db(n.fader_db), false);
        push(scope(), "mute", fmt_bool(o.mute), fmt_bool(n.mute), false);
        push(scope(), "cue", fmt_bool(o.cue), fmt_bool(n.cue), false);
        push(scope(), "mono", fmt_bool(o.mono), fmt_bool(n.mono), false);
        push(scope(), "delay", format!("{:.0} ms", o.delay_ms), format!("{:.0} ms", n.delay_ms), false);
        if o.limiter != n.limiter {
            push(scope(), "limiter", format!("{:.1} dB", o.limiter.ceiling_db), format!("{:.1} dB", n.limiter.ceiling_db), false);
        }
    }
    for i in new.buses.len()..old.buses.len() {
        push(ChangeScope::Bus(i, old.buses[i].name.clone()), "removed", describe_output(&old.buses[i].output), "—".into(), true);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{Scenario, scenario_profile};

    #[test]
    fn identical_profiles_have_no_changes() {
        let p = scenario_profile(Scenario::Default);
        assert!(diff_profiles(&p, &p).is_empty());
    }

    #[test]
    fn reports_effects_sends_and_devices() {
        let a = scenario_profile(Scenario::Default);
        let b = scenario_profile(Scenario::Meeting);
        let d = diff_profiles(&a, &b);
        assert!(d.iter().any(|c| c.field == "effects" && c.to.contains("Call")));
        assert!(d.iter().any(|c| c.field == "send"));
        let mut c = a.clone();
        c.buses[0].output = OutputSpec::Device { device: DeviceRef::new("x", "Headset") };
        let d = diff_profiles(&a, &c);
        assert_eq!(d.len(), 1);
        assert!(d[0].affects_devices);
        assert_eq!(d[0].to, "Headset");
    }
}
