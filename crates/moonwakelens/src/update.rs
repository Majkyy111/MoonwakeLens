use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const REPO: &str = "Majkyy111/MoonwakeLens";
const ASSET: &str = "MoonwakeLens.exe";
const RELEASE_PUBLIC_KEY: &str = "swWauYiBQWH7bzeb3wQXeM3bt/SwNxilZemvZXtcJgg=";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    #[serde(skip)]
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset(&self, name: &str) -> Result<&str> {
        self.assets.iter().find(|(n, _)| n == name).map(|(_, url)| url.as_str()).ok_or_else(|| anyhow!("release v{} has no {name}", self.version))
    }
}

fn sibling(extension: &str) -> Result<PathBuf> {
    Ok(std::env::current_exe()?.with_extension(extension))
}

pub fn remove_previous() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem().and_then(|s| s.to_str())) else { return };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(stem) && name.ends_with(".old") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().user_agent("moonwakelens-updater").timeout(Duration::from_secs(180)).build().expect("http client")
}

fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(remote) > parse(local)
}

pub async fn check() -> Result<Option<Release>> {
    let release: Value = client()
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, VERSION) {
        return Ok(None);
    }
    Ok(Some(Release {
        version: tag.trim_start_matches('v').into(),
        notes: release["body"].as_str().unwrap_or_default().into(),
        assets: release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
            .collect(),
    }))
}

async fn download(url: &str) -> Result<Vec<u8>> {
    Ok(client().get(url).send().await?.error_for_status()?.bytes().await?.to_vec())
}

pub fn verify(exe: &[u8], signature_base64: &str, sha256_hex: &str) -> Result<()> {
    let digest = format!("{:x}", Sha256::digest(exe));
    if !sha256_hex.trim().eq_ignore_ascii_case(&digest) {
        bail!("the downloaded update does not match its published SHA-256");
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = engine.decode(RELEASE_PUBLIC_KEY)?.try_into().map_err(|_| anyhow!("bad release key"))?;
    let signature: [u8; 64] = engine.decode(signature_base64.trim())?.try_into().map_err(|_| anyhow!("the update signature is malformed"))?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(exe, &Signature::from_bytes(&signature))
        .map_err(|_| anyhow!("the update is not signed by the MoonwakeLens release key"))
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
    let (exe, signature, sha256) = tokio::try_join!(
        download(release.asset(ASSET)?),
        download(release.asset(&format!("{ASSET}.sig"))?),
        download(release.asset(&format!("{ASSET}.sha256"))?)
    )?;
    verify(&exe, &String::from_utf8_lossy(&signature), &String::from_utf8_lossy(&sha256))?;
    let (current, new) = (std::env::current_exe()?, sibling("new")?);
    let old = sibling(&format!("{}.old", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()))?;
    std::fs::write(&new, &exe)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
    }

    #[test]
    fn rejects_tampered_or_unsigned_updates() {
        let exe = b"not really an exe";
        let sha = format!("{:x}", Sha256::digest(exe));
        let fake_signature = base64::engine::general_purpose::STANDARD.encode([7u8; 64]);
        assert!(verify(exe, &fake_signature, &sha).unwrap_err().to_string().contains("not signed"));
        assert!(verify(exe, &fake_signature, "00").unwrap_err().to_string().contains("SHA-256"));
    }
}
