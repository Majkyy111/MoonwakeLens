use crate::client::{Kind, Window, YouTube};
use crate::metrics::*;
use crate::model::{ChannelInfo, VideoDetails, VideoSummary};
use crate::parse::{normalize, video_id};
use crate::seo::{self, SeoInput, SeoReport};
use anyhow::{anyhow, Result};
use futures::{stream, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;

const PARALLEL: usize = 8;
const STOPWORDS: &[&str] = &[
    "a", "added", "all", "an", "and", "are", "as", "at", "be", "but", "by", "can", "do", "everything", "for", "from", "fyp", "get", "got", "has", "have", "how",
    "i", "if", "in", "is", "it", "its", "just", "made", "make", "me", "my", "new", "no", "not", "now", "of", "on", "or", "our", "out", "part", "short",
    "shorts", "so", "that", "the", "this", "to", "video", "viral", "vs", "was", "we", "what", "when", "who", "why", "will", "with", "you", "your",
];

async fn concurrent<T, R, F, Fut>(items: Vec<T>, f: F) -> Vec<R>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = R>,
{
    stream::iter(items).map(f).buffered(PARALLEL).collect().await
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordScore {
    pub keyword: String,
    pub overall: u8,
    pub volume: u8,
    pub competition: u8,
    pub autocomplete_strength: f64,
    pub estimated_results: u64,
    pub median_top_views: u64,
    pub title_match_ratio: f64,
    pub verified_ratio: f64,
    pub recent_ratio: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RankedVideo {
    #[serde(flatten)]
    pub video: VideoSummary,
    pub channel_median_views: Option<u64>,
    pub outlier_score: Option<f64>,
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
    pub views_per_hour: Option<f64>,
    pub engagement_rate: Option<f64>,
    pub like_rate: Option<f64>,
    pub comment_rate: Option<f64>,
    pub channel_subscribers: Option<u64>,
    pub channel_median_views: Option<u64>,
    pub outlier_score: Option<f64>,
    pub views_per_subscriber: Option<f64>,
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
    pub median_views: Option<u64>,
    pub median_short_views: Option<u64>,
    pub average_views: Option<u64>,
    pub uploads_per_week: Option<f64>,
    pub last_upload_hours: Option<f64>,
    pub videos: Vec<RankedVideo>,
    pub shorts: Vec<RankedVideo>,
    pub top_tags: Vec<TagCount>,
}

fn ranked(video: VideoSummary, median: Option<u64>) -> RankedVideo {
    RankedVideo { outlier_score: multiple(video.views, median), channel_median_views: median, video }
}

#[derive(Debug, Clone, Serialize)]
pub struct TagSuggestion {
    pub tag: String,
    pub score: u8,
    pub used_by: usize,
}

pub struct Engine {
    pub yt: YouTube,
}

impl Engine {
    pub fn new(hl: &str, gl: &str) -> Self {
        Self { yt: YouTube::new(hl, gl) }
    }

    pub async fn trending(&self, queries: &[String], window: Window, kind: Kind, limit: usize) -> Result<TrendReport> {
        let pages = concurrent(queries.to_vec(), |q| async move { self.yt.search_filtered(&q, window, kind, 2).await }).await;
        let mut found: Vec<VideoSummary> = Vec::new();
        for page in pages.into_iter().filter_map(Result::ok) {
            for video in page.results {
                if !found.iter().any(|f| f.id == video.id) {
                    found.push(video);
                }
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
        let mut videos: Vec<VideoSummary> = concurrent(found, |v| async move { self.complete(v).await })
            .await
            .into_iter()
            .filter(|v| v.age_hours.is_some_and(|age| age <= max_age))
            .collect();
        videos.sort_by(|a, b| b.views_per_hour.unwrap_or(0.0).total_cmp(&a.views_per_hour.unwrap_or(0.0)));
        videos.truncate(limit);
        let videos = self.rank_against_channels(videos).await;
        let topics = topic_terms(&videos, queries);
        Ok(TrendReport { queries: queries.to_vec(), window, kind, videos, topics })
    }

    async fn complete(&self, video: VideoSummary) -> VideoSummary {
        if video.age_hours.is_some() && video.channel_id.is_some() {
            return video;
        }
        match self.yt.video_basic(&video.id).await {
            Ok(d) => VideoSummary {
                age_hours: d.age_hours,
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

    async fn channel_baselines(&self, videos: &[VideoSummary]) -> HashMap<String, (Option<u64>, Option<u64>)> {
        let mut ids: Vec<String> = Vec::new();
        for id in videos.iter().filter_map(|v| v.channel_id.clone()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.truncate(20);
        let baselines = concurrent(ids.clone(), |id| async move {
            self.yt.channel(&id, 30).await.map(|c| (c.baseline(false), c.baseline(true))).unwrap_or_default()
        })
        .await;
        ids.into_iter().zip(baselines).collect()
    }

    async fn rank_against_channels(&self, videos: Vec<VideoSummary>) -> Vec<RankedVideo> {
        let baselines = self.channel_baselines(&videos).await;
        videos
            .into_iter()
            .map(|video| {
                let median = video
                    .channel_id
                    .as_ref()
                    .and_then(|c| baselines.get(c))
                    .and_then(|(long, short)| if video.is_short { *short } else { *long });
                ranked(video, median)
            })
            .collect()
    }

    pub async fn outliers(&self, query: &str, window: Window, kind: Kind) -> Result<Vec<RankedVideo>> {
        let page = self.yt.search_filtered(query, window, kind, 2).await?;
        let videos = concurrent(page.results, |v| async move { self.complete(v).await }).await;
        let mut ranked = self.rank_against_channels(videos).await;
        ranked.sort_by(|a, b| b.outlier_score.unwrap_or(0.0).total_cmp(&a.outlier_score.unwrap_or(0.0)));
        Ok(ranked)
    }

    pub async fn autocomplete_strength(&self, keyword: &str) -> f64 {
        let target = normalize(keyword);
        let chars: Vec<char> = target.chars().collect();
        let n = chars.len();
        let mut lengths: Vec<usize> = (1..=8).map(|i| (i * n).div_ceil(8)).filter(|l| *l >= 2).collect();
        lengths.dedup();
        let target = &target;
        let hits = concurrent(lengths.clone(), |len| {
            let prefix: String = chars[..len].iter().collect();
            async move { self.yt.autocomplete(&prefix).await.is_ok_and(|s| s.iter().any(|x| normalize(x) == *target)) }
        })
        .await;
        lengths
            .iter()
            .zip(hits)
            .find(|(_, hit)| *hit)
            .map(|(len, _)| 1.0 - (*len as f64 - 1.0) / n as f64)
            .unwrap_or(0.0)
    }

    pub async fn keyword_score(&self, keyword: &str) -> Result<KeywordScore> {
        let (page, strength) = futures::join!(self.yt.search(keyword), self.autocomplete_strength(keyword));
        let page = page?;
        let top: Vec<&VideoSummary> = page.results.iter().filter(|v| !v.is_short).take(20).collect();
        let words: Vec<String> = normalize(keyword).split(' ').map(String::from).collect();
        let median_top_views = median(top.iter().filter_map(|v| v.views).collect()).unwrap_or(0);
        let title_match_ratio = share(&top, |v| {
            let title = normalize(&v.title);
            words.iter().all(|w| title.contains(w.as_str()))
        });
        let verified_ratio = share(&top, |v| v.verified_channel);
        let recent_ratio = share(&top, |v| v.age_hours.is_some_and(|a| a <= 24.0 * 31.0));
        let demand = norm_log(median_top_views as f64, 3.0, 7.0);
        let volume = 0.6 * demand + 0.4 * strength;
        let competition = 0.35 * title_match_ratio + 0.35 * verified_ratio + 0.3 * demand;
        Ok(KeywordScore {
            keyword: normalize(keyword),
            overall: percent(0.6 * volume + 0.4 * (1.0 - competition)),
            volume: percent(volume),
            competition: percent(competition),
            autocomplete_strength: strength,
            estimated_results: page.estimated_results,
            median_top_views,
            title_match_ratio,
            verified_ratio,
            recent_ratio,
        })
    }

    pub async fn keyword_research(&self, seed: &str, limit: usize, deep: bool) -> Result<Vec<KeywordScore>> {
        let mut queries = vec![seed.to_string(), format!("{seed} ")];
        if deep {
            queries.extend(('a'..='z').map(|c| format!("{seed} {c}")));
        }
        let lists = concurrent(queries, |q| async move { self.yt.autocomplete(&q).await.unwrap_or_default() }).await;
        let mut keywords = vec![normalize(seed)];
        for keyword in lists.into_iter().flatten().map(|k| normalize(&k)) {
            if !keywords.contains(&keyword) {
                keywords.push(keyword);
            }
        }
        keywords.truncate(limit.max(1));
        let mut scores: Vec<KeywordScore> = concurrent(keywords, |k| async move { self.keyword_score(&k).await })
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        scores.sort_by(|a, b| b.overall.cmp(&a.overall));
        Ok(scores)
    }

    pub async fn tag_suggestions(&self, keyword: &str, limit: usize) -> Result<Vec<TagSuggestion>> {
        let page = self.yt.search(keyword).await?;
        let ids: Vec<String> = page.results.iter().filter(|v| !v.is_short).take(10).map(|v| v.id.clone()).collect();
        let videos = concurrent(ids, |id| async move { self.yt.video_basic(&id).await.ok() }).await;
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
        Ok(tags
            .into_iter()
            .take(limit)
            .map(|(tag, (weight, used_by))| TagSuggestion { tag, score: percent(weight / top), used_by })
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
                Some(c) => self.yt.channel(c, 30).await.ok(),
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
        let median = channel.as_ref().and_then(|c| {
            let is_short = video.duration_seconds <= 60 || c.recent_shorts.iter().any(|s| s.id == video.id);
            c.baseline(is_short)
        });
        let subscribers = channel.as_ref().and_then(|c| c.subscribers);
        Ok(VideoReport {
            views_per_hour: views_per_hour(Some(video.views), video.age_hours),
            engagement_rate: rate(video.likes.map(|l| l + video.comments.unwrap_or(0)), video.views),
            like_rate: rate(video.likes, video.views),
            comment_rate: rate(video.comments, video.views),
            channel_subscribers: subscribers,
            channel_median_views: median,
            outlier_score: multiple(Some(video.views), median),
            views_per_subscriber: subscribers.filter(|s| *s > 0).map(|s| video.views as f64 / s as f64),
            tag_ranks,
            seo: seo?,
            video,
        })
    }

    pub async fn channel_audit(&self, input: &str) -> Result<ChannelAudit> {
        let mut channel = self.yt.channel(input, 60).await?;
        let (median, median_short) = (channel.baseline(false), channel.baseline(true));
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
        Ok(ChannelAudit {
            median_views: median,
            median_short_views: median_short,
            average_views: (!views.is_empty()).then(|| views.iter().sum::<u64>() / views.len() as u64),
            uploads_per_week: (oldest > 0.0).then(|| ages.len() as f64 / (oldest / 168.0)),
            last_upload_hours: ages.iter().copied().reduce(f64::min),
            videos: recent.into_iter().map(|video| ranked(video, median)).collect(),
            shorts: shorts.into_iter().map(|short| ranked(short, median_short)).collect(),
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
