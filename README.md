# dig2crawl

Universal agnostic web crawler with Claude-powered CSS selector discovery.

Given a URL and a natural-language goal, Claude reads the raw HTML, discovers
CSS selectors, validates extraction, and produces a `SiteProfile` — a
reusable config for machine-loop extraction without any agent involvement.

## What it does

- Fetches a page (plain HTTP or stealth headless browser via dig2browser)
- Detects anti-bot protection, extracts JSON-LD and page metadata
- Runs a Claude agent session that reads the HTML, discovers container + field selectors, and writes a structured JSON response
- Validates discovered selectors by running the pure-Rust `SelectorExtractor` on the same page and asking Claude to confirm data quality
- Saves a `SiteProfile` (container selector, field selectors, pagination config, confidence score)
- Exports a `DaemonSpec` for scheduled monitoring — consumed by external daemons via cron or watchdog

## Architecture

Single-crate layout — all modules live under `src/`:

```
dig2crawl/
├── src/
│   ├── main.rs    — CLI binary (clap)
│   ├── lib.rs     — Library root
│   ├── core/      — Types, traits, error types, rate limiter, engine
│   ├── fetch/     — HttpFetcher (reqwest) + BrowserFetcher (dig2browser stealth)
│   ├── agent/     — AgentSession (Claude CLI bridge), prompts (discovery + validation)
│   ├── parser/    — SelectorExtractor, JsonLdExtractor, AntiBotDetector, MetadataExtractor, LinkExtractor
│   ├── config/    — TOML job config, SiteProfile, DaemonSpec serialisation
│   └── storage/   — SQLite + JSONL output backends
```

## How it works

Discovery runs in 5 steps:

1. **Fetch** — page is fetched via HTTP or headless browser; anti-bot check runs immediately
2. **Context extraction** — JSON-LD blocks and page metadata are extracted and injected into the agent prompt as bonus context
3. **Discovery** — Claude reads `page.html` from disk (using its `Read` tool), analyses the DOM, and writes `response.json` with container selector, field selectors, pagination config, and confidence score
4. **Validation** — the pure-Rust `SelectorExtractor` applies the discovered selectors; Claude reviews the extracted sample in a follow-up turn of the same session and emits a `validation_result` block
5. **Save** — `SiteProfile` is written to `output/<domain>/profile.json`; temp files are removed

After discovery, `extract` applies the saved profile in pure Rust — no agent needed.

## Browser mode

Pass `--browser` to fetch through dig2browser — a stealth headless Chrome driver.

- Uses CDP/BiDi with stealth scripts that patch `navigator.webdriver` and related fingerprint vectors
- Bypasses Cloudflare and other WAF challenges that block plain HTTP clients
- Waits for a CSS selector to appear before capturing HTML (`--wait-selector`)
- Required for sites with JS-rendered pricing tables (e.g. ruvds.com, hostkey.com, ishosting.com)

Without `--browser`, dig2crawl uses a plain `reqwest` HTTP client, which is faster and sufficient for static or server-rendered pages.

## CLI

```bash
# Discover selectors and produce a SiteProfile
dig2crawl discover <url> --goal "Extract VPS plans: name, price, cpu, ram, disk"
dig2crawl discover <url> --goal "..." --browser --wait-selector "div.tariffs"
dig2crawl discover <url> --goal "..." --output-dir ./profiles/mysite

# Extract data using a saved profile (pure Rust, no agent)
dig2crawl extract <url> --profile output/<domain>/profile.json
dig2crawl extract <url> --profile output/<domain>/profile.json --max-pages 5 --output records.jsonl

# Export a DaemonSpec for scheduled monitoring
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.json
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.toml

# Debug tools
dig2crawl fetch <url> [--browser] [--output page.html] [--metadata] [--jsonld] [--antibot]
dig2crawl test-selector <url> --selector "div.item" --field "title:h2.name" --field "price:.price"
dig2crawl collect-links <url> [--depth 2] [--domain-only]
```

Global flag: `--verbose` / `-v` enables debug logging.

## Agent internals

`AgentSession` drives the Claude CLI (`@anthropic-ai/claude-code`) via subprocess.

**Bootstrap pattern** — avoids the Windows cmd.exe 8191-character argument limit:

- The full prompt is written to `%TEMP%/dig2crawl_<pid>/prompt.md`
- The expected response path is `%TEMP%/dig2crawl_<pid>/response.json`
- The `-p` argument sent to Claude stays under 300 bytes: `"Read and execute the instructions in <prompt.md> — write your JSON response to <response.json>"`
- Claude uses its `Read` tool to load the prompt and HTML, then its `Write` tool to save the response
- On subsequent turns `--resume <session_id>` is passed so full context is retained across discovery → validation
- `--dangerously-skip-permissions` enables file tool use in one-shot mode

**Temp directory layout during a `discover` run:**

```
%TEMP%/dig2crawl_<pid>/
├── page.html                  — raw fetched HTML (unmodified)
├── prompt.md                  — discovery instructions for Claude
├── response.json              — Claude's discovery JSON response
├── validation_prompt.md       — validation instructions for Claude
└── validation_response.json   — Claude's validation JSON response
```

The directory is deleted when the session closes.

**Robust JSON parsing** — the agent protocol tolerates several classes of model output variation:

- Regex escape sequences in `transform` patterns (e.g. `\\d+`) parsed without double-escape errors
- `null` selectors for fields absent from the page (skipped gracefully, not treated as errors)
- Untagged `next_urls` values — accepted as either a plain string or a `{"url": "..."}` object
- Mixed-type `url_patterns` and field specs — arrays and scalars both accepted at every field

## Batch crawl results

Tested against 18 L2 (bare-metal / VPS) hosting providers to profile pricing pages.

| Metric | Result |
|--------|--------|
| Providers attempted | 18 |
| Successfully profiled | 16 |
| Validated (confidence >= 0.55) | 15 |
| Average confidence | ~0.80 |
| Recovered via browser mode | 3 (ruvds.com, hostkey.com, ishosting.com) |

Sites that required `--browser` had JS-rendered pricing grids that returned empty containers under plain HTTP. Browser mode resolved all three.

## Output

```
output/<domain>/
└── profile.json
```

Example (`output/adminvps.ru/profile.json`):

```json
{
  "domain": "adminvps.ru",
  "container_selector": "#tariffs .slider__block-content .swiper-slide .slider__item",
  "fields": [
    { "name": "name",  "selector": ".products-new__item-head-title", "extract": "text", "transform": "trim" },
    { "name": "price", "selector": ".slide-title p",                 "extract": "text", "transform": "parse_number" },
    { "name": "cpu",   "selector": ".slide-params-list li:nth-child(1) .char", "extract": "text", "transform": "trim" }
  ],
  "pagination": null,
  "requires_browser": true,
  "confidence": 0.9,
  "validated": true
}
```

`output/` is gitignored.

## Prerequisites

- **Claude CLI** — `npm install -g @anthropic-ai/claude-code`
- **Chrome** — required for `--browser` mode (auto-detected by dig2browser)
- **Rust 1.75+**

## Building

```bash
cargo build --release --bin dig2crawl
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| dig2browser | Stealth browser automation (CDP + BiDi) — crates.io |
| scraper | CSS selector engine |
| reqwest | HTTP client |
| rusqlite | SQLite storage |
| clap | CLI framework |
| governor | Rate limiter |
| tokio | Async runtime |

## Support the Project

If you find this tool useful, consider supporting development:

| Currency | Network | Address |
|----------|---------|---------|
| USDT | TRC20 | `TNxMKsvVLYViQ5X5sgCYmkzH4qjhhh5U7X` |
| USDC | Arbitrum | `0xEF3B94Fe845E21371b4C4C5F2032E1f23A13Aa6e` |
| ETH | Ethereum | `0xEF3B94Fe845E21371b4C4C5F2032E1f23A13Aa6e` |
| BTC | Bitcoin | `bc1qjgzthxja8umt5tvrp5tfcf9zeepmhn0f6mnt40` |
| SOL | Solana | `DZJjmH8Cs5wEafz5Ua86wBBkurSA4xdWXa3LWnBUR94c` |

## License

MIT
