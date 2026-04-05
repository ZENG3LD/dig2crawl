use chrono::Utc;
use crate::core::error::CrawlError;
use crate::core::traits::Fetcher;
use crate::core::types::{FetchMethod, FetchedPage};
use crate::fetch::retry::RetryConfig;
use dig2browser::bot_auth::RequestSigner;
use dig2browser::{LaunchConfig, StealthBrowser, StealthConfig};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Instant;
use url::Url;

/// Browser fetcher backed by a `dig2browser::StealthBrowser`.
///
/// Uses `StealthBrowser` directly (not `BrowserPool`) — same pattern as
/// `daemon4russian-parser`'s Yandex Maps enrichment daemon.
///
/// Optionally retries failed navigations with exponential backoff when a
/// [`RetryConfig`] is supplied.
///
/// Optionally injects Web Bot Auth headers via a [`RequestSigner`].
pub struct BrowserFetcher {
    browser: StealthBrowser,
    /// Optional CSS selector to wait for before capturing HTML.
    wait_selector: Option<String>,
    /// Optional retry configuration.
    retry: Option<RetryConfig>,
    /// Optional Web Bot Auth signer.
    signer: Option<Arc<RequestSigner>>,
}

impl BrowserFetcher {
    /// Launch a stealth browser with the given config.
    pub async fn new(
        launch: LaunchConfig,
        stealth: StealthConfig,
        wait_selector: Option<String>,
        signer: Option<Arc<RequestSigner>>,
    ) -> Result<Self, CrawlError> {
        let browser = StealthBrowser::launch_with(launch, stealth)
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser launch: {e}")))?;
        Ok(Self {
            browser,
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

    /// Shut down the browser.
    pub async fn shutdown(self) -> Result<(), CrawlError> {
        self.browser
            .close()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser shutdown: {e}")))
    }

    /// Perform a single browser navigation without retry.
    async fn navigate_once(&self, url: &Url) -> Result<FetchedPage, CrawlError> {
        let start = Instant::now();

        let page = self
            .browser
            .new_blank_page()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser new page: {e}")))?;

        if let Some(signer) = &self.signer {
            if let Ok(headers) = signer.sign_request("GET", url.as_str()) {
                let mut extra_headers = std::collections::HashMap::new();
                extra_headers.insert("Signature-Agent".to_string(), headers.signature_agent);
                extra_headers.insert("Signature-Input".to_string(), headers.signature_input);
                extra_headers.insert("Signature".to_string(), headers.signature);
                page.set_extra_http_headers(extra_headers)
                    .await
                    .map_err(|e| CrawlError::Fetch(format!("browser set headers: {e}")))?;
            }
        }

        page.goto(url.as_str())
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser goto: {e}")))?;

        if let Some(selector) = &self.wait_selector {
            page.wait()
                .for_element(selector.as_str())
                .await
                .map_err(|e| CrawlError::Fetch(format!("browser wait for element: {e}")))?;
        }

        let body = page
            .html()
            .await
            .map_err(|e| CrawlError::Fetch(format!("browser html: {e}")))?;

        let screenshot = page
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
