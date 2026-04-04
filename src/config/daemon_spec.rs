//! Daemon spec — the final artefact that a daemon/scheduler consumes.
//!
//! A `DaemonSpec` is the complete self-contained specification for a recurring
//! crawl job: it bundles the job parameters, the discovered site profile, the
//! output format, and a cron schedule.  It is serialisable to both JSON and
//! TOML so it can be stored, version-controlled, and loaded by daemon runners.

use crate::config::job::{RateLimitConfig};
use crate::config::profile::SiteProfile;
use crate::core::types::FetchMethod;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Error type for daemon spec I/O.
#[derive(Debug, Error)]
pub enum DaemonSpecError {
    #[error("failed to read daemon spec file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to deserialize daemon spec JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to serialize daemon spec to TOML: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("failed to deserialize daemon spec from TOML: {0}")]
    TomlDe(#[from] toml::de::Error),
}

/// Output format for extracted records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// One JSON object per file (array of records).
    Json,
    /// One JSON object per line (newline-delimited JSON).
    Jsonl,
    /// Comma-separated values.
    Csv,
}

/// Output destination configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Format in which extracted records are written.
    pub format: OutputFormat,
    /// File path where output is written.
    pub path: PathBuf,
    /// Whether to append to an existing file (`true`) or overwrite it (`false`).
    #[serde(default)]
    pub append: bool,
}

/// Complete, self-contained specification consumed by a crawler daemon.
///
/// The spec contains everything needed to run the job on a recurring schedule
/// without any interactive input.
///
/// # Serialisation
///
/// Both JSON and TOML round-trips are supported via [`DaemonSpec::load_json`],
/// [`DaemonSpec::save_json`], [`DaemonSpec::load_toml`], and
/// [`DaemonSpec::save_toml`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSpec {
    /// Human-readable name for this daemon job.
    pub name: String,
    /// Primary domain being crawled (e.g. `"hh.ru"`).
    pub domain: String,
    /// Seed URLs where each run starts.
    pub seed_urls: Vec<String>,
    /// Site profile discovered by the agent.
    pub profile: SiteProfile,
    /// Cron expression describing how often to run (e.g. `"0 6 * * *"`).
    pub schedule: String,
    /// Fetch strategy — plain HTTP or headless browser.
    pub fetch_method: FetchMethod,
    /// Rate-limiting parameters applied during each run.
    pub rate_limit: RateLimitConfig,
    /// Output destination.
    pub output: OutputConfig,
}

impl DaemonSpec {
    /// Deserialise a `DaemonSpec` from a JSON file at `path`.
    pub fn load_json(path: &Path) -> Result<Self, DaemonSpecError> {
        let content = std::fs::read_to_string(path)?;
        let spec: Self = serde_json::from_str(&content)?;
        Ok(spec)
    }

    /// Serialise this spec to pretty-printed JSON and write to `path`.
    pub fn save_json(&self, path: &Path) -> Result<(), DaemonSpecError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Deserialise a `DaemonSpec` from a TOML file at `path`.
    pub fn load_toml(path: &Path) -> Result<Self, DaemonSpecError> {
        let content = std::fs::read_to_string(path)?;
        let spec: Self = toml::from_str(&content)?;
        Ok(spec)
    }

    /// Serialise this spec to TOML and write to `path`.
    pub fn save_toml(&self, path: &Path) -> Result<(), DaemonSpecError> {
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Return this spec serialised as a TOML string (without writing to disk).
    pub fn to_toml_string(&self) -> Result<String, DaemonSpecError> {
        Ok(toml::to_string_pretty(self)?)
    }
}
