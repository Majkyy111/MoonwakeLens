use crate::analysis::{concurrent, Engine};
use crate::config::{CalibrationInfo, VolumeInputs, Weights};
use crate::metrics::{least_squares, spearman};
use crate::parse::normalize;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_TERMS: usize = 100;
const MIN_TERMS: usize = 5;
const RIDGE: f64 = 1e-3;
const TRENDS_COVERAGE_NEEDED: f64 = 0.8;

#[derive(Debug, Clone, Deserialize)]
pub struct SearchTermRow {
    pub search_term: String,
    pub views: f64,
    #[serde(default)]
    pub impressions: Option<f64>,
    #[serde(default)]
    pub period: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Fit {
    pub spearman: Option<f64>,
    pub mean_abs_error_log10: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationRow {
    pub search_term: String,
    pub actual_views: f64,
    pub volume_before: u8,
    pub volume_after: u8,
    pub estimated_views_after: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationReport {
    pub terms_in_file: usize,
    pub terms_used: usize,
    pub features: Vec<String>,
    pub before: Fit,
    pub after_in_sample: Fit,
    pub after_cross_validated: Fit,
    pub applied: bool,
    pub weights: Weights,
    pub rows: Vec<CalibrationRow>,
    pub notes: Vec<String>,
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' | ';' | '\t' if !quoted => cells.push(std::mem::take(&mut cell).trim().to_string()),
            _ => cell.push(c),
        }
    }
    cells.push(cell.trim().to_string());
    cells
}

fn number(cell: &str) -> Option<f64> {
    cell.replace([',', ' ', '\u{a0}'], "").parse().ok()
}

pub fn parse_rows(text: &str) -> Result<Vec<SearchTermRow>> {
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('[') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    let mut lines = trimmed.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = split_csv_line(lines.next().ok_or_else(|| anyhow!("the file is empty"))?).iter().map(|h| normalize(&h.replace('_', " "))).collect();
    let find = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let term = find(&["search term", "term", "query", "search query", "youtube search term"]).ok_or_else(|| anyhow!("no search_term column in the header"))?;
    let views = find(&["views"]).ok_or_else(|| anyhow!("no views column in the header"))?;
    let impressions = find(&["impressions"]);
    let period = find(&["period", "date", "date range"]);
    Ok(lines
        .filter_map(|line| {
            let cells = split_csv_line(line);
            Some(SearchTermRow {
                search_term: cells.get(term)?.to_string(),
                views: number(cells.get(views)?)?,
                impressions: impressions.and_then(|i| cells.get(i)).and_then(|c| number(c)),
                period: period.and_then(|i| cells.get(i)).cloned(),
            })
        })
        .filter(|r| !r.search_term.is_empty() && !r.search_term.eq_ignore_ascii_case("total"))
        .collect())
}

fn evaluate(predicted: &[f64], log_views: &[f64], low: f64, high: f64) -> Fit {
    let errors: f64 = predicted.iter().zip(log_views).map(|(p, y)| (low + p * (high - low) - y).abs()).sum();
    Fit { spearman: spearman(predicted, log_views), mean_abs_error_log10: errors / predicted.len() as f64 }
}

pub fn fit_volume(features: &[Vec<f64>], log_views: &[f64]) -> Result<(Vec<f64>, Vec<f64>, Vec<f64>, f64, f64)> {
    let (low, high) = log_views.iter().fold((f64::MAX, f64::MIN), |(lo, hi), y| (lo.min(*y), hi.max(*y)));
    if high - low < 0.1 {
        bail!("all search terms have almost the same views; calibration needs a spread");
    }
    let targets: Vec<f64> = log_views.iter().map(|y| (y - low) / (high - low)).collect();
    let design: Vec<Vec<f64>> = features.iter().map(|f| std::iter::once(1.0).chain(f.iter().copied()).collect()).collect();
    let beta = least_squares(&design, &targets, RIDGE).ok_or_else(|| anyhow!("the fit failed; the inputs are too similar across terms"))?;
    let predict = |row: &[f64], beta: &[f64]| row.iter().zip(beta).map(|(x, b)| x * b).sum::<f64>().clamp(0.0, 1.0);
    let in_sample: Vec<f64> = design.iter().map(|row| predict(row, &beta)).collect();
    let cross_validated: Vec<f64> = (0..design.len())
        .map(|leave| {
            let rows: Vec<Vec<f64>> = design.iter().enumerate().filter(|(i, _)| *i != leave).map(|(_, r)| r.clone()).collect();
            let ys: Vec<f64> = targets.iter().enumerate().filter(|(i, _)| *i != leave).map(|(_, y)| *y).collect();
            least_squares(&rows, &ys, RIDGE).map_or(0.5, |b| predict(&design[leave], &b))
        })
        .collect();
    Ok((beta, in_sample, cross_validated, low, high))
}

impl Engine {
    pub async fn calibrate(&self, rows: Vec<SearchTermRow>, apply: bool, use_trends: bool) -> Result<CalibrationReport> {
        let terms_in_file = rows.len();
        let mut totals: BTreeMap<String, f64> = BTreeMap::new();
        for row in rows.into_iter().filter(|r| r.views > 0.0) {
            *totals.entry(normalize(&row.search_term)).or_default() += row.views;
        }
        let mut notes = Vec::new();
        if totals.len() > MAX_TERMS {
            notes.push(format!("Used the {MAX_TERMS} terms with the most views out of {}.", totals.len()));
        }
        let mut terms: Vec<(String, f64)> = totals.into_iter().collect();
        terms.sort_by(|a, b| b.1.total_cmp(&a.1));
        terms.truncate(MAX_TERMS);
        let inputs = concurrent(terms.clone(), |(term, _)| async move { self.volume_inputs(&term, false).await.ok().map(|(inputs, _)| inputs) }).await;
        let mut measured: Vec<(String, f64, VolumeInputs)> = terms.into_iter().zip(inputs).filter_map(|((t, v), i)| Some((t, v, i?))).collect();
        if use_trends {
            for item in measured.iter_mut() {
                item.2.trends = self.trends_signal(&item.0).await.map(|s| s.volume_input());
            }
        }
        if measured.len() < MIN_TERMS {
            bail!("need at least {MIN_TERMS} search terms with views that YouTube search can score, got {}", measured.len());
        }
        let trends_coverage = measured.iter().filter(|m| m.2.trends.is_some()).count() as f64 / measured.len() as f64;
        let with_trends = use_trends && trends_coverage >= TRENDS_COVERAGE_NEEDED;
        if use_trends && !with_trends {
            notes.push(format!("Google Trends answered for {:.0}% of terms, so the fit leaves it out.", trends_coverage * 100.0));
        }
        let features: Vec<Vec<f64>> = measured
            .iter()
            .map(|(_, _, i)| {
                let mut row = vec![i.demand, i.autocomplete];
                if with_trends {
                    row.push(i.trends.unwrap_or(0.0));
                }
                row
            })
            .collect();
        let log_views: Vec<f64> = measured.iter().map(|m| (m.1 + 1.0).log10()).collect();
        let (beta, in_sample, cross_validated, low, high) = fit_volume(&features, &log_views)?;

        let current = self.weights();
        let before: Vec<f64> = measured.iter().map(|(_, _, i)| current.volume(if with_trends { *i } else { VolumeInputs { trends: None, ..*i } })).collect();
        let before_fit = evaluate(&before, &log_views, low, high);
        let after_cv = evaluate(&cross_validated, &log_views, low, high);
        let mut fitted = current.clone();
        fitted.volume_intercept = beta[0];
        fitted.volume_demand = beta[1];
        fitted.volume_autocomplete = beta[2];
        fitted.volume_trends = if with_trends { beta[3] } else { 0.0 };
        fitted.views_log_low = low;
        fitted.views_log_high = high;
        fitted.calibration = Some(CalibrationInfo {
            calibrated_at: chrono::Utc::now().to_rfc3339(),
            samples: measured.len(),
            features: if with_trends { vec!["demand".into(), "autocomplete".into(), "trends".into()] } else { vec!["demand".into(), "autocomplete".into()] },
            spearman_before: before_fit.spearman,
            spearman_after_cross_validated: after_cv.spearman,
        });
        if after_cv.spearman.unwrap_or(-1.0) <= before_fit.spearman.unwrap_or(-1.0) {
            notes.push("Cross-validated ranking is not better than before; consider not applying these weights.".into());
        }
        if apply {
            self.set_weights(fitted.clone())?;
        }
        Ok(CalibrationReport {
            terms_in_file,
            terms_used: measured.len(),
            features: fitted.calibration.as_ref().map(|c| c.features.clone()).unwrap_or_default(),
            before: before_fit,
            after_in_sample: evaluate(&in_sample, &log_views, low, high),
            after_cross_validated: after_cv,
            applied: apply,
            rows: measured
                .iter()
                .zip(before.iter().zip(&in_sample))
                .map(|((term, views, _), (b, a))| CalibrationRow {
                    search_term: term.clone(),
                    actual_views: *views,
                    volume_before: crate::metrics::percent(*b),
                    volume_after: crate::metrics::percent(*a),
                    estimated_views_after: 10f64.powf(low + a * (high - low)).round() as u64,
                })
                .collect(),
            weights: fitted,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_analytics_style_csv() {
        let csv = "\u{feff}Search term,Views,Impressions,Period\n\"roblox, horror\",\"1,234\",5000,Last 28 days\nTotal,9999,,\nroblox obby,56,,\n";
        let rows = parse_rows(csv).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].search_term, "roblox, horror");
        assert_eq!(rows[0].views, 1234.0);
        assert_eq!(rows[0].impressions, Some(5000.0));
        assert_eq!(rows[1].views, 56.0);
        let json = parse_rows(r#"[{"search_term": "a", "views": 3}]"#).unwrap();
        assert_eq!(json[0].views, 3.0);
    }

    #[test]
    fn fit_recovers_a_planted_relation_and_cross_validates() {
        let features: Vec<Vec<f64>> = (0..20).map(|i| vec![(i % 10) as f64 / 10.0, ((i * 7) % 11) as f64 / 11.0]).collect();
        let log_views: Vec<f64> = features.iter().map(|f| 2.0 + 3.0 * f[0] + 1.0 * f[1]).collect();
        let (_, in_sample, cross_validated, low, high) = fit_volume(&features, &log_views).unwrap();
        assert!(evaluate(&in_sample, &log_views, low, high).spearman.unwrap() > 0.99);
        assert!(evaluate(&cross_validated, &log_views, low, high).spearman.unwrap() > 0.95);
    }
}
