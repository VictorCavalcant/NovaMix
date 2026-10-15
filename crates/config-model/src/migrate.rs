//! Schema migrations and format handling (TOML / JSON).
//!
//! Files are parsed into a generic JSON value, migrated step by step up to
//! [`SCHEMA_VERSION`], then deserialized with `deny_unknown_fields` and
//! sanitized. Files written by a newer version are rejected with a clear
//! message instead of being silently truncated.

use serde_json::Value;

use crate::ConfigError;
use crate::model::{AppConfig, Profile, SCHEMA_VERSION};
use crate::validate::Issue;

/// Serialization format of a configuration or profile file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Json,
}

impl Format {
    /// Guesses the format from a file extension (defaults to TOML).
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
            Some(e) if e == "json" => Format::Json,
            _ => Format::Toml,
        }
    }
}

/// TOML allows `nan`/`inf`, JSON does not. Replace them with an
/// out-of-range finite value so the field survives conversion and is then
/// reset by sanitization (with an issue reported) instead of failing to load.
fn scrub_non_finite(v: &mut toml::Value) {
    match v {
        toml::Value::Float(f) if !f.is_finite() => *f = f64::MAX,
        toml::Value::Array(items) => items.iter_mut().for_each(scrub_non_finite),
        toml::Value::Table(t) => t.iter_mut().for_each(|(_, v)| scrub_non_finite(v)),
        _ => {}
    }
}

fn to_value(text: &str, format: Format) -> Result<Value, ConfigError> {
    match format {
        Format::Json => serde_json::from_str(text).map_err(|e| ConfigError::Parse(e.to_string())),
        Format::Toml => {
            let mut v: toml::Value = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
            scrub_non_finite(&mut v);
            serde_json::to_value(v).map_err(|e| ConfigError::Parse(e.to_string()))
        }
    }
}

fn version_of(v: &Value) -> u32 {
    v.get("schema_version").and_then(Value::as_u64).map(|x| x as u32).unwrap_or(0)
}

fn rename_in_array(v: &mut Value, array: &str, from: &str, to: &str) {
    if let Some(items) = v.get_mut(array).and_then(Value::as_array_mut) {
        for item in items {
            if let Some(obj) = item.as_object_mut()
                && let Some(x) = obj.remove(from)
            {
                obj.entry(to.to_string()).or_insert(x);
            }
        }
    }
}

/// Migrates a profile value in place. Returns the version it started from.
pub fn migrate_profile_value(v: &mut Value) -> Result<u32, ConfigError> {
    let from = version_of(v);
    if from > SCHEMA_VERSION {
        return Err(ConfigError::NewerVersion { found: from, supported: SCHEMA_VERSION });
    }
    let mut ver = from;
    while ver < SCHEMA_VERSION {
        match ver {
            // v0 = pre-release format: channel gain was `gain_db`, bus cue
            // was called `solo`.
            0 => {
                rename_in_array(v, "channels", "gain_db", "input_gain_db");
                rename_in_array(v, "buses", "solo", "cue");
            }
            _ => unreachable!("missing migration step from v{ver}"),
        }
        ver += 1;
    }
    if let Some(obj) = v.as_object_mut() {
        obj.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
    }
    Ok(from)
}

/// Migrates an application configuration value in place.
pub fn migrate_config_value(v: &mut Value) -> Result<u32, ConfigError> {
    let from = version_of(v);
    if from > SCHEMA_VERSION {
        return Err(ConfigError::NewerVersion { found: from, supported: SCHEMA_VERSION });
    }
    // v0 → v1: no structural change, only the version stamp.
    if let Some(obj) = v.as_object_mut() {
        obj.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
    }
    Ok(from)
}

/// Result of loading a file: the value plus any corrections made.
#[derive(Debug, Clone)]
pub struct Loaded<T> {
    pub value: T,
    pub migrated_from: Option<u32>,
    pub issues: Vec<Issue>,
}

pub fn parse_profile(text: &str, format: Format) -> Result<Loaded<Profile>, ConfigError> {
    let mut v = to_value(text, format)?;
    let from = migrate_profile_value(&mut v)?;
    let mut p: Profile = serde_json::from_value(v).map_err(|e| ConfigError::Parse(e.to_string()))?;
    let issues = p.sanitize();
    Ok(Loaded { value: p, migrated_from: (from != SCHEMA_VERSION).then_some(from), issues })
}

pub fn parse_config(text: &str, format: Format) -> Result<Loaded<AppConfig>, ConfigError> {
    let mut v = to_value(text, format)?;
    let from = migrate_config_value(&mut v)?;
    let mut c: AppConfig = serde_json::from_value(v).map_err(|e| ConfigError::Parse(e.to_string()))?;
    let issues = c.sanitize();
    Ok(Loaded { value: c, migrated_from: (from != SCHEMA_VERSION).then_some(from), issues })
}

pub fn profile_to_string(p: &Profile, format: Format) -> Result<String, ConfigError> {
    match format {
        Format::Toml => toml::to_string_pretty(p).map_err(|e| ConfigError::Serialize(e.to_string())),
        Format::Json => serde_json::to_string_pretty(p).map_err(|e| ConfigError::Serialize(e.to_string())),
    }
}

pub fn config_to_string(c: &AppConfig) -> Result<String, ConfigError> {
    toml::to_string_pretty(c).map_err(|e| ConfigError::Serialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{Scenario, scenario_profile};

    #[test]
    fn toml_and_json_roundtrip_every_scenario() {
        for s in Scenario::ALL {
            let p = scenario_profile(s);
            for f in [Format::Toml, Format::Json] {
                let text = profile_to_string(&p, f).unwrap();
                let back = parse_profile(&text, f).unwrap();
                assert_eq!(back.value, p, "{s:?} {f:?}");
                assert!(back.issues.is_empty());
                assert_eq!(back.migrated_from, None);
            }
        }
    }

    #[test]
    fn config_roundtrip() {
        let c = AppConfig::default();
        let text = config_to_string(&c).unwrap();
        let back = parse_config(&text, Format::Toml).unwrap();
        assert_eq!(back.value, c);
    }

    #[test]
    fn rejects_newer_versions() {
        let text = "schema_version = 99\nname = \"x\"\n";
        match parse_profile(text, Format::Toml) {
            Err(ConfigError::NewerVersion { found: 99, .. }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_fields() {
        let p = scenario_profile(Scenario::Default);
        let mut v = serde_json::to_value(&p).unwrap();
        v["channels"][0]["evil_field"] = Value::from(1);
        let text = serde_json::to_string(&v).unwrap();
        assert!(matches!(parse_profile(&text, Format::Json), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn migrates_v0_profiles() {
        let p = scenario_profile(Scenario::Default);
        let mut v = serde_json::to_value(&p).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("schema_version");
        let ch = v["channels"][0].as_object_mut().unwrap();
        ch.remove("input_gain_db");
        ch.insert("gain_db".into(), Value::from(6.0));
        let bus = v["buses"][0].as_object_mut().unwrap();
        bus.remove("cue");
        bus.insert("solo".into(), Value::from(true));
        let loaded = parse_profile(&serde_json::to_string(&v).unwrap(), Format::Json).unwrap();
        assert_eq!(loaded.migrated_from, Some(0));
        assert_eq!(loaded.value.channels[0].input_gain_db, 6.0);
        assert!(loaded.value.buses[0].cue);
    }

    #[test]
    fn rejects_non_finite_toml_values_by_sanitizing() {
        let p = scenario_profile(Scenario::Default);
        let text = profile_to_string(&p, Format::Toml).unwrap().replacen("fader_db = 0.0", "fader_db = nan", 1);
        let loaded = parse_profile(&text, Format::Toml).unwrap();
        assert!(loaded.value.channels.iter().all(|c| c.fader_db.is_finite()));
        assert!(!loaded.issues.is_empty());
    }
}
