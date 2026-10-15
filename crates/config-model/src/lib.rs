//! # novamix-config
//!
//! Versioned configuration and profile model with validation, migrations,
//! atomic persistence, backups and auditable profile diffs.

pub mod defaults;
pub mod diff;
pub mod migrate;
pub mod model;
pub mod store;
pub mod validate;

pub use defaults::{DEFAULT_PROFILE_NAME, Scenario, default_profile, safe_mode_of, scenario_profile};
pub use diff::{ChangeScope, ProfileChange, diff_profiles};
pub use migrate::{Format, Loaded};
pub use model::*;
pub use novamix_mixer_graph::SoloMode;
pub use store::ConfigStore;
pub use validate::Issue;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error on {path}: {message}")]
    Io { path: String, message: String },
    #[error("invalid file: {0}")]
    Parse(String),
    #[error("could not serialize: {0}")]
    Serialize(String),
    #[error("file is too large ({size} bytes, limit {limit})")]
    TooLarge { size: u64, limit: u64 },
    #[error("file was created by a newer NovaMix (schema v{found}, this version supports up to v{supported})")]
    NewerVersion { found: u32, supported: u32 },
    #[error("profile not found: {0}")]
    NotFound(String),
    #[error("a profile named {0} already exists")]
    AlreadyExists(String),
}
