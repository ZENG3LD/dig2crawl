//! Outbound link extractor.
//!
//! [`LinkExtractor`] collects all `<a href>` links from an HTML page,
//! resolves relative URLs against the page's base URL, strips fragments,
//! and optionally filters by domain or URL pattern.

use scraper::{Html, Selector};
use url::Url;

/// Options for link filtering.
#[derive(Debug, Clone, Default)]
pub struct LinkFilter {
    /// When `Some`, only keep links whose host matches one of these domains
    /// (exact match or sub-domain, e.g. `"example.com"` matches
    /// `"sub.example.com"`).
    pub allowed_domains: Option<Vec<String>>,
    /// When `Some`, only keep links whose full URL contains at least one of
    /// these substrings.
    pub url_patterns: Option<Vec<String>>,
    /// When `true`, strip `#fragment` from extracted URLs.
    pub strip_fragments: bool,
    /// When `true`, skip `mailto:`, `tel:`, `javascript:`, and similar
    /// non-HTTP links (default: `true`).
    pub http_only: bool,
}

impl LinkFilter {
    /// Create a filter that keeps all HTTP/HTTPS links.
    pub fn http_only() -> Self {
        Self {
            http_only: true,
            ..Default::default()
        }
    }
}

/// Stateless link extractor.
pub struct LinkExtractor;

impl LinkExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Extract all links from `html`, resolve them against `base_url`, and
    /// apply `filter`.
    ///
    /// The returned `Vec` is deduplicated and in document order.
    pub fn extract(&self, html: &str, base_url: &Url, filter: &LinkFilter) -> Vec<Url> {
        let document = Html::parse_document(html);

        // Respect <base href="…"> if present.
        let effective_base = Self::resolve_base_href(&document, base_url);

        let a_sel = Selector::parse("a[href]").expect("static selector");
        let mut seen = std::collections::HashSet::new();
        let mut links = Vec::new();

        for el in document.select(&a_sel) {
            let href = match el.value().attr("href") {
                Some(h) => h,
                None => continue,
            };

            // Resolve relative URL.
            let mut url = match effective_base.join(href) {
                Ok(u) => u,
                Err(_) => continue,
            };

            // Strip fragment if requested.
            if filter.strip_fragments {
                url.set_fragment(None);
            }

            // HTTP-only filter.
            if filter.http_only && url.scheme() != "http" && url.scheme() != "https" {
                continue;
            }

            // Domain filter.
            if let Some(ref domains) = filter.allowed_domains {
                let host = url.host_str().unwrap_or("");
                if !domains.iter().any(|d| is_same_or_subdomain(host, d)) {
                    continue;
                }
            }

            // Pattern filter.
            if let Some(ref patterns) = filter.url_patterns {
                let url_str = url.as_str();
                if !patterns.iter().any(|p| url_str.contains(p.as_str())) {
                    continue;
                }
            }

            // Deduplicate.
            if seen.insert(url.as_str().to_owned()) {
                links.push(url);
            }
        }

        links
    }

    /// Extract all links, returning only those on the same domain as
    /// `base_url`.
    pub fn extract_internal(&self, html: &str, base_url: &Url) -> Vec<Url> {
        let domain = base_url.host_str().unwrap_or("").to_owned();
        let filter = LinkFilter {
            allowed_domains: Some(vec![domain]),
            strip_fragments: true,
            http_only: true,
            ..Default::default()
        };
        self.extract(html, base_url, &filter)
    }

    // ------------------------------------------------------------------ //
    // Private helpers
    // ------------------------------------------------------------------ //

    fn resolve_base_href(document: &Html, page_url: &Url) -> Url {
        let sel = Selector::parse("base[href]").expect("static selector");
        if let Some(el) = document.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                if let Ok(base) = page_url.join(href) {
                    return base;
                }
            }
        }
        page_url.clone()
    }
}

impl Default for LinkExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if `host` is equal to `domain` or is a subdomain of it.
fn is_same_or_subdomain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{}", domain))
}

// ------------------------------------------------------------------ //
// Tests
// ------------------------------------------------------------------ //

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/catalog/").unwrap()
    }

    #[test]
    fn test_relative_links_resolved() {
        let html = r#"<html><body>
            <a href="/page/1/">Page 1</a>
            <a href="../other/">Other</a>
        </body></html>"#;

        let links = LinkExtractor::new().extract(html, &base(), &LinkFilter::http_only());
        assert!(links.iter().any(|u| u.as_str() == "https://example.com/page/1/"));
        assert!(links.iter().any(|u| u.as_str() == "https://example.com/other/"));
    }

    #[test]
    fn test_absolute_links_kept() {
        let html = r#"<html><body><a href="https://example.com/a/">A</a></body></html>"#;
        let links = LinkExtractor::new().extract(html, &base(), &LinkFilter::http_only());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "https://example.com/a/");
    }

    #[test]
    fn test_non_http_filtered_when_http_only() {
        let html = r#"<html><body>
            <a href="mailto:foo@bar.com">Email</a>
            <a href="tel:+1234">Call</a>
            <a href="javascript:void(0)">JS</a>
            <a href="https://example.com/real/">Real</a>
        </body></html>"#;

        let links = LinkExtractor::new().extract(html, &base(), &LinkFilter::http_only());
        assert_eq!(links.len(), 1);
        assert!(links[0].as_str().starts_with("https://"));
    }

    #[test]
    fn test_domain_filter() {
        let html = r#"<html><body>
            <a href="https://example.com/a/">Internal</a>
            <a href="https://sub.example.com/b/">Subdomain</a>
            <a href="https://other.com/c/">External</a>
        </body></html>"#;

        let filter = LinkFilter {
            allowed_domains: Some(vec!["example.com".to_owned()]),
            http_only: true,
            ..Default::default()
        };
        let links = LinkExtractor::new().extract(html, &base(), &filter);
        // Internal + subdomain kept; external excluded.
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|u| u.host_str().unwrap_or("").ends_with("example.com")));
    }

    #[test]
    fn test_fragment_stripped() {
        let html = r#"<html><body><a href="/page/#section">Link</a></body></html>"#;
        let filter = LinkFilter {
            strip_fragments: true,
            http_only: true,
            ..Default::default()
        };
        let links = LinkExtractor::new().extract(html, &base(), &filter);
        assert_eq!(links.len(), 1);
        assert!(!links[0].as_str().contains('#'));
    }

    #[test]
    fn test_deduplication() {
        let html = r#"<html><body>
            <a href="/page/">Link</a>
            <a href="/page/">Duplicate</a>
        </body></html>"#;

        let links = LinkExtractor::new().extract(html, &base(), &LinkFilter::http_only());
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn test_base_href_respected() {
        let html = r#"<html><head>
            <base href="https://cdn.example.com/">
        </head><body>
            <a href="assets/page.html">Asset link</a>
        </body></html>"#;

        let links = LinkExtractor::new().extract(html, &base(), &LinkFilter::http_only());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "https://cdn.example.com/assets/page.html");
    }

    #[test]
    fn test_pattern_filter() {
        let html = r#"<html><body>
            <a href="/catalog/item/123/">Item</a>
            <a href="/about/">About</a>
            <a href="/catalog/item/456/">Item 2</a>
        </body></html>"#;

        let filter = LinkFilter {
            url_patterns: Some(vec!["/catalog/item/".to_owned()]),
            http_only: true,
            ..Default::default()
        };
        let links = LinkExtractor::new().extract(html, &base(), &filter);
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|u| u.as_str().contains("/catalog/item/")));
    }

    #[test]
    fn test_extract_internal_only() {
        let html = r#"<html><body>
            <a href="/internal/">Internal</a>
            <a href="https://external.com/">External</a>
        </body></html>"#;

        let links = LinkExtractor::new().extract_internal(html, &base());
        assert_eq!(links.len(), 1);
        assert!(links[0].as_str().contains("example.com"));
    }
}
