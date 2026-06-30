//! In-app update checks against GitHub Releases and native install helpers.
//!
//! Flow: background check → tab-bar badge → download signed package → quit → replace →
//! relaunch. macOS consumes a DMG; Linux consumes the AppImage that is currently running.
#![cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::Duration;

/// GitHub repository that publishes release packages (`owner/repo`).
pub const GITHUB_REPO: &str = "HakimIno/plusplus";

/// Workspace version baked in at compile time.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether this installation can safely replace itself. Linux updates are enabled only
/// inside an AppImage; binaries owned by a distro package manager must not overwrite
/// themselves behind that package manager's back.
#[cfg(target_os = "macos")]
pub fn automatic_updates_supported() -> bool {
    true
}

#[cfg(target_os = "linux")]
pub fn automatic_updates_supported() -> bool {
    std::env::var_os("APPIMAGE").is_some()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn automatic_updates_supported() -> bool {
    false
}

/// Minisign public key (the **second**, non-comment line of a `minisign -G` public-key
/// file) that genuine release packages are signed with. A downloaded update is installed only
/// if its detached `.minisig` signature verifies against this key, which roots trust in a
/// private key the maintainer holds offline rather than in GitHub: a tampered or
/// substituted DMG — even on a compromised release or account — can't be signed and is
/// refused.
///
/// **Empty = no key configured → every update is refused (fail closed).** To enable signed
/// updates: run `minisign -G` once, paste the public key's second line here, store the
/// secret key as the CI `MINISIGN_SECRET_KEY`/`MINISIGN_PASSWORD` secrets, and the release
/// workflow publishes a `<package>.minisig` beside each package. See
/// docs/RELEASE_SIGNING.md.
pub const MINISIGN_PUBLIC_KEY: &str = "RWSI1RaK/u6g2lxVL3YxMT8pRzTnQMP1N46eIBdWDVXH7U5kjtAzYIY4";

/// A newer release found on GitHub.
#[derive(Clone, Debug)]
pub struct UpdateOffer {
    pub version: String,
    pub asset_name: String,
    pub download_url: String,
    /// URL of the package's detached minisign signature (`<asset>.minisig`). Empty when the
    /// release published no signature — such an update fails verification and is refused.
    pub signature_url: String,
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
        package_path: PathBuf,
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

#[cfg(target_os = "macos")]
fn release_asset_name(version: &str) -> String {
    format!("plusplus-{version}.dmg")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn release_asset_name(version: &str) -> String {
    format!("plusplus-{version}-x86_64.AppImage")
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn release_asset_name(version: &str) -> String {
    format!("plusplus-{version}-aarch64.AppImage")
}

#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
fn release_asset_name(version: &str) -> String {
    format!("plusplus-{version}-unsupported")
}

/// Query GitHub Releases for a native package newer than [`CURRENT_VERSION`].
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

    let expected_name = release_asset_name(&version);
    let package = release
        .assets
        .iter()
        .find(|a| a.name == expected_name)
        .ok_or_else(|| format!("release v{version} has no {expected_name} asset"))?;

    // The detached signature is published as `<asset-name>.minisig`. A missing one leaves
    // `signature_url` empty; verification then refuses the update at download time rather
    // than installing something unsigned.
    let sig_name = format!("{}.minisig", package.name);
    let signature_url = release
        .assets
        .iter()
        .find(|a| a.name == sig_name)
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_default();

    Ok(Some(UpdateOffer {
        version,
        asset_name: package.name.clone(),
        download_url: package.browser_download_url.clone(),
        signature_url,
        notes: release.body.unwrap_or_default(),
    }))
}

/// Verify `data` (the downloaded DMG bytes) against its detached minisign signature
/// `minisig` (the full `.minisig` file contents) using the built-in [`MINISIGN_PUBLIC_KEY`].
///
/// Fails closed: an unconfigured key, a malformed signature, or a signature that doesn't
/// match all return `Err`, so a caller that only installs on `Ok` can never run an
/// unverified or tampered update.
fn verify_signature(public_key_b64: &str, data: &[u8], minisig: &str) -> Result<(), String> {
    use minisign_verify::{PublicKey, Signature};

    let key = public_key_b64.trim();
    if key.is_empty() {
        return Err(
            "update verification is not configured (no signing key built into this app) — \
             refusing to install"
                .into(),
        );
    }
    let public_key = PublicKey::from_base64(key)
        .map_err(|e| format!("invalid built-in update signing key: {e}"))?;
    let signature =
        Signature::decode(minisig).map_err(|e| format!("malformed update signature: {e}"))?;
    public_key.verify(data, &signature, false).map_err(|_| {
        "update signature does not match the expected signing key — refusing to install".into()
    })
}

/// Stream a release package into the system temp directory, reporting byte progress.
pub async fn download_update(
    offer: &UpdateOffer,
    mut on_progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("plusplus/{CURRENT_VERSION}"))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())?;

    let safe_name = Path::new(&offer.asset_name)
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "release has an invalid asset name".to_string())?;
    let dest = std::env::temp_dir().join(safe_name);
    let partial = dest.with_file_name(format!("{}.partial", safe_name.to_string_lossy()));

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

    // Verify the signature before the file is ever promoted to its final path, so a failed
    // (or absent) signature leaves nothing installable behind. The package stays at `.partial`
    // until it is proven authentic.
    if let Err(e) = verify_download(&partial, &offer.signature_url, &client).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }

    tokio::fs::rename(&partial, &dest)
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    Ok(dest)
}

/// Fetch the detached signature at `signature_url` and verify the file at `package_path`
/// against it with the built-in key. A release that published no signature
/// (`signature_url` empty) is rejected — an unsigned update is never installed.
async fn verify_download(
    package_path: &Path,
    signature_url: &str,
    client: &reqwest::Client,
) -> Result<(), String> {
    if signature_url.trim().is_empty() {
        return Err(
            "this release is not signed — refusing to install an unverified update".into(),
        );
    }
    let minisig = client
        .get(signature_url)
        .send()
        .await
        .map_err(|e| format!("could not fetch update signature: {e}"))?
        .error_for_status()
        .map_err(|e| format!("could not fetch update signature: {e}"))?
        .text()
        .await
        .map_err(|e| format!("could not read update signature: {e}"))?;
    let bytes = tokio::fs::read(package_path)
        .await
        .map_err(|e| format!("could not re-read downloaded update: {e}"))?;
    verify_signature(MINISIGN_PUBLIC_KEY, &bytes, &minisig)
}

/// Spawn a detached installer that waits for this process to exit, then replaces the app.
#[cfg(target_os = "macos")]
pub fn schedule_install_and_quit(package_path: &Path) -> Result<(), String> {
    let dmg = package_path
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

/// Atomically replace the AppImage that launched this process, then relaunch it after the
/// current process exits. Refuse non-AppImage installs: distro packages must be upgraded by
/// their package manager and a development binary has no stable install location.
#[cfg(target_os = "linux")]
pub fn schedule_install_and_quit(package_path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let current = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "automatic install requires the AppImage build; update this installation with its package manager"
                .to_string()
        })?;
    let current = current
        .canonicalize()
        .map_err(|e| format!("could not locate the running AppImage: {e}"))?;
    if !current.is_file() {
        return Err("the running AppImage path is not a regular file".into());
    }

    let file_name = current
        .file_name()
        .ok_or_else(|| "the running AppImage has an invalid path".to_string())?;
    let staged = current.with_file_name(format!("{}.update", file_name.to_string_lossy()));
    std::fs::copy(package_path, &staged)
        .map_err(|e| format!("could not stage update beside the current AppImage: {e}"))?;
    let install_result = (|| -> Result<(), String> {
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not make the update executable: {e}"))?;
        std::fs::rename(&staged, &current)
            .map_err(|e| format!("could not replace the current AppImage: {e}"))?;
        Ok(())
    })();
    if install_result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    install_result?;

    let script_path = std::env::temp_dir().join("plusplus-relaunch-update.sh");
    std::fs::write(
        &script_path,
        r#"#!/bin/sh
pid="$1"
appimage="$2"
while kill -0 "$pid" 2>/dev/null; do sleep 0.25; done
rm -f -- "$0"
exec "$appimage"
"#,
    )
    .map_err(|e| format!("could not create update relaunch helper: {e}"))?;

    std::process::Command::new("nohup")
        .arg("/bin/sh")
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(&current)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start update relaunch helper: {e}"))?;

    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn schedule_install_and_quit(_package_path: &Path) -> Result<(), String> {
    Err("in-app updates are not supported on this platform".into())
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

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn linux_release_uses_versioned_x86_64_appimage() {
        assert_eq!(
            release_asset_name("1.2.3"),
            "plusplus-1.2.3-x86_64.AppImage"
        );
    }

    // A real positive vector would require a minisign keypair + signature generated with the
    // `minisign` CLI, which can't be produced in-tree; the end-to-end happy path is covered by
    // the release workflow signing a DMG that the app then verifies. These tests pin the
    // fail-closed behaviour, which is what protects users.

    #[test]
    fn verify_refuses_when_no_key_is_configured() {
        // An empty built-in key must fail closed, never silently accept.
        let err = verify_signature("", b"anything", "untrusted comment: x\n").unwrap_err();
        assert!(err.contains("not configured"), "got: {err}");
    }

    #[test]
    fn verify_rejects_unparseable_key_or_signature() {
        // A malformed built-in key is refused rather than treated as "no signature needed".
        assert!(verify_signature("not-a-valid-key", b"data", "untrusted comment: x\n").is_err());
        // Likewise garbage signature text can't be decoded → refused.
        assert!(verify_signature("not-a-valid-key", b"data", "not a signature at all").is_err());
    }
}
