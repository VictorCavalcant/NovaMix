//! Validation and sanitization. Every value read from disk or imported by
//! the user passes through here before it reaches the engine.

use novamix_dsp::params::clampf;
use novamix_mixer_graph::{MAX_BUSES, MAX_CHANNELS};

use crate::model::*;

pub const MAX_NAME_LEN: usize = 48;
pub const MAX_DESCRIPTION_LEN: usize = 500;
pub const MAX_DEVICE_STRING_LEN: usize = 512;
pub const FADER_MIN_DB: f32 = -90.0;
pub const FADER_MAX_DB: f32 = 12.0;

/// A problem found (and corrected) during sanitization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

struct Ctx {
    issues: Vec<Issue>,
}

impl Ctx {
    fn issue(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue { path: path.into(), message: message.into() });
    }

    fn float(&mut self, path: &str, v: &mut f32, lo: f32, hi: f32, default: f32) {
        let fixed = clampf(*v, lo, hi, default);
        if fixed != *v {
            self.issue(path, format!("value {} out of range [{lo}, {hi}], set to {fixed}", *v));
            *v = fixed;
        }
    }

    fn text(&mut self, path: &str, s: &mut String, max: usize, fallback: &str) {
        let cleaned: String = s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
        let mut cleaned: String = cleaned.chars().take(max).collect();
        if cleaned.is_empty() && !fallback.is_empty() {
            cleaned = fallback.to_string();
        }
        if cleaned != *s {
            self.issue(path, "text normalized");
            *s = cleaned;
        }
    }

    fn device(&mut self, path: &str, d: &mut DeviceRef) {
        self.text(&format!("{path}.id"), &mut d.id, MAX_DEVICE_STRING_LEN, "");
        self.text(&format!("{path}.name"), &mut d.name, MAX_DEVICE_STRING_LEN, "");
    }
}

/// Makes a profile name safe to use as a file name.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(MAX_NAME_LEN)
        .collect();
    if s.is_empty() { "Perfil".to_string() } else { s }
}

impl Profile {
    /// Clamps every value into range, fixes structural problems and returns
    /// the list of corrections made.
    pub fn sanitize(&mut self) -> Vec<Issue> {
        let mut c = Ctx { issues: Vec::new() };
        self.schema_version = SCHEMA_VERSION;
        let fixed_name = sanitize_name(&self.name);
        if fixed_name != self.name {
            c.issue("name", "invalid characters removed");
            self.name = fixed_name;
        }
        c.text("description", &mut self.description, MAX_DESCRIPTION_LEN, "");

        if self.buses.is_empty() {
            c.issue("buses", "no buses; added a monitor bus");
            self.buses.push(BusConfig { name: "Monitor".into(), role: BusRole::Monitor, output: OutputSpec::DefaultOutput, ..BusConfig::default() });
        }
        if self.buses.len() > MAX_BUSES {
            c.issue("buses", format!("more than {MAX_BUSES} buses; extra removed"));
            self.buses.truncate(MAX_BUSES);
        }
        if self.channels.is_empty() {
            c.issue("channels", "no channels; added one");
            self.channels.push(ChannelConfig::default());
        }
        if self.channels.len() > MAX_CHANNELS {
            c.issue("channels", format!("more than {MAX_CHANNELS} channels; extra removed"));
            self.channels.truncate(MAX_CHANNELS);
        }

        let nbus = self.buses.len();
        for (i, ch) in self.channels.iter_mut().enumerate() {
            let p = format!("channels[{i}]");
            c.text(&format!("{p}.name"), &mut ch.name, MAX_NAME_LEN, &format!("Canal {}", i + 1));
            c.float(&format!("{p}.input_gain_db"), &mut ch.input_gain_db, -24.0, 24.0, 0.0);
            c.float(&format!("{p}.fader_db"), &mut ch.fader_db, FADER_MIN_DB, FADER_MAX_DB, 0.0);
            c.float(&format!("{p}.pan"), &mut ch.pan, -1.0, 1.0, 0.0);
            let chain = ch.chain.sanitized();
            if chain != ch.chain {
                c.issue(format!("{p}.chain"), "effect parameters clamped to valid ranges");
                ch.chain = chain;
            }
            match &mut ch.source {
                SourceSpec::Input { device } | SourceSpec::Loopback { device } => c.device(&format!("{p}.source.device"), device),
                SourceSpec::TestTone { frequency_hz, level_db, .. } => {
                    c.float(&format!("{p}.source.frequency_hz"), frequency_hz, 20.0, 20_000.0, 440.0);
                    c.float(&format!("{p}.source.level_db"), level_db, -60.0, 0.0, -20.0);
                }
                _ => {}
            }
            if let InputMap::Mono(ch_idx) | InputMap::Stereo(ch_idx) = &mut ch.input_map
                && *ch_idx > 63
            {
                c.issue(format!("{p}.input_map"), "channel index too large");
                *ch_idx = 0;
            }
            if ch.sends.len() != nbus {
                c.issue(format!("{p}.sends"), format!("expected {nbus} sends, found {}", ch.sends.len()));
                ch.sends.resize(nbus, SendConfig::default());
            }
            for (j, s) in ch.sends.iter_mut().enumerate() {
                c.float(&format!("{p}.sends[{j}].level_db"), &mut s.level_db, FADER_MIN_DB, FADER_MAX_DB, 0.0);
            }
        }
        for (i, b) in self.buses.iter_mut().enumerate() {
            let p = format!("buses[{i}]");
            c.text(&format!("{p}.name"), &mut b.name, MAX_NAME_LEN, &format!("Bus {}", i + 1));
            c.float(&format!("{p}.fader_db"), &mut b.fader_db, FADER_MIN_DB, FADER_MAX_DB, 0.0);
            c.float(&format!("{p}.delay_ms"), &mut b.delay_ms, 0.0, 500.0, 0.0);
            let lim = b.limiter.sanitized();
            if lim != b.limiter {
                c.issue(format!("{p}.limiter"), "limiter parameters clamped");
                b.limiter = lim;
            }
            if let OutputSpec::Device { device } = &mut b.output {
                c.device(&format!("{p}.output.device"), device);
            }
        }
        c.issues
    }
}

impl AppConfig {
    pub fn sanitize(&mut self) -> Vec<Issue> {
        let mut c = Ctx { issues: Vec::new() };
        self.schema_version = SCHEMA_VERSION;
        let g = &mut self.general;
        c.float("general.ui_scale", &mut g.ui_scale, 0.75, 2.0, 1.0);
        if !(10..=120).contains(&g.meter_fps) {
            c.issue("general.meter_fps", "out of range [10, 120]");
            g.meter_fps = g.meter_fps.clamp(10, 120);
        }
        let a = &mut self.audio;
        if !SAMPLE_RATES.contains(&a.preferred_sample_rate) {
            c.issue("audio.preferred_sample_rate", "unsupported rate, using 48000");
            a.preferred_sample_rate = 48_000;
        }
        if let Some(b) = a.buffer_frames
            && (!(32..=4096).contains(&b) || !b.is_power_of_two())
        {
            c.issue("audio.buffer_frames", "must be a power of two in [32, 4096]");
            a.buffer_frames = None;
        }
        if !(500..=60_000).contains(&a.reconnect_interval_ms) {
            c.issue("audio.reconnect_interval_ms", "out of range [500, 60000]");
            a.reconnect_interval_ms = a.reconnect_interval_ms.clamp(500, 60_000);
        }
        c.float("audio.smoothing_ms", &mut a.smoothing_ms, 5.0, 200.0, 20.0);
        if let Some(cb) = a.clock_bus
            && cb as usize >= MAX_BUSES
        {
            c.issue("audio.clock_bus", "invalid bus index");
            a.clock_bus = None;
        }
        if let Some(d) = &mut a.fallback_output {
            c.device("audio.fallback_output", d);
        }
        if let Some(h) = &mut a.host {
            c.text("audio.host", h, 32, "");
        }
        for (i, b) in self.hotkeys.bindings.iter_mut().enumerate() {
            c.text(&format!("hotkeys.bindings[{i}].accelerator"), &mut b.accelerator, 64, "");
            if let Some(p) = &mut b.profile {
                c.text(&format!("hotkeys.bindings[{i}].profile"), p, MAX_NAME_LEN, "");
            }
            if let Some(bus) = b.bus
                && bus as usize >= MAX_BUSES
            {
                b.bus = None;
            }
        }
        if self.hotkeys.bindings.len() > 64 {
            self.hotkeys.bindings.truncate(64);
            c.issue("hotkeys.bindings", "too many bindings");
        }
        if !(1..=90).contains(&self.diagnostics.keep_log_days) {
            self.diagnostics.keep_log_days = self.diagnostics.keep_log_days.clamp(1, 90);
        }
        let name = sanitize_name(&self.active_profile);
        if name != self.active_profile {
            self.active_profile = name;
        }
        c.issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_clean() {
        let mut p = crate::defaults::default_profile();
        let issues = p.sanitize();
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn fixes_nan_ranges_and_send_counts() {
        let mut p = crate::defaults::default_profile();
        p.channels[0].fader_db = f32::NAN;
        p.channels[1].pan = 7.0;
        p.channels[2].sends.pop();
        p.buses[0].delay_ms = 1e6;
        let issues = p.sanitize();
        assert_eq!(p.channels[0].fader_db, 0.0);
        assert_eq!(p.channels[1].pan, 1.0);
        assert_eq!(p.channels[2].sends.len(), p.buses.len());
        assert_eq!(p.buses[0].delay_ms, 500.0);
        assert_eq!(issues.len(), 4, "{issues:?}");
    }

    #[test]
    fn limits_counts_and_names() {
        let mut p = crate::defaults::default_profile();
        for i in 0..20 {
            p.add_channel(&format!("x{i}"));
        }
        assert_eq!(p.channels.len(), MAX_CHANNELS);
        p.name = "  bad/name:*?  ".into();
        p.sanitize();
        assert_eq!(p.name, "badname");
    }

    #[test]
    fn config_rejects_bad_buffer_and_rate() {
        let mut c = AppConfig::default();
        c.audio.buffer_frames = Some(300);
        c.audio.preferred_sample_rate = 12_345;
        let issues = c.sanitize();
        assert_eq!(c.audio.buffer_frames, None);
        assert_eq!(c.audio.preferred_sample_rate, 48_000);
        assert_eq!(issues.len(), 2);
    }
}
