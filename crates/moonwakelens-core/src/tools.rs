use crate::{analysis::Preset, backtest::PredictionInput, calibrate::parse_rows, seo::SeoInput, Engine, Kind, Window};
use anyhow::{anyhow, Result};
use serde_json::{json, to_value, Value};

pub fn definitions() -> Value {
    let window = json!({"type": "string", "enum": ["any", "hour", "today", "week", "month", "year"], "description": "Upload date window"});
    let kind = json!({"type": "string", "enum": ["any", "video", "short"], "description": "Long videos, Shorts or both"});
    let target = json!({"type": "string", "description": "@handle, channel link, UC... id, or a video link/id"});
    json!([
        {"name": "trending", "description": "What is blowing up right now in a niche: fresh videos or Shorts for one or more topics, ranked by views per hour (measured over the last 24 h when MoonwakeLens has snapshots, otherwise the lifetime average; see views_per_hour_basis), with outlier scores and the hottest title topics.",
         "inputSchema": {"type": "object", "properties": {"queries": {"type": "array", "items": {"type": "string"}, "description": "Niche topics, e.g. [\"roblox game dev\", \"roblox update\"]"}, "window": window, "kind": kind, "limit": {"type": "integer", "default": 40}}, "required": ["queries"]}},
        {"name": "discover", "description": "Find videos for a search with a preset: outliers (beat their channel's median views most), hidden_gems (outliers from channels under 100K subscribers), underdogs (most views per subscriber on small channels), blowing_up (fastest growth, boosted by measured acceleration), todays_top, trending_shorts, evergreens (over a year old, still getting views), recent_uploads. Outlier scores carry a confidence and a range when the channel baseline is small; same_age_outlier compares with the channel's other uploads at the same age from snapshots.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "preset": {"type": "string", "enum": ["outliers", "hidden_gems", "underdogs", "blowing_up", "todays_top", "trending_shorts", "evergreens", "recent_uploads"], "default": "outliers"}}, "required": ["query"]}},
        {"name": "keyword_research", "description": "Related keywords from YouTube autocomplete, each scored 0-100 for volume, competition and overall opportunity, with confidence, ranges for small samples and a rising/falling trend from stored daily history.",
         "inputSchema": {"type": "object", "properties": {"seed": {"type": "string"}, "limit": {"type": "integer", "default": 20}, "deep": {"type": "boolean", "default": false, "description": "Also expand seed + a..z"}}, "required": ["seed"]}},
        {"name": "keyword_score", "description": "Full score for one keyword: volume, competition and overall with their inputs (demand, autocomplete, Google Trends YouTube interest and 90-day direction, title match, verified share, channel strength), confidence, estimated search views when calibrated, search-term stats, top channels, related keywords and daily history.",
         "inputSchema": {"type": "object", "properties": {"keyword": {"type": "string"}}, "required": ["keyword"]}},
        {"name": "video_report", "description": "Scorecard for a video: measured or lifetime views per hour, acceleration, engagement, outlier score with confidence and same-age comparison, tags and where it ranks for them, channel tags, SEO score.",
         "inputSchema": {"type": "object", "properties": {"video": {"type": "string", "description": "Video URL or id"}}, "required": ["video"]}},
        {"name": "channel_audit", "description": "Channel stats: subscribers, total views, median/average views (baselines skip uploads younger than 7 days, Shorts 3 days), upload rate, every recent video and Short with outlier scores, most used tags.",
         "inputSchema": {"type": "object", "properties": {"channel": {"type": "string", "description": "@handle, channel URL or UC... id"}}, "required": ["channel"]}},
        {"name": "tag_suggestions", "description": "Tags the top-ranking videos for a keyword use, weighted by their rank.",
         "inputSchema": {"type": "object", "properties": {"keyword": {"type": "string"}, "limit": {"type": "integer", "default": 30}}, "required": ["keyword"]}},
        {"name": "seo_check", "description": "SEO score (0-100) and checklist for a title, description and tags against a target keyword, including controversial words.",
         "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "description": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}, "keyword": {"type": "string"}}, "required": ["title"]}},
        {"name": "search", "description": "Plain YouTube search results with views, age and views per hour.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "window": window, "kind": kind}, "required": ["query"]}},
        {"name": "autocomplete", "description": "YouTube search autocomplete suggestions for a phrase.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
        {"name": "track", "description": "Start tracking a channel or video: MoonwakeLens re-checks it every few hours and stores view snapshots, which power measured views per hour, same-age outliers and backtests.",
         "inputSchema": {"type": "object", "properties": {"target": target}, "required": ["target"]}},
        {"name": "untrack", "description": "Stop tracking a channel or video.", "inputSchema": {"type": "object", "properties": {"target": target}, "required": ["target"]}},
        {"name": "tracked", "description": "Everything being tracked, with last check time and snapshot counts.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "tracking_pass", "description": "Re-check all tracked channels and videos now and store snapshots.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "predict", "description": "Save the keyword and SEO scores of a video before upload, to compare with its real 7-day views later. Pass video once it exists (unlisted/scheduled is fine) or link it later.",
         "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "description": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}, "keyword": {"type": "string"}, "video": {"type": "string"}}, "required": ["title"]}},
        {"name": "link_prediction", "description": "Attach the uploaded video to a saved prediction and start tracking it.",
         "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}, "video": {"type": "string"}}, "required": ["id", "video"]}},
        {"name": "backtest", "description": "Compare saved predictions with the videos' actual views at 7 days (from snapshots) and report how well each score ranked the uploads.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "calibrate", "description": "Fit the keyword volume weights to real search-term views (YouTube Analytics search-terms report: search_term, views, impressions?, period). Give the CSV/JSON text as data or a file path. Reports ranking quality before, in-sample and leave-one-out after; apply saves the weights.",
         "inputSchema": {"type": "object", "properties": {"data": {"type": "string"}, "path": {"type": "string"}, "apply": {"type": "boolean", "default": false}, "use_trends": {"type": "boolean", "default": true}}}},
        {"name": "settings", "description": "Current configuration: YouTube client version, region, Google Trends anchor, tracking interval and score weights.", "inputSchema": {"type": "object", "properties": {}}}
    ])
}

pub async fn call(engine: &Engine, name: &str, args: &Value) -> Result<Value> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| anyhow!("missing argument: {key}"));
    let number = |key: &str, default: usize| args.get(key).and_then(Value::as_u64).map_or(default, |v| v as usize);
    let flag = |key: &str, default: bool| args.get(key).and_then(Value::as_bool).unwrap_or(default);
    let window: Window = serde_json::from_value(args.get("window").cloned().unwrap_or(json!("any")))?;
    let kind: Kind = serde_json::from_value(args.get("kind").cloned().unwrap_or(json!("any")))?;
    Ok(match name {
        "trending" => {
            let queries: Vec<String> = serde_json::from_value(args.get("queries").cloned().ok_or_else(|| anyhow!("missing argument: queries"))?)?;
            to_value(engine.trending(&queries, window, kind, number("limit", 40).min(100)).await?)?
        }
        "discover" => {
            let preset: Preset = serde_json::from_value(args.get("preset").cloned().unwrap_or(json!("outliers")))?;
            to_value(engine.discover(text("query")?, preset).await?)?
        }
        "keyword_research" => to_value(engine.keyword_research(text("seed")?, number("limit", 20).min(60), flag("deep", false)).await?)?,
        "keyword_score" => to_value(engine.keyword_score(text("keyword")?).await?)?,
        "video_report" => to_value(engine.video_report(text("video")?).await?)?,
        "channel_audit" => to_value(engine.channel_audit(text("channel")?).await?)?,
        "tag_suggestions" => to_value(engine.tag_suggestions(text("keyword")?, number("limit", 30)).await?)?,
        "seo_check" => to_value(engine.seo_check(serde_json::from_value::<SeoInput>(args.clone())?).await?)?,
        "search" => to_value(engine.yt.search_filtered(text("query")?, window, kind, 1).await?)?,
        "autocomplete" => to_value(engine.yt.autocomplete(text("query")?).await?)?,
        "track" => to_value(engine.track(text("target")?).await?)?,
        "untrack" => to_value(engine.untrack(text("target")?).await?)?,
        "tracked" => to_value(engine.tracked()?)?,
        "tracking_pass" => to_value(engine.tracking_pass().await?)?,
        "predict" => to_value(engine.predict(serde_json::from_value::<PredictionInput>(args.clone())?).await?)?,
        "link_prediction" => {
            let id = args.get("id").and_then(Value::as_i64).ok_or_else(|| anyhow!("missing argument: id"))?;
            to_value(engine.link_prediction(id, text("video")?).await?)?
        }
        "backtest" => to_value(engine.backtest()?)?,
        "calibrate" => {
            let data = match (text("data"), text("path")) {
                (Ok(data), _) => data.to_string(),
                (_, Ok(path)) => std::fs::read_to_string(path)?,
                _ => return Err(anyhow!("give the search-terms CSV/JSON as data or a file path")),
            };
            to_value(engine.calibrate(parse_rows(&data)?, flag("apply", false), flag("use_trends", true)).await?)?
        }
        "settings" => to_value(engine.config())?,
        other => return Err(anyhow!("unknown tool: {other}")),
    })
}
