use crate::memory::DomainMemory;
use crate::protocol::{AgentGoalSpec, AgentRequest, SiteMemorySnapshot, PROTOCOL_VERSION};
use crate::spawn::ClaudeSpawner;
use chrono::Utc;
use crawl_core::error::AgentError;
use crawl_core::traits::{AgentResult, CrawlAgent};
use crawl_core::types::{AgentGoal, ExtractedRecord, FetchedPage, Priority, QueuedUrl};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;
use url::Url;
use uuid::Uuid;

/// Implements [`CrawlAgent`] by wiring together [`ClaudeSpawner`] and [`DomainMemory`].
pub struct AgentBridge {
    spawner: ClaudeSpawner,
    memories: Arc<Mutex<HashMap<String, DomainMemory>>>,
    work_dir: PathBuf,
}

impl AgentBridge {
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            spawner: ClaudeSpawner::new(),
            memories: Arc::new(Mutex::new(HashMap::new())),
            work_dir,
        }
    }

    pub fn with_spawner(mut self, spawner: ClaudeSpawner) -> Self {
        self.spawner = spawner;
        self
    }

    fn domain_from_url(url: &Url) -> String {
        url.host_str().unwrap_or("unknown").to_string()
    }

    async fn get_or_create_memory(&self, domain: &str) -> DomainMemory {
        let mut memories = self.memories.lock().await;
        memories
            .entry(domain.to_string())
            .or_insert_with(|| DomainMemory::new(domain.to_string()))
            .clone()
    }
}

impl CrawlAgent for AgentBridge {
    fn process_page<'a>(
        &'a self,
        page: &'a FetchedPage,
        goal: &'a AgentGoal,
        job_id: Uuid,
        depth: usize,
    ) -> BoxFuture<'a, Result<AgentResult, AgentError>> {
        Box::pin(async move {
            let domain = Self::domain_from_url(&page.url);
            let memory = self.get_or_create_memory(&domain).await;
            let snapshot: SiteMemorySnapshot = memory.snapshot().await;

            // Create a per-task work directory.
            let task_id = Uuid::new_v4().to_string();
            let task_dir = self.work_dir.join(&task_id);
            tokio::fs::create_dir_all(&task_dir)
                .await
                .map_err(|e| AgentError::Spawn(format!("create task dir: {e}")))?;

            // Write the raw HTML so the agent can read it from disk.
            let html_path = task_dir.join("page.html");
            tokio::fs::write(&html_path, &page.body)
                .await
                .map_err(|e| AgentError::Spawn(format!("write page.html: {e}")))?;

            let request = AgentRequest {
                version: PROTOCOL_VERSION.to_string(),
                task_id: task_id.clone(),
                url: page.url.to_string(),
                html_path: html_path.to_string_lossy().to_string(),
                screenshot_path: None,
                goal: AgentGoalSpec {
                    target: goal.target.clone(),
                    fields: goal.fields.clone(),
                    notes: goal.notes.clone(),
                },
                site_memory: snapshot,
                context: {
                    let mut ctx = HashMap::new();
                    ctx.insert("job_id".to_string(), job_id.to_string());
                    ctx.insert("depth".to_string(), depth.to_string());
                    ctx
                },
            };

            info!(task_id = %task_id, url = %page.url, "Invoking agent");
            let response = self.spawner.invoke(&request, &task_dir).await?;

            // Persist updated domain knowledge if the agent returned it.
            if let Some(updated) = response.updated_memory {
                memory.apply_update(updated).await;
            }
            memory.increment_pages().await;
            memory.add_records(response.records.len()).await;

            let records: Vec<ExtractedRecord> = response
                .records
                .into_iter()
                .map(|data| ExtractedRecord {
                    job_id,
                    url: page.url.clone(),
                    data,
                    extracted_at: Utc::now(),
                    confidence: response.confidence,
                })
                .collect();

            let next_urls: Vec<QueuedUrl> = response
                .next_urls
                .into_iter()
                .filter_map(|nu| {
                    let url = Url::parse(&nu.url).ok()?;
                    let priority = match nu.priority.as_str() {
                        "high" => Priority::High,
                        "low" => Priority::Low,
                        _ => Priority::Normal,
                    };
                    Some(QueuedUrl::new(url, depth + 1).with_priority(priority))
                })
                .collect();

            // Best-effort cleanup — ignore errors.
            let _ = tokio::fs::remove_dir_all(&task_dir).await;

            info!(
                task_id = %task_id,
                records = records.len(),
                next_urls = next_urls.len(),
                "Agent completed"
            );

            Ok(AgentResult { records, next_urls })
        })
    }
}
