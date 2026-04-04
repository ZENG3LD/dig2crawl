use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "dig2crawl", version, about = "Generic stealth web crawler with AI extraction")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Enable verbose (debug) logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Discover site structure with Claude: fetch page, extract selectors, validate
    Discover {
        /// URL of the page to analyse
        url: String,
        /// Natural-language goal describing what data to extract
        #[arg(short, long)]
        goal: String,
        /// Use headless browser instead of plain HTTP
        #[arg(long)]
        browser: bool,
        /// CSS selector to wait for (browser mode only)
        #[arg(long)]
        wait_selector: Option<String>,
        /// Claude model to use
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,
        /// Directory to save discovered profile (default: output/<domain>/)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },

    /// Extract data from a URL using a saved site profile
    Extract {
        /// URL to extract data from
        url: String,
        /// Path to profile.json produced by `discover`
        #[arg(short, long)]
        profile: PathBuf,
        /// Use headless browser
        #[arg(long)]
        browser: bool,
        /// Follow pagination up to this many pages (default: 1)
        #[arg(long, default_value = "1")]
        max_pages: usize,
        /// Save output to this file (JSONL); prints to stdout if omitted
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Build a DaemonSpec from a profile and export it as JSON or TOML
    ExportSpec {
        /// Path to profile.json
        profile: PathBuf,
        /// Cron expression, e.g. "0 6 * * *"
        #[arg(short, long)]
        schedule: String,
        /// Output path (.json or .toml)
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Fetch a single page and print its HTML (debug tool)
    Fetch {
        /// URL to fetch
        url: String,
        /// Use headless browser
        #[arg(long)]
        browser: bool,
        /// CSS selector to wait for (browser mode only)
        #[arg(long)]
        wait_selector: Option<String>,
        /// Save HTML to this file instead of printing to stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Show page metadata (title, description, canonical)
        #[arg(long)]
        metadata: bool,
        /// Show JSON-LD blocks
        #[arg(long)]
        jsonld: bool,
        /// Show antibot detection result
        #[arg(long)]
        antibot: bool,
    },

    /// Apply a CSS selector to a page and print matches (debug tool)
    TestSelector {
        /// URL to fetch
        url: String,
        /// Container CSS selector
        #[arg(short, long)]
        selector: String,
        /// Additional field selectors in "name:css" format
        #[arg(short, long = "field")]
        fields: Vec<String>,
        /// Use headless browser
        #[arg(long)]
        browser: bool,
    },

    /// Collect all links from a page (optionally follow to depth N)
    CollectLinks {
        /// Seed URL
        url: String,
        /// Follow links up to this depth (0 = seed page only)
        #[arg(short, long, default_value = "0")]
        depth: usize,
        /// Only return links on the same domain as the seed URL
        #[arg(long)]
        domain_only: bool,
        /// Use headless browser
        #[arg(long)]
        browser: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_target(false)
        .init();

    match cli.command {
        Command::Discover {
            url,
            goal,
            browser,
            wait_selector,
            model,
            output_dir,
        } => cmd_discover(url, goal, browser, wait_selector, model, output_dir).await,

        Command::Extract {
            url,
            profile,
            browser,
            max_pages,
            output,
        } => cmd_extract(url, profile, browser, max_pages, output).await,

        Command::ExportSpec {
            profile,
            schedule,
            output,
        } => cmd_export_spec(profile, schedule, output),

        Command::Fetch {
            url,
            browser,
            wait_selector,
            output,
            metadata,
            jsonld,
            antibot,
        } => cmd_fetch(url, browser, wait_selector, output, metadata, jsonld, antibot).await,

        Command::TestSelector {
            url,
            selector,
            fields,
            browser,
        } => cmd_test_selector(url, selector, fields, browser).await,

        Command::CollectLinks {
            url,
            depth,
            domain_only,
            browser,
        } => cmd_collect_links(url, depth, domain_only, browser).await,
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn make_fetcher(
    browser: bool,
    wait_selector: Option<String>,
) -> Result<Box<dyn crawl_core::traits::Fetcher>> {
    if browser {
        let fetcher = crawl_fetch::browser::BrowserFetcher::new(
            1,
            dig2browser::StealthConfig::russian(),
            wait_selector,
        )
        .await
        .context("Failed to start browser")?;
        Ok(Box::new(fetcher))
    } else {
        let fetcher = crawl_fetch::http::HttpFetcher::new(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 dig2crawl/0.1",
        )
        .context("Failed to create HTTP client")?;
        Ok(Box::new(fetcher))
    }
}

async fn fetch_page(
    fetcher: &dyn crawl_core::traits::Fetcher,
    url_str: &str,
) -> Result<crawl_core::types::FetchedPage> {
    use url::Url;
    let url = Url::parse(url_str).with_context(|| format!("Invalid URL: {url_str}"))?;
    fetcher
        .fetch(&url)
        .await
        .with_context(|| format!("Failed to fetch {url_str}"))
}

/// Parse the raw agent response string into `AgentResponse`.
///
/// Claude sometimes wraps JSON in a markdown code fence — we strip those.
fn parse_agent_response(raw: &str) -> Result<crawl_agent::protocol::AgentResponse> {
    let json_str = extract_json_block(raw);
    serde_json::from_str(json_str)
        .with_context(|| format!("Failed to parse agent response as JSON:\n{json_str}"))
}

/// Extract the first JSON object from a string that may contain markdown fences.
fn extract_json_block(s: &str) -> &str {
    // Try to find ```json ... ``` or ``` ... ```
    if let Some(start) = s.find("```json") {
        let inner = &s[start + 7..];
        if let Some(end) = inner.find("```") {
            return inner[..end].trim();
        }
    }
    if let Some(start) = s.find("```") {
        let inner = &s[start + 3..];
        if let Some(end) = inner.find("```") {
            return inner[..end].trim();
        }
    }
    // No fences — find first '{' to last '}'
    if let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) {
        if start <= end {
            return &s[start..=end];
        }
    }
    s.trim()
}

/// Build a `crawl_core::types::SiteProfile` from an `AgentResponse`.
///
/// The discovery response contains `field_configs` (v2) and `updated_memory`
/// with the container selector. We prefer the v2 field configs when present.
fn build_site_profile(
    domain: &str,
    response: &crawl_agent::protocol::AgentResponse,
    browser_required: bool,
) -> Result<crawl_core::types::SiteProfile> {
    use chrono::Utc;
    use crawl_core::types::{ExtractMode, FieldConfig, SiteProfile};

    // Container selector comes from updated_memory.selectors
    let container_selector = response
        .updated_memory
        .as_ref()
        .and_then(|m| m.selectors.values().next())
        .and_then(|s| s.container_selector.clone())
        .unwrap_or_else(|| "div".to_string());

    // Field configs — prefer v2 field_configs, fall back to updated_memory selectors
    let fields: Vec<FieldConfig> = if !response.field_configs.is_empty() {
        response
            .field_configs
            .iter()
            .map(|fc| FieldConfig {
                name: fc.name.clone(),
                selector: fc.selector.clone(),
                extract: match &fc.extract {
                    crawl_agent::protocol::ExtractMode::Text => ExtractMode::Text,
                    crawl_agent::protocol::ExtractMode::Attribute(a) => {
                        ExtractMode::Attribute(a.clone())
                    }
                    crawl_agent::protocol::ExtractMode::Html => ExtractMode::Html,
                    crawl_agent::protocol::ExtractMode::OuterHtml => ExtractMode::OuterHtml,
                },
                prefix: fc.prefix.clone(),
                transform: fc.transform.as_ref().map(|t| match t {
                    crawl_agent::protocol::Transform::Trim => crawl_core::types::Transform::Trim,
                    crawl_agent::protocol::Transform::Lowercase => {
                        crawl_core::types::Transform::Lowercase
                    }
                    crawl_agent::protocol::Transform::Uppercase => {
                        crawl_core::types::Transform::Uppercase
                    }
                    crawl_agent::protocol::Transform::Regex(p) => {
                        crawl_core::types::Transform::Regex(p.clone())
                    }
                    crawl_agent::protocol::Transform::Replace(f, t) => {
                        crawl_core::types::Transform::Replace(f.clone(), t.clone())
                    }
                    crawl_agent::protocol::Transform::ParseNumber => {
                        crawl_core::types::Transform::ParseNumber
                    }
                }),
            })
            .collect()
    } else {
        response
            .updated_memory
            .as_ref()
            .and_then(|m| m.selectors.values().next())
            .map(|s| {
                s.fields
                    .iter()
                    .map(|(name, sel)| FieldConfig {
                        name: name.clone(),
                        selector: sel.clone(),
                        extract: ExtractMode::Text,
                        prefix: None,
                        transform: None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let confidence = response.confidence.map(|c| c as f64).unwrap_or(0.5);
    let now = Utc::now();

    Ok(SiteProfile {
        domain: domain.to_string(),
        container_selector,
        fields,
        pagination: None,
        requires_browser: browser_required,
        confidence,
        validated: false,
        created_at: now,
        last_used_at: now,
    })
}

// ─── commands ────────────────────────────────────────────────────────────────

async fn cmd_discover(
    url_str: String,
    goal: String,
    browser: bool,
    wait_selector: Option<String>,
    model: String,
    output_dir: Option<PathBuf>,
) -> Result<()> {
    println!("Discovering site structure for: {url_str}");
    println!("Goal: {goal}");
    println!();

    // 1. Fetch the page
    println!("[1/5] Fetching page...");
    let fetcher = make_fetcher(browser, wait_selector).await?;
    let page = fetch_page(fetcher.as_ref(), &url_str).await?;
    println!(
        "      OK — {} bytes, {}ms",
        page.body.len(),
        page.fetch_ms
    );

    // 2. Anti-bot check
    println!("[2/5] Checking for anti-bot protection...");
    let antibot = dig2crawl_parser::AntiBotDetector::new().detect(&page.body);
    if antibot.detected {
        eprintln!(
            "WARNING: Anti-bot detected! Provider: {}, Type: {}",
            antibot.provider.as_deref().unwrap_or("unknown"),
            antibot.challenge_type.as_deref().unwrap_or("unknown")
        );
        eprintln!("         Extraction may fail or produce empty results.");
    } else {
        println!("      Clean (no anti-bot detected)");
    }

    // 3. Extract JSON-LD and metadata as bonus context
    println!("[3/5] Extracting JSON-LD and metadata...");
    let jsonld_items = dig2crawl_parser::JsonLdExtractor::extract_jsonld(&page.body);
    let parsed_url = url::Url::parse(&url_str)?;
    let metadata = dig2crawl_parser::MetadataExtractor::new().extract(&page.body, Some(&parsed_url));
    if !jsonld_items.is_empty() {
        println!("      JSON-LD: {} block(s) found", jsonld_items.len());
    }
    if let Some(title) = &metadata.title {
        println!("      Title: {title}");
    }

    // 4. Start AgentSession and send discovery prompt
    println!("[4/5] Starting Claude session ({model})...");
    let mut session = crawl_agent::session::AgentSession::start(&model)
        .await
        .context("Failed to start Claude agent session. Is `claude` CLI installed?")?;

    // Build discovery prompt — include truncated HTML (limit to ~50KB for context window)
    const HTML_LIMIT: usize = 50_000;
    let html_for_prompt = if page.body.len() > HTML_LIMIT {
        &page.body[..HTML_LIMIT]
    } else {
        &page.body
    };

    // Enrich prompt with JSON-LD context if available
    let jsonld_context = if !jsonld_items.is_empty() {
        let jsonld_str = serde_json::to_string_pretty(&jsonld_items)
            .unwrap_or_default();
        format!("\n\n## JSON-LD structured data found on page\n```json\n{jsonld_str}\n```")
    } else {
        String::new()
    };

    let full_goal = format!("{goal}{jsonld_context}");
    let discovery_prompt = crawl_agent::prompts::build_discovery_prompt(html_for_prompt, &full_goal);

    println!("      Sending discovery prompt ({} chars)...", discovery_prompt.len());
    let discovery_raw = session
        .send(&discovery_prompt)
        .await
        .context("Discovery prompt failed")?;

    tracing::debug!(response_len = discovery_raw.len(), "Discovery response received");

    // Parse response
    let discovery_response = parse_agent_response(&discovery_raw)
        .context("Could not parse discovery response")?;

    // Extract domain for profile
    let domain = parsed_url
        .host_str()
        .unwrap_or("unknown")
        .to_string();

    // Build SiteProfile from discovery response
    let mut profile = build_site_profile(&domain, &discovery_response, browser)?;

    println!(
        "      Discovered {} field(s), container: {:?}, confidence: {:.2}",
        profile.fields.len(),
        profile.container_selector,
        profile.confidence,
    );

    // 5. Apply SelectorExtractor to validate on the same page
    println!("[5/5] Validating selectors on fetched page...");
    let extractor = dig2crawl_parser::SelectorExtractor::new();
    let extracted_records = extractor.extract(&page.body, &profile);

    println!("      Extracted {} record(s) with discovered selectors", extracted_records.len());

    // Send validation prompt to Claude (same session)
    if !extracted_records.is_empty() {
        let snapshot = discovery_response.updated_memory.clone().unwrap_or_default();
        let validation_prompt =
            crawl_agent::prompts::build_validation_prompt(&extracted_records, &snapshot);

        println!("      Sending validation prompt...");
        let validation_raw = session
            .send(&validation_prompt)
            .await
            .context("Validation prompt failed")?;

        if let Ok(validation_response) = parse_agent_response(&validation_raw) {
            if let Some(vr) = &validation_response.validation_result {
                println!(
                    "      Validation: {} — {} items extracted, confidence {:.2}",
                    if vr.passed { "PASSED" } else { "FAILED" },
                    vr.items_extracted,
                    vr.confidence,
                );
                if !vr.issues.is_empty() {
                    println!("      Issues:");
                    for issue in &vr.issues {
                        println!("        - {issue}");
                    }
                }
                profile.validated = vr.passed;
                if vr.confidence > 0.0 {
                    profile.confidence = vr.confidence as f64;
                }
            }
        }
    } else {
        println!("      No records extracted — validation skipped");
    }

    session.close().await;

    // Save profile
    let out_dir = output_dir.unwrap_or_else(|| PathBuf::from("output").join(&domain));
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("Failed to create output directory: {}", out_dir.display()))?;

    let profile_path = out_dir.join("profile.json");
    let profile_json = serde_json::to_string_pretty(&profile)?;
    std::fs::write(&profile_path, &profile_json)
        .with_context(|| format!("Failed to write profile: {}", profile_path.display()))?;

    println!();
    println!("Profile saved to: {}", profile_path.display());
    println!();
    println!("Summary:");
    println!("  Domain:     {}", profile.domain);
    println!("  Container:  {}", profile.container_selector);
    println!("  Fields:     {}", profile.fields.len());
    for f in &profile.fields {
        println!("    - {} ({})", f.name, f.selector);
    }
    println!("  Confidence: {:.2}", profile.confidence);
    println!("  Validated:  {}", profile.validated);
    if !extracted_records.is_empty() {
        println!();
        println!("Sample extracted data ({} record(s)):", extracted_records.len().min(3));
        for record in extracted_records.iter().take(3) {
            println!("  {}", serde_json::to_string(record).unwrap_or_default());
        }
    }

    Ok(())
}

async fn cmd_extract(
    url_str: String,
    profile_path: PathBuf,
    browser: bool,
    max_pages: usize,
    output: Option<PathBuf>,
) -> Result<()> {
    // Load profile
    let profile_json = std::fs::read_to_string(&profile_path)
        .with_context(|| format!("Failed to read profile: {}", profile_path.display()))?;
    let profile: crawl_core::types::SiteProfile = serde_json::from_str(&profile_json)
        .with_context(|| format!("Failed to parse profile: {}", profile_path.display()))?;

    let fetcher = make_fetcher(browser || profile.requires_browser, None).await?;
    let extractor = dig2crawl_parser::SelectorExtractor::new();

    let mut all_records: Vec<serde_json::Value> = Vec::new();
    let mut current_url = url_str.clone();
    let mut pages_done = 0;

    while pages_done < max_pages {
        println!("Fetching page {}/{}: {current_url}", pages_done + 1, max_pages);
        let page = fetch_page(fetcher.as_ref(), &current_url).await?;
        let records = extractor.extract(&page.body, &profile);
        println!("  Extracted {} record(s)", records.len());
        all_records.extend(records);
        pages_done += 1;

        // Follow pagination if configured (NextButton only for now)
        if pages_done < max_pages {
            if let Some(crawl_core::types::PaginationConfig::NextButton { selector }) =
                &profile.pagination
            {
                // Extract the href from the next-button element using a single-field profile
                let nav_profile = crawl_core::types::SiteProfile {
                    domain: profile.domain.clone(),
                    container_selector: selector.clone(),
                    fields: vec![crawl_core::types::FieldConfig {
                        name: "href".to_string(),
                        selector: selector.clone(),
                        extract: crawl_core::types::ExtractMode::Attribute("href".to_string()),
                        prefix: None,
                        transform: None,
                    }],
                    pagination: None,
                    requires_browser: browser || profile.requires_browser,
                    confidence: 1.0,
                    validated: true,
                    created_at: profile.created_at,
                    last_used_at: profile.last_used_at,
                };
                let nav_extractor = dig2crawl_parser::SelectorExtractor::new();
                let nav_records = nav_extractor.extract(&page.body, &nav_profile);
                if let Some(rec) = nav_records.first() {
                    if let Some(href) = rec.get("href").and_then(|v| v.as_str()) {
                        let base = url::Url::parse(&current_url)?;
                        let next = base.join(href)?;
                        current_url = next.to_string();
                        continue;
                    }
                }
            }
        }
        break;
    }

    println!("\nTotal records: {}", all_records.len());

    // Output
    match output {
        Some(path) => {
            let mut writer = std::io::BufWriter::new(std::fs::File::create(&path)?);
            use std::io::Write;
            for rec in &all_records {
                writeln!(writer, "{}", serde_json::to_string(rec)?)?;
            }
            println!("Saved to: {}", path.display());
        }
        None => {
            for rec in &all_records {
                println!("{}", serde_json::to_string(rec)?);
            }
        }
    }

    Ok(())
}

fn cmd_export_spec(
    profile_path: PathBuf,
    schedule: String,
    output: PathBuf,
) -> Result<()> {
    let profile_json = std::fs::read_to_string(&profile_path)
        .with_context(|| format!("Failed to read profile: {}", profile_path.display()))?;
    let site_profile: crawl_core::types::SiteProfile = serde_json::from_str(&profile_json)
        .context("Failed to parse profile JSON")?;

    let spec = crawl_core::types::DaemonSpec {
        name: format!("{}-crawler", site_profile.domain),
        domain: site_profile.domain.clone(),
        seed_urls: vec![format!("https://{}/", site_profile.domain)],
        site_profile,
        schedule: crawl_core::types::CronSchedule {
            expression: schedule,
        },
        fetch_method: crawl_core::types::FetchMethod::Http,
        rate_limit: crawl_core::types::RateLimitConfig {
            requests_per_second: 1.0,
            min_delay_ms: 1000,
            concurrent_requests: 1,
        },
        output_format: crawl_core::types::OutputFormat::Jsonl,
        created_at: chrono::Utc::now(),
        spec_version: "1.0".to_string(),
    };

    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("json");
    if ext == "toml" {
        let toml_str = toml::to_string_pretty(&spec).context("Failed to serialize spec to TOML")?;
        std::fs::write(&output, toml_str)?;
    } else {
        let json_str = serde_json::to_string_pretty(&spec)?;
        std::fs::write(&output, json_str)?;
    }

    println!("DaemonSpec saved to: {}", output.display());
    Ok(())
}

async fn cmd_fetch(
    url_str: String,
    browser: bool,
    wait_selector: Option<String>,
    output: Option<PathBuf>,
    show_metadata: bool,
    show_jsonld: bool,
    show_antibot: bool,
) -> Result<()> {
    let fetcher = make_fetcher(browser, wait_selector).await?;
    let page = fetch_page(fetcher.as_ref(), &url_str).await?;

    eprintln!(
        "Fetched: {} ({} bytes, {}ms, status {:?})",
        url_str,
        page.body.len(),
        page.fetch_ms,
        page.status_code,
    );

    if show_antibot {
        let result = dig2crawl_parser::AntiBotDetector::new().detect(&page.body);
        if result.detected {
            eprintln!(
                "Anti-bot: {} ({})",
                result.provider.as_deref().unwrap_or("unknown"),
                result.challenge_type.as_deref().unwrap_or("unknown"),
            );
        } else {
            eprintln!("Anti-bot: none detected");
        }
    }

    if show_metadata {
        let parsed_url = url::Url::parse(&url_str)?;
        let meta =
            dig2crawl_parser::MetadataExtractor::new().extract(&page.body, Some(&parsed_url));
        eprintln!("Metadata:");
        if let Some(t) = &meta.title {
            eprintln!("  title: {t}");
        }
        if let Some(d) = &meta.description {
            eprintln!("  description: {d}");
        }
        if let Some(lang) = &meta.language {
            eprintln!("  language: {lang}");
        }
    }

    if show_jsonld {
        let items = dig2crawl_parser::JsonLdExtractor::extract_jsonld(&page.body);
        eprintln!("JSON-LD ({} blocks):", items.len());
        for item in &items {
            eprintln!(
                "  {}",
                serde_json::to_string(item).unwrap_or_default()
            );
        }
    }

    match output {
        Some(path) => {
            std::fs::write(&path, &page.body)?;
            eprintln!("HTML saved to: {}", path.display());
        }
        None => print!("{}", page.body),
    }

    Ok(())
}

async fn cmd_test_selector(
    url_str: String,
    selector: String,
    field_specs: Vec<String>,
    browser: bool,
) -> Result<()> {
    use chrono::Utc;
    use crawl_core::types::{ExtractMode, FieldConfig, SiteProfile};

    let fetcher = make_fetcher(browser, None).await?;
    let page = fetch_page(fetcher.as_ref(), &url_str).await?;

    // Parse field specs: "name:css_selector"
    let fields: Vec<FieldConfig> = field_specs
        .iter()
        .filter_map(|spec| {
            let mut parts = spec.splitn(2, ':');
            let name = parts.next()?.to_string();
            let sel = parts.next()?.to_string();
            Some(FieldConfig {
                name,
                selector: sel,
                extract: ExtractMode::Text,
                prefix: None,
                transform: None,
            })
        })
        .collect();

    let parsed_url = url::Url::parse(&url_str)?;
    let domain = parsed_url.host_str().unwrap_or("unknown").to_string();

    let now = Utc::now();
    let profile = SiteProfile {
        domain,
        container_selector: selector.clone(),
        fields,
        pagination: None,
        requires_browser: browser,
        confidence: 1.0,
        validated: true,
        created_at: now,
        last_used_at: now,
    };

    let extractor = dig2crawl_parser::SelectorExtractor::new();
    let records = extractor.extract(&page.body, &profile);

    println!("Selector: {selector}");
    println!("Matches: {}", records.len());
    for (i, rec) in records.iter().enumerate() {
        println!("[{i}] {}", serde_json::to_string(rec).unwrap_or_default());
    }

    Ok(())
}

async fn cmd_collect_links(
    url_str: String,
    depth: usize,
    domain_only: bool,
    browser: bool,
) -> Result<()> {
    use std::collections::{HashSet, VecDeque};

    let fetcher = make_fetcher(browser, None).await?;
    let parsed_url = url::Url::parse(&url_str)?;
    let seed_domain = parsed_url.host_str().unwrap_or("").to_string();

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    let mut all_links: Vec<String> = Vec::new();

    queue.push_back((url_str.clone(), 0));
    visited.insert(url_str.clone());

    while let Some((current_url, current_depth)) = queue.pop_front() {
        let page = match fetch_page(fetcher.as_ref(), &current_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to fetch {current_url}: {e}");
                continue;
            }
        };

        let link_extractor = dig2crawl_parser::LinkExtractor::new();
        let filter = dig2crawl_parser::links::LinkFilter {
            http_only: true,
            allowed_domains: if domain_only {
                Some(vec![seed_domain.clone()])
            } else {
                None
            },
            strip_fragments: true,
            ..Default::default()
        };
        let links = link_extractor.extract(&page.body, &page.url, &filter);

        for link in links {
            let link_str = link.to_string();
            if visited.contains(&link_str) {
                continue;
            }
            all_links.push(link_str.clone());
            visited.insert(link_str.clone());
            if current_depth < depth {
                queue.push_back((link_str, current_depth + 1));
            }
        }
    }

    println!("Collected {} links:", all_links.len());
    for link in &all_links {
        println!("{link}");
    }

    Ok(())
}
