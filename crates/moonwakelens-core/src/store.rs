use crate::metrics::interpolate;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

const SNAPSHOT_SPACING_SECONDS: i64 = 20 * 60;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS videos (
    id TEXT PRIMARY KEY,
    channel_id TEXT,
    title TEXT,
    is_short INTEGER NOT NULL DEFAULT 0,
    published_at INTEGER,
    published_precision REAL
);
CREATE INDEX IF NOT EXISTS videos_channel ON videos(channel_id);
CREATE TABLE IF NOT EXISTS snapshots (
    video_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    views INTEGER NOT NULL,
    likes INTEGER,
    comments INTEGER,
    PRIMARY KEY (video_id, at)
);
CREATE TABLE IF NOT EXISTS tracked (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT,
    added_at INTEGER NOT NULL,
    last_checked INTEGER,
    PRIMARY KEY (kind, id)
);
CREATE TABLE IF NOT EXISTS keyword_history (
    keyword TEXT NOT NULL,
    day TEXT NOT NULL,
    overall INTEGER NOT NULL,
    volume INTEGER NOT NULL,
    competition INTEGER NOT NULL,
    PRIMARY KEY (keyword, day)
);
CREATE TABLE IF NOT EXISTS predictions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    title TEXT NOT NULL,
    keyword TEXT,
    overall INTEGER,
    volume INTEGER,
    competition INTEGER,
    seo_score INTEGER NOT NULL,
    video_id TEXT
);
CREATE TABLE IF NOT EXISTS cache (key TEXT PRIMARY KEY, at INTEGER NOT NULL, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub video_id: String,
    pub channel_id: Option<String>,
    pub title: String,
    pub is_short: bool,
    pub published_at: Option<i64>,
    pub published_precision_hours: Option<f64>,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RecentGrowth {
    pub views_per_hour: f64,
    pub hours: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tracked {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub added_at: i64,
    pub last_checked: Option<i64>,
    pub snapshots: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordPoint {
    pub day: String,
    pub overall: u8,
    pub volume: u8,
    pub competition: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prediction {
    pub id: i64,
    pub created_at: i64,
    pub title: String,
    pub keyword: Option<String>,
    pub overall: Option<u8>,
    pub volume: Option<u8>,
    pub competition: Option<u8>,
    pub seo_score: u8,
    pub video_id: Option<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn record(&self, at: i64, observations: &[Observation]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for o in observations {
            tx.execute(
                "INSERT INTO videos (id, channel_id, title, is_short, published_at, published_precision) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                   channel_id = COALESCE(excluded.channel_id, channel_id),
                   title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE title END,
                   is_short = MAX(is_short, excluded.is_short),
                   published_at = CASE WHEN excluded.published_at IS NOT NULL AND (published_precision IS NULL OR excluded.published_precision < published_precision)
                                       THEN excluded.published_at ELSE published_at END,
                   published_precision = CASE WHEN excluded.published_at IS NOT NULL AND (published_precision IS NULL OR excluded.published_precision < published_precision)
                                       THEN excluded.published_precision ELSE published_precision END",
                params![o.video_id, o.channel_id, o.title, o.is_short, o.published_at, o.published_precision_hours],
            )?;
            let Some(views) = o.views else { continue };
            let recent: Option<i64> =
                tx.query_row("SELECT MAX(at) FROM snapshots WHERE video_id = ?1", params![o.video_id], |r| r.get(0)).optional()?.flatten();
            if recent.is_none_or(|last| at - last >= SNAPSHOT_SPACING_SECONDS) {
                tx.execute(
                    "INSERT OR REPLACE INTO snapshots (video_id, at, views, likes, comments) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![o.video_id, at, views as i64, o.likes.map(|v| v as i64), o.comments.map(|v| v as i64)],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn snapshots(conn: &Connection, video_id: &str) -> Result<Vec<(i64, u64)>> {
        let mut statement = conn.prepare_cached("SELECT at, views FROM snapshots WHERE video_id = ?1 ORDER BY at")?;
        let rows = statement.query_map(params![video_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn snapshot_count(&self, video_id: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT COUNT(*) FROM snapshots WHERE video_id = ?1", params![video_id], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn recent_growth(&self, video_id: &str) -> Result<Option<RecentGrowth>> {
        let conn = self.conn.lock().unwrap();
        let points = Self::snapshots(&conn, video_id)?;
        let Some(&(last_at, last_views)) = points.last() else { return Ok(None) };
        let target = last_at - 24 * 3600;
        let earlier = points
            .iter()
            .filter(|(at, _)| (last_at - 36 * 3600..=last_at - 12 * 3600).contains(at))
            .min_by_key(|(at, _)| (at - target).abs());
        Ok(earlier.map(|&(at, views)| {
            let hours = (last_at - at) as f64 / 3600.0;
            RecentGrowth { views_per_hour: last_views.saturating_sub(views) as f64 / hours, hours }
        }))
    }

    fn age_points(conn: &Connection, video_id: &str) -> Result<Vec<(f64, u64)>> {
        let published: Option<i64> =
            conn.query_row("SELECT published_at FROM videos WHERE id = ?1", params![video_id], |r| r.get(0)).optional()?.flatten();
        let Some(published) = published else { return Ok(Vec::new()) };
        Ok(Self::snapshots(conn, video_id)?.into_iter().map(|(at, views)| ((at - published) as f64 / 3600.0, views)).collect())
    }

    pub fn views_at_age(&self, video_id: &str, age_hours: f64) -> Result<Option<u64>> {
        let conn = self.conn.lock().unwrap();
        Ok(interpolate(&Self::age_points(&conn, video_id)?, age_hours))
    }

    pub fn channel_views_at_age(&self, channel_id: &str, is_short: bool, age_hours: f64, exclude: &str) -> Result<Vec<u64>> {
        let conn = self.conn.lock().unwrap();
        let ids: Vec<String> = {
            let mut statement = conn.prepare_cached(
                "SELECT id FROM videos WHERE channel_id = ?1 AND is_short = ?2 AND id <> ?3 AND published_at IS NOT NULL AND published_precision <= 24",
            )?;
            let rows = statement.query_map(params![channel_id, is_short, exclude], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut values = Vec::new();
        for id in ids {
            if let Some(views) = interpolate(&Self::age_points(&conn, &id)?, age_hours) {
                values.push(views);
            }
        }
        Ok(values)
    }

    pub fn video_age_hours(&self, video_id: &str, now: i64) -> Result<Option<f64>> {
        let conn = self.conn.lock().unwrap();
        let published: Option<i64> =
            conn.query_row("SELECT published_at FROM videos WHERE id = ?1", params![video_id], |r| r.get(0)).optional()?.flatten();
        Ok(published.map(|p| (now - p) as f64 / 3600.0))
    }

    pub fn without_publish_time(&self, ids: &[String]) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare_cached("SELECT published_precision FROM videos WHERE id = ?1")?;
        let mut missing = Vec::new();
        for id in ids {
            let precision: Option<f64> = statement.query_row(params![id], |r| r.get(0)).optional()?.flatten();
            if precision.is_none_or(|p| p > 24.0) {
                missing.push(id.clone());
            }
        }
        Ok(missing)
    }

    pub fn track(&self, kind: &str, id: &str, name: Option<&str>, at: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO tracked (kind, id, name, added_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(kind, id) DO UPDATE SET name = COALESCE(excluded.name, name)",
            params![kind, id, name, at],
        )?;
        Ok(())
    }

    pub fn untrack(&self, kind: &str, id: &str) -> Result<bool> {
        Ok(self.conn.lock().unwrap().execute("DELETE FROM tracked WHERE kind = ?1 AND id = ?2", params![kind, id])? > 0)
    }

    pub fn mark_checked(&self, kind: &str, id: &str, at: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("UPDATE tracked SET last_checked = ?3 WHERE kind = ?1 AND id = ?2", params![kind, id, at])?;
        Ok(())
    }

    pub fn tracked(&self) -> Result<Vec<Tracked>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT t.kind, t.id, t.name, t.added_at, t.last_checked,
                    CASE t.kind WHEN 'video' THEN (SELECT COUNT(*) FROM snapshots s WHERE s.video_id = t.id)
                                ELSE (SELECT COUNT(*) FROM snapshots s JOIN videos v ON v.id = s.video_id WHERE v.channel_id = t.id) END
             FROM tracked t ORDER BY t.added_at",
        )?;
        let rows = statement.query_map([], |r| {
            Ok(Tracked { kind: r.get(0)?, id: r.get(1)?, name: r.get(2)?, added_at: r.get(3)?, last_checked: r.get(4)?, snapshots: r.get::<_, i64>(5)? as u64 })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_keyword(&self, keyword: &str, day: &str, overall: u8, volume: u8, competition: u8) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO keyword_history (keyword, day, overall, volume, competition) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![keyword, day, overall, volume, competition],
        )?;
        Ok(())
    }

    pub fn keyword_history(&self, keyword: &str) -> Result<Vec<KeywordPoint>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare_cached("SELECT day, overall, volume, competition FROM keyword_history WHERE keyword = ?1 ORDER BY day")?;
        let rows = statement.query_map(params![keyword], |r| Ok(KeywordPoint { day: r.get(0)?, overall: r.get(1)?, volume: r.get(2)?, competition: r.get(3)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_prediction(&self, at: i64, prediction: &Prediction) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO predictions (created_at, title, keyword, overall, volume, competition, seo_score, video_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![at, prediction.title, prediction.keyword, prediction.overall, prediction.volume, prediction.competition, prediction.seo_score, prediction.video_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn link_prediction(&self, id: i64, video_id: &str) -> Result<bool> {
        Ok(self.conn.lock().unwrap().execute("UPDATE predictions SET video_id = ?2 WHERE id = ?1", params![id, video_id])? > 0)
    }

    pub fn predictions(&self) -> Result<Vec<Prediction>> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT id, created_at, title, keyword, overall, volume, competition, seo_score, video_id FROM predictions ORDER BY id DESC")?;
        let rows = statement.query_map([], |r| {
            Ok(Prediction {
                id: r.get(0)?,
                created_at: r.get(1)?,
                title: r.get(2)?,
                keyword: r.get(3)?,
                overall: r.get(4)?,
                volume: r.get(5)?,
                competition: r.get(6)?,
                seo_score: r.get(7)?,
                video_id: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cached(&self, key: &str, max_age_seconds: i64, now: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row("SELECT value FROM cache WHERE key = ?1 AND at >= ?2", params![key, now - max_age_seconds], |r| r.get(0))
            .optional()?)
    }

    pub fn cache(&self, key: &str, value: &str, now: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("INSERT OR REPLACE INTO cache (key, at, value) VALUES (?1, ?2, ?3)", params![key, now, value])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(store: &Store, at: i64, id: &str, views: u64, published: i64) {
        let o = Observation {
            video_id: id.into(),
            channel_id: Some("UCx".into()),
            title: id.into(),
            published_at: Some(published),
            published_precision_hours: Some(1.0),
            views: Some(views),
            ..Default::default()
        };
        store.record(at, &[o]).unwrap();
    }

    #[test]
    fn measures_growth_over_the_last_day() {
        let store = Store::in_memory().unwrap();
        let h = 3600;
        observe(&store, 0, "a", 1_000, -10 * h);
        observe(&store, 10 * h, "a", 2_000, -10 * h);
        observe(&store, 24 * h, "a", 4_400, -10 * h);
        observe(&store, 24 * h + 60, "a", 9_999, -10 * h);
        let growth = store.recent_growth("a").unwrap().unwrap();
        assert_eq!(growth.hours, 24.0);
        assert!((growth.views_per_hour - 3_400.0 / 24.0).abs() < 1e-9);
    }

    #[test]
    fn compares_views_at_the_same_age() {
        let store = Store::in_memory().unwrap();
        let h = 3600;
        observe(&store, 0, "old", 100, -2 * h);
        observe(&store, 20 * h, "old", 500, -2 * h);
        observe(&store, 0, "new", 50, 0);
        assert_eq!(store.views_at_age("old", 12.0).unwrap(), Some(300));
        assert_eq!(store.channel_views_at_age("UCx", false, 12.0, "new").unwrap(), vec![300]);
    }

    #[test]
    fn keeps_the_most_precise_publish_time() {
        let store = Store::in_memory().unwrap();
        let mut o = Observation { video_id: "v".into(), published_at: Some(1000), published_precision_hours: Some(730.0), ..Default::default() };
        store.record(0, &[o.clone()]).unwrap();
        o.published_at = Some(5000);
        o.published_precision_hours = Some(1.0);
        store.record(0, &[o.clone()]).unwrap();
        o.published_at = Some(9000);
        o.published_precision_hours = Some(24.0);
        store.record(0, &[o]).unwrap();
        assert_eq!(store.published_at_for_test("v"), Some(5000));
    }

    impl Store {
        fn published_at_for_test(&self, id: &str) -> Option<i64> {
            self.conn.lock().unwrap().query_row("SELECT published_at FROM videos WHERE id = ?1", params![id], |r| r.get(0)).ok()
        }
    }

    #[test]
    fn tracks_and_predicts() {
        let store = Store::in_memory().unwrap();
        store.track("channel", "UCx", Some("X"), 1).unwrap();
        assert_eq!(store.tracked().unwrap().len(), 1);
        let id = store
            .add_prediction(1, &Prediction { id: 0, created_at: 1, title: "t".into(), keyword: None, overall: Some(50), volume: None, competition: None, seo_score: 70, video_id: None })
            .unwrap();
        assert!(store.link_prediction(id, "abc").unwrap());
        assert_eq!(store.predictions().unwrap()[0].video_id.as_deref(), Some("abc"));
        store.save_keyword("k", "2026-10-24", 50, 60, 40).unwrap();
        assert_eq!(store.keyword_history("k").unwrap().len(), 1);
    }
}
