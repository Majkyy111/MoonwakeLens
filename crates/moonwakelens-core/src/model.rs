use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct VideoSummary {
    pub id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub views: Option<u64>,
    pub age_hours: Option<f64>,
    #[serde(skip)]
    pub age_precision_hours: Option<f64>,
    pub duration_seconds: Option<u64>,
    pub is_short: bool,
    pub verified_channel: bool,
    pub has_captions: bool,
    pub description_snippet: Option<String>,
    pub views_per_hour: Option<f64>,
}

impl VideoSummary {
    pub fn with_vph(mut self) -> Self {
        self.views_per_hour = crate::metrics::views_per_hour(self.views, self.age_hours);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoDetails {
    pub id: String,
    pub title: String,
    pub description: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub views: u64,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
    pub published: Option<String>,
    pub age_hours: Option<f64>,
    pub duration_seconds: u64,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub hashtags: Vec<String>,
    pub has_chapters: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub description: String,
    pub keywords: Vec<String>,
    pub subscribers: Option<u64>,
    pub video_count: Option<u64>,
    pub recent_videos: Vec<VideoSummary>,
    pub recent_shorts: Vec<VideoSummary>,
}

pub const MIN_BASELINE_AGE_HOURS: f64 = 168.0;
pub const MIN_SHORT_BASELINE_AGE_HOURS: f64 = 72.0;

impl ChannelInfo {
    pub fn baseline(&self, short: bool) -> Option<crate::metrics::Spread> {
        let (videos, min_age) = if short { (&self.recent_shorts, MIN_SHORT_BASELINE_AGE_HOURS) } else { (&self.recent_videos, MIN_BASELINE_AGE_HOURS) };
        let views: Vec<u64> = videos.iter().filter(|v| v.age_hours.is_none_or(|age| age >= min_age)).filter_map(|v| v.views).collect();
        if views.len() < 3 {
            return None;
        }
        crate::metrics::spread(views)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(views: u64, age_hours: Option<f64>) -> VideoSummary {
        VideoSummary { views: Some(views), age_hours, ..Default::default() }
    }

    #[test]
    fn baseline_leaves_out_uploads_that_are_too_young() {
        let channel = ChannelInfo {
            id: String::new(),
            name: String::new(),
            handle: None,
            description: String::new(),
            keywords: vec![],
            subscribers: None,
            video_count: None,
            recent_videos: vec![video(10, Some(5.0)), video(20, Some(30.0)), video(1000, Some(200.0)), video(2000, Some(400.0)), video(3000, Some(900.0))],
            recent_shorts: vec![video(5, Some(10.0)), video(500, None), video(600, Some(100.0)), video(700, None)],
        };
        assert_eq!(channel.baseline(false).map(|s| (s.median, s.n)), Some((2000, 3)));
        assert_eq!(channel.baseline(true).map(|s| (s.median, s.n)), Some((600, 3)));
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ChannelAbout {
    pub total_views: Option<u64>,
    pub country: Option<String>,
    pub joined: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub query: String,
    pub estimated_results: u64,
    pub results: Vec<VideoSummary>,
}
