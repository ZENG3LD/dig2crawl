# dig2crawl

Universal agnostic web crawler with multi-level AI extraction.

Given a URL and a natural-language goal, the crawler auto-escalates through 4 extraction levels — CSS selectors → browser interactions → visual (screenshot + Claude Vision) → captcha — until enough data is extracted.

## What it does

- **L1 — CSS selectors**: Claude reads raw HTML, discovers container + field selectors, validates extraction, produces a reusable `SiteProfile`
- **L2 — Interactive**: When L1 yields too few records, Claude suggests browser actions (clicks, scrolls, dismiss overlays) and the crawler executes them via dig2browser, then re-extracts
- **L3 — Visual**: When L2 still falls short, takes a screenshot, sends it to Claude Vision for coordinate-based actions (click at x,y), executes them, re-extracts
- **L4 — Captcha**: Architecture stub with `CaptchaSolver` trait — not implemented unless forced by life
- **Auto-navigation** — if Claude lands on a homepage/landing without data, it finds the right sub-page (e.g. `/vps/`) and redirects automatically (up to 2 hops)
- **SPA JSON extraction** — detects `__NEXT_DATA__` (Next.js), `__NUXT_DATA__` (Nuxt), `window.__NUXT__`, `window.__INITIAL_STATE__` before HTML stripping removes `<script>` tags; Claude can return `json_path` extraction mode instead of CSS selectors
- Detects anti-bot protection, extracts JSON-LD and page metadata as bonus context
- Saves a `SiteProfile` and exports a `DaemonSpec` for scheduled monitoring

## Architecture

Single-crate layout — all modules live under `src/`:

```
dig2crawl/
├── src/
│   ├── main.rs    — CLI binary (clap) + escalation coordinator
│   ├── lib.rs     — Library root
│   ├── core/      — Types, traits, error types, rate limiter, engine
│   ├── fetch/     — HttpFetcher, BrowserFetcher, interactive action executor
│   ├── agent/     — AgentSession, prompts, protocol, actions, visual, captcha
│   ├── parser/    — SelectorExtractor, JsonLdExtractor, AntiBotDetector, MetadataExtractor, LinkExtractor, SpaJsonExtractor
│   ├── config/    — TOML job config, SiteProfile, DaemonSpec serialisation
│   └── storage/   — SQLite + JSONL output backends
```

## How it works

Discovery runs in 5 steps + escalation:

1. **Fetch** — page is fetched via HTTP or headless browser; anti-bot check runs immediately
2. **Context extraction** — SPA JSON blocks (`__NEXT_DATA__`, `__NUXT_DATA__`) extracted before HTML cleaning; JSON-LD and page metadata injected as bonus context
3. **Auto-navigation** — if Claude determines the page is a homepage without target data, it returns a `navigate` response with the target URL; the crawler fetches the new page and restarts discovery (max 2 hops)
4. **Discovery (L1)** — Claude reads `page.html` from disk, analyses the DOM, writes selectors + confidence score; for SPA sites it may return `json_path` extraction mode instead of CSS selectors
4. **Validation** — the pure-Rust `SelectorExtractor` applies discovered selectors; Claude reviews the sample
5. **Save** — `SiteProfile` is written to `output/<domain>/profile.json`

**Escalation** — if L1 yields fewer records than `--min-records` or confidence below `--min-confidence`:

6. **L2 Interactive** — Claude suggests `BrowserAction`s (Click, ScrollTo, DismissOverlay, Type, WaitForElement, etc.), the crawler executes them on a live browser page, then re-extracts with the same selectors
7. **L3 Visual** — a screenshot is taken and sent to Claude Vision; Claude responds with coordinate-based `VisualAction`s (click at x,y), which are converted to browser actions and executed
8. **L4 Captcha** — if anti-bot is detected and `--max-level 4`, prints a warning (solver trait exists but is not implemented)

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

# Discover with escalation control
dig2crawl discover <url> --goal "..." --max-level 2          # stop at L2 (no visual)
dig2crawl discover <url> --goal "..." --min-records 5         # escalate until 5+ records
dig2crawl discover <url> --goal "..." --min-confidence 0.8    # escalate until 80% confidence

# Extract data using a saved profile (pure Rust, no agent)
dig2crawl extract <url> --profile output/<domain>/profile.json
dig2crawl extract <url> --profile output/<domain>/profile.json --max-pages 5 --output records.jsonl

# Export a DaemonSpec for scheduled monitoring
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.json
dig2crawl export-spec output/<domain>/profile.json --schedule "0 6 * * *" --output spec.toml

# Cookie auth — separate binary, see below
cookie-auth <url>
cookie-auth <url> --profile %TEMP%/custom-profile     # explicit profile path
cookie-auth <url> --fingerprint russian.json           # custom fingerprint for auth

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
- `--model <MODEL>` — Claude model to use (default: `claude-sonnet-4-6`)
- `--max-level <N>` — maximum extraction level: 1=CSS, 2=interactive, 3=visual (default: 3)
- `--min-records <N>` — minimum records before considering L1 successful (default: 1)
- `--min-confidence <F>` — minimum confidence threshold (default: 0.5)

## Cookie auth (`cookie-auth` binary)

Separate binary for sites behind captcha or login walls (e.g. Yandex SmartCaptcha). Opens a visible browser where you log in manually, saves cookies to a persistent profile reused by subsequent `discover`/`extract`/`fetch` commands.

```bash
# Build
cargo build --release --bin cookie-auth

# Step 1: Open browser, pass captcha, close the window
# Profile auto-created at %TEMP%/dig2crawl-profiles/yandex.cloud/
cookie-auth https://yandex.cloud/ru/prices --fingerprint russian.json

# Step 2: Use the saved profile for headless crawling (same fingerprint!)
dig2crawl discover https://yandex.cloud/ru/prices \
    --goal "Extract cloud VM pricing" \
    --fingerprint russian.json
```

| Flag | Description |
|------|-------------|
| `--fingerprint <PATH>` | JSON fingerprint config (browser, locale) |
| `--profile <PATH>` | Explicit profile directory (default: `%TEMP%/dig2crawl-profiles/<domain>/`) |

The `--fingerprint` flag ensures auth and headless sessions share the same browser fingerprint. Without it, default fingerprint (en-US) is used for both.

Under the hood, `cookie-auth` calls `dig2browser::cookies::open_auth_session_with_locale()` which launches the browser with the same stealth args as headless mode. The profile directory is passed via `--user-data-dir`.

### Browser testing (`dev-fetch` in dig2browser)

For quick browser testing without the full crawler, use `dev-fetch` from dig2browser — DevTools in your terminal:

```bash
cargo install dig2browser
dev-fetch https://cloud.vk.com/pricing --fingerprint russian.json --network-log --cookies --save-html out.html
```

See [dig2browser README](https://github.com/ZENG3LD/dig2browser#cli-tools) for full flag reference.

## Agent internals

`AgentSession` wraps [gate4agent](https://crates.io/crates/gate4agent) `PipeSession` — the same library used by `agent2overlay`.

**Pipe mode** — Claude CLI runs in headless NDJSON-streaming mode:

- Prompts are delivered via **stdin** (no file intermediary, no cmd.exe argument length limits)
- Claude responds via **stdout** as NDJSON `stream-json` events (`PipeText`, `PipeToolStart`, `PipeSessionEnd`, etc.)
- `--resume <session_id>` is captured from `PipeSessionStart` events and passed on subsequent calls — L1 → validation → L2 → L3 all share the **same conversational context**
- `--dangerously-skip-permissions` enables tool use (Read, Grep, Bash) in one-shot mode
- Claude reads HTML/screenshot files referenced in the prompt via its Read tool (with `offset`/`limit` for large files)

**HTML cleaning** — before saving to disk, `<script>`, `<style>`, `<svg>`, `<noscript>` tags are stripped and whitespace collapsed (typically 70-90% size reduction). Claude reads the cleaned file in chunks if needed.

**Temp directory layout during a `discover` run:**

```
%TEMP%/dig2crawl_<pid>/
├── page.html              — cleaned fetched HTML (scripts/styles stripped)
├── spa_data.json           — extracted SPA JSON (if __NEXT_DATA__ / __NUXT_DATA__ found)
├── l2_page.html           — post-action HTML (if L2 escalated)
└── l3_screenshot.png      — page screenshot (if L3 escalated)
```

The directory is deleted when the session closes.

**Robust JSON parsing** — the agent protocol tolerates several classes of model output variation:

- Regex escape sequences in `transform` patterns (e.g. `\\d+`, `\\\s*`) sanitized — the state machine correctly consumes both characters of valid JSON escapes (`\\`, `\"`) before inspecting the next character
- `null` selectors for fields absent from the page (skipped gracefully, not treated as errors)
- Untagged `next_urls` values — accepted as either a plain string or a `{"url": "..."}` object
- Mixed-type `url_patterns` and field specs — arrays and scalars both accepted at every field

## Batch crawl results

Tested against VPS hosting providers with auto-navigation and SPA extraction (v0.3.19):

| Provider | Confidence | Records | Notes |
|----------|-----------|---------|-------|
| serverspace.io | 0.88 | 9 | L1 CSS selectors |
| ruvds.com | 0.93 | 3 | L1, browser mode |
| 4dedic.io | 0.92 | 4233 | L1, large catalog |
| robovps.biz | 0.91 | 7 | Auto-nav homepage → /vps/ |
| ishosting.com | 0.92 | 6 | Auto-nav ×2, through Cloudflare |
| timeweb.cloud | 0.90 | 4 | L3 visual (Nuxt SPA) |
| adminvps.ru | 0.88 | — | L1 CSS selectors |

Auto-navigation successfully finds VPS pricing pages from homepages. SPA sites (Next.js, Nuxt) are handled via embedded JSON extraction or L3 visual fallback.

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
| gate4agent | Claude CLI pipe session with NDJSON streaming — crates.io |
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
