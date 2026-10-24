use crate::model::VideoSummary;

pub fn views_per_hour(views: Option<u64>, age_hours: Option<f64>) -> Option<f64> {
    Some(views? as f64 / age_hours?.max(1.0))
}

pub fn rate(part: Option<u64>, views: u64) -> Option<f64> {
    part.filter(|_| views > 0).map(|p| p as f64 * 100.0 / views as f64)
}

pub fn median(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 { (values[mid - 1] + values[mid]) / 2 } else { values[mid] })
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
    ((value.max(1.0).log10() - low_exp) / (high_exp - low_exp)).clamp(0.0, 1.0)
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
