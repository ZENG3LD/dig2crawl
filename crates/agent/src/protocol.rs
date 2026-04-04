use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: &str = "2.0.0";

/// Serialised to JSON, written to a temp file, path passed to claude CLI.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentRequest {
    pub version: String,
    pub task_id: String,
    pub url: String,
    pub html_path: String,
    pub screenshot_path: Option<String>,
    pub goal: AgentGoalSpec,
    pub site_memory: SiteMemorySnapshot,
    pub context: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentGoalSpec {
    pub target: String,
    pub fields: Vec<String>,
    pub notes: Option<String>,
}

/// Agent response — parsed from claude CLI stdout JSON.
///
/// Version 2 adds `field_configs`, `pagination`, and `validation_result`
/// while keeping all v1 fields intact for backward compatibility.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentResponse {
    pub version: String,
    pub task_id: String,
    pub status: AgentStatus,
    #[serde(default)]
    pub records: Vec<serde_json::Value>,
    #[serde(default)]
    pub next_urls: Vec<NextUrl>,
    pub updated_memory: Option<SiteMemorySnapshot>,
    pub confidence: Option<f32>,
    #[serde(default)]
    pub logs: Vec<String>,

    // ── v2 additions ──────────────────────────────────────────────────────────

    /// Rich per-field extraction config discovered by Claude (Phase 1).
    /// When present, callers should prefer this over the flat `updated_memory.selectors`.
    #[serde(default)]
    pub field_configs: Vec<FieldConfig>,

    /// Pagination strategy discovered for this page (Phase 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationConfig>,

    /// Result from Phase 2 validation — present only when the agent was asked
    /// to validate previously discovered selectors against real extracted data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_result: Option<ValidationResult>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Success,
    Partial,
    PartialSuccess,
    NoData,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NextUrl {
    pub url: String,
    pub priority: String,
    pub reason: String,
}

/// Per-domain knowledge accumulated by the agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiteMemorySnapshot {
    pub domain: String,
    pub selectors: HashMap<String, LearnedSelectors>,
    pub url_patterns: HashMap<String, String>,
    pub pages_seen: usize,
    pub records_found: usize,
    pub requires_browser: bool,
    #[serde(default)]
    pub notes: Vec<String>,

    // ── v2 additions ──────────────────────────────────────────────────────────

    /// Anti-bot behaviour observed on the site (rate-limiting, CAPTCHAs, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub antibot_notes: Option<String>,

    /// URLs that the agent flagged as promising but has not yet visited.
    #[serde(default)]
    pub pending_urls: Vec<String>,

    /// URLs that failed extraction with reasons — used to avoid retrying bad paths.
    #[serde(default)]
    pub failed_urls: Vec<FailedUrl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSelectors {
    pub container_selector: Option<String>,
    /// v1: flat map from field name → CSS selector string.
    pub fields: HashMap<String, String>,
    pub confidence: f32,
    pub validated_on_pages: usize,
}

// ── v2 types ─────────────────────────────────────────────────────────────────

/// Rich per-field extraction spec returned by Phase 1 discovery.
///
/// Carries not just the CSS selector but also *how* to extract from the matched
/// element and an optional transform to apply to the raw string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConfig {
    /// Name of the field as specified in the original goal.
    pub name: String,
    /// CSS selector targeting the element that holds this field's value.
    pub selector: String,
    /// How to read the value from the matched element.
    pub extract: ExtractMode,
    /// Prepend this string to the extracted value (e.g. domain for relative URLs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Optional post-processing transform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
}

/// Controls how a value is read from a matched DOM element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    /// `element.text_content()` trimmed of whitespace.
    Text,
    /// `element.attr(name)` — the inner `String` is the attribute name.
    Attribute(String),
    /// `element.inner_html()` — serialised inner HTML.
    Html,
    /// `element.outer_html()` — serialised outer HTML including the element itself.
    OuterHtml,
}

/// Optional post-processing applied to the raw extracted string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Trim,
    Lowercase,
    Uppercase,
    /// Apply a regex and return capture group 1.
    Regex(String),
    /// Replace all occurrences: `(from, to)`.
    Replace(String, String),
    ParseNumber,
}

/// Pagination strategy discovered by Claude for this site.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaginationConfig {
    /// Classic "Next" button with a fixed CSS selector.
    NextButton { selector: String },
    /// URL contains a page number placeholder `{n}`.
    UrlPattern {
        template: String,
        start: u32,
        end: Option<u32>,
        step: u32,
    },
    /// Infinite scroll — trigger when within `trigger_px` of the bottom.
    InfiniteScroll {
        trigger_px: u32,
        max_scrolls: u32,
    },
    /// "Load more" button clicked up to `max_clicks` times.
    LoadMore {
        button_selector: String,
        max_clicks: u32,
    },
    /// Offset-based query parameter (`?offset=0`, `?offset=20`, …).
    OffsetParam {
        param_name: String,
        page_size: u32,
        max_pages: Option<u32>,
    },
}

/// Result from Phase 2 validation — Claude's assessment of whether the
/// selectors discovered in Phase 1 actually work on real page data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the selectors are considered valid and production-ready.
    pub passed: bool,
    /// How many sample items were successfully extracted during validation.
    pub items_extracted: usize,
    /// Per-field validation status — field name → `true` if the selector worked.
    pub field_status: HashMap<String, bool>,
    /// Human-readable summary of what was checked and whether it passed.
    pub summary: String,
    /// Claude's confidence that the config generalises beyond the sample pages.
    pub confidence: f32,
    /// Issues found during validation (empty on full pass).
    #[serde(default)]
    pub issues: Vec<String>,
}

/// Record of a URL that failed extraction, stored to avoid re-attempting bad paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedUrl {
    pub url: String,
    pub reason: String,
    pub attempts: usize,
}
