use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const REPO: &str = "Majkyy111/MoonwakeLens";
const ASSET: &str = "MoonwakeLens.exe";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    #[serde(skip)]
    asset_url: String,
}

fn sibling(extension: &str) -> Result<PathBuf> {
    Ok(std::env::current_exe()?.with_extension(extension))
}

pub fn remove_previous() {
    if let Ok(old) = sibling("old") {
        let _ = std::fs::remove_file(old);
    }
}

fn github_token() -> Option<String> {
    let mut git = Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    git.stdin.take()?.write_all(b"protocol=https\nhost=github.com\n\n").ok()?;
    let output = git.wait_with_output().ok()?;
    String::from_utf8(output.stdout).ok()?.lines().find_map(|line| line.strip_prefix("password=").map(String::from))
}

fn request(url: &str, accept: &str) -> reqwest::RequestBuilder {
    let client = reqwest::Client::builder().user_agent("MoonwakeLens-updater").timeout(Duration::from_secs(180)).build().expect("http client");
    let request = client.get(url).header("Accept", accept);
    match github_token() {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(remote) > parse(local)
}

pub async fn check() -> Result<Option<Release>> {
    let release: Value = request(&format!("https://api.github.com/repos/{REPO}/releases/latest"), "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, VERSION) {
        return Ok(None);
    }
    let asset_url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == ASSET)
        .and_then(|a| a["url"].as_str())
        .ok_or_else(|| anyhow!("release {tag} has no {ASSET}"))?;
    Ok(Some(Release {
        version: tag.trim_start_matches('v').into(),
        notes: release["body"].as_str().unwrap_or_default().into(),
        asset_url: asset_url.into(),
    }))
}

pub async fn install(release: &Release) -> Result<()> {
    let lock = sibling("lock")?;
    if std::fs::metadata(&lock).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(600)) {
        let _ = std::fs::remove_file(&lock);
    }
    std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).context("another update is already running")?;
    let result = replace_executable(release).await;
    let _ = std::fs::remove_file(&lock);
    result
}

async fn replace_executable(release: &Release) -> Result<()> {
    let bytes = request(&release.asset_url, "application/octet-stream").send().await?.error_for_status()?.bytes().await?;
    let (current, new, old) = (std::env::current_exe()?, sibling("new")?, sibling("old")?);
    std::fs::write(&new, &bytes)?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current, &old)?;
    if let Err(error) = std::fs::rename(&new, &current) {
        std::fs::rename(&old, &current)?;
        return Err(error.into());
    }
    Ok(())
}

pub fn relaunch(args: &[&str]) -> Result<()> {
    Command::new(std::env::current_exe()?).args(args).spawn()?;
    Ok(())
}

pub async fn apply_at_startup() -> bool {
    let Ok(Ok(Some(release))) = tokio::time::timeout(Duration::from_secs(8), check()).await else { return false };
    install(&release).await.is_ok() && relaunch(&[]).is_ok()
}

pub async fn install_in_background() {
    if let Ok(Some(release)) = check().await {
        let _ = install(&release).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compares_versions() {
        assert!(super::is_newer("v0.2.0", "0.1.9"));
        assert!(super::is_newer("0.1.10", "0.1.9"));
        assert!(!super::is_newer("v0.1.0", "0.1.0"));
    }
}
