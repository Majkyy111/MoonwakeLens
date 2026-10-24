use moonwakelens_core::{Config, Engine, Kind, Window};

fn engine() -> Engine {
    Engine::new(Config::default())
}

#[tokio::test]
#[ignore = "network: checks YouTube's live response format"]
async fn video_details_still_parse() {
    let video = engine().yt.video("jNQXAC9IVRw").await.unwrap();
    assert_eq!(video.title, "Me at the zoo");
    assert!(video.views > 100_000_000);
    assert!(video.likes.unwrap_or(0) > 1_000_000, "likes moved: {:?}", video.likes);
    assert!(video.comments.unwrap_or(0) > 100_000, "comment count moved: {:?}", video.comments);
    assert!(video.age_hours.unwrap_or(0.0) > 150_000.0);
}

#[tokio::test]
#[ignore = "network: checks YouTube's live response format"]
async fn channel_tabs_still_parse() {
    let channel = engine().yt.channel("@MrBeast", 30).await.unwrap();
    assert!(channel.subscribers.unwrap_or(0) > 100_000_000);
    assert!(channel.recent_videos.len() >= 10, "videos tab: {}", channel.recent_videos.len());
    assert!(channel.recent_videos.iter().all(|v| v.views.is_some() && v.age_hours.is_some()));
    assert!(!channel.recent_shorts.is_empty(), "shorts tab returned nothing");
}

#[tokio::test]
#[ignore = "network: checks YouTube's live response format"]
async fn filtered_search_still_parses() {
    let page = engine().yt.search_filtered("minecraft", Window::Week, Kind::Video, 1).await.unwrap();
    let complete = page.results.iter().filter(|v| v.views.is_some() && v.age_hours.is_some() && v.channel_id.is_some()).count();
    assert!(complete >= 10, "only {complete} complete results");
    assert!(page.estimated_results > 0);
    let shorts = engine().yt.search_filtered("minecraft", Window::Week, Kind::Short, 1).await.unwrap();
    let with_views = shorts.results.iter().filter(|v| v.is_short && v.views.is_some()).count();
    assert!(with_views >= 5, "{with_views} of {} Shorts results have views", shorts.results.len());
}

#[tokio::test]
#[ignore = "network: checks YouTube's live response format"]
async fn autocomplete_and_about_still_parse() {
    let engine = engine();
    assert!(!engine.yt.autocomplete("minecraft").await.unwrap().is_empty());
    let about = engine.yt.channel_about("UCX6OQ3DkcsbYNE6H8uQQuVA").await.unwrap();
    assert!(about.total_views.unwrap_or(0) > 1_000_000_000);
}
