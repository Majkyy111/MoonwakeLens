#![windows_subsystem = "windows"]

mod mcp;
mod server;
mod update;

use server::Launch;
use std::sync::Arc;
use moonwakelens_core::Engine;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    update::remove_previous();
    let engine = Arc::new(Engine::new("en", "US"));
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "mcp" => {
            tokio::spawn(update::install_in_background());
            mcp::run(engine).await
        }
        "serve" => server::run(engine, Launch::Headless).await,
        "--restarted" => server::run(engine, Launch::Restarted).await,
        _ if update::apply_at_startup().await => Ok(()),
        _ => server::run(engine, Launch::Window).await,
    }
}
