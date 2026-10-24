use crate::{seo::SeoInput, Engine, Kind, Window};
use anyhow::{anyhow, Result};
use serde_json::{json, to_value, Value};

pub fn definitions() -> Value {
    let window = json!({"type": "string", "enum": ["any", "hour", "today", "week", "month", "year"], "description": "Upload date window"});
    let kind = json!({"type": "string", "enum": ["any", "video", "short"], "description": "Long videos, Shorts or both"});
    json!([
        {"name": "trending", "description": "What is blowing up right now in a niche: fresh videos or Shorts for one or more topics, ranked by views per hour, with outlier scores (views vs the channel's median) and the hottest title topics.",
         "inputSchema": {"type": "object", "properties": {"queries": {"type": "array", "items": {"type": "string"}, "description": "Niche topics, e.g. [\"roblox game dev\", \"roblox update\"]"}, "window": window, "kind": kind, "limit": {"type": "integer", "default": 40}}, "required": ["queries"]}},
        {"name": "outliers", "description": "Videos for a search that beat their own channel's median views the most (viral outliers worth copying the format of).",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "window": window, "kind": kind}, "required": ["query"]}},
        {"name": "keyword_research", "description": "Related keywords from YouTube autocomplete, each scored for search volume, competition and overall opportunity (0-100).",
         "inputSchema": {"type": "object", "properties": {"seed": {"type": "string"}, "limit": {"type": "integer", "default": 20}, "deep": {"type": "boolean", "default": false, "description": "Also expand seed + a..z"}}, "required": ["seed"]}},
        {"name": "keyword_score", "description": "Volume, competition and overall score (0-100) for one keyword, with the evidence behind it.",
         "inputSchema": {"type": "object", "properties": {"keyword": {"type": "string"}}, "required": ["keyword"]}},
        {"name": "video_report", "description": "Full scorecard for a video: views per hour, engagement, outlier score vs the channel, tags and where it ranks for them, SEO score.",
         "inputSchema": {"type": "object", "properties": {"video": {"type": "string", "description": "Video URL or id"}}, "required": ["video"]}},
        {"name": "channel_audit", "description": "Channel stats: subscribers, median/average views, upload rate, every recent video with its outlier score, most used tags.",
         "inputSchema": {"type": "object", "properties": {"channel": {"type": "string", "description": "@handle, channel URL or UC... id"}}, "required": ["channel"]}},
        {"name": "tag_suggestions", "description": "Tags the top-ranking videos for a keyword use, weighted by their rank.",
         "inputSchema": {"type": "object", "properties": {"keyword": {"type": "string"}, "limit": {"type": "integer", "default": 30}}, "required": ["keyword"]}},
        {"name": "seo_check", "description": "SEO score (0-100) and checklist for a title, description and tags against a target keyword.",
         "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "description": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}, "keyword": {"type": "string"}}, "required": ["title"]}},
        {"name": "search", "description": "Plain YouTube search results with views, age and views per hour.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "window": window, "kind": kind}, "required": ["query"]}},
        {"name": "autocomplete", "description": "YouTube search autocomplete suggestions for a phrase.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}
    ])
}

pub async fn call(engine: &Engine, name: &str, args: &Value) -> Result<Value> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| anyhow!("missing argument: {key}"));
    let number = |key: &str, default: usize| args.get(key).and_then(Value::as_u64).map_or(default, |v| v as usize);
    let window: Window = serde_json::from_value(args.get("window").cloned().unwrap_or(json!("any")))?;
    let kind: Kind = serde_json::from_value(args.get("kind").cloned().unwrap_or(json!("any")))?;
    Ok(match name {
        "trending" => {
            let queries: Vec<String> = serde_json::from_value(args.get("queries").cloned().ok_or_else(|| anyhow!("missing argument: queries"))?)?;
            to_value(engine.trending(&queries, window, kind, number("limit", 40).min(100)).await?)?
        }
        "outliers" => to_value(engine.outliers(text("query")?, window, kind).await?)?,
        "keyword_research" => to_value(engine.keyword_research(text("seed")?, number("limit", 20).min(60), args.get("deep").and_then(Value::as_bool).unwrap_or(false)).await?)?,
        "keyword_score" => to_value(engine.keyword_score(text("keyword")?).await?)?,
        "video_report" => to_value(engine.video_report(text("video")?).await?)?,
        "channel_audit" => to_value(engine.channel_audit(text("channel")?).await?)?,
        "tag_suggestions" => to_value(engine.tag_suggestions(text("keyword")?, number("limit", 30)).await?)?,
        "seo_check" => to_value(engine.seo_check(serde_json::from_value::<SeoInput>(args.clone())?).await?)?,
        "search" => to_value(engine.yt.search_filtered(text("query")?, window, kind, 1).await?)?,
        "autocomplete" => to_value(engine.yt.autocomplete(text("query")?).await?)?,
        other => return Err(anyhow!("unknown tool: {other}")),
    })
}
