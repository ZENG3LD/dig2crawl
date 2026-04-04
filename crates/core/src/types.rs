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
