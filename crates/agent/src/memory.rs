use crate::protocol::SiteMemorySnapshot;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread-safe per-domain memory wrapper.
#[derive(Clone)]
pub struct DomainMemory {
    inner: Arc<RwLock<SiteMemorySnapshot>>,
}

impl DomainMemory {
    pub fn new(domain: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SiteMemorySnapshot {
                domain,
                ..Default::default()
            })),
        }
    }

    pub fn from_snapshot(snap: SiteMemorySnapshot) -> Self {
        Self {
            inner: Arc::new(RwLock::new(snap)),
        }
    }

    pub async fn snapshot(&self) -> SiteMemorySnapshot {
        self.inner.read().await.clone()
    }

    pub async fn apply_update(&self, updated: SiteMemorySnapshot) {
        *self.inner.write().await = updated;
    }

    pub async fn increment_pages(&self) {
        self.inner.write().await.pages_seen += 1;
    }

    pub async fn add_records(&self, count: usize) {
        self.inner.write().await.records_found += count;
    }
}
