use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cookie-auth",
    version,
    about = "Open a visible browser for manual login/captcha, then save cookies to a persistent profile"
)]
struct Cli {
    /// URL to open in the browser
    url: String,

    /// Path to fingerprint JSON config (locale, browser preference, etc.)
    #[arg(long, value_name = "PATH")]
    fingerprint: Option<PathBuf>,

    /// Persistent profile directory. If omitted, auto-created at
    /// %TEMP%/dig2crawl-profiles/<domain>/
    #[arg(long, value_name = "PATH")]
    profile: Option<PathBuf>,
}

/// JSON-deserialisable fingerprint configuration for cookie-auth.
/// Only the fields relevant to auth session setup are extracted here;
/// additional fields in the JSON are silently ignored by serde.
#[derive(serde::Deserialize, Default)]
struct FingerprintConfig {
    browser: Option<String>,
    locale: Option<String>,
}

impl FingerprintConfig {
    fn browser_pref(&self) -> dig2browser::BrowserPreference {
        match self.browser.as_deref() {
            Some("chrome") => dig2browser::BrowserPreference::ChromeOnly,
            Some("edge") => dig2browser::BrowserPreference::EdgeOnly,
            Some("firefox") => dig2browser::BrowserPreference::Firefox,
            _ => dig2browser::BrowserPreference::Auto,
        }
    }

    fn locale(&self) -> Option<String> {
        self.locale.clone()
    }
}

fn load_fingerprint(path: &Option<PathBuf>) -> Result<FingerprintConfig> {
    match path {
        Some(p) => {
            let json = std::fs::read_to_string(p)
                .with_context(|| format!("Failed to read fingerprint config: {}", p.display()))?;
            let cfg: FingerprintConfig = serde_json::from_str(&json)
                .with_context(|| format!("Failed to parse fingerprint config: {}", p.display()))?;
            Ok(cfg)
        }
        None => Ok(FingerprintConfig::default()),
    }
}

/// Resolve profile directory: use explicit path if given, otherwise auto-create
/// `<TEMP>/dig2crawl-profiles/<domain>/` from the URL.
fn resolve_profile(explicit: Option<PathBuf>, url_str: &str) -> Result<PathBuf> {
    if let Some(p) = explicit {
        std::fs::create_dir_all(&p)
            .with_context(|| format!("Failed to create profile directory: {}", p.display()))?;
        return Ok(p);
    }

    let parsed = url::Url::parse(url_str)
        .with_context(|| format!("Invalid URL: {url_str}"))?;
    let domain = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host: {url_str}"))?;

    let dir = std::env::temp_dir()
        .join("dig2crawl-profiles")
        .join(domain);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create profile directory: {}", dir.display()))?;
    Ok(dir)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_target(false)
        .init();

    let fingerprint = load_fingerprint(&cli.fingerprint)?;
    let browser_pref = fingerprint.browser_pref();
    let locale = fingerprint.locale();

    let profile_path = resolve_profile(cli.profile, &cli.url)?;

    dig2browser::cookies::open_auth_session_with_locale(
        &cli.url,
        &profile_path,
        browser_pref,
        locale.as_deref(),
    )
    .await
    .context("auth session failed")?;

    println!("Profile saved to: {}", profile_path.display());
    Ok(())
}
