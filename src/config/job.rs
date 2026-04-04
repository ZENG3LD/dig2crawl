//! Job configuration loaded from TOML.
//!
//! A `JobConfig` describes a single crawl job: where to start, how to fetch
//! pages, rate-limiting, budget constraints, and optional proxy settings.

use crate::core::types::FetchMethod;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Error type for job config loading.
#[derive(Debug, Error)]
pub enum JobConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Rate-limit configuration for a crawl job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Target requests per second (may be fractional, e.g. 0.5 = 1 req/2 s).
    pub requests_per_second: f64,
    /// Minimum delay between requests in milliseconds.
    pub min_delay_ms: u64,
    /// Maximum number of concurrent in-flight requests.
    pub concurrent_requests: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 1.0,
            min_delay_ms: 500,
            concurrent_requests: 1,
        }
    }
}

/// Budget / termination constraints for a crawl job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Stop after fetching this many pages (None = unlimited).
    pub max_pages: Option<usize>,
    /// Maximum link-follow depth from seed URLs (None = unlimited).
    pub max_depth: Option<usize>,
    /// Maximum wall-clock duration in seconds (None = unlimited).
    pub max_duration_secs: Option<u64>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_pages: Some(1000),
            max_depth: Some(5),
            max_duration_secs: Some(3600),
        }
    }
}

/// HTTP/SOCKS proxy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy URL, e.g. `socks5://127.0.0.1:1080` or `http://proxy:3128`.
    pub url: String,
    /// Optional username for proxy authentication.
    pub username: Option<String>,
    /// Optional password for proxy authentication.
    pub password: Option<String>,
}

/// Top-level job configuration loaded from a TOML file.
///
/// Example TOML:
/// ```toml
/// name = "hh-jobs"
/// seed_urls = ["https://hh.ru/vacancies/"]
/// goal = "Extract job listings: title, company, salary, location"
/// follow_links = true
/// allowed_domains = ["hh.ru"]
///
/// [fetch_method]
/// Browser = { wait_selector = ".vacancy-card" }
///
/// [rate_limit]
/// requests_per_second = 0.5
/// min_delay_ms = 2000
/// concurrent_requests = 1
///
/// [budget]
/// max_pages = 500
/// max_depth = 3
/// max_duration_secs = 1800
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobConfig {
    /// Human-readable name for this job.
    pub name: String,
    /// Seed URLs where the crawl starts.
    pub seed_urls: Vec<String>,
    /// Optional natural-language goal passed to the Claude agent for discovery.
    /// When None, agent-based discovery is skipped.
    pub goal: Option<String>,
    /// How to fetch pages — plain HTTP or headless browser.
    pub fetch_method: FetchMethod,
    /// Rate-limiting parameters.
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    /// Budget / termination constraints.
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Whether to follow links found on fetched pages.
    #[serde(default = "default_true")]
    pub follow_links: bool,
    /// Restrict crawling to these domains. Empty list = no restriction.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Optional HTTP/SOCKS proxy.
    pub proxy: Option<ProxyConfig>,
}

fn default_true() -> bool {
    true
}

impl JobConfig {
    /// Load a `JobConfig` from a TOML file at `path`.
    pub fn load(path: &Path) -> Result<Self, JobConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
