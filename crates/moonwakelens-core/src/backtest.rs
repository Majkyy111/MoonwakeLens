use crate::analysis::{now, Engine};
use crate::metrics::spearman;
use crate::parse::video_id;
use crate::seo::SeoInput;
use crate::store::Prediction;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub const BACKTEST_AGE_HOURS: f64 = 168.0;
const MIN_FOR_CORRELATION: usize = 3;

#[derive(Debug, Clone, Deserialize)]
pub struct PredictionInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub keyword: Option<String>,
    pub video: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestStatus {
    Measured,
    NotLinked,
    TooYoung,
    NoSnapshotsNearDay7,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestRow {
    pub prediction: Prediction,
    pub age_hours: Option<f64>,
    pub views_at_7_days: Option<u64>,
    pub status: BacktestStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    pub rows: Vec<BacktestRow>,
    pub measured: usize,
    pub spearman_overall: Option<f64>,
    pub spearman_volume: Option<f64>,
    pub spearman_seo: Option<f64>,
    pub note: String,
}

impl Engine {
    async fn link_video(&self, input: &str) -> Result<String> {
        let id = video_id(input).ok_or_else(|| anyhow!("not a YouTube video link or id: {input}"))?;
        let store = self.store.as_ref().ok_or_else(|| anyhow!("predictions need the local database"))?;
        let video = self.yt.video_basic(&id).await?;
        self.observe_details(&video, video.duration_seconds <= 60);
        store.track("video", &id, Some(&video.title), now())?;
        Ok(id)
    }

    pub async fn predict(&self, input: PredictionInput) -> Result<Prediction> {
        let store = self.store.as_ref().ok_or_else(|| anyhow!("predictions need the local database"))?;
        let keyword = input.keyword.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let seo = self.seo_check(SeoInput { title: input.title.clone(), description: input.description.clone(), tags: input.tags.clone(), keyword: keyword.map(String::from) });
        let score = async {
            match keyword {
                Some(k) => self.keyword_basic(k, true).await.ok(),
                None => None,
            }
        };
        let (seo, score) = futures::join!(seo, score);
        let video_id = match input.video.as_deref().filter(|v| !v.trim().is_empty()) {
            Some(video) => Some(self.link_video(video).await?),
            None => None,
        };
        let mut prediction = Prediction {
            id: 0,
            created_at: now(),
            title: input.title,
            keyword: keyword.map(String::from),
            overall: score.as_ref().map(|s| s.overall),
            volume: score.as_ref().map(|s| s.volume),
            competition: score.as_ref().map(|s| s.competition),
            seo_score: seo?.score,
            video_id,
        };
        prediction.id = store.add_prediction(prediction.created_at, &prediction)?;
        Ok(prediction)
    }

    pub async fn link_prediction(&self, id: i64, video: &str) -> Result<BacktestReport> {
        let video_id = self.link_video(video).await?;
        let store = self.store.as_ref().ok_or_else(|| anyhow!("predictions need the local database"))?;
        if !store.link_prediction(id, &video_id)? {
            return Err(anyhow!("no prediction with id {id}"));
        }
        self.backtest()
    }

    pub fn backtest(&self) -> Result<BacktestReport> {
        let store = self.store.as_ref().ok_or_else(|| anyhow!("backtesting needs the local database"))?;
        let rows: Vec<BacktestRow> = store
            .predictions()?
            .into_iter()
            .map(|prediction| {
                let Some(video) = prediction.video_id.clone() else {
                    return BacktestRow { prediction, age_hours: None, views_at_7_days: None, status: BacktestStatus::NotLinked };
                };
                let age_hours = store.video_age_hours(&video, now()).ok().flatten();
                let views = store.views_at_age(&video, BACKTEST_AGE_HOURS).ok().flatten();
                let status = match (age_hours, views) {
                    (_, Some(_)) => BacktestStatus::Measured,
                    (Some(age), None) if age < BACKTEST_AGE_HOURS => BacktestStatus::TooYoung,
                    _ => BacktestStatus::NoSnapshotsNearDay7,
                };
                BacktestRow { prediction, age_hours, views_at_7_days: views, status }
            })
            .collect();
        let measured: Vec<&BacktestRow> = rows.iter().filter(|r| r.status == BacktestStatus::Measured).collect();
        let log_views: Vec<f64> = measured.iter().map(|r| (r.views_at_7_days.unwrap_or(0) as f64 + 1.0).log10()).collect();
        let correlate = |score: fn(&Prediction) -> Option<u8>| -> Option<f64> {
            let pairs: Vec<(f64, f64)> = measured.iter().zip(&log_views).filter_map(|(r, y)| Some((score(&r.prediction)? as f64, *y))).collect();
            if pairs.len() < MIN_FOR_CORRELATION {
                return None;
            }
            let (x, y): (Vec<f64>, Vec<f64>) = pairs.into_iter().unzip();
            spearman(&x, &y)
        };
        let note = if measured.len() < MIN_FOR_CORRELATION {
            format!("{} of {} predictions have 7-day views so far; correlations start at {MIN_FOR_CORRELATION}.", measured.len(), rows.len())
        } else {
            format!("Spearman rank correlation between each score and 7-day views across {} uploads (1 = scores ordered uploads perfectly, 0 = no relation).", measured.len())
        };
        Ok(BacktestReport {
            measured: measured.len(),
            spearman_overall: correlate(|p| p.overall),
            spearman_volume: correlate(|p| p.volume),
            spearman_seo: correlate(|p| Some(p.seo_score)),
            note,
            rows,
        })
    }
}
