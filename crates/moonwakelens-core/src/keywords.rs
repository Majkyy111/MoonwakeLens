use crate::analysis::{concurrent, now, Engine};
use crate::config::{VolumeInputs, Weights};
use crate::metrics::*;
use crate::model::VideoSummary;
use crate::parse::{is_noise_keyword, normalize};
use crate::store::KeywordPoint;
use crate::trends::TrendsSignal;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

const TRENDS_CACHE_SECONDS: i64 = 24 * 3600;
const TRENDS_PAUSE_SECONDS: i64 = 30 * 60;
const BOOTSTRAP_ROUNDS: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct KeywordInputs {
    pub demand: f64,
    pub autocomplete: f64,
    pub trends: Option<TrendsSignal>,
    pub title_match: f64,
    pub verified: f64,
    pub channel_strength: Option<f64>,
    pub views_per_subscriber_median: Option<f64>,
    pub channels_with_subscribers: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordTrend {
    pub direction: &'static str,
    pub overall_change: i16,
    pub since_day: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordScore {
    pub keyword: String,
    pub overall: u8,
    pub volume: u8,
    pub competition: u8,
    pub overall_range: Option<(u8, u8)>,
    pub volume_range: Option<(u8, u8)>,
    pub competition_range: Option<(u8, u8)>,
    pub confidence: Confidence,
    pub sample_size: usize,
    pub estimated_search_views: Option<u64>,
    pub inputs: KeywordInputs,
    pub estimated_results: u64,
    pub median_top_views: u64,
    pub highest_views: u64,
    pub average_views: u64,
    pub average_age_hours: Option<f64>,
    pub in_title: usize,
    pub in_description: usize,
    pub with_captions: usize,
    pub uploaded_last_7_days: usize,
    pub average_subscribers: Option<u64>,
    pub top_channels: Vec<ChannelStat>,
    pub related: Vec<RelatedKeyword>,
    pub history: Vec<KeywordPoint>,
    pub trend: Option<KeywordTrend>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelatedKeyword {
    pub keyword: String,
    pub overall: u8,
    pub volume: u8,
    pub competition: u8,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelStat {
    pub id: String,
    pub name: String,
    pub subscribers: Option<u64>,
    pub best_views: u64,
}

struct Scored {
    volume: f64,
    competition: f64,
    overall: f64,
}

fn contains_words(text: &str, words: &[String]) -> bool {
    let text = normalize(text);
    words.iter().all(|w| text.contains(w.as_str()))
}

fn views_per_subscriber(top: &[&VideoSummary], subscribers: &HashMap<String, u64>) -> Vec<f64> {
    top.iter()
        .filter_map(|v| {
            let subs = *subscribers.get(v.channel_id.as_ref()?)?;
            let views = v.views?;
            (subs > 0).then(|| views as f64 / subs as f64)
        })
        .collect()
}

fn score_sample(top: &[&VideoSummary], words: &[String], subscribers: &HashMap<String, u64>, autocomplete: f64, trends: Option<f64>, w: &Weights) -> Scored {
    let median_views = median(top.iter().filter_map(|v| v.views).collect()).unwrap_or(0);
    let demand = norm_log(median_views as f64, w.demand_log_low, w.demand_log_high);
    let mut ratios = views_per_subscriber(top, subscribers);
    let strength = if ratios.len() >= 3 {
        ratios.sort_by(f64::total_cmp);
        1.0 - norm_log(ratios[ratios.len() / 2], w.strength_log_low, w.strength_log_high)
    } else {
        0.5
    };
    let volume = w.volume(VolumeInputs { demand, autocomplete, trends });
    let competition = w.competition(share(top, |v| contains_words(&v.title, words)), share(top, |v| v.verified_channel), strength);
    Scored { volume, competition, overall: w.overall(volume, competition) }
}

fn bootstrap_ranges(
    top: &[&VideoSummary],
    words: &[String],
    subscribers: &HashMap<String, u64>,
    autocomplete: f64,
    trends: Option<f64>,
    w: &Weights,
    seed: u64,
) -> [(u8, u8); 3] {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut samples: [Vec<f64>; 3] = Default::default();
    for _ in 0..BOOTSTRAP_ROUNDS {
        let resample: Vec<&VideoSummary> = (0..top.len()).map(|_| top[(next() % top.len() as u64) as usize]).collect();
        let s = score_sample(&resample, words, subscribers, autocomplete, trends, w);
        for (bucket, value) in samples.iter_mut().zip([s.overall, s.volume, s.competition]) {
            bucket.push(value);
        }
    }
    samples.map(|mut values| {
        values.sort_by(f64::total_cmp);
        let at = |q: f64| percent(values[((values.len() - 1) as f64 * q).round() as usize]);
        (at(0.1), at(0.9))
    })
}

fn trend_from_history(history: &[KeywordPoint], today: &str) -> Option<KeywordTrend> {
    let current = history.iter().find(|p| p.day == today)?;
    let week_ago = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()? - chrono::Duration::days(7);
    let reference = history
        .iter()
        .filter(|p| p.day != today)
        .filter(|p| chrono::NaiveDate::parse_from_str(&p.day, "%Y-%m-%d").is_ok_and(|d| d <= week_ago))
        .next_back()
        .or_else(|| history.iter().find(|p| p.day != today))?;
    let change = current.overall as i16 - reference.overall as i16;
    let direction = if change >= 5 {
        "rising"
    } else if change <= -5 {
        "falling"
    } else {
        "flat"
    };
    Some(KeywordTrend { direction, overall_change: change, since_day: reference.day.clone() })
}

impl Engine {
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

    pub async fn trends_signal(&self, keyword: &str) -> Option<TrendsSignal> {
        let config = self.config();
        let key = format!("trends|{}|{}|{}", config.gl, config.trends_anchor, normalize(keyword));
        if let Some(cached) = self.store.as_ref().and_then(|s| s.cached(&key, TRENDS_CACHE_SECONDS, now()).ok().flatten()) {
            return serde_json::from_str(&cached).ok();
        }
        if now() < self.trends_paused_until.load(Ordering::Relaxed) {
            return None;
        }
        match self.trends.youtube_interest(keyword, &config.trends_anchor, &config.gl).await {
            Ok(signal) => {
                if let Some(store) = &self.store {
                    let _ = store.cache(&key, &serde_json::to_string(&signal).unwrap_or_default(), now());
                }
                signal
            }
            Err(_) => {
                self.trends_paused_until.store(now() + TRENDS_PAUSE_SECONDS, Ordering::Relaxed);
                None
            }
        }
    }

    pub(crate) async fn volume_inputs(&self, keyword: &str, with_trends: bool) -> Result<(VolumeInputs, Option<TrendsSignal>)> {
        let (page, autocomplete) = futures::join!(self.yt.search(keyword), self.autocomplete_strength(keyword));
        let page = page?;
        let w = self.weights();
        let median_views = median(page.results.iter().filter(|v| !v.is_short).take(20).filter_map(|v| v.views).collect()).unwrap_or(0);
        let trends = if with_trends { self.trends_signal(keyword).await } else { None };
        let inputs = VolumeInputs {
            demand: norm_log(median_views as f64, w.demand_log_low, w.demand_log_high),
            autocomplete,
            trends: trends.as_ref().map(TrendsSignal::volume_input),
        };
        Ok((inputs, trends))
    }

    async fn subscribers_of(&self, top: &[&VideoSummary]) -> HashMap<String, u64> {
        let mut ids: Vec<String> = Vec::new();
        for id in top.iter().filter_map(|v| v.channel_id.clone()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.truncate(10);
        let counts = concurrent(ids.clone(), |id| async move { self.yt.subscribers(&id).await.ok().flatten() }).await;
        ids.into_iter().zip(counts).filter_map(|(id, count)| Some((id, count?))).collect()
    }

    pub(crate) async fn keyword_basic(&self, keyword: &str, with_trends: bool) -> Result<KeywordScore> {
        let ((volume_inputs, trends), page) = futures::try_join!(self.volume_inputs(keyword, with_trends), self.yt.search(keyword))?;
        self.observe(&page.results);
        let top: Vec<&VideoSummary> = page.results.iter().filter(|v| !v.is_short).take(20).collect();
        let subscribers = self.subscribers_of(&top[..top.len().min(10)]).await;
        let words: Vec<String> = normalize(keyword).split(' ').map(String::from).collect();
        let w = self.weights();
        let scored = score_sample(&top, &words, &subscribers, volume_inputs.autocomplete, volume_inputs.trends, &w);
        let mut ratios = views_per_subscriber(&top, &subscribers);
        ratios.sort_by(f64::total_cmp);
        let ratio_median = (ratios.len() >= 3).then(|| ratios[ratios.len() / 2]);
        let keyword = normalize(keyword);
        let ranges = (top.len() < RANGE_BELOW_SAMPLE && !top.is_empty()).then(|| {
            let seed = keyword.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
            bootstrap_ranges(&top, &words, &subscribers, volume_inputs.autocomplete, volume_inputs.trends, &w, seed)
        });
        let mut confidence = Confidence::from_sample(top.len());
        if ratio_median.is_none() && confidence == Confidence::High {
            confidence = Confidence::Medium;
        }
        let views: Vec<u64> = top.iter().filter_map(|v| v.views).collect();
        let ages: Vec<f64> = top.iter().filter_map(|v| v.age_hours).collect();
        let (overall, volume, competition) = (percent(scored.overall), percent(scored.volume), percent(scored.competition));
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let history = match &self.store {
            Some(store) => {
                let _ = store.save_keyword(&keyword, &today, overall, volume, competition);
                store.keyword_history(&keyword).unwrap_or_default()
            }
            None => Vec::new(),
        };
        Ok(KeywordScore {
            overall,
            volume,
            competition,
            overall_range: ranges.map(|r| r[0]),
            volume_range: ranges.map(|r| r[1]),
            competition_range: ranges.map(|r| r[2]),
            confidence,
            sample_size: top.len(),
            estimated_search_views: w.estimated_views(scored.volume),
            inputs: KeywordInputs {
                demand: volume_inputs.demand,
                autocomplete: volume_inputs.autocomplete,
                trends,
                title_match: share(&top, |v| contains_words(&v.title, &words)),
                verified: share(&top, |v| v.verified_channel),
                channel_strength: ratio_median.map(|r| 1.0 - norm_log(r, w.strength_log_low, w.strength_log_high)),
                views_per_subscriber_median: ratio_median,
                channels_with_subscribers: subscribers.len(),
            },
            estimated_results: page.estimated_results,
            median_top_views: median(views.clone()).unwrap_or(0),
            highest_views: views.iter().copied().max().unwrap_or(0),
            average_views: if views.is_empty() { 0 } else { views.iter().sum::<u64>() / views.len() as u64 },
            average_age_hours: (!ages.is_empty()).then(|| ages.iter().sum::<f64>() / ages.len() as f64),
            in_title: top.iter().filter(|v| contains_words(&v.title, &words)).count(),
            in_description: top.iter().filter(|v| v.description_snippet.as_deref().is_some_and(|d| contains_words(d, &words))).count(),
            with_captions: top.iter().filter(|v| v.has_captions).count(),
            uploaded_last_7_days: top.iter().filter(|v| v.age_hours.is_some_and(|a| a <= 168.0)).count(),
            average_subscribers: (!subscribers.is_empty()).then(|| subscribers.values().sum::<u64>() / subscribers.len() as u64),
            top_channels: Vec::new(),
            related: Vec::new(),
            trend: trend_from_history(&history, &today),
            history,
            keyword,
        })
    }

    pub async fn keyword_score(&self, keyword: &str) -> Result<KeywordScore> {
        let mut score = self.keyword_basic(keyword, true).await?;
        let page = self.yt.search(keyword).await?;
        let mut top: Vec<VideoSummary> = page.results.iter().filter(|v| !v.is_short).take(20).cloned().collect();
        top.sort_by(|a, b| b.views.cmp(&a.views));
        let top_refs: Vec<&VideoSummary> = top.iter().collect();
        let related: Vec<String> =
            self.yt.autocomplete(keyword).await.unwrap_or_default().iter().map(|k| normalize(k)).filter(|k| *k != score.keyword && !is_noise_keyword(k)).take(6).collect();
        let (subscribers, related) =
            futures::join!(self.subscribers_of(&top_refs), concurrent(related, |k| async move { self.keyword_basic(&k, false).await.ok() }));
        let mut channels: Vec<ChannelStat> = Vec::new();
        for video in &top {
            let (Some(id), Some(views)) = (&video.channel_id, video.views) else { continue };
            match channels.iter_mut().find(|c| &c.id == id) {
                Some(channel) => channel.best_views = channel.best_views.max(views),
                None => channels.push(ChannelStat {
                    id: id.clone(),
                    name: video.channel_name.clone().unwrap_or_default(),
                    subscribers: subscribers.get(id).copied(),
                    best_views: views,
                }),
            }
        }
        channels.truncate(5);
        score.top_channels = channels;
        score.related = related
            .into_iter()
            .flatten()
            .map(|k| RelatedKeyword { keyword: k.keyword, overall: k.overall, volume: k.volume, competition: k.competition, confidence: k.confidence })
            .collect();
        Ok(score)
    }

    pub async fn keyword_research(&self, seed: &str, limit: usize, deep: bool) -> Result<Vec<KeywordScore>> {
        let mut queries = vec![seed.to_string(), format!("{seed} ")];
        if deep {
            queries.extend(('a'..='z').map(|c| format!("{seed} {c}")));
        }
        let lists = concurrent(queries, |q| async move { self.yt.autocomplete(&q).await.unwrap_or_default() }).await;
        let mut keywords = vec![normalize(seed)];
        for keyword in lists.into_iter().flatten().map(|k| normalize(&k)) {
            if !keywords.contains(&keyword) && !is_noise_keyword(&keyword) {
                keywords.push(keyword);
            }
        }
        keywords.truncate(limit.max(1));
        let mut scores: Vec<KeywordScore> =
            concurrent(keywords, |k| async move { self.keyword_basic(&k, false).await }).await.into_iter().filter_map(Result::ok).collect();
        scores.sort_by(|a, b| b.overall.cmp(&a.overall));
        Ok(scores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(views: u64, channel: &str, title: &str, verified: bool) -> VideoSummary {
        VideoSummary { views: Some(views), channel_id: Some(channel.into()), title: title.into(), verified_channel: verified, ..Default::default() }
    }

    #[test]
    fn competition_no_longer_counts_demand() {
        let w = Weights::default();
        let words = vec!["roblox".to_string()];
        let subs: HashMap<String, u64> = [("a", 1_000_000), ("b", 1_000_000), ("c", 1_000_000)].map(|(k, v)| (k.to_string(), v)).into();
        let small: Vec<VideoSummary> = ["a", "b", "c"].iter().map(|c| video(10_000, c, "other", false)).collect();
        let big: Vec<VideoSummary> = ["a", "b", "c"].iter().map(|c| video(10_000_000, c, "other", false)).collect();
        let small_refs: Vec<&VideoSummary> = small.iter().collect();
        let big_refs: Vec<&VideoSummary> = big.iter().collect();
        let quiet = score_sample(&small_refs, &words, &subs, 0.5, None, &w);
        let busy = score_sample(&big_refs, &words, &subs, 0.5, None, &w);
        assert!(busy.volume > quiet.volume);
        assert!(busy.competition < quiet.competition, "big views on the same channels means those channels are beatable");
    }

    #[test]
    fn small_samples_get_ranges() {
        let w = Weights::default();
        let videos: Vec<VideoSummary> = (0..5).map(|i| video(1_000 * (i + 1) * (i + 1), "a", if i % 2 == 0 { "roblox" } else { "x" }, i == 0)).collect();
        let refs: Vec<&VideoSummary> = videos.iter().collect();
        let ranges = bootstrap_ranges(&refs, &["roblox".into()], &HashMap::new(), 0.5, None, &w, 7);
        for (low, high) in ranges {
            assert!(low <= high);
        }
        assert!(ranges[2].0 < ranges[2].1, "title match varies across resamples");
    }

    #[test]
    fn trend_compares_with_a_week_ago() {
        let point = |day: &str, overall| KeywordPoint { day: day.into(), overall, volume: 0, competition: 0 };
        let history = vec![point("2026-10-10", 40), point("2026-10-16", 45), point("2026-10-24", 60)];
        let trend = trend_from_history(&history, "2026-10-24").unwrap();
        assert_eq!((trend.direction, trend.overall_change, trend.since_day.as_str()), ("rising", 15, "2026-10-16"));
        assert!(trend_from_history(&history[..1], "2026-10-24").is_none());
    }
}
