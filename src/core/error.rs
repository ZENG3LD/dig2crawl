use thiserror::Error;

/// Why L2 interactive actions failed.
#[derive(Debug, Error)]
pub enum InteractiveError {
    #[error("all {count} selectors in action plan not found in DOM")]
    SelectorsNotFound { count: usize },
    #[error("action sequence produced empty HTML")]
    EmptyResult,
    #[error("browser error during action: {0}")]
    Browser(String),
}

/// Outcome returned from each extraction level to the coordinator.
#[derive(Debug)]
pub enum EscalationResult {
    /// Level produced usable records — stop here.
    Success {
        records: Vec<serde_json::Value>,
        level: u8,
    },
    /// Level produced nothing — escalate to next level.
    Escalate { reason: String },
    /// Captcha detected — L4 stub.
    CaptchaBlocked { provider: String },
    /// Max level reached, no records found.
    Failed,
}

#[derive(Debug, Error)]
pub enum CrawlError {
    #[error("fetch error: {0}")]
    Fetch(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("queue error: {0}")]
    Queue(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("storage: {source}")]
    Storage {
        #[from]
        source: StorageError,
    },
    #[error("agent: {source}")]
    Agent {
        #[from]
        source: AgentError,
    },
    #[error("robots.txt blocked: {url}")]
    RobotsBlocked { url: String },
    #[error("budget exhausted")]
    BudgetExhausted,
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("timeout after {secs}s")]
    Timeout { secs: u64 },
    #[error("parse response: {0}")]
    ParseResponse(String),
    #[error("process failed: {0}")]
    ProcessFailed(String),
    #[error("interactive extraction failed: {0}")]
    Interactive(#[from] InteractiveError),
}

impl AgentError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::ProcessFailed(_))
    }
}
