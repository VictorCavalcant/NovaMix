//! On-disk storage: atomic writes, automatic backups, profile import and
//! export with size limits.
//!
//! Layout (under `%APPDATA%\NovaMix`, or `<exe dir>\data` in portable mode):
//!
//! ```text
//! config.toml
//! profiles/<name>.toml
//! backups/auto/<file>.<timestamp>.toml   (rotating, last 10 per file)
//! backups/<timestamp>/...                (manual full snapshots)
//! logs/
//! diagnostics/
//! ```

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::ConfigError;
use crate::migrate::{self, Format, Loaded};
use crate::model::{AppConfig, Profile};
use crate::validate::sanitize_name;

/// Imported files larger than this are rejected.
pub const MAX_IMPORT_BYTES: u64 = 512 * 1024;
/// Automatic backups kept per file.
pub const AUTO_BACKUPS_PER_FILE: usize = 10;
/// Marker file that enables portable mode when placed next to the executable.
pub const PORTABLE_MARKER: &str = "portable.txt";

#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
}

fn io_err(path: &Path, e: std::io::Error) -> ConfigError {
    ConfigError::Io { path: path.display().to_string(), message: e.to_string() }
}

/// UTC timestamp `YYYYMMDD-HHMMSS` (no external time crate needed).
pub fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Writes a file atomically: temp file + fsync + rename.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
        f.write_all(contents).map_err(|e| io_err(&tmp, e))?;
        f.sync_all().map_err(|e| io_err(&tmp, e))?;
    }
    fs::rename(&tmp, path).map_err(|e| io_err(path, e))
}

impl ConfigStore {
    /// Uses portable mode when the marker file exists next to the
    /// executable, otherwise the per-user roaming configuration directory.
    pub fn default_location() -> Result<Self, ConfigError> {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
            && dir.join(PORTABLE_MARKER).exists()
        {
            return Ok(Self::at(dir.join("data")));
        }
        let base = directories::BaseDirs::new()
            .ok_or_else(|| ConfigError::Io { path: "<home>".into(), message: "could not determine the user configuration directory".into() })?;
        Ok(Self::at(base.config_dir().join("NovaMix")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn diagnostics_dir(&self) -> PathBuf {
        self.root.join("diagnostics")
    }

    pub fn ensure_dirs(&self) -> Result<(), ConfigError> {
        for d in [self.root.clone(), self.profiles_dir(), self.backups_dir(), self.logs_dir(), self.diagnostics_dir()] {
            fs::create_dir_all(&d).map_err(|e| io_err(&d, e))?;
        }
        Ok(())
    }

    fn profile_path(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(format!("{}.toml", sanitize_name(name)))
    }

    /// Copies an existing file into `backups/auto` and prunes old copies.
    fn auto_backup(&self, path: &Path) {
        if !path.exists() {
            return;
        }
        let dir = self.backups_dir().join("auto");
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file").to_string();
        let target = dir.join(format!("{stem}.{}.toml", utc_timestamp()));
        let _ = fs::copy(path, target);
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut mine: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with(&format!("{stem}."))).unwrap_or(false))
                .collect();
            mine.sort();
            while mine.len() > AUTO_BACKUPS_PER_FILE {
                let _ = fs::remove_file(mine.remove(0));
            }
        }
    }

    /// Loads `config.toml`. A missing file yields defaults; a corrupt file
    /// is preserved as `config.corrupt-<ts>.toml` and defaults are used.
    pub fn load_config(&self) -> (AppConfig, Vec<String>) {
        let path = self.config_path();
        let Ok(text) = fs::read_to_string(&path) else {
            return (AppConfig::default(), Vec::new());
        };
        match migrate::parse_config(&text, Format::Toml) {
            Ok(Loaded { value, issues, migrated_from }) => {
                let mut notes: Vec<String> = issues.iter().map(|i| i.to_string()).collect();
                if let Some(v) = migrated_from {
                    notes.push(format!("config migrated from schema v{v}"));
                }
                (value, notes)
            }
            Err(e) => {
                let keep = self.root.join(format!("config.corrupt-{}.toml", utc_timestamp()));
                let _ = fs::rename(&path, &keep);
                (AppConfig::default(), vec![format!("config.toml could not be read ({e}); defaults loaded, original kept as {}", keep.display())])
            }
        }
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<(), ConfigError> {
        let text = migrate::config_to_string(config)?;
        let path = self.config_path();
        if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        self.auto_backup(&path);
        write_atomic(&path, text.as_bytes())
    }

    /// Names of stored profiles (as written inside the files), sorted.
    pub fn list_profiles(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = fs::read_dir(self.profiles_dir()) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("toml") {
                    continue;
                }
                if let Ok(text) = fs::read_to_string(&p)
                    && let Ok(l) = migrate::parse_profile(&text, Format::Toml)
                {
                    names.push(l.value.name);
                }
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    }

    pub fn profile_exists(&self, name: &str) -> bool {
        self.profile_path(name).exists()
    }

    pub fn load_profile(&self, name: &str) -> Result<Loaded<Profile>, ConfigError> {
        let path = self.profile_path(name);
        let text = fs::read_to_string(&path).map_err(|_| ConfigError::NotFound(name.to_string()))?;
        migrate::parse_profile(&text, Format::Toml)
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<(), ConfigError> {
        let mut p = profile.clone();
        p.sanitize();
        let text = migrate::profile_to_string(&p, Format::Toml)?;
        let path = self.profile_path(&p.name);
        if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        self.auto_backup(&path);
        write_atomic(&path, text.as_bytes())
    }

    /// Deletes a profile (a backup copy is kept).
    pub fn delete_profile(&self, name: &str) -> Result<(), ConfigError> {
        let path = self.profile_path(name);
        if !path.exists() {
            return Err(ConfigError::NotFound(name.to_string()));
        }
        self.auto_backup(&path);
        fs::remove_file(&path).map_err(|e| io_err(&path, e))
    }

    pub fn rename_profile(&self, old: &str, new: &str) -> Result<Profile, ConfigError> {
        let mut p = self.load_profile(old)?.value;
        let new = sanitize_name(new);
        if self.profile_exists(&new) && sanitize_name(old) != new {
            return Err(ConfigError::AlreadyExists(new));
        }
        p.name = new;
        self.save_profile(&p)?;
        if sanitize_name(old) != p.name {
            self.delete_profile(old)?;
        }
        Ok(p)
    }

    /// Imports a TOML or JSON profile, validating size, schema and values.
    pub fn import_profile(&self, path: &Path) -> Result<Loaded<Profile>, ConfigError> {
        let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
        if meta.len() > MAX_IMPORT_BYTES {
            return Err(ConfigError::TooLarge { size: meta.len(), limit: MAX_IMPORT_BYTES });
        }
        let text = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
        migrate::parse_profile(&text, Format::from_path(path))
    }

    /// Exports a profile; the format follows the file extension.
    pub fn export_profile(&self, profile: &Profile, path: &Path) -> Result<(), ConfigError> {
        let text = migrate::profile_to_string(profile, Format::from_path(path))?;
        write_atomic(path, text.as_bytes())
    }

    /// Creates a full snapshot of the configuration and all profiles.
    pub fn create_backup(&self) -> Result<PathBuf, ConfigError> {
        let dir = self.backups_dir().join(utc_timestamp());
        let pdir = dir.join("profiles");
        fs::create_dir_all(&pdir).map_err(|e| io_err(&pdir, e))?;
        let cfg = self.config_path();
        if cfg.exists() {
            fs::copy(&cfg, dir.join("config.toml")).map_err(|e| io_err(&cfg, e))?;
        }
        if let Ok(entries) = fs::read_dir(self.profiles_dir()) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file()
                    && let Some(name) = p.file_name()
                {
                    fs::copy(&p, pdir.join(name)).map_err(|e| io_err(&p, e))?;
                }
            }
        }
        Ok(dir)
    }

    /// Manual snapshots, newest first.
    pub fn list_backups(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.backups_dir())
            .map(|it| {
                it.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(String::from)).filter(|n| n != "auto").collect()
            })
            .unwrap_or_default();
        v.sort();
        v.reverse();
        v
    }

    /// Restores a snapshot (the current state is snapshotted first).
    pub fn restore_backup(&self, name: &str) -> Result<(), ConfigError> {
        let src = self.backups_dir().join(sanitize_name(name));
        if !src.is_dir() {
            return Err(ConfigError::NotFound(name.to_string()));
        }
        self.create_backup()?;
        let cfg = src.join("config.toml");
        if cfg.exists() {
            let text = fs::read(&cfg).map_err(|e| io_err(&cfg, e))?;
            write_atomic(&self.config_path(), &text)?;
        }
        if let Ok(entries) = fs::read_dir(src.join("profiles")) {
            for e in entries.flatten() {
                let p = e.path();
                if let Some(n) = p.file_name() {
                    let text = fs::read(&p).map_err(|e| io_err(&p, e))?;
                    write_atomic(&self.profiles_dir().join(n), &text)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{Scenario, scenario_profile};

    fn temp_store(tag: &str) -> ConfigStore {
        let dir = std::env::temp_dir().join(format!("novamix-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let s = ConfigStore::at(dir);
        s.ensure_dirs().unwrap();
        s
    }

    #[test]
    fn timestamp_format() {
        let t = utc_timestamp();
        assert_eq!(t.len(), 15);
        assert_eq!(&t[8..9], "-");
        assert!(t.starts_with("20"));
    }

    #[test]
    fn save_load_list_delete_profiles() {
        let s = temp_store("profiles");
        for sc in Scenario::ALL {
            s.save_profile(&scenario_profile(sc)).unwrap();
        }
        let names = s.list_profiles();
        assert_eq!(names.len(), 4);
        let p = s.load_profile("Reunião").unwrap();
        assert_eq!(p.value, scenario_profile(Scenario::Meeting));
        s.delete_profile("Reunião").unwrap();
        assert_eq!(s.list_profiles().len(), 3);
        assert!(s.backups_dir().join("auto").read_dir().unwrap().count() >= 1);
        let _ = fs::remove_dir_all(s.root());
    }

    #[test]
    fn config_roundtrip_and_corrupt_recovery() {
        let s = temp_store("config");
        let mut c = AppConfig::default();
        c.general.ui_scale = 1.25;
        s.save_config(&c).unwrap();
        let (back, notes) = s.load_config();
        assert_eq!(back, c);
        assert!(notes.is_empty());
        fs::write(s.config_path(), "this is [not toml").unwrap();
        let (back, notes) = s.load_config();
        assert_eq!(back, AppConfig::default());
        assert_eq!(notes.len(), 1);
        let _ = fs::remove_dir_all(s.root());
    }

    #[test]
    fn import_rejects_large_files_and_exports_json() {
        let s = temp_store("import");
        let big = s.root().join("big.toml");
        fs::write(&big, vec![b'#'; (MAX_IMPORT_BYTES + 1) as usize]).unwrap();
        assert!(matches!(s.import_profile(&big), Err(ConfigError::TooLarge { .. })));
        let out = s.root().join("export.json");
        let p = scenario_profile(Scenario::Streaming);
        s.export_profile(&p, &out).unwrap();
        let back = s.import_profile(&out).unwrap();
        assert_eq!(back.value, p);
        let _ = fs::remove_dir_all(s.root());
    }

    #[test]
    fn backup_and_restore() {
        let s = temp_store("backup");
        s.save_profile(&scenario_profile(Scenario::Gaming)).unwrap();
        let b = s.create_backup().unwrap();
        s.delete_profile("Jogo").unwrap();
        assert!(s.list_profiles().is_empty());
        let name = b.file_name().unwrap().to_str().unwrap().to_string();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        s.restore_backup(&name).unwrap();
        assert_eq!(s.list_profiles(), vec!["Jogo".to_string()]);
        let _ = fs::remove_dir_all(s.root());
    }
}
