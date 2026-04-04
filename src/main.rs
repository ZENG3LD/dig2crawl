use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "dig2crawl", version, about = "Generic stealth web crawler with AI extraction")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a new crawl job
    Crawl {
        /// Path to site config TOML
        #[arg(short, long)]
        config: PathBuf,
        /// Path to SQLite database
        #[arg(short, long, default_value = "crawl.db")]
        db: PathBuf,
        /// Override max_pages
        #[arg(long)]
        max_pages: Option<usize>,
        /// Override max_depth
        #[arg(long)]
        max_depth: Option<usize>,
        /// Enable verbose logging
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show job status
    Status {
        #[arg(short, long, default_value = "crawl.db")]
        db: PathBuf,
        #[arg(long)]
        job_id: Option<String>,
    },

    /// Export records from a job
    Export {
        #[arg(short, long, default_value = "crawl.db")]
        db: PathBuf,
        #[arg(long)]
        job_id: String,
        /// Output format: jsonl, json, csv
        #[arg(short, long, default_value = "jsonl")]
        format: String,
        /// Output file (stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Print example config TOML
    Init {
        #[arg(long, default_value = "my-site")]
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Crawl {
            config,
            db,
            max_pages,
            max_depth,
            verbose,
        } => {
            // Init logging
            let filter = if verbose { "debug" } else { "info" };
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::new(filter))
                .init();

            // Load config
            let mut crawl_config =
                crawl_core::config::load_config(&config).context("Failed to load config")?;

            // Apply CLI overrides
            if let Some(mp) = max_pages {
                crawl_config.max_pages = Some(mp);
            }
            if let Some(md) = max_depth {
                crawl_config.max_depth = Some(md);
            }

            // Create storage
            let storage = std::sync::Arc::new(
                crawl_storage::db::SqliteStorage::open(&db)
                    .context("Failed to open database")?,
            );

            // Create fetcher (browser if config says so, otherwise HTTP)
            let fetcher: std::sync::Arc<dyn crawl_core::traits::Fetcher> =
                match &crawl_config.fetch_method {
                    crawl_core::types::FetchMethod::Browser { .. } => {
                        let browser = crawl_fetch::browser::BrowserFetcher::new(
                            1,
                            dig2browser::StealthConfig::russian(),
                        )
                        .await
                        .context("Failed to start browser")?;
                        std::sync::Arc::new(browser)
                    }
                    crawl_core::types::FetchMethod::Http => {
                        let http = crawl_fetch::http::HttpFetcher::new(
                            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 dig2crawl/0.1",
                        )
                        .context("Failed to create HTTP client")?;
                        std::sync::Arc::new(http)
                    }
                };

            // Create queue
            let queue = std::sync::Arc::new(crawl_core::queue::InMemoryQueue::new());

            // Create link extractor
            let domains: Vec<String> = crawl_config
                .domains
                .iter()
                .map(|d| d.to_string())
                .collect();
            let link_extractor = std::sync::Arc::new(crawl_core::extract::HtmlLinkExtractor::new(
                domains,
            ));

            // Create rate limiter
            let rate_limiter = std::sync::Arc::new(
                crawl_core::scheduler::PerDomainRateLimiter::new(
                    crawl_config.rate.requests_per_second,
                ),
            );

            // Create agent (if goals are set)
            let agent: Option<std::sync::Arc<dyn crawl_core::traits::CrawlAgent>> =
                if crawl_config.agent_goal.is_some() {
                    let work_dir = db
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join("agent-work");
                    std::fs::create_dir_all(&work_dir)?;
                    let bridge = crawl_agent::bridge::AgentBridge::new(work_dir);
                    Some(std::sync::Arc::new(bridge))
                } else {
                    None
                };

            // Build engine
            let mut engine_builder = crawl_core::engine::CrawlEngine::builder()
                .fetcher(fetcher)
                .queue(queue)
                .storage(storage.clone())
                .link_extractor(link_extractor)
                .rate_limiter(rate_limiter)
                .config(crawl_config.clone());

            if let Some(a) = agent {
                engine_builder = engine_builder.agent(a);
            }

            let engine = engine_builder.build().context("Failed to build engine")?;

            // Create job
            let job = crawl_core::types::CrawlJob {
                id: uuid::Uuid::new_v4(),
                config: crawl_config,
                started_at: chrono::Utc::now(),
                status: crawl_core::types::JobStatus::Running,
            };

            // Save job to DB
            storage
                .create_job(&job)
                .await
                .context("Failed to create job")?;

            tracing::info!(job_id = %job.id, "Job created");

            // Run
            let stats = engine.run(job.clone()).await?;

            // Update status
            storage
                .update_job_status(
                    job.id,
                    "completed",
                    Some(&chrono::Utc::now().to_rfc3339()),
                )
                .await?;

            println!("\nCrawl completed:");
            println!("  Job ID:    {}", job.id);
            println!("  Pages:     {}", stats.pages_fetched);
            println!("  Records:   {}", stats.records_extracted);
            println!("  Errors:    {}", stats.errors);
            println!("  Duration:  {}s", stats.elapsed_secs);
            println!("  Database:  {}", db.display());
        }

        Command::Status { db, job_id } => {
            println!("Status for {:?} in {}", job_id, db.display());
        }

        Command::Export {
            db,
            job_id,
            format,
            output,
        } => {
            let conn =
                rusqlite::Connection::open(&db).context("Failed to open database")?;

            let mut writer: Box<dyn std::io::Write> = match output {
                Some(path) => Box::new(std::fs::File::create(&path)?),
                None => Box::new(std::io::stdout()),
            };

            let count = match format.as_str() {
                "jsonl" => {
                    crawl_storage::export::export_jsonl(&conn, &job_id, &mut writer)?
                }
                "json" => {
                    crawl_storage::export::export_json(&conn, &job_id, &mut writer)?
                }
                "csv" => {
                    crawl_storage::export::export_csv(&conn, &job_id, &mut writer)?
                }
                other => anyhow::bail!("Unknown format: {other}. Use jsonl, json, or csv"),
            };

            eprintln!("Exported {count} records");
        }

        Command::Init { name } => {
            println!(
                r#"# dig2crawl site config
name = "{name}"
domains = ["{name}.com"]
follow_links = true
fetch_method = "Http"

[rate]
requests_per_second = 2.0
min_delay_ms = 500
concurrent_requests = 1

# Optional: start_urls (if not specified, uses https://{{domains[0]}}/)
# start_urls = ["https://{name}.com/"]

# Optional: AI agent extraction goal
# [agent_goal]
# target = "products"
# fields = ["name", "price", "description"]
# notes = "Extract product catalog"
"#
            );
        }
    }

    Ok(())
}
