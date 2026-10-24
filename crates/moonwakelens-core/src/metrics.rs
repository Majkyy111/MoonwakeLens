use crate::model::VideoSummary;
use serde::Serialize;

pub fn views_per_hour(views: Option<u64>, age_hours: Option<f64>) -> Option<f64> {
    Some(views? as f64 / age_hours?.max(1.0))
}

pub fn rate(part: Option<u64>, views: u64) -> Option<f64> {
    part.filter(|_| views > 0).map(|p| p as f64 * 100.0 / views as f64)
}

pub fn median(values: Vec<u64>) -> Option<u64> {
    spread(values).map(|s| s.median)
}

pub fn median_views(videos: &[VideoSummary]) -> Option<u64> {
    median(videos.iter().filter_map(|v| v.views).collect())
}

pub fn multiple(views: Option<u64>, baseline: Option<u64>) -> Option<f64> {
    match (views, baseline) {
        (Some(v), Some(b)) if b > 0 => Some(v as f64 / b as f64),
        _ => None,
    }
}

pub fn norm_log(value: f64, low_exp: f64, high_exp: f64) -> f64 {
    ((value.max(1e-9).log10() - low_exp) / (high_exp - low_exp)).clamp(0.0, 1.0)
}

pub fn share<T>(items: &[T], test: impl Fn(&T) -> bool) -> f64 {
    if items.is_empty() {
        0.0
    } else {
        items.iter().filter(|i| test(i)).count() as f64 / items.len() as f64
    }
}

pub fn percent(ratio: f64) -> u8 {
    (ratio.clamp(0.0, 1.0) * 100.0).round() as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Spread {
    pub median: u64,
    pub q1: u64,
    pub q3: u64,
    pub n: usize,
}

fn quantile(sorted: &[u64], q: f64) -> u64 {
    let position = q * (sorted.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    (sorted[low] as f64 + (sorted[high] as f64 - sorted[low] as f64) * (position - low as f64)).round() as u64
}

pub fn spread(mut values: Vec<u64>) -> Option<Spread> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(Spread { median: quantile(&values, 0.5), q1: quantile(&values, 0.25), q3: quantile(&values, 0.75), n: values.len() })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn from_sample(n: usize) -> Self {
        match n {
            0..=9 => Confidence::Low,
            10..=14 => Confidence::Medium,
            _ => Confidence::High,
        }
    }
}

pub const RANGE_BELOW_SAMPLE: usize = 10;

pub fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut ranks = vec![0.0; values.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        let average = (i + j) as f64 / 2.0;
        for k in i..=j {
            ranks[order[k]] = average;
        }
        i = j + 1;
    }
    ranks
}

pub fn pearson(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len() as f64;
    if x.len() < 3 || x.len() != y.len() {
        return None;
    }
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let cov: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let (vx, vy): (f64, f64) = (x.iter().map(|a| (a - mx).powi(2)).sum(), y.iter().map(|b| (b - my).powi(2)).sum());
    (vx > 0.0 && vy > 0.0).then(|| cov / (vx * vy).sqrt())
}

pub fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    pearson(&ranks(x), &ranks(y))
}

pub fn least_squares(rows: &[Vec<f64>], targets: &[f64], ridge: f64) -> Option<Vec<f64>> {
    let k = rows.first()?.len();
    let mut a = vec![vec![0.0; k + 1]; k];
    for (row, target) in rows.iter().zip(targets) {
        for i in 0..k {
            for j in 0..k {
                a[i][j] += row[i] * row[j];
            }
            a[i][k] += row[i] * target;
        }
    }
    for (i, row) in a.iter_mut().enumerate().skip(1) {
        row[i] += ridge;
    }
    for col in 0..k {
        let pivot = (col..k).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        for row in 0..k {
            if row != col {
                let factor = a[row][col] / a[col][col];
                for c in col..=k {
                    a[row][c] -= factor * a[col][c];
                }
            }
        }
    }
    Some((0..k).map(|i| a[i][k] / a[i][i]).collect())
}

pub fn interpolate(points: &[(f64, u64)], x: f64) -> Option<u64> {
    let before = points.iter().filter(|p| p.0 <= x).max_by(|a, b| a.0.total_cmp(&b.0));
    let after = points.iter().filter(|p| p.0 >= x).min_by(|a, b| a.0.total_cmp(&b.0));
    match (before, after) {
        (Some(b), Some(a)) if a.0 > b.0 => Some((b.1 as f64 + (a.1 as f64 - b.1 as f64) * (x - b.0) / (a.0 - b.0)).round() as u64),
        (Some(b), Some(_)) => Some(b.1),
        (Some(b), None) if x - b.0 <= x * 0.15 => Some(b.1),
        (None, Some(a)) if a.0 - x <= x * 0.15 => Some(a.1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_quartiles() {
        let s = spread(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]).unwrap();
        assert_eq!((s.q1, s.median, s.q3, s.n), (3, 5, 7, 9));
    }

    #[test]
    fn spearman_handles_ties_and_monotonic_data() {
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[10.0, 20.0, 30.0, 400.0]).unwrap() - 1.0).abs() < 1e-9);
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[4.0, 3.0, 2.0, 1.0]).unwrap() + 1.0).abs() < 1e-9);
        assert_eq!(ranks(&[5.0, 1.0, 5.0]), vec![1.5, 0.0, 1.5]);
    }

    #[test]
    fn least_squares_recovers_a_line() {
        let rows: Vec<Vec<f64>> = (0..10).map(|i| vec![1.0, i as f64, (i * i % 7) as f64]).collect();
        let targets: Vec<f64> = rows.iter().map(|r| 2.0 + 0.5 * r[1] - 0.25 * r[2]).collect();
        let beta = least_squares(&rows, &targets, 0.0).unwrap();
        assert!((beta[0] - 2.0).abs() < 1e-6 && (beta[1] - 0.5).abs() < 1e-6 && (beta[2] + 0.25).abs() < 1e-6);
    }

    #[test]
    fn interpolates_views_at_age() {
        let points = [(10.0, 100), (30.0, 300)];
        assert_eq!(interpolate(&points, 20.0), Some(200));
        assert_eq!(interpolate(&points, 34.0), Some(300));
        assert_eq!(interpolate(&points, 60.0), None);
    }
}
