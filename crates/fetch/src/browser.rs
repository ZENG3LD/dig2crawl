use chrono::Utc;
use crawl_core::error::CrawlError;
use crawl_core::traits::Fetcher;
use crawl_core::types::{FetchMethod, FetchedPage};
use dig2browser::{BrowserPool, PoolConfig, StealthConfig};
use futures::future::BoxFuture;
use std::time::Instant;
use url::Url;

/// Headless-browser fetcher backed by a `dig2browser::BrowserPool`.
///
/// Renders pages with stealth Chrome, making it suitable for JavaScript-heavy
/// sites or pages that block plain HTTP clients.
pub struct BrowserFetcher {
    pool: BrowserPool,
}

impl BrowserFetcher {
    /// Launch a pool of `pool_size` stealth browser instances.
    pub async fn new(pool_size: usize, stealth: StealthConfig) -> Result<Self, CrawlError> {
        let config = PoolConfig {
            size: pool_size,
            stealth,
            ..PoolConfig::default()
        };
        let pool = BrowserPool::new(config)
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser pool init: {e}")))?;
        Ok(Self { pool })
    }

    /// Shut down all browser instances in the pool.
    pub async fn shutdown(self) -> Result<(), CrawlError> {
        self.pool
            .shutdown()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser pool shutdown: {e}")))
    }
}

impl Fetcher for BrowserFetcher {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, CrawlError>> {
        Box::pin(async move {
            let start = Instant::now();
            let page = self
                .pool
                .acquire(url.as_str())
                .await
                .map_err(|e| CrawlError::Fetch(format!("browser acquire: {e}")))?;
            let body = page
                .html()
                .await
                .map_err(|e| CrawlError::Fetch(format!("browser html: {e}")))?;
            Ok(FetchedPage {
                url: url.clone(),
                status_code: Some(200),
                body,
                fetched_at: Utc::now(),
                fetch_ms: start.elapsed().as_millis() as u64,
                method: FetchMethod::Browser {
                    wait_selector: None,
                },
                screenshot: None,
            })
        })
    }
}
