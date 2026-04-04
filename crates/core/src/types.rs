use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashMap;
use url::Url;
use uuid::Uuid;

// ---- Deduplication ----

/// 64-bit hash of a normalised URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UrlHash(pub u64);

// ---- Fetch ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FetchMethod {
    Http,
    Browser { wait_selector: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedPage {
    pub url: Url,
    pub status_code: Option<u16>,
    pub body: String,
    pub fetched_at: DateTime<Utc>,
    pub fetch_ms: u64,
    pub method: FetchMethod,
    /// Optional screenshot bytes (PNG). Set only when browser mode is used
    /// and the config requests screenshots.
    #[serde(skip)]
    pub screenshot: Option<Vec<u8>>,
}

// ---- Queue ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Low = 1,
    Normal = 2,
    High = 3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedUrl {
    pub url: Url,
    pub priority: Priority,
    pub depth: usize,
    pub queued_at: DateTime<Utc>,
}

impl QueuedUrl {
    pub fn new(url: Url, depth: usize) -> Self {
        Self {
            url,
            priority: Priority::Normal,
            depth,
            queued_at: Utc::now(),
        }
    }

    pub fn with_priority(self, p: Priority) -> Self {
        Self { priority: p, ..self }
    }
}

// ---- Job / Config ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlConfig {
    /// Human-readable name for this crawl job.
    pub name: SmolStr,
    /// Allowed domains (crawl stays within these).
    pub domains: Vec<SmolStr>,
    pub start_urls: Vec<Url>,
    pub max_depth: Option<usize>,
    pub max_pages: Option<usize>,
    pub rate: RateConfig,
    pub fetch_method: FetchMethod,
    /// Optional extra HTTP headers to send with every request.
    pub headers: Option<HashMap<String, String>>,
    pub follow_links: bool,
    pub link_patterns: Option<Vec<String>>,
    pub exclude_patterns: Option<Vec<String>>,
    /// Goal description passed to the agent. If None, agent mode is disabled.
    pub agent_goal: Option<AgentGoal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateConfig {
    pub requests_per_second: f64,
    pub min_delay_ms: u64,
    pub concurrent_requests: usize,
}

/// Generic, domain-agnostic extraction goal description.
/// The caller (CLI or library user) decides what to extract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentGoal {
    /// One-line description of what to extract. E.g. "job listings",
    /// "news article bodies", "real-estate listings", "restaurant menus".
    pub target: String,
    /// Field names to extract. E.g. ["title", "date", "body", "author"].
    pub fields: Vec<String>,
    /// Optional extra notes / constraints for the agent.
    pub notes: Option<String>,
}

// ---- Job runtime state ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlJob {
    pub id: Uuid,
    pub config: CrawlConfig,
    pub started_at: DateTime<Utc>,
    pub status: JobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed { reason: String },
    Cancelled,
}

// ---- Extraction result ----

/// A single extracted record. The `data` field is an arbitrary JSON object
/// whose keys are whatever the agent or built-in extractor produced.
/// There is no product-specific schema — callers define their own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedRecord {
    pub job_id: Uuid,
    pub url: Url,
    /// Raw JSON object with extracted fields.
    pub data: serde_json::Value,
    pub extracted_at: DateTime<Utc>,
    /// Confidence [0.0, 1.0] reported by the agent, None for built-in extraction.
    pub confidence: Option<f32>,
}

// ---- Stats ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlStats {
    pub job_id: Uuid,
    pub pages_fetched: u64,
    pub records_extracted: u64,
    pub errors: u64,
    pub queue_size: usize,
    pub elapsed_secs: u64,
}

// ---- Page metadata (extracted from HTML) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical_url: Option<Url>,
    pub language: Option<String>,
}

// ---- Site profile (learned by Claude, consumed by SelectorExtractor) ----

/// What Claude discovers about a site during Phase 1+2.
/// Stored per domain, persisted to the site_profiles table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteProfile {
    pub domain: String,
    /// CSS selector for the repeating container element.
    pub container_selector: String,
    /// One entry per field in the user's goal.
    pub fields: Vec<FieldConfig>,
    pub pagination: Option<PaginationConfig>,
    pub requires_browser: bool,
    /// Claude's confidence after validation [0.0, 1.0].
    pub confidence: f64,
    /// Whether Phase 2 validation was completed.
    pub validated: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

/// Rich per-field extraction spec learned by Claude, consumed by `SelectorExtractor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConfig {
    pub name: String,
    pub selector: String,
    pub extract: ExtractMode,
    /// Prepend domain for relative URLs (e.g. `"https://example.com"`).
    pub prefix: Option<String>,
    pub transform: Option<Transform>,
}

/// How to extract the value from a matched element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    /// Trimmed text content of the element.
    Text,
    /// Value of the named HTML attribute.
    Attribute(String),
    /// Inner HTML of the element (excludes the element tag itself).
    Html,
    /// Outer HTML of the element (includes the element tag).
    OuterHtml,
}

/// Optional post-extraction string transformation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Trim,
    Lowercase,
    Uppercase,
    /// Extract regex capture group 1.
    Regex(String),
    /// Replace `from` with `to`.
    Replace(String, String),
    /// Strip non-numeric chars, parse as f64 string.
    ParseNumber,
}

/// How the crawler should follow pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaginationConfig {
    NextButton {
        selector: String,
    },
    UrlPattern {
        /// e.g. `"https://example.com/page/{n}"`
        template: String,
        start: u32,
        end: Option<u32>,
        step: u32,
    },
    InfiniteScroll {
        /// Pixels from bottom to trigger scroll.
        trigger_px: u32,
        /// Maximum scroll iterations.
        max_scrolls: u32,
    },
    LoadMore {
        button_selector: String,
        max_clicks: u32,
    },
    /// Offset query parameter: `?offset=0`, `?offset=20` …
    OffsetParam {
        param_name: String,
        page_size: u32,
        max_pages: Option<u32>,
    },
}

// ---- Daemon spec ----

/// Daemon-ready config exported after Phase 4.
/// Fully self-contained — no agent needed at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSpec {
    pub name: String,
    pub domain: String,
    pub seed_urls: Vec<String>,
    pub site_profile: SiteProfile,
    pub schedule: CronSchedule,
    pub fetch_method: FetchMethod,
    pub rate_limit: RateLimitConfig,
    pub output_format: OutputFormat,
    pub created_at: DateTime<Utc>,
    /// Semver string, e.g. `"1.0"`.
    pub spec_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    /// Standard 5-field cron expression.
    pub expression: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_second: f64,
    pub min_delay_ms: u64,
    pub concurrent_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Jsonl,
    Json,
    Csv,
    Sqlite,
}
