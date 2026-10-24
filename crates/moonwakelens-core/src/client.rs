use crate::model::{ChannelInfo, SearchPage, VideoDetails, VideoSummary};
use crate::parse::*;
use anyhow::{anyhow, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

const CLIENT_VERSION: &str = "2.20261020.00.00";
const VIDEOS_TAB: &str = "EgZ2aWRlb3PyBgQKAjoA";
const SHORTS_TAB: &str = "EgZzaG9ydHPyBgUKA5oBAA==";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";
const MAX_IN_FLIGHT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    #[default]
    Any,
    Hour,
    Today,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Any,
    Video,
    Short,
}

fn search_params(window: Window, kind: Kind) -> Option<String> {
    let date = match window {
        Window::Any => None,
        Window::Hour => Some(1),
        Window::Today => Some(2),
        Window::Week => Some(3),
        Window::Month => Some(4),
        Window::Year => Some(5),
    };
    let kind_code = match kind {
        Kind::Any => None,
        Kind::Video => Some(1),
        Kind::Short => Some(9),
    };
    let mut filters = Vec::new();
    if let Some(d) = date {
        filters.extend([0x08, d]);
    }
    if let Some(k) = kind_code {
        filters.extend([0x10, k]);
    }
    let mut bytes = Vec::new();
    if kind == Kind::Video && date.is_some() {
        bytes.extend([0x08, 0x03]);
    }
    if !filters.is_empty() {
        bytes.extend([0x12, filters.len() as u8]);
        bytes.extend(filters);
    }
    (!bytes.is_empty()).then(|| base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub struct YouTube {
    http: reqwest::Client,
    hl: String,
    gl: String,
    cache: Mutex<HashMap<String, (Instant, Arc<Value>)>>,
    ttl: Duration,
    in_flight: Semaphore,
}

impl YouTube {
    pub fn new(hl: &str, gl: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(20))
                .build()
                .expect("http client"),
            hl: hl.into(),
            gl: gl.into(),
            cache: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(600),
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    fn cached(&self, key: &str) -> Option<Arc<Value>> {
        let cache = self.cache.lock().unwrap();
        cache.get(key).filter(|(at, _)| at.elapsed() < self.ttl).map(|(_, v)| v.clone())
    }

    fn store(&self, key: String, value: Value) -> Arc<Value> {
        let value = Arc::new(value);
        let mut cache = self.cache.lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < self.ttl);
        cache.insert(key, (Instant::now(), value.clone()));
        value
    }

    async fn innertube(&self, endpoint: &str, mut body: Value) -> Result<Arc<Value>> {
        body["context"] = json!({"client": {"clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": self.hl, "gl": self.gl}});
        let key = format!("{endpoint}|{body}");
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let _permit = self.in_flight.acquire().await?;
        let value: Value = self
            .http
            .post(format!("https://www.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false"))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(self.store(key, value))
    }

    pub async fn autocomplete(&self, query: &str) -> Result<Vec<String>> {
        let key = format!("suggest|{}|{}|{query}", self.hl, self.gl);
        let value = match self.cached(&key) {
            Some(hit) => hit,
            None => {
                let _permit = self.in_flight.acquire().await?;
                let raw = self
                    .http
                    .get("https://suggestqueries-clients6.youtube.com/complete/search")
                    .query(&[("client", "youtube"), ("ds", "yt"), ("hl", self.hl.as_str()), ("gl", self.gl.as_str()), ("q", query)])
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                let (start, end) = raw.find('(').zip(raw.rfind(')')).ok_or_else(|| anyhow!("unexpected autocomplete response"))?;
                self.store(key, serde_json::from_str(&raw[start + 1..end])?)
            }
        };
        Ok(value
            .get(1)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|i| i.get(0)?.as_str().map(String::from)).collect())
            .unwrap_or_default())
    }

    pub async fn search(&self, query: &str) -> Result<SearchPage> {
        self.search_filtered(query, Window::Any, Kind::Any, 1).await
    }

    pub async fn search_filtered(&self, query: &str, window: Window, kind: Kind, pages: usize) -> Result<SearchPage> {
        let mut body = json!({"query": query});
        if let Some(params) = search_params(window, kind) {
            body["params"] = json!(params);
        }
        let first_page = self.innertube("search", body).await?;
        let estimated_results = first_page
            .get("estimatedResults")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            .unwrap_or(0);
        let mut results = Vec::new();
        let mut page = first_page;
        for page_index in 0..pages.max(1) {
            for summary in search_results(&page) {
                if !results.iter().any(|r: &VideoSummary| r.id == summary.id) {
                    results.push(summary);
                }
            }
            let token = all(&page, "continuationItemRenderer").last().and_then(|c| first_str(c, "token"));
            match token {
                Some(token) if page_index + 1 < pages => page = self.innertube("search", json!({"continuation": token})).await?,
                _ => break,
            }
        }
        Ok(SearchPage { query: query.into(), estimated_results, results })
    }

    pub async fn video_basic(&self, id: &str) -> Result<VideoDetails> {
        details_from_player(&*self.innertube("player", json!({"videoId": id})).await?)
    }

    pub async fn video(&self, id: &str) -> Result<VideoDetails> {
        let (player, next) = futures::try_join!(
            self.innertube("player", json!({"videoId": id})),
            self.innertube("next", json!({"videoId": id}))
        )?;
        let mut details = details_from_player(&player)?;
        details.likes = likes_from_next(&next);
        details.comments = comments_from_next(&next);
        details.has_chapters |= all(&next, "panelIdentifier").iter().any(|p| p.as_str().is_some_and(|s| s.contains("chapters")));
        Ok(details)
    }

    pub async fn channel_id(&self, input: &str) -> Result<String> {
        let s = input.trim();
        if s.starts_with("UC") && s.len() == 24 {
            return Ok(s.into());
        }
        if let Some(i) = s.find("/channel/") {
            return Ok(s[i + 9..].chars().take(24).collect());
        }
        let url = if s.starts_with("http") {
            s.to_string()
        } else {
            format!("https://www.youtube.com/@{}", s.trim_start_matches('@'))
        };
        let resolved = self.innertube("navigation/resolve_url", json!({"url": url})).await?;
        first_str(&resolved, "browseId").ok_or_else(|| anyhow!("channel not found: {input}"))
    }

    pub async fn channel(&self, input: &str, max_videos: usize) -> Result<ChannelInfo> {
        let id = self.channel_id(input).await?;
        let (home, videos_tab, shorts_tab) = futures::try_join!(
            self.innertube("browse", json!({"browseId": id})),
            self.innertube("browse", json!({"browseId": id, "params": VIDEOS_TAB})),
            self.innertube("browse", json!({"browseId": id, "params": SHORTS_TAB}))
        )?;
        let meta = home.pointer("/metadata/channelMetadataRenderer").ok_or_else(|| anyhow!("channel not found: {input}"))?;
        let field = |k: &str| meta.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let name = field("title");
        let header_texts: Vec<&str> = home.get("header").map(|h| all(h, "content")).unwrap_or_default().into_iter().filter_map(Value::as_str).collect();
        let header_count = |suffix: &str| header_texts.iter().find(|t| t.ends_with(suffix)).and_then(|t| parse_count(t));

        let (recent_videos, recent_shorts) = futures::try_join!(
            self.paged(videos_tab, max_videos, |page| channel_videos(page, &id, &name)),
            self.paged(shorts_tab, max_videos, |page| channel_shorts(page, &id, &name))
        )?;

        Ok(ChannelInfo {
            handle: field("vanityChannelUrl").rsplit('/').next().filter(|h| h.starts_with('@')).map(String::from),
            description: field("description"),
            keywords: split_keywords(&field("keywords")),
            subscribers: header_count("subscribers"),
            video_count: header_count("videos"),
            id,
            name,
            recent_videos,
            recent_shorts,
        })
    }

    async fn paged(&self, first_page: Arc<Value>, max: usize, parse: impl Fn(&Value) -> Vec<VideoSummary>) -> Result<Vec<VideoSummary>> {
        let mut items = parse(&first_page);
        let mut page = first_page;
        while items.len() < max {
            let Some(token) = all(&page, "continuationItemRenderer").last().and_then(|c| first_str(c, "token")) else { break };
            page = self.innertube("browse", json!({"continuation": token})).await?;
            let more = parse(&page);
            if more.is_empty() {
                break;
            }
            items.extend(more);
        }
        items.truncate(max);
        Ok(items)
    }
}

fn channel_shorts(page: &Value, channel_id: &str, channel_name: &str) -> Vec<VideoSummary> {
    all(page, "shortsLockupViewModel")
        .into_iter()
        .filter_map(from_short_lockup)
        .map(|short| VideoSummary { channel_id: Some(channel_id.to_string()), channel_name: Some(channel_name.to_string()), ..short })
        .collect()
}

fn search_results(page: &Value) -> Vec<VideoSummary> {
    find_any(page, &["videoRenderer", "shortsLockupViewModel"])
        .into_iter()
        .filter_map(|(kind, node)| if kind == "videoRenderer" { from_video_renderer(node) } else { from_short_lockup(node) })
        .collect()
}

fn from_video_renderer(r: &Value) -> Option<VideoSummary> {
    let owner = r.get("ownerText").or_else(|| r.get("longBylineText"));
    let label = |k: &str| r.get(k).and_then(text);
    Some(
        VideoSummary {
            id: r.get("videoId")?.as_str()?.to_string(),
            title: label("title").unwrap_or_default(),
            channel_id: owner.and_then(|o| first_str(o, "browseId")),
            channel_name: owner.and_then(text),
            views: label("viewCountText").and_then(|t| parse_count(&t)),
            age_hours: label("publishedTimeText").and_then(|t| parse_age_hours(&t)),
            duration_seconds: label("lengthText").and_then(|t| parse_duration(&t)),
            is_short: false,
            verified_channel: r.get("ownerBadges").is_some_and(|b| b.to_string().contains("VERIFIED")),
            views_per_hour: None,
        }
        .with_vph(),
    )
}

fn from_short_lockup(s: &Value) -> Option<VideoSummary> {
    let id = first(s, "reelWatchEndpoint")?.get("videoId")?.as_str()?.to_string();
    let label = s.get("accessibilityText")?.as_str()?;
    let (title, views) = label.rsplit_once(", ")?;
    Some(VideoSummary {
        id,
        title: title.to_string(),
        channel_id: None,
        channel_name: None,
        views: parse_count(views),
        age_hours: None,
        duration_seconds: None,
        is_short: true,
        verified_channel: false,
        views_per_hour: None,
    })
}

fn channel_videos(page: &Value, channel_id: &str, channel_name: &str) -> Vec<VideoSummary> {
    all(page, "lockupViewModel")
        .into_iter()
        .filter(|l| l.get("contentType").and_then(Value::as_str) == Some("LOCKUP_CONTENT_TYPE_VIDEO"))
        .filter_map(|l| {
            let meta = l.pointer("/metadata/lockupMetadataViewModel")?;
            let parts: Vec<String> = all(meta, "metadataParts")
                .into_iter()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(|p| p.get("text").and_then(text))
                .collect();
            Some(
                VideoSummary {
                    id: l.get("contentId")?.as_str()?.to_string(),
                    title: meta.pointer("/title/content").and_then(Value::as_str).unwrap_or_default().to_string(),
                    channel_id: Some(channel_id.to_string()),
                    channel_name: Some(channel_name.to_string()),
                    views: parts.iter().find(|p| p.contains("view")).and_then(|p| parse_count(p)),
                    age_hours: parts.iter().find(|p| p.contains("ago")).and_then(|p| parse_age_hours(p)),
                    duration_seconds: all(l, "thumbnailBadgeViewModel").into_iter().filter_map(|b| b.get("text")?.as_str()).find_map(parse_duration),
                    is_short: false,
                    verified_channel: false,
                    views_per_hour: None,
                }
                .with_vph(),
            )
        })
        .collect()
}

fn details_from_player(player: &Value) -> Result<VideoDetails> {
    let vd = player.get("videoDetails").ok_or_else(|| anyhow!("video not found or unavailable"))?;
    let mf = player.pointer("/microformat/playerMicroformatRenderer");
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let title = s(vd, "title").unwrap_or_default();
    let description = s(vd, "shortDescription").unwrap_or_default();
    let published = mf.and_then(|m| s(m, "publishDate"));
    Ok(VideoDetails {
        id: s(vd, "videoId").unwrap_or_default(),
        channel_id: s(vd, "channelId"),
        channel_name: s(vd, "author"),
        views: s(vd, "viewCount").and_then(|v| v.parse().ok()).unwrap_or(0),
        likes: None,
        comments: None,
        age_hours: published.as_deref().and_then(hours_since),
        published,
        duration_seconds: s(vd, "lengthSeconds").and_then(|v| v.parse().ok()).unwrap_or(0),
        category: mf.and_then(|m| s(m, "category")),
        tags: vd
            .get("keywords")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        hashtags: hashtags(&format!("{title} {description}")),
        has_chapters: has_timestamps(&description),
        title,
        description,
    })
}

fn likes_from_next(next: &Value) -> Option<u64> {
    let from_label = all(next, "accessibilityText").into_iter().filter_map(Value::as_str).find_map(|label| {
        let after = label.split("along with ").nth(1)?;
        parse_count(after.split(" other").next()?)
    });
    from_label.or_else(|| {
        all(next, "factoidRenderer")
            .into_iter()
            .find(|f| f.get("label").and_then(text).is_some_and(|l| l.eq_ignore_ascii_case("likes")))
            .and_then(|f| f.get("value").and_then(text))
            .and_then(|v| parse_count(&v))
    })
}

fn comments_from_next(next: &Value) -> Option<u64> {
    all(next, "engagementPanelSectionListRenderer")
        .into_iter()
        .find(|p| p.get("panelIdentifier").and_then(Value::as_str) == Some("engagement-panel-comments-section"))
        .and_then(|p| p.pointer("/header/engagementPanelTitleHeaderRenderer/contextualInfo"))
        .and_then(text)
        .and_then(|t| parse_count(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_params_match_youtube() {
        assert_eq!(search_params(Window::Week, Kind::Video).as_deref(), Some("CAMSBAgDEAE="));
        assert_eq!(search_params(Window::Week, Kind::Short).as_deref(), Some("EgQIAxAJ"));
        assert_eq!(search_params(Window::Any, Kind::Short).as_deref(), Some("EgIQCQ=="));
        assert_eq!(search_params(Window::Any, Kind::Any), None);
    }
}
