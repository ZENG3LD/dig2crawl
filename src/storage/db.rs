use ahash::RandomState;
use chrono::Utc;
use crate::core::error::StorageError;
use crate::core::traits::Storage;
use crate::core::types::{CrawlJob, CrawlStats, ExtractedRecord, JobStatus};
use futures::future::BoxFuture;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS crawl_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    config_json TEXT NOT NULL,
    goal_json TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    started_at TEXT NOT NULL,
    completed_at TEXT,
    pages_fetched INTEGER NOT NULL DEFAULT 0,
    records_found INTEGER NOT NULL DEFAULT 0,
    errors INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS visited_urls (
    job_id TEXT NOT NULL REFERENCES crawl_jobs(id),
    url TEXT NOT NULL,
    url_hash INTEGER NOT NULL,
    depth INTEGER NOT NULL DEFAULT 0,
    status_code INTEGER,
    visited_at TEXT NOT NULL,
    PRIMARY KEY (job_id, url_hash)
);

CREATE TABLE IF NOT EXISTS records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL REFERENCES crawl_jobs(id),
    url TEXT NOT NULL,
    data_json TEXT NOT NULL,
    confidence REAL,
    extracted_at TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'agent'
);

CREATE TABLE IF NOT EXISTS site_memory (
    job_id TEXT NOT NULL REFERENCES crawl_jobs(id),
    domain TEXT NOT NULL,
    memory_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (job_id, domain)
);

CREATE INDEX IF NOT EXISTS idx_records_job ON records(job_id);
CREATE INDEX IF NOT EXISTS idx_visited_job ON visited_urls(job_id);
";

/// Compute a deterministic 64-bit hash of a URL string.
/// Uses fixed seeds so the hash is consistent across calls.
fn url_hash(url: &str) -> i64 {
    // RandomState::with_seeds takes four u64 values; fixed constants give deterministic output.
    let state = RandomState::with_seeds(0xdead_beef, 0xcafe_babe, 0x1234_5678, 0xabcd_ef01);
    state.hash_one(url) as i64
}

/// SQLite-backed crawl storage with WAL mode.
pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    /// Open (or create) a SQLite database at the given path.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path)
            .map_err(|e| StorageError::Database(format!("open: {e}")))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| StorageError::Database(format!("pragmas: {e}")))?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory SQLite database (useful for testing).
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| StorageError::Database(format!("open_in_memory: {e}")))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| StorageError::Database(format!("pragmas: {e}")))?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_schema(conn: &Connection) -> Result<(), StorageError> {
        conn.execute_batch(SCHEMA)
            .map_err(|e| StorageError::Database(format!("init schema: {e}")))?;
        Ok(())
    }

    /// Insert a new crawl job record into the database.
    pub async fn create_job(&self, job: &CrawlJob) -> Result<(), StorageError> {
        let conn = self.conn.lock().await;
        let config_json = serde_json::to_string(&job.config)?;
        let goal_json = job
            .config
            .agent_goal
            .as_ref()
            .map(|g| serde_json::to_string(g))
            .transpose()?;
        let status_str = job_status_str(&job.status);
        conn.execute(
            "INSERT INTO crawl_jobs (id, name, config_json, goal_json, status, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id.to_string(),
                job.config.name.as_str(),
                config_json,
                goal_json,
                status_str,
                job.started_at.to_rfc3339(),
            ],
        )
        .map_err(|e| StorageError::Database(format!("create_job: {e}")))?;
        Ok(())
    }

    /// Update the status (and optional completed_at timestamp) for a job.
    pub async fn update_job_status(
        &self,
        job_id: Uuid,
        status: &str,
        completed_at: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE crawl_jobs SET status = ?1, completed_at = ?2 WHERE id = ?3",
            params![status, completed_at, job_id.to_string()],
        )
        .map_err(|e| StorageError::Database(format!("update_job_status: {e}")))?;
        Ok(())
    }

    /// Persist agent site-memory for a domain (upsert).
    pub async fn save_memory(
        &self,
        job_id: Uuid,
        domain: &str,
        memory_json: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO site_memory (job_id, domain, memory_json, updated_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                job_id.to_string(),
                domain,
                memory_json,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| StorageError::Database(format!("save_memory: {e}")))?;
        Ok(())
    }

    /// Load agent site-memory for a domain. Returns `None` if not found.
    pub async fn load_memory(
        &self,
        job_id: Uuid,
        domain: &str,
    ) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock().await;
        let result = conn.query_row(
            "SELECT memory_json FROM site_memory WHERE job_id = ?1 AND domain = ?2",
            params![job_id.to_string(), domain],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => Ok(Some(json)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Database(format!("load_memory: {e}"))),
        }
    }

}

fn job_status_str(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Failed { .. } => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

impl Storage for SqliteStorage {
    fn save_record<'a>(
        &'a self,
        record: &'a ExtractedRecord,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            let data_json = serde_json::to_string(&record.data)?;
            conn.execute(
                "INSERT INTO records (job_id, url, data_json, confidence, extracted_at, source) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'agent')",
                params![
                    record.job_id.to_string(),
                    record.url.as_str(),
                    data_json,
                    record.confidence,
                    record.extracted_at.to_rfc3339(),
                ],
            )
            .map_err(|e| StorageError::Database(format!("save_record: {e}")))?;
            conn.execute(
                "UPDATE crawl_jobs SET records_found = records_found + 1 WHERE id = ?1",
                params![record.job_id.to_string()],
            )
            .map_err(|e| StorageError::Database(format!("increment records: {e}")))?;
            Ok(())
        })
    }

    fn save_stats<'a>(
        &'a self,
        stats: &'a CrawlStats,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let conn = self.conn.lock().await;
            conn.execute(
                "UPDATE crawl_jobs SET pages_fetched = ?1, records_found = ?2, errors = ?3 \
                 WHERE id = ?4",
                params![
                    stats.pages_fetched as i64,
                    stats.records_extracted as i64,
                    stats.errors as i64,
                    stats.job_id.to_string(),
                ],
            )
            .map_err(|e| StorageError::Database(format!("save_stats: {e}")))?;
            Ok(())
        })
    }

    fn mark_visited<'a>(
        &'a self,
        job_id: Uuid,
        url: &'a Url,
        status: Option<u16>,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let hash = url_hash(url.as_str());
            let conn = self.conn.lock().await;
            conn.execute(
                "INSERT OR IGNORE INTO visited_urls \
                 (job_id, url, url_hash, status_code, visited_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    job_id.to_string(),
                    url.as_str(),
                    hash,
                    status.map(|s| s as i64),
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| StorageError::Database(format!("mark_visited: {e}")))?;
            conn.execute(
                "UPDATE crawl_jobs SET pages_fetched = pages_fetched + 1 WHERE id = ?1",
                params![job_id.to_string()],
            )
            .map_err(|e| StorageError::Database(format!("increment pages: {e}")))?;
            Ok(())
        })
    }

    fn is_visited<'a>(
        &'a self,
        job_id: Uuid,
        url: &'a Url,
    ) -> BoxFuture<'a, Result<bool, StorageError>> {
        Box::pin(async move {
            let hash = url_hash(url.as_str());
            let conn = self.conn.lock().await;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM visited_urls WHERE job_id = ?1 AND url_hash = ?2",
                    params![job_id.to_string(), hash],
                    |row| row.get(0),
                )
                .map_err(|e| StorageError::Database(format!("is_visited: {e}")))?;
            Ok(count > 0)
        })
    }
}
