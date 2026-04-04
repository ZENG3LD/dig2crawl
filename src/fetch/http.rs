use chrono::Utc;
use crate::core::error::CrawlError;
use crate::core::traits::Fetcher;
use crate::core::types::{FetchMethod, FetchedPage};
use futures::future::BoxFuture;
use reqwest::Client;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

use crate::fetch::cache::ResponseCache;
use crate::fetch::proxy::{ProxyConfig, ProxyPool};
use crate::fetch::retry::RetryConfig;

/// Builder for [`HttpFetcher`].
///
/// Allows progressive configuration of retry, caching, and proxy options
/// before constructing the fetcher.
pub struct HttpFetcherBuilder {
    user_agent: String,
    timeout: Duration,
    retry: Option<RetryConfig>,
    cache: Option<ResponseCache>,
    proxy: Option<ProxyPool>,
}

impl HttpFetcherBuilder {
    /// Start building an `HttpFetcher` with the given `User-Agent` string.
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            user_agent: user_agent.into(),
            timeout: Duration::from_secs(30),
            retry: None,
            cache: None,
            proxy: None,
        }
    }

    /// Override the request timeout (default: 30 s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Enable retry with the given configuration.
    pub fn retry(mut self, config: RetryConfig) -> Self {
        self.retry = Some(config);
        self
    }

    /// Enable response caching with the given TTL and capacity.
    pub fn cache(mut self, ttl: Duration, max_size: usize) -> Self {
        self.cache = Some(ResponseCache::new(ttl, max_size));
        self
    }

    /// Enable proxy rotation from the given configuration.
    ///
    /// If `config.urls` is empty the proxy pool is silently omitted.
    pub fn proxy(mut self, config: ProxyConfig) -> Self {
        self.proxy = ProxyPool::from_config(config);
        self
    }

    /// Construct the `HttpFetcher`, building a `reqwest::Client` internally.
    pub fn build(self) -> Result<HttpFetcher, CrawlError> {
        let mut builder = Client::builder()
            .user_agent(&self.user_agent)
            .timeout(self.timeout);

        if let Some(ref pool) = self.proxy {
            if let Some(url) = pool.next() {
                let proxy = reqwest::Proxy::all(url).map_err(|e| {
                    CrawlError::Fetch(format!("invalid proxy URL {url}: {e}"))
                })?;
                builder = builder.proxy(proxy);
            }
        }

        let client = builder
            .build()
            .map_err(|e| CrawlError::Fetch(e.to_string()))?;

        Ok(HttpFetcher {
            client,
            retry: self.retry,
            cache: self.cache.map(|c| Arc::new(Mutex::new(c))),
            proxy: self.proxy,
        })
    }
}

/// HTTP fetcher backed by `reqwest::Client`.
///
/// Performs a plain GET request and returns the response body as a string.
/// Suitable for static pages that do not require JavaScript rendering.
///
/// Optionally supports:
/// - Exponential-backoff **retry** on transient errors (429, 5xx, network failures).
/// - Simple TTL-based **response cache** (no external deps).
/// - **Proxy rotation** via a `ProxyPool`.
pub struct HttpFetcher {
    client: Client,
    retry: Option<RetryConfig>,
    cache: Option<Arc<Mutex<ResponseCache>>>,
    proxy: Option<ProxyPool>,
}

impl HttpFetcher {
    /// Build a new `HttpFetcher` with the given `User-Agent` string and a
    /// 30-second request timeout.  No retry, cache, or proxy is configured.
    pub fn new(user_agent: &str) -> Result<Self, CrawlError> {
        HttpFetcherBuilder::new(user_agent).build()
    }

    /// Build an `HttpFetcher` from an existing `reqwest::Client`.
    ///
    /// No retry, cache, or proxy is configured.
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            retry: None,
            cache: None,
            proxy: None,
        }
    }

    /// Return a [`HttpFetcherBuilder`] for full configuration.
    pub fn builder(user_agent: impl Into<String>) -> HttpFetcherBuilder {
        HttpFetcherBuilder::new(user_agent)
    }

    /// Perform a single GET request without retry.
    async fn fetch_once(&self, url: &Url) -> Result<FetchedPage, CrawlError> {
        let start = Instant::now();

        // Build a per-request client when a proxy pool is active so we can
        // rotate the proxy on each call.
        let resp = if let Some(pool) = &self.proxy {
            if let Some(proxy_url) = pool.next() {
                let proxy = reqwest::Proxy::all(proxy_url).map_err(|e| {
                    CrawlError::Fetch(format!("invalid proxy URL {proxy_url}: {e}"))
                })?;
                // Build a temporary client with this proxy.
                let tmp = Client::builder()
                    .proxy(proxy)
                    .build()
                    .map_err(|e| CrawlError::Fetch(e.to_string()))?;
                tmp.get(url.as_str())
                    .send()
                    .await
                    .map_err(|e| CrawlError::Fetch(format!("{url}: {e}")))?
            } else {
                self.client
                    .get(url.as_str())
                    .send()
                    .await
                    .map_err(|e| CrawlError::Fetch(format!("{url}: {e}")))?
            }
        } else {
            self.client
                .get(url.as_str())
                .send()
                .await
                .map_err(|e| CrawlError::Fetch(format!("{url}: {e}")))?
        };

        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| CrawlError::Fetch(format!("{url}: body read: {e}")))?;

        Ok(FetchedPage {
            url: url.clone(),
            status_code: Some(status),
            body,
            fetched_at: Utc::now(),
            fetch_ms: start.elapsed().as_millis() as u64,
            method: FetchMethod::Http,
            screenshot: None,
        })
    }
}

impl Fetcher for HttpFetcher {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, CrawlError>> {
        Box::pin(async move {
            // 1. Cache lookup.
            if let Some(cache) = &self.cache {
                if let Ok(mut guard) = cache.lock() {
                    if let Some(cached) = guard.get(url.as_str()) {
                        tracing::debug!(url = %url, "cache hit");
                        return Ok(cached.page);
                    }
                }
            }

            // 2. Fetch (with optional retry).
            let page = if let Some(retry_cfg) = &self.retry {
                let retry_cfg = retry_cfg.clone();
                // retry_reqwest operates on the raw reqwest::Response so the
                // status is visible before the body is read.  We use a simpler
                // approach here — retry the full fetch_once which already maps
                // errors to CrawlError.
                let config = retry_cfg;
                retry_with_backoff_fetch(self, url, &config).await?
            } else {
                self.fetch_once(url).await?
            };

            // 3. Store successful (2xx) responses in the cache.
            if let Some(cache) = &self.cache {
                if page.status_code.map_or(false, |s| s < 400) {
                    if let Ok(mut guard) = cache.lock() {
                        guard.put(url.as_str(), page.clone());
                    }
                }
            }

            Ok(page)
        })
    }
}

/// Helper to call `HttpFetcher::fetch_once` through the retry machinery.
///
/// This is a free function rather than a method so that the borrow checker
/// is satisfied when we move `config` into the closure.
async fn retry_with_backoff_fetch(
    fetcher: &HttpFetcher,
    url: &Url,
    config: &RetryConfig,
) -> Result<FetchedPage, CrawlError> {
    use crate::fetch::retry::retry_with_backoff;

    retry_with_backoff(config, |_attempt| async move {
        fetcher.fetch_once(url).await.and_then(|page| {
            // Treat retryable HTTP statuses as errors so the retry loop
            // can handle them.
            if let Some(status) = page.status_code {
                if crate::fetch::retry::is_retryable_status(status) {
                    return Err(CrawlError::Fetch(format!(
                        "{url}: status {status}"
                    )));
                }
            }
            Ok(page)
        })
    })
    .await
}
