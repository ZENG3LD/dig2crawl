use crate::{
    error::{AgentError, CrawlError, StorageError},
    types::{AgentGoal, CrawlStats, ExtractedRecord, FetchedPage, PageMeta, QueuedUrl},
};
use futures::future::BoxFuture;
use url::Url;
use uuid::Uuid;

/// Fetches a single URL. Implementations: HttpFetcher, BrowserFetcher.
///
/// Uses `BoxFuture` for dyn compatibility (`Arc<dyn Fetcher>`).
pub trait Fetcher: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, CrawlError>>;
}

/// Priority URL queue with deduplication.
pub trait UrlQueue: Send + Sync {
    fn push<'a>(&'a self, url: QueuedUrl) -> BoxFuture<'a, Result<(), CrawlError>>;
    fn push_batch<'a>(&'a self, urls: Vec<QueuedUrl>) -> BoxFuture<'a, Result<(), CrawlError>>;
    fn pop<'a>(&'a self) -> BoxFuture<'a, Result<Option<QueuedUrl>, CrawlError>>;
    fn is_empty<'a>(&'a self) -> BoxFuture<'a, bool>;
    fn size<'a>(&'a self) -> BoxFuture<'a, usize>;
    fn is_visited<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, bool>;
    fn mark_visited<'a>(&'a self, url: Url) -> BoxFuture<'a, Result<(), CrawlError>>;
}

/// Extracts outbound links from a fetched page.
pub trait LinkExtractor: Send + Sync {
    fn extract_links(&self, page: &FetchedPage) -> Vec<Url>;
    fn extract_meta(&self, page: &FetchedPage) -> PageMeta;
}

/// Persists crawl results.
pub trait Storage: Send + Sync {
    fn save_record<'a>(
        &'a self,
        record: &'a ExtractedRecord,
    ) -> BoxFuture<'a, Result<(), StorageError>>;

    fn save_stats<'a>(&'a self, stats: &'a CrawlStats) -> BoxFuture<'a, Result<(), StorageError>>;

    fn mark_visited<'a>(
        &'a self,
        job_id: Uuid,
        url: &'a Url,
        status: Option<u16>,
    ) -> BoxFuture<'a, Result<(), StorageError>>;

    fn is_visited<'a>(
        &'a self,
        job_id: Uuid,
        url: &'a Url,
    ) -> BoxFuture<'a, Result<bool, StorageError>>;
}

/// Controls request rate per domain.
pub trait RateLimiter: Send + Sync {
    fn acquire<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, ()>;
    fn report_rate_limit(&self, domain: &str);
}

/// Result produced by an agent processing a single page.
pub struct AgentResult {
    pub records: Vec<ExtractedRecord>,
    pub next_urls: Vec<QueuedUrl>,
}

/// AI agent that processes a fetched page and extracts records.
pub trait CrawlAgent: Send + Sync {
    fn process_page<'a>(
        &'a self,
        page: &'a FetchedPage,
        goal: &'a AgentGoal,
        job_id: Uuid,
        depth: usize,
    ) -> BoxFuture<'a, Result<AgentResult, AgentError>>;
}
