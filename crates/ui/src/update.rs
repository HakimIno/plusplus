//! In-app update checks against GitHub Releases and macOS install helpers.
//!
//! Flow: background check → tab-bar badge → download DMG → quit → replace
//! `/Applications/plusplus.app` → relaunch.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// GitHub repository that publishes release DMGs (`owner/repo`).
pub const GITHUB_REPO: &str = "HakimIno/plusplus";

/// Workspace version baked in at compile time.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A newer release found on GitHub.
#[derive(Clone, Debug)]
pub struct UpdateOffer {
    pub version: String,
    pub download_url: String,
    pub notes: String,
}

/// UI-facing update state (driven from background tasks via [`super::AppMessage`]).
#[derive(Clone, Debug, Default)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Available(UpdateOffer),
    Downloading {
        offer: UpdateOffer,
        progress: f32,
    },
    Ready {
        offer: UpdateOffer,
        dmg_path: PathBuf,
    },
    Failed(String),
}

impl UpdatePhase {
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading { .. })
    }
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(serde::Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Compare dotted numeric versions (`0.1.0` / `v0.2.0`). Returns true when `a` is newer than `b`.
pub fn version_gt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let n = va.len().max(vb.len());
    for i in 0..n {
        let a = va.get(i).copied().unwrap_or(0);
        let b = vb.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("plusplus/{CURRENT_VERSION}"))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())
}

fn normalize_version(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_string()
}

/// Query GitHub Releases for a DMG newer than [`CURRENT_VERSION`].
pub async fn check_for_update() -> Result<Option<UpdateOffer>, String> {
    let client = http_client()?;
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!(
            "update check failed: GitHub returned {}",
            resp.status()
        ));
    }

    let release: GhRelease = resp
        .json()
        .await
        .map_err(|e| format!("update check failed: invalid response ({e})"))?;

    let version = normalize_version(&release.tag_name);
    if !version_gt(&version, CURRENT_VERSION) {
        return Ok(None);
    }

    let dmg = release
        .assets
        .iter()
        .find(|a| a.name.starts_with("plusplus-") && a.name.ends_with(".dmg"))
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|a| a.name.ends_with(".dmg"))
        })
        .ok_or_else(|| format!("release v{version} has no .dmg asset"))?;

    Ok(Some(UpdateOffer {
        version,
        download_url: dmg.browser_download_url.clone(),
        notes: release.body.unwrap_or_default(),
    }))
}

/// Stream a release DMG into the system temp directory, reporting byte progress.
pub async fn download_update(
    offer: &UpdateOffer,
    mut on_progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("plusplus/{CURRENT_VERSION}"))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())?;

    let dest = std::env::temp_dir().join(format!("plusplus-{}.dmg", offer.version));
    let partial = dest.with_extension("dmg.partial");

    let resp = client
        .get(&offer.download_url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }

    let total = resp.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download failed: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }

    file.flush()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    drop(file);

    if let Some(total) = total {
        if downloaded < total {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err("download failed: incomplete file".into());
        }
    }

    tokio::fs::rename(&partial, &dest)
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    Ok(dest)
}

/// Spawn a detached installer that waits for this process to exit, then replaces the app.
#[cfg(target_os = "macos")]
pub fn schedule_install_and_quit(dmg_path: &Path) -> Result<(), String> {
    let dmg = dmg_path
        .to_str()
        .ok_or_else(|| "invalid DMG path".to_string())?;
    let pid = std::process::id();
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail
DMG={dmg:?}
PID={pid}
while kill -0 "$PID" 2>/dev/null; do sleep 0.25; done
MOUNT=$(mktemp -d /tmp/plusplus-update.XXXXXX)
cleanup() {{ hdiutil detach "$MOUNT" -quiet 2>/dev/null || true; }}
trap cleanup EXIT
hdiutil attach "$DMG" -mountpoint "$MOUNT" -nobrowse -quiet
APP_SRC="$MOUNT/plusplus.app"
[ -d "$APP_SRC" ] || {{ echo "plusplus.app not found in DMG" >&2; exit 1; }}
rm -rf /Applications/plusplus.app
ditto "$APP_SRC" /Applications/plusplus.app
sleep 0.5
open -a /Applications/plusplus.app
"#
    );

    let script_path = std::env::temp_dir().join("plusplus-install-update.sh");
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;

    std::process::Command::new("nohup")
        .arg("/bin/bash")
        .arg(&script_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start installer: {e}"))?;

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn schedule_install_and_quit(_dmg_path: &Path) -> Result<(), String> {
    Err("in-app updates are only supported on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gt_orders_semver_like_tags() {
        assert!(version_gt("0.2.0", "0.1.0"));
        assert!(version_gt("v0.1.1", "0.1.0"));
        assert!(!version_gt("0.1.0", "0.1.0"));
        assert!(!version_gt("0.1.0", "0.2.0"));
        assert!(version_gt("1.0.0", "0.9.9"));
    }
}
