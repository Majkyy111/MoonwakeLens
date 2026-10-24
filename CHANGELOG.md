# Changelog

## 0.4.0 - 2026-10-24

Scores are now measured where possible and checked against outcomes.

- Fixed: competition no longer counts demand, so popular keywords no longer look crowded by default. It now uses channel strength (median views per subscriber of the top results) next to title match and verified channels.
- Fixed: outlier baselines leave out uploads younger than 7 days (Shorts 3 days), which used to inflate outlier scores.
- All score weights moved into `config.json` (`%APPDATA%\MoonwakeLens`), next to a configurable YouTube client version, region, Google Trends anchor and tracking interval.
- New local SQLite database of view snapshots; channels and videos can be tracked and are re-checked every 4 hours (in the app, or `MoonwakeLens.exe track` from Task Scheduler).
- Views per hour is measured over the last 24 hours once snapshots exist (labelled 24h vs avg), with acceleration; Blowing up uses it.
- Same-age outlier score: views compared with the channel's other uploads at the same age.
- Keyword scores are stored daily and show whether a keyword is rising or falling.
- Google Trends (YouTube Search, unofficial) adds interest and 90-day direction to volume; cached for a day and skipped when it fails.
- Calibrate: fit the volume weights to our search-terms report and see rank correlation and error before, after and leave-one-out.
- Backtest: save scores before upload and compare them with the video's real 7-day views.
- Every keyword and outlier score carries a confidence level and sample size, with ranges when the sample is under 10.
- Updates are signed (Ed25519 + SHA-256), verified before install, and only install when you click Update; MCP and serve modes never update themselves.
- YouTube rate limits get retried with backoff; consent pages, bot checks and format changes are reported as errors. CI and a daily workflow run live smoke tests against YouTube.

## 0.3.0 - 2026-10-24

- Search-term stats (keyword in description, captions, related keywords), channel overview (total views, views per video, country, join date), channel tags in the video scorecard, controversial-keyword check.

## 0.2.1 - 2026-10-24

- Table actions stay on one line.

## 0.2.0 - 2026-10-24

- Discover presets, search-term stats with top channels, CSV export.

## 0.1.0 - 2026-10-24

- First release: trending, outliers, keyword research, video scorecard, channel audit, tags, SEO check, MCP server, self-update.
