use crate::{error::CrawlError, types::CrawlConfig};
use std::path::Path;

/// Load a `CrawlConfig` from a TOML file.
pub fn load_config(path: &Path) -> Result<CrawlConfig, CrawlError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        CrawlError::Config(format!("failed to read {}: {}", path.display(), e))
    })?;

    let config: CrawlConfig = toml::from_str(&content).map_err(|e| {
        CrawlError::Config(format!("failed to parse {}: {}", path.display(), e))
    })?;

    Ok(config)
}
