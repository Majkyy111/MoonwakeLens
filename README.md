# MoonwakeLens

YouTube research for our own videos: what is trending in a niche right now, which videos beat their channel's normal views, which keywords people search, and how a video or channel performs. Everything comes from public YouTube data, with no logins, no API keys and no usage limits. Private data (like our search-terms report) only ever arrives as a file you give it.

MoonwakeLens is a private tool. It never asks for Google login data and is not part of the public Moonwake Studio.

## What it does

| Tool | What you get |
|---|---|
| Trending | Fresh videos or Shorts for your topics (today / week / month / year), ranked by views per hour, with outlier scores and the hottest words in their titles |
| Discover | Presets for any search: Outliers, Hidden gems (small channels), Underdogs (views per subscriber), Blowing up (growth × acceleration), Trending Shorts, Today's top, Evergreens, Recent uploads |
| Keyword research | Autocomplete keywords scored 0-100 for volume, competition and overall opportunity, with confidence, ranges for small samples and a rising/falling trend; per keyword: its inputs, Google Trends direction, search-term stats, top channels, related keywords and daily history |
| Video scorecard | Views per hour (measured or lifetime), acceleration, likes, comments, engagement, outlier and same-age outlier scores, tag rankings, channel tags, SEO score, controversial keywords |
| Channel audit | Subscribers, total views, views per video, country, join date, median views for videos and Shorts, upload rate, best recent uploads, most used tags |
| Tag suggestions | Tags the top-ranking videos for a keyword use, weighted by rank |
| SEO check | Score and checklist for a title, description and tags before upload, including words advertisers avoid |
| Tracking | Channels and videos re-checked every few hours; their view snapshots power the measured numbers below |
| Backtest | Scores saved before upload, compared with the video's real 7-day views |
| Calibrate | Fits the keyword volume weights to our real search-term views and reports whether that helped |

Every result list can be downloaded as CSV.

## Keyword scores: what is measured and what is estimated

**Measured** (read straight from YouTube or from our own snapshots):
- Views, likes, comments, upload age, subscribers and search-result facts (keyword in title or description, captions, uploads in the last 7 days, verified channels).
- **Views per hour, last 24 h**: the views a video gained between two snapshots about 24 hours apart. Shown as "24h". Until a video has snapshots, the lifetime average is shown instead, labelled "avg".
- **Acceleration**: last-24-hour speed ÷ lifetime average speed.
- **Outlier score**: views ÷ the median views of the channel's recent uploads of the same kind. The baseline leaves out uploads younger than 7 days (Shorts: 3 days), because those haven't finished growing. Under 10 baseline videos it is shown as a range (views ÷ upper quartile … views ÷ lower quartile) with low confidence.
- **Same-age outlier**: views ÷ the median views the channel's other uploads had at the same age, read from snapshots. Appears once tracking has collected enough history.
- **Keyword history**: each keyword's scores are stored once a day; the arrow compares today with about a week ago.

**Estimated** (our own formulas; the weights live in `%APPDATA%\MoonwakeLens\config.json`):
- **Volume** = weighted mix of *demand* (median views of the top 20 results, log scale), *autocomplete strength* (how short a prefix makes YouTube suggest the phrase) and *Google Trends* (YouTube Search interest relative to an anchor term, adjusted by its own 90-day direction; unofficial endpoint, cached for a day, skipped when it fails).
- **Competition** = keyword in top titles + share of verified channels + *channel strength*. Channel strength comes from the median views-per-subscriber of the top results: big views on small channels means the spot is beatable, small views on big channels means it isn't. Demand is no longer part of competition.
- **Overall** = volume and (100 − competition) combined.
- **Confidence**: high with 15+ top results and subscriber data, medium with 10–14, low under 10. Under 10 results the scores come with a 10th–90th percentile range from resampling the results.

**Calibration** turns volume from a guess into a fit: give the Calibrate view (or the `calibrate` tool) our YouTube Analytics search-terms report (`search_term, views, impressions, period` as CSV or JSON). It fits volume to log views with least squares and reports the rank correlation and typical error before, after on the same data, and after leave-one-out (the honest number). Only save the weights when leave-one-out beats "before". Once calibrated, keyword scores also show an estimated number of search views. `examples/sample-search-terms.csv` shows the format; its numbers are made up.

**Backtesting** checks the whole thing: save a prediction before uploading, link the video, and after 7 days the Backtest view shows how well overall, volume and SEO scores ranked our uploads by real views.

## Install and use

Download `MoonwakeLens.exe` from the latest [release](https://github.com/Majkyy111/MoonwakeLens/releases/) and run it. It opens its own window (Microsoft Edge app mode) on `http://127.0.0.1:47821` and closes its server when the window closes. Its data (snapshots, keyword history, predictions, settings) lives in `%APPDATA%\MoonwakeLens`.

```
MoonwakeLens.exe          open the app window
MoonwakeLens.exe serve    run only the local HTTP API
MoonwakeLens.exe mcp      run as an MCP server over stdio
MoonwakeLens.exe track    re-check tracked channels and videos if a pass is due, then exit
```

Tracking runs while the app is open. To keep it running when the app is closed, schedule `MoonwakeLens.exe track` hourly (it only does work when a pass is due):

```
schtasks /Create /TN "MoonwakeLens tracker" /TR "\"D:\MoonwakeLens\MoonwakeLens.exe\" track" /SC HOURLY /F
```

### Updates

The sidebar shows **Update to vX** when a newer release exists. Updates only install when you click it: the app downloads the new exe with its SHA-256 and Ed25519 signature, verifies both against the release key built into the app, swaps the exe and restarts. MCP and `serve` modes never update themselves.

The signing key is the `SIGNING_KEY` Actions secret (an Ed25519 seed in base64); the matching public key is `RELEASE_PUBLIC_KEY` in `crates/moonwakelens/src/update.rs`. If the secret is ever lost, generate a new pair, update both, and reinstall the app by hand once, because older copies only accept the old key.

## Use it from other apps

- **Agents (MCP):** register `MoonwakeLens.exe mcp` as a stdio MCP server. Tools: `trending`, `discover`, `keyword_research`, `keyword_score`, `video_report`, `channel_audit`, `tag_suggestions`, `seo_check`, `search`, `autocomplete`, `track`, `untrack`, `tracked`, `tracking_pass`, `predict`, `link_prediction`, `backtest`, `calibrate`, `settings`.
- **Any language (HTTP):** with the app or `serve` running, `POST http://127.0.0.1:47821/api/<tool>` with the tool's arguments as JSON. `GET /api/tools` lists the tools and their input schemas.
- **Rust:** depend on the engine crate and call it directly.

```toml
moonwakelens-core = { git = "https://github.com/Majkyy111/MoonwakeLens" }
```

```rust
use moonwakelens_core::{Config, Engine, Kind, Window};

let engine = Engine::new(Config::default());
let report = engine.trending(&["roblox game dev".into()], Window::Week, Kind::Short, 40).await?;
```

`Engine::new` keeps everything in memory. `Engine::open(dir)` adds the snapshot database and `config.json` in `dir`, which tracking, keyword history, backtests and calibration need.

## Configuration

`%APPDATA%\MoonwakeLens\config.json` is created with defaults on first run:
- `client_version`: the YouTube web client version sent with requests. Update it if YouTube starts rejecting requests.
- `hl`, `gl`: language and region for YouTube and Google Trends.
- `trends_anchor`: the term Google Trends interest is compared against (default `minecraft`).
- `tracking_interval_hours`: how often tracked channels and videos are re-checked (default 4).
- `weights`: every score weight; Calibrate rewrites the volume part.

When YouTube rate-limits (HTTP 429/503) MoonwakeLens backs off and retries, then reports it. Consent pages, bot checks and unexpected formats are reported as errors instead of empty results.

## Develop

```
cargo test                            unit tests
cargo test -- --include-ignored       plus live YouTube smoke tests
cargo build --release
```

`crates/moonwakelens-core` is the engine (YouTube client, Google Trends, metrics, keyword scoring, store, tracking, calibration, backtests, tool registry). `crates/moonwakelens` is the Windows app (window, HTTP API, MCP, signed updates, `ui/index.html`).

Releases are automatic: bump `version` in the root `Cargo.toml`, add a `CHANGELOG.md` entry and push to `main`. GitHub Actions runs the unit and smoke tests, builds, signs and publishes `vX.Y.Z`. A daily workflow runs the smoke tests against a fixed video and channel, so a YouTube format change shows up as a failed check even without a push.
