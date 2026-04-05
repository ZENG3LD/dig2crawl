use chrono::Utc;
use crate::core::error::CrawlError;
use crate::core::traits::Fetcher;
use crate::core::types::{FetchMethod, FetchedPage};
use crate::fetch::retry::RetryConfig;
use dig2browser::bot_auth::RequestSigner;
use dig2browser::{BrowserPool, PoolConfig, StealthConfig};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Instant;
use url::Url;

/// Headless-browser fetcher backed by a `dig2browser::BrowserPool`.
///
/// Renders pages with stealth Chrome, making it suitable for JavaScript-heavy
/// sites or pages that block plain HTTP clients.
///
/// Optionally retries failed navigations with exponential backoff when a
/// [`RetryConfig`] is supplied.
///
/// Optionally injects Web Bot Auth headers via a [`RequestSigner`].
pub struct BrowserFetcher {
    pool: BrowserPool,
    /// Optional CSS selector to wait for before capturing HTML.
    ///
    /// When set, the fetcher calls `page.wait().for_element(selector)` after
    /// navigation. This ensures dynamic content has rendered before the HTML
    /// is captured.
    wait_selector: Option<String>,
    /// Optional retry configuration.
    retry: Option<RetryConfig>,
    /// Optional Web Bot Auth signer.
    signer: Option<Arc<RequestSigner>>,
}

impl BrowserFetcher {
    /// Launch a pool of `pool_size` stealth browser instances.
    ///
    /// Pass `wait_selector = Some("…")` to wait for a CSS selector on every
    /// page before capturing HTML (useful for SPAs).
    ///
    /// Pass `signer = Some(arc)` to inject Web Bot Auth headers before each navigation.
    pub async fn new(
        pool_size: usize,
        stealth: StealthConfig,
        wait_selector: Option<String>,
        signer: Option<Arc<RequestSigner>>,
    ) -> Result<Self, CrawlError> {
        let config = PoolConfig {
            size: pool_size,
            stealth,
            ..PoolConfig::default()
        };
        Self::with_config(config, wait_selector, signer).await
    }

    /// Launch a browser pool with a fully specified [`PoolConfig`].
    ///
    /// Use this when you need control over `launch.headless`, `launch.profile`,
    /// or other low-level settings that `new()` does not expose.
    pub async fn with_config(
        config: PoolConfig,
        wait_selector: Option<String>,
        signer: Option<Arc<RequestSigner>>,
    ) -> Result<Self, CrawlError> {
        let pool = BrowserPool::new(config)
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser pool init: {e}")))?;
        Ok(Self {
            pool,
            wait_selector,
            retry: None,
            signer,
        })
    }

    /// Enable exponential-backoff retry for failed browser navigations.
    pub fn with_retry(mut self, config: RetryConfig) -> Self {
        self.retry = Some(config);
        self
    }

    /// Shut down all browser instances in the pool.
    pub async fn shutdown(self) -> Result<(), CrawlError> {
        self.pool
            .shutdown()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser pool shutdown: {e}")))
    }

    /// Perform a single browser navigation without retry.
    async fn navigate_once(&self, url: &Url) -> Result<FetchedPage, CrawlError> {
        let start = Instant::now();

        let pool_page = self
            .pool
            .acquire()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser acquire: {e}")))?;

        if let Some(signer) = &self.signer {
            if let Ok(headers) = signer.sign_request("GET", url.as_str()) {
                let mut extra_headers = std::collections::HashMap::new();
                extra_headers.insert("Signature-Agent".to_string(), headers.signature_agent);
                extra_headers.insert("Signature-Input".to_string(), headers.signature_input);
                extra_headers.insert("Signature".to_string(), headers.signature);
                pool_page
                    .page()
                    .set_extra_http_headers(extra_headers)
                    .await
                    .map_err(|e| CrawlError::Fetch(format!("browser set headers: {e}")))?;
            }
        }

        pool_page
            .page()
            .goto(url.as_str())
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser goto: {e}")))?;

        if let Some(selector) = &self.wait_selector {
            pool_page
                .page()
                .wait()
                .for_element(selector.as_str())
                .await
                .map_err(|e| CrawlError::Fetch(format!("browser wait for element: {e}")))?;
        }

        let body = pool_page
            .page()
            .html()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser html: {e}")))?;

        let screenshot = pool_page
            .page()
            .screenshot()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser screenshot: {e}")))?;

        Ok(FetchedPage {
            url: url.clone(),
            status_code: Some(200),
            body,
            fetched_at: Utc::now(),
            fetch_ms: start.elapsed().as_millis() as u64,
            method: FetchMethod::Browser {
                wait_selector: self.wait_selector.clone(),
            },
            screenshot: Some(screenshot),
        })
    }
}

impl Fetcher for BrowserFetcher {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, CrawlError>> {
        Box::pin(async move {
            if let Some(retry_cfg) = &self.retry {
                use crate::fetch::retry::retry_with_backoff;
                retry_with_backoff(retry_cfg, |_attempt| async move {
                    self.navigate_once(url).await
                })
                .await
            } else {
                self.navigate_once(url).await
            }
        })
    }
}
