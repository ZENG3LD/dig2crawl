use crate::core::{
    budget::CrawlBudget,
    error::CrawlError,
    traits::{CrawlAgent, Fetcher, LinkExtractor, RateLimiter, Storage, UrlQueue},
    types::{CrawlConfig, CrawlJob, CrawlStats, QueuedUrl},
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing::{info, warn};

pub struct CrawlEngine {
    fetcher: Arc<dyn Fetcher>,
    queue: Arc<dyn UrlQueue>,
    storage: Arc<dyn Storage>,
    link_extractor: Arc<dyn LinkExtractor>,
    agent: Option<Arc<dyn CrawlAgent>>,
    rate_limiter: Arc<dyn RateLimiter>,
    budget: CrawlBudget,
    config: CrawlConfig,
}

pub struct CrawlEngineBuilder {
    fetcher: Option<Arc<dyn Fetcher>>,
    queue: Option<Arc<dyn UrlQueue>>,
    storage: Option<Arc<dyn Storage>>,
    link_extractor: Option<Arc<dyn LinkExtractor>>,
    agent: Option<Arc<dyn CrawlAgent>>,
    rate_limiter: Option<Arc<dyn RateLimiter>>,
    config: Option<CrawlConfig>,
}

impl CrawlEngineBuilder {
    pub fn new() -> Self {
        Self {
            fetcher: None,
            queue: None,
            storage: None,
            link_extractor: None,
            agent: None,
            rate_limiter: None,
            config: None,
        }
    }

    pub fn fetcher(mut self, f: Arc<dyn Fetcher>) -> Self {
        self.fetcher = Some(f);
        self
    }

    pub fn queue(mut self, q: Arc<dyn UrlQueue>) -> Self {
        self.queue = Some(q);
        self
    }

    pub fn storage(mut self, s: Arc<dyn Storage>) -> Self {
        self.storage = Some(s);
        self
    }

    pub fn link_extractor(mut self, l: Arc<dyn LinkExtractor>) -> Self {
        self.link_extractor = Some(l);
        self
    }

    pub fn agent(mut self, a: Arc<dyn CrawlAgent>) -> Self {
        self.agent = Some(a);
        self
    }

    pub fn rate_limiter(mut self, r: Arc<dyn RateLimiter>) -> Self {
        self.rate_limiter = Some(r);
        self
    }

    pub fn config(mut self, c: CrawlConfig) -> Self {
        self.config = Some(c);
        self
    }

    pub fn build(self) -> Result<CrawlEngine, CrawlError> {
        let config = self
            .config
            .ok_or_else(|| CrawlError::Config("config required".into()))?;
        let budget = CrawlBudget::new(config.max_pages, config.max_depth);
        Ok(CrawlEngine {
            fetcher: self
                .fetcher
                .ok_or_else(|| CrawlError::Config("fetcher required".into()))?,
            queue: self
                .queue
                .ok_or_else(|| CrawlError::Config("queue required".into()))?,
            storage: self
                .storage
                .ok_or_else(|| CrawlError::Config("storage required".into()))?,
            link_extractor: self
                .link_extractor
                .ok_or_else(|| CrawlError::Config("link_extractor required".into()))?,
            agent: self.agent,
            rate_limiter: self
                .rate_limiter
                .ok_or_else(|| CrawlError::Config("rate_limiter required".into()))?,
            budget,
            config,
        })
    }
}

impl Default for CrawlEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CrawlEngine {
    pub fn builder() -> CrawlEngineBuilder {
        CrawlEngineBuilder::new()
    }

    pub async fn run(&self, job: CrawlJob) -> Result<CrawlStats, CrawlError> {
        let start = Instant::now();
        let pages_fetched = Arc::new(AtomicU64::new(0));
        let records_extracted = Arc::new(AtomicU64::new(0));
        let errors = Arc::new(AtomicU64::new(0));
        let concurrency = self.config.rate.concurrent_requests.max(1);
        let semaphore = Arc::new(Semaphore::new(concurrency));

        info!(job_id = %job.id, name = %self.config.name, "Starting crawl");

        // Seed the queue with start URLs
        for url in &self.config.start_urls {
            self.queue.push(QueuedUrl::new(url.clone(), 0)).await?;
        }

        // Main crawl loop
        loop {
            // Check budget
            if self.budget.is_exhausted() {
                info!("Budget exhausted");
                break;
            }

            // Pop next URL
            let queued = match self.queue.pop().await? {
                Some(q) => q,
                None => {
                    // Queue empty — wait a bit for in-flight tasks, then check again
                    if semaphore.available_permits() == concurrency {
                        // All permits free = nothing in flight, we're done
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
            };

            // Check depth budget
            if !self.budget.can_fetch(queued.depth) {
                continue;
            }

            // Skip already visited
            if self.queue.is_visited(&queued.url).await {
                continue;
            }

            // Acquire rate limit
            let domain = queued.url.host_str().unwrap_or("unknown");
            self.rate_limiter.acquire(domain).await;

            // Acquire concurrency permit
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| CrawlError::Cancelled)?;

            // Clone what we need for the task
            let fetcher = self.fetcher.clone();
            let queue = self.queue.clone();
            let storage = self.storage.clone();
            let link_extractor = self.link_extractor.clone();
            let agent = self.agent.clone();
            let goal = self.config.agent_goal.clone();
            let follow_links = self.config.follow_links;
            let job_id = job.id;
            let pages = pages_fetched.clone();
            let records = records_extracted.clone();
            let errs = errors.clone();
            let url = queued.url.clone();
            let depth = queued.depth;

            // Record budget usage
            self.budget.record_fetch();

            // Mark visited immediately to prevent duplicates
            queue.mark_visited(url.clone()).await?;

            tokio::spawn(async move {
                let _permit = permit; // held until task completes

                // Fetch
                let page = match fetcher.fetch(&url).await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(url = %url, error = %e, "Fetch failed");
                        errs.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                };

                pages.fetch_add(1, Ordering::Relaxed);

                // Record visit in storage
                let _ = storage.mark_visited(job_id, &url, page.status_code).await;

                // If agent is available and goals are set, use agent
                if let (Some(agent), Some(goal)) = (&agent, &goal) {
                    match agent.process_page(&page, goal, job_id, depth).await {
                        Ok(result) => {
                            let count = result.records.len() as u64;
                            for record in &result.records {
                                if let Err(e) = storage.save_record(record).await {
                                    warn!(error = %e, "Failed to save record");
                                }
                            }
                            records.fetch_add(count, Ordering::Relaxed);

                            // Enqueue next URLs from agent
                            for next in result.next_urls {
                                let _ = queue.push(next).await;
                            }
                        }
                        Err(e) => {
                            warn!(url = %url, error = %e, "Agent failed");
                            errs.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                // Extract and follow links if configured
                if follow_links {
                    let links = link_extractor.extract_links(&page);
                    for link in links {
                        let queued = QueuedUrl::new(link, depth + 1);
                        let _ = queue.push(queued).await;
                    }
                }
            });

            // Log stats periodically
            let elapsed = start.elapsed().as_secs();
            let pf = pages_fetched.load(Ordering::Relaxed);
            if pf % 10 == 0 && pf > 0 {
                info!(
                    pages = pf,
                    records = records_extracted.load(Ordering::Relaxed),
                    errors = errors.load(Ordering::Relaxed),
                    queue_size = self.queue.size().await,
                    elapsed_secs = elapsed,
                    "Crawl progress"
                );
            }
        }

        let stats = CrawlStats {
            job_id: job.id,
            pages_fetched: pages_fetched.load(Ordering::Relaxed),
            records_extracted: records_extracted.load(Ordering::Relaxed),
            errors: errors.load(Ordering::Relaxed),
            queue_size: self.queue.size().await,
            elapsed_secs: start.elapsed().as_secs(),
        };

        let _ = self.storage.save_stats(&stats).await;

        info!(
            job_id = %job.id,
            pages = stats.pages_fetched,
            records = stats.records_extracted,
            errors = stats.errors,
            elapsed = stats.elapsed_secs,
            "Crawl completed"
        );

        Ok(stats)
    }
}
