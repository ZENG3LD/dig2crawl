//! dig2crawl-parser — generic HTML parsing utilities.
//!
//! This crate provides domain-agnostic extraction primitives:
//! - [`selector`] — CSS selector-based item extraction (fast path, no agent)
//! - [`jsonld`]   — JSON-LD and microdata extraction
//! - [`antibot`]  — CAPTCHA / challenge page detection
//! - [`metadata`] — page title, description, Open Graph, canonical URL
//! - [`links`]    — outbound link extraction with relative URL resolution

pub mod antibot;
pub mod jsonld;
pub mod links;
pub mod metadata;
pub mod selector;

pub use antibot::{AntiBotResult, AntiBotDetector};
pub use jsonld::JsonLdExtractor;
pub use links::LinkExtractor;
pub use metadata::{PageMetadata, MetadataExtractor};
pub use selector::SelectorExtractor;
