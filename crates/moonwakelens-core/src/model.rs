use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct VideoSummary {
    pub id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub views: Option<u64>,
    pub age_hours: Option<f64>,
    pub duration_seconds: Option<u64>,
    pub is_short: bool,
    pub verified_channel: bool,
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

impl ChannelInfo {
    pub fn baseline(&self, short: bool) -> Option<u64> {
        let videos = if short { &self.recent_shorts } else { &self.recent_videos };
        (videos.iter().filter(|v| v.views.is_some()).count() >= 3).then(|| crate::metrics::median_views(videos)).flatten()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub query: String,
    pub estimated_results: u64,
    pub results: Vec<VideoSummary>,
}
