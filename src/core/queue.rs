use crate::core::{
    error::CrawlError,
    traits::UrlQueue,
    types::{QueuedUrl, UrlHash},
};
use ahash::RandomState;
use futures::future::BoxFuture;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::hash::{BuildHasher, Hasher};
use tokio::sync::Mutex;
use url::Url;

// ---- Priority-ordered heap entry ----

struct HeapEntry(QueuedUrl);

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.priority == other.0.priority && self.0.queued_at == other.0.queued_at
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first; break ties by earlier queued_at (FIFO within same priority).
        match self.0.priority.cmp(&other.0.priority) {
            Ordering::Equal => other.0.queued_at.cmp(&self.0.queued_at),
            ord => ord,
        }
    }
}

// ---- URL hashing ----

fn hash_url(url: &Url, hasher_state: &RandomState) -> u64 {
    let mut h = hasher_state.build_hasher();
    h.write(url.as_str().as_bytes());
    h.finish()
}

// ---- InMemoryQueue ----

/// Thread-safe in-memory priority queue with URL deduplication.
pub struct InMemoryQueue {
    inner: Mutex<QueueInner>,
    hasher: RandomState,
}

struct QueueInner {
    heap: BinaryHeap<HeapEntry>,
    visited: HashSet<u64>,
    queued: HashSet<u64>,
}

impl InMemoryQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                heap: BinaryHeap::new(),
                visited: HashSet::new(),
                queued: HashSet::new(),
            }),
            hasher: RandomState::new(),
        }
    }

    async fn push_inner(&self, url: QueuedUrl) -> Result<(), CrawlError> {
        let h = hash_url(&url.url, &self.hasher);
        let mut inner = self.inner.lock().await;
        if inner.visited.contains(&h) || inner.queued.contains(&h) {
            return Ok(());
        }
        inner.queued.insert(h);
        inner.heap.push(HeapEntry(url));
        Ok(())
    }

    async fn push_batch_inner(&self, urls: Vec<QueuedUrl>) -> Result<(), CrawlError> {
        let mut inner = self.inner.lock().await;
        for url in urls {
            let h = hash_url(&url.url, &self.hasher);
            if inner.visited.contains(&h) || inner.queued.contains(&h) {
                continue;
            }
            inner.queued.insert(h);
            inner.heap.push(HeapEntry(url));
        }
        Ok(())
    }

    async fn pop_inner(&self) -> Result<Option<QueuedUrl>, CrawlError> {
        let mut inner = self.inner.lock().await;
        match inner.heap.pop() {
            Some(entry) => {
                let h = hash_url(&entry.0.url, &self.hasher);
                inner.queued.remove(&h);
                Ok(Some(entry.0))
            }
            None => Ok(None),
        }
    }

    async fn is_empty_inner(&self) -> bool {
        self.inner.lock().await.heap.is_empty()
    }

    async fn size_inner(&self) -> usize {
        self.inner.lock().await.heap.len()
    }

    async fn is_visited_inner(&self, url: &Url) -> bool {
        let h = hash_url(url, &self.hasher);
        self.inner.lock().await.visited.contains(&h)
    }

    async fn mark_visited_inner(&self, url: Url) -> Result<(), CrawlError> {
        let h = hash_url(&url, &self.hasher);
        let mut inner = self.inner.lock().await;
        inner.visited.insert(h);
        inner.queued.remove(&h);
        Ok(())
    }
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlQueue for InMemoryQueue {
    fn push<'a>(&'a self, url: QueuedUrl) -> BoxFuture<'a, Result<(), CrawlError>> {
        Box::pin(self.push_inner(url))
    }

    fn push_batch<'a>(&'a self, urls: Vec<QueuedUrl>) -> BoxFuture<'a, Result<(), CrawlError>> {
        Box::pin(self.push_batch_inner(urls))
    }

    fn pop<'a>(&'a self) -> BoxFuture<'a, Result<Option<QueuedUrl>, CrawlError>> {
        Box::pin(self.pop_inner())
    }

    fn is_empty<'a>(&'a self) -> BoxFuture<'a, bool> {
        Box::pin(self.is_empty_inner())
    }

    fn size<'a>(&'a self) -> BoxFuture<'a, usize> {
        Box::pin(self.size_inner())
    }

    fn is_visited<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool> {
        Box::pin(self.is_visited_inner(url))
    }

    fn mark_visited<'a>(&'a self, url: Url) -> BoxFuture<'a, Result<(), CrawlError>> {
        Box::pin(self.mark_visited_inner(url))
    }
}

impl From<&Url> for UrlHash {
    fn from(url: &Url) -> Self {
        let state = RandomState::new();
        let mut h = state.build_hasher();
        h.write(url.as_str().as_bytes());
        UrlHash(h.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Priority;

    fn make_url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let q = InMemoryQueue::new();
        q.push(QueuedUrl::new(make_url("https://example.com/low"), 0).with_priority(Priority::Low))
            .await
            .unwrap();
        q.push(
            QueuedUrl::new(make_url("https://example.com/high"), 0).with_priority(Priority::High),
        )
        .await
        .unwrap();
        q.push(QueuedUrl::new(make_url("https://example.com/normal"), 0))
            .await
            .unwrap();

        let first = q.pop().await.unwrap().unwrap();
        assert_eq!(first.priority, Priority::High);
        let second = q.pop().await.unwrap().unwrap();
        assert_eq!(second.priority, Priority::Normal);
        let third = q.pop().await.unwrap().unwrap();
        assert_eq!(third.priority, Priority::Low);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let q = InMemoryQueue::new();
        let url = make_url("https://example.com/page");
        q.push(QueuedUrl::new(url.clone(), 0)).await.unwrap();
        q.push(QueuedUrl::new(url.clone(), 0)).await.unwrap();
        assert_eq!(q.size().await, 1);

        q.mark_visited(url.clone()).await.unwrap();
        q.push(QueuedUrl::new(url.clone(), 0)).await.unwrap();
        assert_eq!(q.size().await, 0);
    }
}
