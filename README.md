# MoonwakeLens

YouTube research for our own videos: what is trending in a niche right now, which videos beat their channel's normal views, which keywords people search, and how a video or channel performs. Everything comes from public YouTube data, with no API key and no usage limits.

## What it does

| Tool | What you get |
|---|---|
| Trending | Fresh videos or Shorts for your topics (today / week / month / year), ranked by views per hour, with outlier scores and the hottest words in their titles |
| Outliers | Videos for a search that beat their own channel's median views the most |
| Keyword research | Autocomplete keywords scored 0-100 for volume, competition and overall opportunity |
| Video scorecard | Views per hour, likes, comments, engagement, outlier score, tag rankings, SEO score |
| Channel audit | Subscribers, median views for videos and Shorts, upload rate, best recent uploads, most used tags |
| Tag suggestions | Tags the top-ranking videos for a keyword use, weighted by rank |
| SEO check | Score and checklist for a title, description and tags before upload |

Outlier score = a video's views divided by the median views of its channel's recent uploads of the same kind (Shorts are compared with Shorts). Keyword scores are our own heuristics: volume combines how early YouTube autocomplete suggests the phrase with the median views of the top results; competition combines how many top results use the phrase in the title, how many come from verified channels and how strong those results are.

## Install and use

Download `MoonwakeLens.exe` from the latest [release](https://github.com/Majkyy111/MoonwakeLens/releases/latest) and run it. It opens its own window (Microsoft Edge app mode) on `http://127.0.0.1:47821` and closes its server when the window closes.

It updates itself: on every start it checks the latest GitHub release and swaps in the new version. The Update button in the sidebar does the same while it is open.

```
MoonwakeLens.exe          open the app window
MoonwakeLens.exe serve    run only the local HTTP API
MoonwakeLens.exe mcp      run as an MCP server over stdio
```

## Use it from other apps

- **Agents (MCP):** register `MoonwakeLens.exe mcp` as a stdio MCP server. Tools: `trending`, `outliers`, `keyword_research`, `keyword_score`, `video_report`, `channel_audit`, `tag_suggestions`, `seo_check`, `search`, `autocomplete`.
- **Any language (HTTP):** with the app or `serve` running, `POST http://127.0.0.1:47821/api/<tool>` with the tool's arguments as JSON. `GET /api/tools` lists the tools and their input schemas.
- **Rust:** depend on the engine crate and call it directly.

```toml
moonwakelens-core = { git = "https://github.com/Majkyy111/MoonwakeLens" }
```

```rust
let engine = moonwakelens_core::Engine::new("en", "US");
let report = engine.trending(&["roblox game dev".into()], moonwakelens_core::Window::Week, moonwakelens_core::Kind::Short, 40).await?;
```

## Develop

```
cargo test
cargo build --release
```

`crates/moonwakelens-core` is the engine (YouTube client, metrics, analysis, tool registry). `crates/moonwakelens` is the Windows app (window, HTTP API, MCP, self-update, `ui/index.html`).

Releases are automatic: bump `version` in the root `Cargo.toml` and push to `main`. GitHub Actions tests, builds and publishes `vX.Y.Z` with `MoonwakeLens.exe`, and every installed copy updates itself on its next start.
