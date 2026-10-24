use crate::analysis::{concurrent, now, Engine};
use crate::parse::video_id;
use crate::store::{Store, Tracked};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::Duration;

const CHANNEL_PAUSE: Duration = Duration::from_secs(2);
const LAST_PASS_KEY: &str = "tracking_last_pass";

#[derive(Debug, Clone, Serialize)]
pub struct TrackingPass {
    pub channels_checked: usize,
    pub videos_checked: usize,
    pub errors: Vec<String>,
    pub finished_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Channel(String),
    Video(String),
}

pub fn classify(input: &str) -> Option<Target> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let channel_like = s.starts_with('@') || s.contains("/@") || s.contains("/channel/") || (s.starts_with("UC") && s.len() == 24);
    if channel_like {
        return Some(Target::Channel(s.into()));
    }
    Some(video_id(s).map_or_else(|| Target::Channel(s.into()), Target::Video))
}

impl Engine {
    fn require_store(&self) -> Result<&Store> {
        self.store.as_ref().ok_or_else(|| anyhow!("tracking needs the local database; open the engine with Engine::open"))
    }

    pub async fn track(&self, input: &str) -> Result<Vec<Tracked>> {
        let store = self.require_store()?;
        match classify(input).ok_or_else(|| anyhow!("give a channel (@handle, link or UC id) or a video link"))? {
            Target::Channel(handle) => {
                let channel = self.load_channel(&self.yt.channel_id(&handle).await?, 30).await?;
                store.track("channel", &channel.id, Some(&channel.name), now())?;
            }
            Target::Video(id) => {
                let video = self.yt.video_basic(&id).await?;
                self.observe_details(&video, video.duration_seconds <= 60);
                store.track("video", &id, Some(&video.title), now())?;
            }
        }
        store.tracked()
    }

    pub async fn untrack(&self, input: &str) -> Result<Vec<Tracked>> {
        let store = self.require_store()?;
        match classify(input).ok_or_else(|| anyhow!("give a channel or a video"))? {
            Target::Channel(handle) => store.untrack("channel", &self.yt.channel_id(&handle).await?)?,
            Target::Video(id) => store.untrack("video", &id)?,
        };
        store.tracked()
    }

    pub fn tracked(&self) -> Result<Vec<Tracked>> {
        self.require_store()?.tracked()
    }

    pub fn tracking_due(&self) -> bool {
        let Some(store) = &self.store else { return false };
        let interval = (self.config().tracking_interval_hours * 3600.0) as i64;
        let last: i64 = store.meta(LAST_PASS_KEY).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        now() - last >= interval && store.tracked().is_ok_and(|t| !t.is_empty())
    }

    pub async fn tracking_pass(&self) -> Result<TrackingPass> {
        let store = self.require_store()?;
        store.set_meta(LAST_PASS_KEY, &now().to_string())?;
        let tracked = store.tracked()?;
        let mut errors = Vec::new();
        let mut channels_checked = 0;
        for item in tracked.iter().filter(|t| t.kind == "channel") {
            match self.load_channel(&item.id, 30).await {
                Ok(channel) => {
                    let ids: Vec<String> = channel.recent_shorts.iter().map(|s| s.id.clone()).collect();
                    let undated = store.without_publish_time(&ids)?;
                    let details = concurrent(undated, |id| async move { self.yt.video_basic(&id).await.ok() }).await;
                    for video in details.into_iter().flatten() {
                        self.observe_details(&video, true);
                    }
                    store.mark_checked("channel", &item.id, now())?;
                    channels_checked += 1;
                }
                Err(error) => errors.push(format!("{}: {error}", item.name.as_deref().unwrap_or(&item.id))),
            }
            tokio::time::sleep(CHANNEL_PAUSE).await;
        }
        let videos: Vec<String> = tracked.iter().filter(|t| t.kind == "video").map(|t| t.id.clone()).collect();
        let results = concurrent(videos, |id| async move { (id.clone(), self.yt.video(&id).await) }).await;
        let mut videos_checked = 0;
        for (id, result) in results {
            match result {
                Ok(video) => {
                    self.observe_details(&video, video.duration_seconds <= 60);
                    store.mark_checked("video", &id, now())?;
                    videos_checked += 1;
                }
                Err(error) => errors.push(format!("{id}: {error}")),
            }
        }
        Ok(TrackingPass { channels_checked, videos_checked, errors, finished_at: now() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_channels_from_videos() {
        assert_eq!(classify("@ItsZyonic"), Some(Target::Channel("@ItsZyonic".into())));
        assert_eq!(classify("https://www.youtube.com/@ItsZyonic/videos"), Some(Target::Channel("https://www.youtube.com/@ItsZyonic/videos".into())));
        assert_eq!(classify("UCGvtdeBCaEsQJJkUpaS19Cw"), Some(Target::Channel("UCGvtdeBCaEsQJJkUpaS19Cw".into())));
        assert_eq!(classify("https://youtu.be/jNQXAC9IVRw"), Some(Target::Video("jNQXAC9IVRw".into())));
        assert_eq!(classify("jNQXAC9IVRw"), Some(Target::Video("jNQXAC9IVRw".into())));
    }
}
