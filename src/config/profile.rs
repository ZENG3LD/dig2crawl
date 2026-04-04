//! Site profile — what the Claude agent discovers about a site's structure.
//!
//! A `SiteProfile` captures the CSS selectors, field extraction rules, and
//! pagination strategy that the agent learned for a specific domain. Profiles
//! are persisted as JSON so they can be reused on future crawl runs without
//! re-running discovery.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Error type for site profile I/O.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("failed to read profile file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to deserialize profile JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to serialize profile to TOML: {0}")]
    Toml(#[from] toml::ser::Error),
}

/// How to extract a value from a matched DOM element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    /// Inner text content of the element.
    Text,
    /// Value of a named HTML attribute (e.g. `href`, `src`, `data-price`).
    Attribute(String),
    /// Outer HTML of the element (including the element's own tag).
    Html,
    /// Inner HTML of the element (children only, excluding the element tag).
    InnerHtml,
}

/// Text transformation applied to an extracted string value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Transform {
    /// Strip leading and trailing whitespace.
    Trim,
    /// Apply a regex and return the specified capture group.
    Regex {
        pattern: String,
        /// Which capture group to return (0 = full match).
        group: usize,
    },
    /// Parse the string as a number, stripping non-numeric characters first.
    ParseNumber,
    /// Replace all occurrences of `from` with `to`.
    Replace { from: String, to: String },
    /// Remove all HTML tags, leaving plain text.
    StripHtml,
}

/// Configuration for extracting a single named field from a page element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConfig {
    /// Field name in the output record (e.g. `"title"`, `"price"`).
    pub name: String,
    /// CSS selector that locates the element relative to the container.
    pub selector: String,
    /// How to pull the raw string value out of the matched element.
    pub extract: ExtractMode,
    /// Optional prefix prepended to the extracted value (e.g. `"https://"`).
    pub prefix: Option<String>,
    /// Ordered list of transforms applied to the raw value, left to right.
    pub transform: Option<Vec<Transform>>,
}

/// Describes how to navigate to subsequent pages of results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PaginationConfig {
    /// Click a "next page" button/link selected by a CSS selector.
    NextButton {
        selector: String,
    },
    /// Enumerate pages via a URL template.
    ///
    /// The template must contain `{page}` which is replaced by the current
    /// page number.  Example: `"https://example.com/items?page={page}"`.
    UrlPattern {
        template: String,
        start: u32,
        end: Option<u32>,
        step: u32,
    },
    /// Scroll to the bottom repeatedly to trigger infinite scroll loading.
    InfiniteScroll {
        scroll_count: u32,
        wait_ms: u64,
    },
    /// Click a "load more" button up to `max_clicks` times.
    LoadMore {
        button_selector: String,
        max_clicks: u32,
    },
}

/// Site profile learned by the Claude discovery agent.
///
/// Serialised to JSON for persistence and reuse. Can also be emitted as TOML
/// when building a `DaemonSpec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteProfile {
    /// Primary domain this profile applies to (e.g. `"hh.ru"`).
    pub domain: String,
    /// CSS selector that matches the repeating item container on a listing page.
    pub container_selector: String,
    /// Ordered list of field extraction rules.
    pub fields: Vec<FieldConfig>,
    /// How to navigate to subsequent pages, if applicable.
    pub pagination: Option<PaginationConfig>,
    /// Whether headless browser rendering is required for this site.
    pub requires_browser: bool,
    /// Agent-reported confidence score [0.0, 1.0] for this profile.
    pub confidence: f64,
    /// Whether this profile has been validated against real data.
    pub validated: bool,
    /// ISO-8601 timestamp when the profile was first created.
    pub created_at: String,
    /// ISO-8601 timestamp when the profile was last validated, if ever.
    pub validated_at: Option<String>,
}

impl SiteProfile {
    /// Deserialise a `SiteProfile` from a JSON file.
    pub fn load_json(path: &Path) -> Result<Self, ProfileError> {
        let content = std::fs::read_to_string(path)?;
        let profile: Self = serde_json::from_str(&content)?;
        Ok(profile)
    }

    /// Serialise this profile to pretty-printed JSON and write to `path`.
    pub fn save_json(&self, path: &Path) -> Result<(), ProfileError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Serialise this profile to TOML and return the string.
    pub fn to_toml_string(&self) -> Result<String, ProfileError> {
        Ok(toml::to_string_pretty(self)?)
    }
}
