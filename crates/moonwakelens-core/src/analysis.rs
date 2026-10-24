use crate::client::{Kind, Window, YouTube};
use crate::config::{Config, Weights};
use crate::metrics::*;
use crate::model::{ChannelAbout, ChannelInfo, VideoDetails, VideoSummary};
use crate::parse::{normalize, video_id};
use crate::seo::{self, SeoInput, SeoReport};
use crate::store::{Observation, Store};
use crate::trends::Trends;
use anyhow::{anyhow, Result};
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicI64;
use std::sync::RwLock;

const PARALLEL: usize = 8;
const SMALL_CHANNEL_SUBSCRIBERS: u64 = 100_000;
const STOPWORDS: &[&str] = &[
    "a", "added", "all", "an", "and", "are", "as", "at", "be", "but", "by", "can", "do", "everything", "for", "from", "fyp", "get", "got", "has", "have", "how",
    "i", "if", "in", "is", "it", "its", "just", "made", "make", "me", "my", "new", "no", "not", "now", "of", "on", "or", "our", "out", "part", "short",
    "shorts", "so", "that", "the", "this", "to", "video", "viral", "vs", "was", "we", "what", "when", "who", "why", "will", "with", "you", "your",
];

pub(crate) async fn concurrent<T, R, F, Fut>(items: Vec<T>, f: F) -> Vec<R>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = R>,
{
    stream::iter(items).map(f).buffered(PARALLEL).collect().await
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrowthBasis {
    Last24Hours,
    Lifetime,
}

#[derive(Debug, Clone, Serialize)]
pub struct RankedVideo {
    #[serde(flatten)]
    pub video: VideoSummary,
    pub views_per_hour_basis: GrowthBasis,
    pub lifetime_views_per_hour: Option<f64>,
    pub acceleration: Option<f64>,
    pub channel_subscribers: Option<u64>,
    pub channel_median_views: Option<u64>,
    pub outlier_score: Option<f64>,
    pub outlier_low: Option<f64>,
    pub outlier_high: Option<f64>,
    pub outlier_confidence: Option<Confidence>,
    pub baseline_videos: usize,
    pub same_age_outlier: Option<f64>,
    pub same_age_sample: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    #[default]
    Outliers,
    HiddenGems,
    Underdogs,
    BlowingUp,
    TodaysTop,
    TrendingShorts,
    Evergreens,
    RecentUploads,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Baseline {
    pub long: Option<Spread>,
    pub short: Option<Spread>,
    pub subscribers: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicTerm {
    pub term: String,
    pub videos: usize,
    pub views_per_hour: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrendReport {
    pub queries: Vec<String>,
    pub window: Window,
    pub kind: Kind,
    pub videos: Vec<RankedVideo>,
    pub topics: Vec<TopicTerm>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRank {
    pub tag: String,
    pub rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoReport {
    pub video: VideoDetails,
    pub performance: RankedVideo,
    pub engagement_rate: Option<f64>,
    pub like_rate: Option<f64>,
    pub comment_rate: Option<f64>,
    pub views_per_subscriber: Option<f64>,
    pub channel_tags: Vec<String>,
    pub tag_ranks: Vec<TagRank>,
    pub seo: SeoReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelAudit {
    pub channel: ChannelInfo,
    pub about: ChannelAbout,
    pub views_per_video: Option<u64>,
    pub median_views: Option<u64>,
    pub median_short_views: Option<u64>,
    pub average_views: Option<u64>,
    pub uploads_per_week: Option<f64>,
    pub last_upload_hours: Option<f64>,
    pub videos: Vec<RankedVideo>,
    pub shorts: Vec<RankedVideo>,
    pub top_tags: Vec<TagCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagSuggestion {
    pub tag: String,
    pub score: u8,
    pub used_by: usize,
    pub sampled_videos: usize,
    pub confidence: Confidence,
}

pub(crate) fn ranked(video: VideoSummary, baseline: Option<Spread>, subscribers: Option<u64>) -> RankedVideo {
    let outlier = multiple(video.views, baseline.map(|b| b.median));
    let narrow = baseline.filter(|b| b.n < RANGE_BELOW_SAMPLE);
    let lifetime = video.views_per_hour;
    RankedVideo {
        outlier_low: narrow.and_then(|b| multiple(video.views, Some(b.q3))),
        outlier_high: narrow.and_then(|b| multiple(video.views, Some(b.q1))),
        outlier_confidence: baseline.map(|b| Confidence::from_sample(b.n)),
        baseline_videos: baseline.map_or(0, |b| b.n),
        channel_median_views: baseline.map(|b| b.median),
        outlier_score: outlier,
        channel_subscribers: subscribers,
        views_per_hour_basis: GrowthBasis::Lifetime,
        lifetime_views_per_hour: lifetime,
        acceleration: None,
        same_age_outlier: None,
        same_age_sample: 0,
        video,
    }
}

fn preset_score(preset: Preset, r: &RankedVideo) -> Option<f64> {
    let small = r.channel_subscribers.is_some_and(|s| s < SMALL_CHANNEL_SUBSCRIBERS);
    let v = &r.video;
    match preset {
        Preset::Outliers => r.outlier_score,
        Preset::HiddenGems => r.outlier_score.filter(|_| small),
        Preset::Underdogs => v.views.zip(r.channel_subscribers).filter(|_| small).map(|(views, subs)| views as f64 / subs.max(1) as f64),
        Preset::BlowingUp => v.views_per_hour.map(|vph| vph * r.acceleration.unwrap_or(1.0).clamp(0.25, 4.0)),
        Preset::TrendingShorts => v.views_per_hour.filter(|_| v.is_short),
        Preset::TodaysTop => v.views.map(|views| views as f64),
        Preset::Evergreens => v.views_per_hour.filter(|_| v.age_hours.is_some_and(|a| a > 8760.0)),
        Preset::RecentUploads => v.age_hours.map(|a| -a),
    }
}

pub struct Engine {
    pub yt: YouTube,
    pub(crate) trends: Trends,
    pub(crate) trends_paused_until: AtomicI64,
    pub store: Option<Store>,
    config: RwLock<Config>,
    config_path: Option<PathBuf>,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            yt: YouTube::new(&config.hl, &config.gl, &config.client_version),
            trends: Trends::default(),
            trends_paused_until: AtomicI64::new(0),
            store: None,
            config: RwLock::new(config),
            config_path: None,
        }
    }

    pub fn open(data_dir: &Path) -> Result<Self> {
        let config_path = data_dir.join("config.json");
        let mut engine = Self::new(Config::load(&config_path)?);
        engine.store = Some(Store::open(&data_dir.join("lens.db"))?);
        engine.config_path = Some(config_path);
        Ok(engine)
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn weights(&self) -> Weights {
        self.config.read().unwrap().weights.clone()
    }

    pub(crate) fn set_weights(&self, weights: Weights) -> Result<()> {
        let mut config = self.config.write().unwrap();
        config.weights = weights;
        if let Some(path) = &self.config_path {
            config.save(path)?;
        }
        Ok(())
    }

    pub(crate) fn observe(&self, videos: &[VideoSummary]) {
        let Some(store) = &self.store else { return };
        let at = now();
        let observations: Vec<Observation> = videos
            .iter()
            .map(|v| Observation {
                video_id: v.id.clone(),
                channel_id: v.channel_id.clone(),
                title: v.title.clone(),
                is_short: v.is_short,
                published_at: v.age_hours.map(|age| at - (age * 3600.0) as i64),
                published_precision_hours: v.age_hours.and(v.age_precision_hours),
                views: v.views,
                ..Default::default()
            })
            .collect();
        let _ = store.record(at, &observations);
    }

    pub(crate) fn observe_details(&self, video: &VideoDetails, is_short: bool) {
        let Some(store) = &self.store else { return };
        let at = now();
        let observation = Observation {
            video_id: video.id.clone(),
            channel_id: video.channel_id.clone(),
            title: video.title.clone(),
            is_short,
            published_at: video.age_hours.map(|age| at - (age * 3600.0) as i64),
            published_precision_hours: video.age_hours.map(|_| 0.0),
            views: Some(video.views),
            likes: video.likes,
            comments: video.comments,
        };
        let _ = store.record(at, &[observation]);
    }

    pub(crate) fn measure(&self, ranked: &mut RankedVideo) {
        let Some(store) = &self.store else { return };
        let video = &ranked.video;
        if let Ok(Some(growth)) = store.recent_growth(&video.id) {
            ranked.acceleration = ranked.lifetime_views_per_hour.filter(|l| *l > 0.0).map(|l| growth.views_per_hour / l);
            ranked.video.views_per_hour = Some(growth.views_per_hour);
            ranked.views_per_hour_basis = GrowthBasis::Last24Hours;
        }
        let video = &ranked.video;
        if let (Some(channel), Some(age), Some(views)) = (&video.channel_id, video.age_hours, video.views) {
            let peers = store.channel_views_at_age(channel, video.is_short, age, &video.id).unwrap_or_default();
            if peers.len() >= 3 {
                ranked.same_age_outlier = multiple(Some(views), median(peers.clone()));
                ranked.same_age_sample = peers.len();
            }
        }
    }

    async fn complete(&self, video: VideoSummary) -> VideoSummary {
        if video.age_hours.is_some() && video.channel_id.is_some() {
            return video;
        }
        match self.yt.video_basic(&video.id).await {
            Ok(d) => VideoSummary {
                age_hours: d.age_hours,
                age_precision_hours: d.age_hours.map(|_| 0.0),
                channel_id: d.channel_id,
                channel_name: d.channel_name,
                views: Some(d.views),
                duration_seconds: Some(d.duration_seconds),
                ..video
            }
            .with_vph(),
            Err(_) => video,
        }
    }

    pub(crate) async fn date_recent_shorts(&self, channel: &mut ChannelInfo) {
        let undated: Vec<usize> = channel.recent_shorts.iter().take(4).enumerate().filter(|(_, s)| s.age_hours.is_none()).map(|(i, _)| i).collect();
        let ids: Vec<String> = undated.iter().map(|&i| channel.recent_shorts[i].id.clone()).collect();
        let ages = concurrent(ids, |id| async move { self.yt.video_basic(&id).await.ok().and_then(|d| d.age_hours) }).await;
        for (i, age) in undated.into_iter().zip(ages) {
            let short = &mut channel.recent_shorts[i];
            short.age_hours = age;
            short.age_precision_hours = age.map(|_| 0.0);
            short.views_per_hour = views_per_hour(short.views, age);
        }
    }

    pub(crate) async fn load_channel(&self, id: &str, max_videos: usize) -> Result<ChannelInfo> {
        let mut channel = self.yt.channel(id, max_videos).await?;
        self.date_recent_shorts(&mut channel).await;
        self.observe(&channel.recent_videos);
        self.observe(&channel.recent_shorts);
        Ok(channel)
    }

    pub(crate) async fn channel_baselines(&self, videos: &[VideoSummary], max_channels: usize) -> HashMap<String, Baseline> {
        let mut ids: Vec<String> = Vec::new();
        for id in videos.iter().filter_map(|v| v.channel_id.clone()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.truncate(max_channels);
        let baselines = concurrent(ids.clone(), |id| async move {
            self.load_channel(&id, 30)
                .await
                .map(|c| Baseline { long: c.baseline(false), short: c.baseline(true), subscribers: c.subscribers })
                .unwrap_or_default()
        })
        .await;
        ids.into_iter().zip(baselines).collect()
    }

    async fn rank(&self, videos: Vec<VideoSummary>) -> Vec<RankedVideo> {
        self.observe(&videos);
        let baselines = self.channel_baselines(&videos, 25).await;
        videos
            .into_iter()
            .map(|video| {
                let baseline = video.channel_id.as_ref().and_then(|c| baselines.get(c)).copied().unwrap_or_default();
                let spread = if video.is_short { baseline.short } else { baseline.long };
                let mut r = ranked(video, spread, baseline.subscribers);
                self.measure(&mut r);
                r
            })
            .collect()
    }

    pub async fn trending(&self, queries: &[String], window: Window, kind: Kind, limit: usize) -> Result<TrendReport> {
        let pages = concurrent(queries.to_vec(), |q| async move { self.yt.search_filtered(&q, window, kind, 2).await }).await;
        let mut found: Vec<VideoSummary> = Vec::new();
        let mut errors = Vec::new();
        for page in pages {
            match page {
                Ok(page) => {
                    for video in page.results {
                        if !found.iter().any(|f| f.id == video.id) {
                            found.push(video);
                        }
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        if found.is_empty() {
            if let Some(error) = errors.into_iter().next() {
                return Err(error);
            }
        }
        let max_age = match window {
            Window::Any => f64::INFINITY,
            Window::Hour => 1.5,
            Window::Today => 26.0,
            Window::Week => 180.0,
            Window::Month => 760.0,
            Window::Year => 8800.0,
        };
        let completed: Vec<VideoSummary> = concurrent(found, |v| async move { self.complete(v).await })
            .await
            .into_iter()
            .filter(|v| v.age_hours.is_some_and(|age| age <= max_age))
            .collect();
        self.observe(&completed);
        let mut scored: Vec<(f64, VideoSummary)> = completed
            .into_iter()
            .map(|v| {
                let mut probe = ranked(v.clone(), None, None);
                self.measure(&mut probe);
                (probe.video.views_per_hour.unwrap_or(0.0), v)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut videos = self.rank(scored.into_iter().take(limit).map(|(_, v)| v).collect()).await;
        videos.sort_by(|a, b| b.video.views_per_hour.unwrap_or(0.0).total_cmp(&a.video.views_per_hour.unwrap_or(0.0)));
        let topics = topic_terms(&videos, queries);
        Ok(TrendReport { queries: queries.to_vec(), window, kind, videos, topics })
    }

    pub async fn discover(&self, query: &str, preset: Preset) -> Result<Vec<RankedVideo>> {
        let (window, kind) = match preset {
            Preset::Outliers => (Window::Month, Kind::Any),
            Preset::HiddenGems | Preset::Underdogs => (Window::Year, Kind::Any),
            Preset::BlowingUp | Preset::RecentUploads => (Window::Week, Kind::Any),
            Preset::TodaysTop => (Window::Today, Kind::Video),
            Preset::TrendingShorts => (Window::Week, Kind::Short),
            Preset::Evergreens => (Window::Any, Kind::Video),
        };
        let page = self.yt.search_filtered(query, window, kind, 2).await?;
        let videos = concurrent(page.results, |v| async move { self.complete(v).await }).await;
        let mut scored: Vec<(f64, RankedVideo)> =
            self.rank(videos).await.into_iter().filter_map(|r| preset_score(preset, &r).map(|score| (score, r))).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    pub async fn tag_suggestions(&self, keyword: &str, limit: usize) -> Result<Vec<TagSuggestion>> {
        let page = self.yt.search(keyword).await?;
        self.observe(&page.results);
        let ids: Vec<String> = page.results.iter().filter(|v| !v.is_short).take(10).map(|v| v.id.clone()).collect();
        let videos = concurrent(ids, |id| async move { self.yt.video_basic(&id).await.ok() }).await;
        let sampled = videos.iter().flatten().filter(|v| !v.tags.is_empty()).count();
        let mut weights: HashMap<String, (f64, usize)> = HashMap::new();
        for (rank, video) in videos.iter().enumerate() {
            for tag in video.iter().flat_map(|v| &v.tags) {
                let entry = weights.entry(normalize(tag)).or_default();
                entry.0 += 1.0 / (rank as f64 + 1.0);
                entry.1 += 1;
            }
        }
        let mut tags: Vec<(String, (f64, usize))> = weights.into_iter().collect();
        tags.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
        let top = tags.first().map(|t| t.1 .0).unwrap_or(1.0);
        let confidence = match sampled {
            0..=3 => Confidence::Low,
            4..=6 => Confidence::Medium,
            _ => Confidence::High,
        };
        Ok(tags
            .into_iter()
            .take(limit)
            .map(|(tag, (weight, used_by))| TagSuggestion { tag, score: percent(weight / top), used_by, sampled_videos: sampled, confidence })
            .collect())
    }

    pub async fn seo_check(&self, input: SeoInput) -> Result<SeoReport> {
        let suggested: Vec<String> = match input.keyword.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(keyword) => self.tag_suggestions(keyword, 10).await?.into_iter().map(|t| t.tag).collect(),
            None => Vec::new(),
        };
        Ok(seo::score(&input, &suggested))
    }

    pub async fn video_report(&self, input: &str) -> Result<VideoReport> {
        let id = video_id(input).ok_or_else(|| anyhow!("not a YouTube video link or id: {input}"))?;
        let video = self.yt.video(&id).await?;
        let id = &id;
        let channel = async {
            match &video.channel_id {
                Some(c) => self.load_channel(c, 30).await.ok(),
                None => None,
            }
        };
        let tag_ranks = concurrent(video.tags.iter().take(8).cloned().collect(), |tag| async move {
            let rank = self.yt.search(&tag).await.ok().and_then(|p| p.results.iter().position(|r| r.id == *id).map(|i| i + 1));
            TagRank { tag, rank }
        });
        let seo = self.seo_check(SeoInput {
            title: video.title.clone(),
            description: video.description.clone(),
            tags: video.tags.clone(),
            keyword: video.tags.first().cloned(),
        });
        let (channel, tag_ranks, seo) = futures::join!(channel, tag_ranks, seo);
        let is_short = video.duration_seconds <= 60 || channel.as_ref().is_some_and(|c| c.recent_shorts.iter().any(|s| s.id == video.id));
        self.observe_details(&video, is_short);
        let subscribers = channel.as_ref().and_then(|c| c.subscribers);
        let summary = VideoSummary {
            id: video.id.clone(),
            title: video.title.clone(),
            channel_id: video.channel_id.clone(),
            channel_name: video.channel_name.clone(),
            views: Some(video.views),
            age_hours: video.age_hours,
            age_precision_hours: Some(0.0),
            duration_seconds: Some(video.duration_seconds),
            is_short,
            ..Default::default()
        }
        .with_vph();
        let mut performance = ranked(summary, channel.as_ref().and_then(|c| c.baseline(is_short)), subscribers);
        self.measure(&mut performance);
        Ok(VideoReport {
            performance,
            engagement_rate: rate(video.likes.map(|l| l + video.comments.unwrap_or(0)), video.views),
            like_rate: rate(video.likes, video.views),
            comment_rate: rate(video.comments, video.views),
            views_per_subscriber: subscribers.filter(|s| *s > 0).map(|s| video.views as f64 / s as f64),
            channel_tags: channel.map(|c| c.keywords).unwrap_or_default(),
            tag_ranks,
            seo: seo?,
            video,
        })
    }

    pub async fn channel_audit(&self, input: &str) -> Result<ChannelAudit> {
        let id = self.yt.channel_id(input).await?;
        let mut channel = self.load_channel(&id, 60).await?;
        let about = self.yt.channel_about(&channel.id).await.unwrap_or_default();
        let views_per_video = about.total_views.zip(channel.video_count.filter(|n| *n > 0)).map(|(views, videos)| views / videos);
        let (long, short) = (channel.baseline(false), channel.baseline(true));
        let recent = std::mem::take(&mut channel.recent_videos);
        let shorts = std::mem::take(&mut channel.recent_shorts);
        let views: Vec<u64> = recent.iter().filter_map(|v| v.views).collect();
        let ages: Vec<f64> = recent.iter().filter_map(|v| v.age_hours).collect();
        let oldest = ages.iter().copied().fold(0.0, f64::max);
        let tag_lists = concurrent(recent.iter().chain(&shorts).take(15).map(|v| v.id.clone()).collect(), |id| async move {
            self.yt.video_basic(&id).await.map(|d| d.tags).unwrap_or_default()
        })
        .await;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for tag in tag_lists.iter().flatten() {
            *counts.entry(normalize(tag)).or_default() += 1;
        }
        let mut top_tags: Vec<TagCount> = counts.into_iter().map(|(tag, count)| TagCount { tag, count }).collect();
        top_tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        top_tags.truncate(25);
        let measured = |list: Vec<VideoSummary>, baseline: Option<Spread>| -> Vec<RankedVideo> {
            list.into_iter()
                .map(|video| {
                    let mut r = ranked(video, baseline, channel.subscribers);
                    self.measure(&mut r);
                    r
                })
                .collect()
        };
        Ok(ChannelAudit {
            about,
            views_per_video,
            median_views: long.map(|s| s.median),
            median_short_views: short.map(|s| s.median),
            average_views: (!views.is_empty()).then(|| views.iter().sum::<u64>() / views.len() as u64),
            uploads_per_week: (oldest > 0.0).then(|| ages.len() as f64 / (oldest / 168.0)),
            last_upload_hours: ages.iter().copied().reduce(f64::min),
            videos: measured(recent, long),
            shorts: measured(shorts, short),
            top_tags,
            channel,
        })
    }
}

fn topic_terms(videos: &[RankedVideo], queries: &[String]) -> Vec<TopicTerm> {
    let query_words: Vec<String> = queries.iter().flat_map(|q| normalize(q).split(' ').map(String::from).collect::<Vec<_>>()).collect();
    let mut terms: HashMap<String, (usize, f64)> = HashMap::new();
    for ranked in videos {
        let words: Vec<String> = ranked
            .video
            .title
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() > 1 && !STOPWORDS.contains(w) && !query_words.iter().any(|q| q == w))
            .map(String::from)
            .collect();
        let mut seen: Vec<String> = words.clone();
        seen.extend(words.windows(2).filter(|pair| pair[0] != pair[1]).map(|pair| pair.join(" ")));
        seen.sort();
        seen.dedup();
        for term in seen {
            let entry = terms.entry(term).or_default();
            entry.0 += 1;
            entry.1 += ranked.video.views_per_hour.unwrap_or(0.0);
        }
    }
    let mut topics: Vec<TopicTerm> = terms
        .into_iter()
        .filter(|(_, (count, _))| *count >= 2)
        .map(|(term, (videos, views_per_hour))| TopicTerm { term, videos, views_per_hour })
        .collect();
    topics.sort_by(|a, b| b.views_per_hour.total_cmp(&a.views_per_hour));
    topics.truncate(25);
    topics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlier_gets_a_range_when_the_baseline_is_small() {
        let video = VideoSummary { views: Some(10_000), ..Default::default() };
        let small = ranked(video.clone(), Some(Spread { median: 1_000, q1: 500, q3: 2_000, n: 5 }), None);
        assert_eq!((small.outlier_score, small.outlier_low, small.outlier_high), (Some(10.0), Some(5.0), Some(20.0)));
        assert_eq!(small.outlier_confidence, Some(Confidence::Low));
        let large = ranked(video, Some(Spread { median: 1_000, q1: 500, q3: 2_000, n: 20 }), None);
        assert_eq!((large.outlier_low, large.outlier_confidence), (None, Some(Confidence::High)));
    }

    #[test]
    fn measured_growth_replaces_lifetime_speed() {
        let engine = Engine { store: Some(Store::in_memory().unwrap()), ..Engine::new(Config::default()) };
        let store = engine.store.as_ref().unwrap();
        let at = now();
        for (offset, views) in [(24 * 3600, 1_000u64), (0, 25_000)] {
            let observation = Observation { video_id: "v".into(), views: Some(views), ..Default::default() };
            store.record(at - offset, &[observation]).unwrap();
        }
        let video = VideoSummary { id: "v".into(), views: Some(25_000), age_hours: Some(100.0), ..Default::default() }.with_vph();
        let mut r = ranked(video, None, None);
        engine.measure(&mut r);
        assert_eq!(r.views_per_hour_basis, GrowthBasis::Last24Hours);
        assert_eq!(r.video.views_per_hour, Some(1_000.0));
        assert_eq!(r.acceleration, Some(4.0));
    }
}
