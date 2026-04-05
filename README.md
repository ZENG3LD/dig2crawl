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

## Browser mode (default)

dig2crawl uses a stealth headless browser (dig2browser) **by default** for all commands.

Supported browsers:

| Browser | Backend | Detection | Notes |
|---------|---------|-----------|-------|
| Chrome | CDP | Auto (first priority) | Full stealth: UA override, Client Hints, canvas noise, WebGL spoof |
| Edge | CDP | Auto (second priority) | Same Chromium flags as Chrome |
| Firefox | BiDi | `"browser": "firefox"` in fingerprint | Stealth via `moz:firefoxOptions.prefs`, requires geckodriver |

- Uses CDP (Chrome/Edge) or BiDi (Firefox) with stealth scripts that patch `navigator.webdriver` and related fingerprint vectors
- Bypasses Cloudflare and other WAF challenges that block plain HTTP clients
- Waits for a CSS selector to appear before capturing HTML (`--wait-selector`)
- Auto-creates a persistent browser profile per domain at `%TEMP%/dig2crawl-profiles/<domain>/`

By default, dig2browser auto-detects Chrome → Edge. To force a specific browser, set `"browser"` in the fingerprint config.

Pass `--http-only` to fall back to a plain `reqwest` HTTP client — faster, sufficient for static or server-rendered pages.

## Fingerprint configuration

Use `--fingerprint <path>` to load a JSON file that configures the browser fingerprint. All fields are optional — omitted fields use defaults (en-US locale, Standard stealth level, 1920x1080 viewport).

```json
{
  "browser": "chrome",
  "level": "full",
  "locale": "ru-RU",
  "timezone": "Europe/Moscow",
  "viewport": [1440, 900],
  "hardware_concurrency": 4,
  "device_memory_gb": 4,
  "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) ..."
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `browser` | string | `"auto"` | Browser: `"auto"`, `"chrome"`, `"edge"`, `"firefox"` |
| `level` | string | `"standard"` | Stealth level: `"basic"`, `"standard_no_webgl"`, `"standard"`, `"full"` |
| `locale` | string | `"en-US"` | BCP-47 locale tag |
| `timezone` | string | `null` | IANA timezone (e.g. `"Europe/Moscow"`) |
| `viewport` | `[w, h]` | `[1920, 1080]` | Screen resolution |
| `hardware_concurrency` | int | `8` | `navigator.hardwareConcurrency` |
| `device_memory_gb` | int | `8` | `navigator.deviceMemory` (GB) |
| `user_agent` | string | Chrome 131 | Full User-Agent string |

**Firefox note:** Firefox uses the BiDi protocol via geckodriver (must be running at `http://localhost:4444`). Stealth is applied via `moz:firefoxOptions.prefs` — no CDP-level overrides. `set_extra_http_headers` is not supported on Firefox.

The fingerprint applies to both `auth` (visible browser) and all headless commands. This ensures the auth session and subsequent crawling share the same fingerprint — critical for sites that compare session fingerprints against cookie fingerprints (e.g. Yandex SmartCaptcha).

## CLI

```bash
# Discover selectors and produce a SiteProfile (browser by default)
dig2crawl discover <url> --goal "Extract VPS plans: name, price, cpu, ram, disk"
dig2crawl discover <url> --goal "..." --wait-selector "div.tariffs"
dig2crawl discover <url> --goal "..." --http-only --output-dir ./profiles/mysite

# Extract data using a saved profile (pure Rust, no agent)
dig2crawl extract <url> --profile output/<domain>/profile.json
dig2crawl extract <url> --profile output/<domain>/profile.json --max-pages 5 --output records.jsonl

# Export a DaemonSpec for scheduled monitoring
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.json
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.toml

# Cookie auth — open a visible browser, log in / pass captcha, save cookies
# Profile auto-created at %TEMP%/dig2crawl-profiles/<domain>/
dig2crawl auth <url>
dig2crawl auth <url> --browser-profile %TEMP%/custom-profile  # explicit profile path
dig2crawl auth <url> --fingerprint russian.json               # custom fingerprint for auth

# Debug tools (browser by default, add --http-only for plain HTTP)
dig2crawl fetch <url> [--output page.html] [--metadata] [--jsonld] [--antibot]
dig2crawl test-selector <url> --selector "div.item" --field "title:h2.name" --field "price:.price"
dig2crawl collect-links <url> [--depth 2] [--domain-only]
```

Global flags:

- `--verbose` / `-v` — debug logging
- `--headed` — launch browser in visible (non-headless) mode
- `--browser-profile <PATH>` — explicit persistent profile directory (default: auto `%TEMP%/dig2crawl-profiles/<domain>/`)
- `--fingerprint <PATH>` — JSON fingerprint config (locale, timezone, viewport, stealth level, etc.)
- `--bot-auth <JWKS_URL>` — enable Web Bot Auth signing
- `--bot-key <PATH>` — Ed25519 private key for bot auth (default: `keys/bot.key`)

## Cookie interceptor (`auth`)

For sites behind captcha or login walls (e.g. Yandex SmartCaptcha), use the `auth` subcommand to open a visible Chrome window where you can log in manually. Cookies are saved to the persistent profile directory and reused by subsequent `discover`/`extract`/`fetch` commands.

```bash
# Step 1: Open browser, pass captcha, close the window
# Profile auto-created at %TEMP%/dig2crawl-profiles/yandex.cloud/
dig2crawl auth https://yandex.cloud/ru/prices --fingerprint russian.json

# Step 2: Use the saved profile for headless crawling (same fingerprint!)
dig2crawl discover https://yandex.cloud/ru/prices \
    --goal "Extract cloud VM pricing" \
    --fingerprint russian.json
```

The `--fingerprint` flag ensures auth and headless sessions share the same browser fingerprint. Without it, default fingerprint (en-US) is used for both.

Under the hood, `auth` calls `dig2browser::cookies::open_auth_session_with_locale()` which launches Chrome with the same stealth args as headless mode (`LaunchConfig::build_args()`). The profile directory is passed via `--user-data-dir`.

This pattern comes from `daemon4russian-parser` (Yandex Maps enrichment daemon) where the same two-step flow is used: visible browser for initial auth, then `StealthBrowser` headless reuse with the persistent profile.

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
