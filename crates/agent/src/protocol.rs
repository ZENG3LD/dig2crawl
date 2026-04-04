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
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Success,
    PartialSuccess,
    NoData,
    Failed { reason: String },
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSelectors {
    pub container_selector: Option<String>,
    pub fields: HashMap<String, String>,
    pub confidence: f32,
    pub validated_on_pages: usize,
}
