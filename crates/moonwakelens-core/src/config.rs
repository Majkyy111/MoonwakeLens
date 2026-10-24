use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub client_version: String,
    pub hl: String,
    pub gl: String,
    pub trends_anchor: String,
    pub tracking_interval_hours: f64,
    pub weights: Weights,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_version: "2.20261020.00.00".into(),
            hl: "en".into(),
            gl: "US".into(),
            trends_anchor: "minecraft".into(),
            tracking_interval_hours: 4.0,
            weights: Weights::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            let config = Self::default();
            config.save(path)?;
            return Ok(config);
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Weights {
    pub volume_intercept: f64,
    pub volume_demand: f64,
    pub volume_autocomplete: f64,
    pub volume_trends: f64,
    pub demand_log_low: f64,
    pub demand_log_high: f64,
    pub views_log_low: f64,
    pub views_log_high: f64,
    pub competition_title: f64,
    pub competition_verified: f64,
    pub competition_strength: f64,
    pub strength_log_low: f64,
    pub strength_log_high: f64,
    pub overall_volume: f64,
    pub overall_competition: f64,
    pub calibration: Option<CalibrationInfo>,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            volume_intercept: 0.0,
            volume_demand: 0.5,
            volume_autocomplete: 0.35,
            volume_trends: 0.15,
            demand_log_low: 3.0,
            demand_log_high: 7.0,
            views_log_low: 2.0,
            views_log_high: 6.0,
            competition_title: 0.35,
            competition_verified: 0.35,
            competition_strength: 0.3,
            strength_log_low: -1.0,
            strength_log_high: 1.0,
            overall_volume: 0.6,
            overall_competition: 0.4,
            calibration: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationInfo {
    pub calibrated_at: String,
    pub samples: usize,
    pub features: Vec<String>,
    pub spearman_before: Option<f64>,
    pub spearman_after_cross_validated: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeInputs {
    pub demand: f64,
    pub autocomplete: f64,
    pub trends: Option<f64>,
}

impl Weights {
    pub fn volume(&self, inputs: VolumeInputs) -> f64 {
        let (trends_weight, trends) = match inputs.trends {
            Some(t) => (self.volume_trends, t),
            None => (0.0, 0.0),
        };
        let linear = self.volume_demand * inputs.demand + self.volume_autocomplete * inputs.autocomplete + trends_weight * trends;
        if self.calibration.is_some() {
            return (self.volume_intercept + linear).clamp(0.0, 1.0);
        }
        let total = self.volume_demand + self.volume_autocomplete + trends_weight;
        (self.volume_intercept + linear / total.max(f64::EPSILON)).clamp(0.0, 1.0)
    }

    pub fn competition(&self, title_match: f64, verified: f64, strength: f64) -> f64 {
        (self.competition_title * title_match + self.competition_verified * verified + self.competition_strength * strength).clamp(0.0, 1.0)
    }

    pub fn overall(&self, volume: f64, competition: f64) -> f64 {
        (self.overall_volume * volume + self.overall_competition * (1.0 - competition)).clamp(0.0, 1.0)
    }

    pub fn estimated_views(&self, volume: f64) -> Option<u64> {
        self.calibration.as_ref().map(|_| 10f64.powf(self.views_log_low + volume * (self.views_log_high - self.views_log_low)).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncalibrated_volume_renormalizes_without_trends() {
        let w = Weights::default();
        let with = w.volume(VolumeInputs { demand: 1.0, autocomplete: 1.0, trends: Some(1.0) });
        let without = w.volume(VolumeInputs { demand: 1.0, autocomplete: 1.0, trends: None });
        assert!((with - 1.0).abs() < 1e-9 && (without - 1.0).abs() < 1e-9);
    }

    #[test]
    fn config_round_trips_and_fills_missing_fields() {
        let partial: Config = serde_json::from_str(r#"{"client_version": "2.1", "weights": {"volume_demand": 0.9}}"#).unwrap();
        assert_eq!(partial.client_version, "2.1");
        assert_eq!(partial.weights.volume_demand, 0.9);
        assert_eq!(partial.weights.volume_autocomplete, 0.35);
        assert_eq!(partial.hl, "en");
    }
}
