use crate::core::{
    traits::LinkExtractor,
    types::{FetchedPage, PageMeta},
};
use scraper::{Html, Selector};
use url::Url;

/// HTML link extractor using the `scraper` crate.
pub struct HtmlLinkExtractor {
    allowed_domains: Vec<String>,
}

impl HtmlLinkExtractor {
    pub fn new(allowed_domains: Vec<String>) -> Self {
        Self { allowed_domains }
    }

    fn is_allowed(&self, url: &Url) -> bool {
        if self.allowed_domains.is_empty() {
            return true;
        }
        let host = url.host_str().unwrap_or("");
        self.allowed_domains
            .iter()
            .any(|d| host == d.as_str() || host.ends_with(&format!(".{d}")))
    }
}

impl LinkExtractor for HtmlLinkExtractor {
    fn extract_links(&self, page: &FetchedPage) -> Vec<Url> {
        let doc = Html::parse_document(&page.body);
        let sel = match Selector::parse("a[href]") {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let mut links = Vec::new();
        for element in doc.select(&sel) {
            let href = match element.value().attr("href") {
                Some(h) => h,
                None => continue,
            };

            // Skip anchors, javascript: and mailto:
            if href.starts_with('#')
                || href.starts_with("javascript:")
                || href.starts_with("mailto:")
            {
                continue;
            }

            let resolved = if href.starts_with("http://") || href.starts_with("https://") {
                Url::parse(href).ok()
            } else {
                page.url.join(href).ok()
            };

            if let Some(url) = resolved {
                if self.is_allowed(&url) {
                    links.push(url);
                }
            }
        }

        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        links.retain(|u| seen.insert(u.as_str().to_owned()));
        links
    }

    fn extract_meta(&self, page: &FetchedPage) -> PageMeta {
        let doc = Html::parse_document(&page.body);

        let title = extract_text(&doc, "title");

        let description = extract_meta_content(&doc, "name", "description")
            .or_else(|| extract_meta_content(&doc, "property", "og:description"));

        let canonical_url = extract_canonical(&doc, &page.url);

        let language = extract_lang(&doc);

        PageMeta {
            title,
            description,
            canonical_url,
            language,
        }
    }
}

fn extract_text(doc: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    let text: String = doc
        .select(&sel)
        .next()?
        .text()
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_owned();
    if text.is_empty() { None } else { Some(text) }
}

fn extract_meta_content(doc: &Html, attr: &str, value: &str) -> Option<String> {
    let selector_str = format!("meta[{}=\"{}\"]", attr, value);
    let sel = Selector::parse(&selector_str).ok()?;
    let content = doc.select(&sel).next()?.value().attr("content")?.trim().to_owned();
    if content.is_empty() { None } else { Some(content) }
}

fn extract_canonical(doc: &Html, base: &Url) -> Option<Url> {
    let sel = Selector::parse("link[rel=\"canonical\"]").ok()?;
    let href = doc.select(&sel).next()?.value().attr("href")?;
    base.join(href).ok()
}

fn extract_lang(doc: &Html) -> Option<String> {
    let sel = Selector::parse("html[lang]").ok()?;
    let lang = doc.select(&sel).next()?.value().attr("lang")?.trim().to_owned();
    if lang.is_empty() { None } else { Some(lang) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::FetchMethod;
    use chrono::Utc;

    fn make_page(url: &str, html: &str) -> FetchedPage {
        FetchedPage {
            url: Url::parse(url).unwrap(),
            status_code: Some(200),
            body: html.to_owned(),
            fetched_at: Utc::now(),
            fetch_ms: 0,
            method: FetchMethod::Http,
            screenshot: None,
        }
    }

    #[test]
    fn test_extract_links_relative() {
        let extractor = HtmlLinkExtractor::new(vec!["example.com".to_owned()]);
        let page = make_page(
            "https://example.com/page",
            r#"<html><body><a href="/about">About</a><a href="https://other.com">Other</a></body></html>"#,
        );
        let links = extractor.extract_links(&page);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "https://example.com/about");
    }

    #[test]
    fn test_extract_meta() {
        let extractor = HtmlLinkExtractor::new(vec![]);
        let page = make_page(
            "https://example.com/",
            r#"<html lang="en"><head><title>Hello World</title><meta name="description" content="Test desc"></head></html>"#,
        );
        let meta = extractor.extract_meta(&page);
        assert_eq!(meta.title.as_deref(), Some("Hello World"));
        assert_eq!(meta.description.as_deref(), Some("Test desc"));
        assert_eq!(meta.language.as_deref(), Some("en"));
    }
}
