#![windows_subsystem = "windows"]

mod mcp;
mod server;
mod update;

use moonwakelens_core::Engine;
use server::Launch;
use std::path::PathBuf;
use std::sync::Arc;

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("MoonwakeLens")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    update::remove_previous();
    let engine = Arc::new(Engine::open(&data_dir())?);
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "mcp" => mcp::run(engine).await,
        "track" => {
            if engine.tracking_due() {
                engine.tracking_pass().await?;
            }
            Ok(())
        }
        "serve" => server::run(engine, Launch::Headless).await,
        "--restarted" => server::run(engine, Launch::Restarted).await,
        _ => server::run(engine, Launch::Window).await,
    }
}
