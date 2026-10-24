use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Mutex;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";
pub const SOURCE: &str = "Google Trends, YouTube Search (unofficial endpoint)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Rising,
    Flat,
    Falling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendsSignal {
    pub keyword: String,
    pub anchor: String,
    pub interest_vs_anchor: f64,
    pub change_percent: f64,
    pub direction: Direction,
    pub days: usize,
    pub source: String,
}

impl TrendsSignal {
    pub fn volume_input(&self) -> f64 {
        let level = crate::metrics::norm_log(self.interest_vs_anchor, -3.0, 0.0);
        (level * (1.0 + 0.3 * (self.change_percent / 100.0).clamp(-1.0, 1.0))).clamp(0.0, 1.0)
    }
}

pub struct Trends {
    http: reqwest::Client,
    warmed: Mutex<bool>,
}

impl Default for Trends {
    fn default() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .cookie_store(true)
                .timeout(Duration::from_secs(20))
                .build()
                .expect("http client"),
            warmed: Mutex::new(false),
        }
    }
}

fn strip_guard(body: &str) -> Result<Value> {
    let start = body.find('{').ok_or_else(|| anyhow!("unexpected Google Trends response"))?;
    Ok(serde_json::from_str(&body[start..])?)
}

pub fn signal_from_timelines(keyword: &str, anchor: &str, alone: &[f64], against_anchor: &[(f64, f64)]) -> Option<TrendsSignal> {
    if alone.len() < 14 {
        return None;
    }
    let keyword_total: f64 = against_anchor.iter().map(|p| p.0).sum();
    let anchor_total: f64 = against_anchor.iter().map(|p| p.1).sum();
    let window = (alone.len() / 3).max(7);
    let average = |points: &[f64]| points.iter().sum::<f64>() / points.len() as f64;
    let (early, late) = (average(&alone[..window]), average(&alone[alone.len() - window..]));
    let change_percent = if early > 0.0 { (late - early) / early * 100.0 } else if late > 0.0 { 100.0 } else { 0.0 };
    let direction = if change_percent >= 15.0 {
        Direction::Rising
    } else if change_percent <= -15.0 {
        Direction::Falling
    } else {
        Direction::Flat
    };
    Some(TrendsSignal {
        keyword: keyword.into(),
        anchor: anchor.into(),
        interest_vs_anchor: if anchor_total > 0.0 { keyword_total / anchor_total } else { 0.0 },
        change_percent,
        direction,
        days: alone.len(),
        source: SOURCE.into(),
    })
}

impl Trends {
    async fn get(&self, url: &str, query: &[(&str, &str)]) -> Result<String> {
        let response = self.http.get(url).query(query).send().await?;
        if response.status().as_u16() == 429 {
            return Err(anyhow!("Google Trends is rate limiting (429)"));
        }
        Ok(response.error_for_status()?.text().await?)
    }

    pub async fn youtube_interest(&self, keyword: &str, anchor: &str, geo: &str) -> Result<Option<TrendsSignal>> {
        {
            let mut warmed = self.warmed.lock().await;
            if !*warmed {
                self.get("https://trends.google.com/trends/", &[("geo", geo)]).await?;
                *warmed = true;
            }
        }
        let alone: Vec<f64> = self.timeline(&[keyword], geo).await?.into_iter().filter_map(|p| p.first().copied()).collect();
        let against: Vec<(f64, f64)> = self.timeline(&[keyword, anchor], geo).await?.into_iter().filter_map(|p| Some((*p.first()?, *p.get(1)?))).collect();
        Ok(signal_from_timelines(keyword, anchor, &alone, &against))
    }

    async fn timeline(&self, keywords: &[&str], geo: &str) -> Result<Vec<Vec<f64>>> {
        let items: Vec<Value> = keywords.iter().map(|k| json!({"keyword": k, "geo": geo, "time": "today 3-m"})).collect();
        let request = json!({"comparisonItem": items, "category": 0, "property": "youtube"});
        let explore = strip_guard(&self.get("https://trends.google.com/trends/api/explore", &[("hl", "en-US"), ("tz", "0"), ("req", &request.to_string())]).await?)?;
        let widget = explore["widgets"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["id"] == "TIMESERIES"))
            .ok_or_else(|| anyhow!("Google Trends returned no time series"))?;
        let multiline = strip_guard(
            &self
                .get(
                    "https://trends.google.com/trends/api/widgetdata/multiline",
                    &[("hl", "en-US"), ("tz", "0"), ("req", &widget["request"].to_string()), ("token", widget["token"].as_str().unwrap_or_default())],
                )
                .await?,
        )?;
        Ok(multiline["default"]["timelineData"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["value"].as_array().map(|values| values.iter().filter_map(Value::as_f64).collect()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_direction_alone_and_level_against_the_anchor() {
        let rising: Vec<f64> = (0..90).map(|i| 10.0 + i as f64).collect();
        let quantized: Vec<(f64, f64)> = (0..90).map(|i| (if i < 30 { 1.0 } else { 0.0 }, 100.0)).collect();
        let signal = signal_from_timelines("k", "a", &rising, &quantized).unwrap();
        assert_eq!(signal.direction, Direction::Rising, "direction comes from the keyword's own series");
        assert!((signal.interest_vs_anchor - 30.0 / 9000.0).abs() < 1e-12);
        let flat = vec![50.0; 90];
        assert_eq!(signal_from_timelines("k", "a", &flat, &quantized).unwrap().direction, Direction::Flat);
        assert!(signal_from_timelines("k", "a", &flat[..5], &quantized).is_none());
    }
}
