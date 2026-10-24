use crate::update;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use moonwakelens_core::{tools, Engine};

pub const PORT: u16 = 47821;
const INDEX: &str = include_str!("../ui/index.html");
const WINDOW_GONE_AFTER_SECONDS: u64 = 150;

#[derive(Clone, Copy, PartialEq)]
pub enum Launch {
    Window,
    Restarted,
    Headless,
}

#[derive(Clone)]
struct AppState {
    engine: Arc<Engine>,
    last_ping: Arc<AtomicU64>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub async fn run(engine: Arc<Engine>, launch: Launch) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{PORT}");
    let attempts = if launch == Launch::Restarted { 20 } else { 1 };
    let mut listener = None;
    for _ in 0..attempts {
        match TcpListener::bind(("127.0.0.1", PORT)).await {
            Ok(bound) => {
                listener = Some(bound);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    let Some(listener) = listener else {
        if launch == Launch::Window {
            open_window(&url);
            return Ok(());
        }
        anyhow::bail!("port {PORT} is already in use");
    };
    let state = AppState { engine, last_ping: Arc::new(AtomicU64::new(now())) };
    let app = Router::new()
        .route("/", get(|| async { Html(INDEX) }))
        .route("/api/tools", get(|| async { Json(tools::definitions()) }))
        .route("/api/ping", post(ping))
        .route("/api/version", get(version))
        .route("/api/update", post(update_now))
        .route("/api/{tool}", post(call_tool))
        .with_state(state.clone());
    if launch == Launch::Window {
        open_window(&url);
    }
    if launch != Launch::Headless {
        tokio::spawn(exit_when_window_closes(state.last_ping));
    }
    axum::serve(listener, app).await?;
    Ok(())
}

fn open_window(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", "msedge", &format!("--app={url}"), "--window-size=1440,920"])
        .creation_flags(update::CREATE_NO_WINDOW)
        .spawn();
}

async fn exit_when_window_closes(last_ping: Arc<AtomicU64>) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        if now().saturating_sub(last_ping.load(Ordering::Relaxed)) > WINDOW_GONE_AFTER_SECONDS {
            std::process::exit(0);
        }
    }
}

async fn ping(State(state): State<AppState>) -> StatusCode {
    state.last_ping.store(now(), Ordering::Relaxed);
    StatusCode::NO_CONTENT
}

async fn version() -> Json<Value> {
    let available = update::check().await.ok().flatten();
    Json(json!({"version": update::VERSION, "update": available}))
}

async fn update_now() -> (StatusCode, Json<Value>) {
    let release = match update::check().await {
        Ok(Some(release)) => release,
        Ok(None) => return (StatusCode::OK, Json(json!({"latest": true, "version": update::VERSION}))),
        Err(error) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": error.to_string()}))),
    };
    if let Err(error) = update::install(&release).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": error.to_string()})));
    }
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if update::relaunch(&["--restarted"]).is_ok() {
            std::process::exit(0);
        }
    });
    (StatusCode::OK, Json(json!({"updated": release.version})))
}

async fn call_tool(State(state): State<AppState>, Path(tool): Path<String>, Json(args): Json<Value>) -> (StatusCode, Json<Value>) {
    match tools::call(&state.engine, &tool, &args).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))),
    }
}
