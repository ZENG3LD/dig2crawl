use chrono::Utc;
use crawl_core::error::CrawlError;
use crawl_core::traits::Fetcher;
use crawl_core::types::{FetchMethod, FetchedPage};
use futures::future::BoxFuture;
use reqwest::Client;
use std::time::Instant;
use url::Url;

/// HTTP fetcher backed by `reqwest::Client`.
///
/// Performs a plain GET request and returns the response body as a string.
/// Suitable for static pages that do not require JavaScript rendering.
pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    /// Build a new `HttpFetcher` with the given `User-Agent` string and a
    /// 30-second request timeout.
    pub fn new(user_agent: &str) -> Result<Self, CrawlError> {
        let client = Client::builder()
            .user_agent(user_agent)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| CrawlError::Fetch(e.to_string()))?;
        Ok(Self { client })
    }

    /// Build an `HttpFetcher` from an existing `reqwest::Client`.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }
}

impl Fetcher for HttpFetcher {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, CrawlError>> {
        Box::pin(async move {
            let start = Instant::now();
            let resp = self
                .client
                .get(url.as_str())
                .send()
                .await
                .map_err(|e| CrawlError::Fetch(format!("{url}: {e}")))?;
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
        })
    }
}
