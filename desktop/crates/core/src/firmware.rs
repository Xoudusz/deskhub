//! Board firmware from GitHub Releases. The firmware release workflow publishes
//! `deskhub-<variant>.bin` for each board build plus `firmware.json` (version, and per variant
//! url, sha256, size); the fixed release "firmware-latest" always holds the newest manifest.
//! The app picks the image for the board's variant, checks its hash, and sends it with the
//! usual `POST /firmware` (the board asks "Install?" on screen).
//!
//! Release images carry no Wi-Fi or token, so a board only takes them once a local build has
//! stored its settings in NVS: hello_ok reports `creds: "nvs"`.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::http;
use crate::settings::{State, Status};

pub const MANIFEST_URL: &str =
    "https://github.com/Xoudusz/deskhub/releases/download/firmware-latest/firmware.json";
/// Images only come from this repo's releases. There is no signature: the sha256 in the
/// manifest only proves the download is complete and intact.
const ASSET_PREFIX: &str = "https://github.com/Xoudusz/deskhub/releases/download/firmware-v";

/// Board builds with release images (firmware/build.sh).
pub const VARIANTS: [&str; 3] = ["s3", "p4rev1", "p4rev3"];

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_IMAGE: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    /// The release tag, "firmware-vX.Y.Z" (what boards report as their version).
    pub version: String,
    pub assets: std::collections::BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// What the Firmware card shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Check {
    /// Newest release, "firmware-vX.Y.Z".
    pub latest: Option<String>,
    /// The board runs an older (or unknown) version and can take the release image.
    pub update_available: bool,
    /// Why the board can't take release images yet (None when it can).
    pub blocked: Option<String>,
    pub error: Option<String>,
}

pub fn parse_manifest(text: &str) -> Result<Manifest> {
    let m: Manifest = serde_json::from_str(text).context("firmware.json")?;
    if parse_version(&m.version).is_none() {
        bail!("firmware.json: odd version {:?}", m.version);
    }
    Ok(m)
}

pub fn pick_asset<'a>(m: &'a Manifest, variant: &str) -> Result<&'a Asset> {
    let a = m
        .assets
        .get(variant)
        .ok_or_else(|| anyhow!("no {variant} image in {}", m.version))?;
    if !a.url.starts_with(ASSET_PREFIX) {
        bail!("firmware.json points outside the Desk Hub releases");
    }
    Ok(a)
}

/// "firmware-v1.2.3", "v1.2.3" or "1.2.3", optionally followed by git describe's
/// "-N-gHASH" and/or "-dirty". Returns the numbers and whether anything followed.
pub fn parse_version(v: &str) -> Option<((u64, u64, u64), bool)> {
    let v = v.strip_prefix("firmware-").unwrap_or(v);
    let v = v.strip_prefix('v').unwrap_or(v);
    let (core, rest) = match v.split_once('-') {
        Some((c, r)) => (c, !r.is_empty()),
        None => (v, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let n = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    Some((n, rest))
}

/// True when `latest` is newer than the board's version. A board version that isn't a
/// release (a plain git hash from a local build, "?") counts as older; a local build ahead
/// of a tag ("firmware-v1.2.3-4-gabc") counts as that tag.
pub fn needs_update(board: &str, latest: &str) -> bool {
    let Some((latest, _)) = parse_version(latest) else {
        return false;
    };
    match parse_version(board) {
        Some((board, _)) => board < latest,
        None => true,
    }
}

/// The board's variant when it can take release images, else the reason it can't.
pub fn gate(st: &Status) -> std::result::Result<&str, String> {
    if st.state != State::Connected {
        return Err("Connect to the board first".into());
    }
    if st.creds.as_deref() != Some("nvs") {
        return Err("Flash a local build once (firmware/build.sh with sdkconfig.local); after that updates come from GitHub".into());
    }
    match st.variant.as_deref() {
        Some(v) if VARIANTS.contains(&v) => Ok(v),
        _ => Err(
            "The board's firmware doesn't say which build it is; flash a local build once".into(),
        ),
    }
}

pub fn verify(bytes: &[u8], asset: &Asset) -> Result<()> {
    if bytes.len() as u64 != asset.size {
        bail!(
            "download has {} bytes, expected {}",
            bytes.len(),
            asset.size
        );
    }
    let got = hex(&Sha256::digest(bytes));
    if !got.eq_ignore_ascii_case(asset.sha256.trim()) {
        bail!("download is damaged (sha256 mismatch)");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn fetch(url: &str, timeout: Duration, max: u64) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("http client")?;
    let resp = client
        .get(url)
        .header("User-Agent", "deskhub-desktop")
        .send()
        .await
        .map_err(|e| anyhow!("download failed: {e}"))?;
    if !resp.status().is_success() {
        bail!("download failed ({})", resp.status());
    }
    if resp.content_length().is_some_and(|n| n > max) {
        bail!("download too large");
    }
    // Stream, so a response without Content-Length can't grow past `max`.
    let mut resp = resp;
    let mut out = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow!("download failed: {e}"))?
    {
        out.extend_from_slice(&chunk);
        if out.len() as u64 > max {
            bail!("download too large");
        }
    }
    Ok(out)
}

pub async fn fetch_manifest(url: &str) -> Result<Manifest> {
    let bytes = fetch(url, MANIFEST_TIMEOUT, 64 * 1024).await?;
    parse_manifest(&String::from_utf8_lossy(&bytes))
}

/// The Firmware card's state for this board.
pub async fn check(st: &Status) -> Check {
    let blocked = gate(st).err();
    match fetch_manifest(MANIFEST_URL).await {
        Ok(m) => Check {
            update_available: blocked.is_none()
                && st
                    .firmware
                    .as_deref()
                    .is_some_and(|b| needs_update(b, &m.version)),
            latest: Some(m.version),
            blocked,
            error: None,
        },
        Err(e) => Check {
            blocked,
            error: Some(format!("{e:#}")),
            ..Check::default()
        },
    }
}

/// Download the newest image for this board, check it, and send it (the board asks first).
pub async fn update_board(host: &str, token: &str, st: &Status) -> Result<String> {
    let variant = gate(st).map_err(|e| anyhow!(e))?;
    let manifest = fetch_manifest(MANIFEST_URL).await?;
    let asset = pick_asset(&manifest, variant)?;
    let image = fetch(&asset.url, DOWNLOAD_TIMEOUT, MAX_IMAGE).await?;
    verify(&image, asset)?;
    http::check_firmware(&image)?;
    http::firmware(host, token, image).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Location;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const MANIFEST: &str = r#"{
        "version": "firmware-v0.7.0",
        "assets": {
            "p4rev1": {"url": "https://github.com/Xoudusz/deskhub/releases/download/firmware-v0.7.0/deskhub-p4rev1.bin", "sha256": "AB", "size": 3},
            "s3": {"url": "https://evil.example/deskhub-s3.bin", "sha256": "cd", "size": 4}
        }
    }"#;

    fn connected(variant: Option<&str>, creds: Option<&str>) -> Status {
        Status {
            state: State::Connected,
            host: "deskhub.local".into(),
            detail: String::new(),
            firmware: Some("927b5fd".into()),
            since: Some(1),
            variant: variant.map(str::to_string),
            creds: creds.map(str::to_string),
            location: None::<Location>,
        }
    }

    #[test]
    fn manifest_and_assets() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.version, "firmware-v0.7.0");
        assert_eq!(pick_asset(&m, "p4rev1").unwrap().size, 3);
        assert!(pick_asset(&m, "p4rev3").is_err());
        // Only this repo's releases.
        assert!(pick_asset(&m, "s3").is_err());
        assert!(parse_manifest("{}").is_err());
        assert!(parse_manifest(r#"{"version": "latest", "assets": {}}"#).is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("firmware-v0.7.0"), Some(((0, 7, 0), false)));
        assert_eq!(
            parse_version("v1.2.3-4-gabc-dirty"),
            Some(((1, 2, 3), true))
        );
        assert_eq!(parse_version("927b5fd-dirty"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);

        assert!(needs_update("firmware-v0.6.9", "firmware-v0.7.0"));
        assert!(needs_update("927b5fd-dirty", "firmware-v0.7.0"));
        assert!(needs_update("?", "firmware-v0.7.0"));
        assert!(!needs_update("firmware-v0.7.0", "firmware-v0.7.0"));
        assert!(!needs_update("firmware-v0.7.0-2-gabc", "firmware-v0.7.0"));
        assert!(!needs_update("firmware-v0.10.0", "firmware-v0.9.9"));
        assert!(!needs_update("firmware-v0.6.0", "nonsense"));
    }

    #[test]
    fn gate_needs_nvs_creds_and_a_known_variant() {
        assert_eq!(gate(&connected(Some("p4rev1"), Some("nvs"))), Ok("p4rev1"));
        assert!(gate(&connected(Some("p4rev1"), Some("seed"))).is_err());
        assert!(gate(&connected(None, None)).is_err());
        assert!(gate(&connected(Some("unknown"), Some("nvs"))).is_err());
        let mut st = connected(Some("s3"), Some("nvs"));
        st.state = State::Searching;
        assert!(gate(&st).is_err());
    }

    #[test]
    fn verify_hash_and_size() {
        let data = b"abc";
        // sha256("abc")
        let sum = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        let asset = Asset {
            url: String::new(),
            sha256: sum.into(),
            size: 3,
        };
        verify(data, &asset).unwrap();
        assert!(verify(b"abd", &asset).is_err());
        assert!(verify(
            b"abcd",
            &Asset {
                size: 4,
                ..asset.clone()
            }
        )
        .is_err());
        assert!(verify(data, &Asset { size: 4, ..asset }).is_err());
    }

    /// Serve one HTTP response on a local port.
    async fn serve_once(status: &'static str, body: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(&body).await;
        });
        port
    }

    #[tokio::test]
    async fn fetch_manifest_over_http() {
        let port = serve_once("200 OK", MANIFEST.as_bytes().to_vec()).await;
        let m = fetch_manifest(&format!("http://127.0.0.1:{port}/firmware.json"))
            .await
            .unwrap();
        assert_eq!(m.version, "firmware-v0.7.0");

        let port = serve_once("404 Not Found", b"nope".to_vec()).await;
        assert!(fetch_manifest(&format!("http://127.0.0.1:{port}/x"))
            .await
            .is_err());

        let port = serve_once("200 OK", vec![b'x'; 2000]).await;
        assert!(fetch(
            &format!("http://127.0.0.1:{port}/big"),
            MANIFEST_TIMEOUT,
            1000
        )
        .await
        .is_err());
    }
}
