//! Profile files: import validation, failure cases and auditable diffs.

use novamix_config::{ConfigError, ConfigStore, Format, Scenario, diff_profiles, scenario_profile};

fn store(tag: &str) -> ConfigStore {
    let dir = std::env::temp_dir().join(format!("novamix-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let s = ConfigStore::at(dir);
    s.ensure_dirs().unwrap();
    s
}

#[test]
fn invalid_profiles_are_rejected_with_clear_errors() {
    let s = store("invalid");
    let cases = [
        ("garbage.toml", "this is = = not toml", "invalid file"),
        ("future.toml", "schema_version = 7\nname = \"x\"", "newer NovaMix"),
        ("unknown.json", r#"{"schema_version":1,"name":"x","hacker":true}"#, "invalid file"),
    ];
    for (name, text, expect) in cases {
        let path = s.root().join(name);
        std::fs::write(&path, text).unwrap();
        let err = s.import_profile(&path).unwrap_err();
        assert!(err.to_string().contains(expect), "{name}: {err}");
    }
    let big = s.root().join("big.json");
    std::fs::write(&big, vec![b' '; 600_000]).unwrap();
    assert!(matches!(s.import_profile(&big), Err(ConfigError::TooLarge { .. })));
    let _ = std::fs::remove_dir_all(s.root());
}

#[test]
fn out_of_range_values_are_corrected_on_import() {
    let s = store("ranges");
    let mut p = scenario_profile(Scenario::Gaming);
    p.channels[0].fader_db = 500.0;
    p.channels[0].chain.compressor.ratio = 1000.0;
    let text = novamix_config::migrate::profile_to_string(&p, Format::Json).unwrap();
    let path = s.root().join("ranges.json");
    std::fs::write(&path, text).unwrap();
    let l = s.import_profile(&path).unwrap();
    assert_eq!(l.value.channels[0].fader_db, 12.0);
    assert_eq!(l.value.channels[0].chain.compressor.ratio, 20.0);
    assert!(l.issues.len() >= 2);
    let _ = std::fs::remove_dir_all(s.root());
}

#[test]
fn switching_scenarios_lists_effect_and_routing_changes() {
    let a = scenario_profile(Scenario::Default);
    let b = scenario_profile(Scenario::Streaming);
    let changes = diff_profiles(&a, &b);
    assert!(changes.iter().any(|c| c.field == "effects"));
    assert!(!changes.iter().any(|c| c.affects_devices), "same devices in both templates");
}
