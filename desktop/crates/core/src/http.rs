//! HTTP calls to the board: screenshot (BMP to PNG), tap, pairing and the root page.

use std::io::Cursor;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use image::ImageFormat;

pub const SCREEN_W: i64 = 800;
pub const SCREEN_H: i64 = 480;

const QUICK: Duration = Duration::from_secs(5);
const SCREENSHOT: Duration = Duration::from_secs(10);
/// The board waits a minute for Allow / Deny.
const PAIR: Duration = Duration::from_secs(75);
/// Upload plus the minute the board waits for "Install" plus writing the image.
const FIRMWARE: Duration = Duration::from_secs(240);

/// Smallest and largest image the board takes (ota.c: 64 KB .. OTA slot).
const FIRMWARE_MIN: usize = 64 * 1024;
const FIRMWARE_MAX: usize = 8 * 1024 * 1024;

/// Send a firmware image (deskhub.bin, not the merged one) to the board. The board asks
/// on screen; the call returns once it installed ("installed; restarting") or refused.
pub async fn firmware(host: &str, token: &str, image: Vec<u8>) -> Result<String> {
    check_firmware(&image)?;
    let url = format!("http://{host}/firmware");
    let client = reqwest::Client::builder()
        .timeout(FIRMWARE)
        .build()
        .context("http client")?;
    let resp = client
        .post(&url)
        .query(&[("token", token)])
        .header("Content-Type", "application/octet-stream")
        .body(image)
        .send()
        .await
        .map_err(|e| anyhow!("board request failed: {}", e.without_url()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("board said no ({status}): {}", text.trim());
    }
    Ok(text.trim().to_string())
}

/// What the board gives on Allow. Debug leaves the token out.
#[derive(Clone, serde::Deserialize, PartialEq, Eq)]
pub struct Paired {
    pub token: String,
    #[serde(default)]
    pub host: String,
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired")
            .field("token", &format_args!("<{} chars>", self.token.len()))
            .field("host", &self.host)
            .finish()
    }
}

/// A random 4-digit code for a pairing prompt: the app shows it, the board shows it, and
/// the user allows only when both match.
pub fn pair_code() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the OS's random source.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    format!("{:04}", h.finish() % 10_000)
}

/// Ask the board for its token (firmware 0.9.0+): it shows "Pair with <name>?" with `code`
/// and waits up to a minute for Allow or Deny. No token is needed to ask.
pub async fn pair(host: &str, name: &str, code: &str) -> Result<Paired> {
    let url = format!("http://{host}/pair");
    let client = reqwest::Client::builder()
        .timeout(PAIR)
        .build()
        .context("http client")?;
    let resp = client
        .post(&url)
        .query(&[("name", name), ("code", code)])
        .send()
        .await
        .map_err(|e| anyhow!("board request failed: {}", e.without_url()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    match status {
        200 => serde_json::from_str(&text).context("the board's pairing answer"),
        403 => bail!("Denied on the board."),
        404 => bail!("This board's firmware can't pair yet: update it to 0.9.0 or newer."),
        408 => bail!("Nobody answered on the board within a minute."),
        429 => bail!("Denied a moment ago: wait 10 seconds and try again."),
        503 => bail!("The board has no token yet: set it up over USB first."),
        _ => bail!("board said no ({status}): {}", text.trim()),
    }
}

/// An ESP app image starts with 0xE9 and has a sane size; a merged image (bootloader at
/// offset 0) is refused because OTA writes only the app slot.
pub fn check_firmware(image: &[u8]) -> Result<()> {
    if image.len() < FIRMWARE_MIN || image.len() > FIRMWARE_MAX {
        bail!(
            "{} KB does not look like a firmware image",
            image.len() / 1024
        );
    }
    if image[0] != 0xE9 {
        bail!("not an ESP32 firmware image (use deskhub.bin, not deskhub-merged.bin)");
    }
    // The app description sits 32 bytes in; its magic word is 0xABCD5432.
    let magic = image
        .get(32..36)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if magic != Some(0xABCD_5432) {
        bail!("not a Desk Hub app image (use deskhub.bin, not deskhub-merged.bin)");
    }
    Ok(())
}

/// The board's screenshot as PNG bytes (800x480). The board sends a 24-bit BMP.
pub async fn screenshot_png(host: &str, token: &str) -> Result<Vec<u8>> {
    let url = format!("http://{host}/screenshot");
    let bmp = get(&url, &[("token", token.to_string())], SCREENSHOT).await?;
    bmp_to_png(&bmp)
}

/// The screenshot as a `data:image/png;base64,...` URL.
pub async fn screenshot_data_url(host: &str, token: &str) -> Result<String> {
    let png = screenshot_png(host, token).await?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// Tap a board pixel (x 0-799, y 0-479).
pub async fn tap(host: &str, token: &str, x: i64, y: i64) -> Result<()> {
    if !(0..SCREEN_W).contains(&x) || !(0..SCREEN_H).contains(&y) {
        bail!("tap ({x}, {y}) is off the {SCREEN_W}x{SCREEN_H} board");
    }
    let url = format!("http://{host}/tap");
    let params = [
        ("token", token.to_string()),
        ("x", x.to_string()),
        ("y", y.to_string()),
    ];
    get(&url, &params, QUICK).await?;
    Ok(())
}

/// The board's root page text.
pub async fn root(host: &str) -> Result<String> {
    let url = format!("http://{host}/");
    let bytes = get(&url, &[], QUICK).await?;
    String::from_utf8(bytes).context("board root page is not UTF-8")
}

async fn get(url: &str, params: &[(&str, String)], wait: Duration) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(wait)
        .build()
        .context("http client")?;
    // without_url() keeps the token (in the query) out of error text.
    let resp = client
        .get(url)
        .query(params)
        .send()
        .await
        .map_err(|e| anyhow!("board request failed: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow!("board answered: {}", e.without_url()))?;
    let body = resp
        .bytes()
        .await
        .map_err(|e| anyhow!("board body failed: {}", e.without_url()))?;
    Ok(body.to_vec())
}

/// Decode a BMP and re-encode it as PNG.
pub fn bmp_to_png(bmp: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory_with_format(bmp, ImageFormat::Bmp)
        .context("screenshot is not a BMP")?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .context("PNG encode failed")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A 24-bit BMP, bottom-up rows padded to 4 bytes, as the board sends.
    fn make_bmp(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let data_size = row * h;
        let offset = 14 + 40;
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&(offset + data_size).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&offset.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&(w as i32).to_le_bytes());
        b.extend_from_slice(&(h as i32).to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&data_size.to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for y in (0..h).rev() {
            let mut line = Vec::new();
            for x in 0..w {
                let [r, g, bl] = pixel(x, y);
                line.extend_from_slice(&[bl, g, r]);
            }
            line.resize(row as usize, 0);
            b.extend_from_slice(&line);
        }
        b
    }

    fn pixel(x: u32, y: u32) -> [u8; 3] {
        [(x * 60) as u8, (y * 80) as u8, 200]
    }

    #[test]
    fn bmp_converts_to_png_with_same_pixels() {
        // Width 5 forces row padding (15 bytes padded to 16).
        let bmp = make_bmp(5, 3, pixel);
        let png = bmp_to_png(&bmp).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (5, 3));
        for y in 0..3 {
            for x in 0..5 {
                assert_eq!(decoded.get_pixel(x, y).0, pixel(x, y), "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn non_bmp_is_an_error() {
        assert!(bmp_to_png(b"not an image at all").is_err());
    }

    /// One-shot HTTP server: returns the request line and answers with `body`.
    async fn serve_once(body: Vec<u8>) -> (u16, tokio::task::JoinHandle<String>) {
        serve_status("200 OK", body).await
    }

    /// Same, with any status line.
    async fn serve_status(
        status: &'static str,
        body: Vec<u8>,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
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
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(&body).await.unwrap();
            sock.shutdown().await.ok();
            let text = String::from_utf8_lossy(&req).to_string();
            text.lines().next().unwrap_or_default().to_string()
        });
        (port, task)
    }

    #[tokio::test]
    async fn screenshot_fetches_bmp_and_returns_png() {
        let (port, task) = serve_once(make_bmp(4, 3, pixel)).await;
        let png = screenshot_png(&format!("127.0.0.1:{port}"), "tok")
            .await
            .unwrap();
        let line = task.await.unwrap();
        assert_eq!(line, "GET /screenshot?token=tok HTTP/1.1");
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded.get_pixel(3, 2).0, pixel(3, 2));
    }

    #[tokio::test]
    async fn data_url_has_png_prefix() {
        let (port, _task) = serve_once(make_bmp(2, 2, pixel)).await;
        let url = screenshot_data_url(&format!("127.0.0.1:{port}"), "tok")
            .await
            .unwrap();
        assert!(
            url.starts_with("data:image/png;base64,iVBORw0KGgo"),
            "{url}"
        );
    }

    #[tokio::test]
    async fn tap_sends_coordinates_and_validates_range() {
        let (port, task) = serve_once(b"ok".to_vec()).await;
        tap(&format!("127.0.0.1:{port}"), "tok", 10, 20)
            .await
            .unwrap();
        assert_eq!(task.await.unwrap(), "GET /tap?token=tok&x=10&y=20 HTTP/1.1");

        // Out of range is rejected before any request is made.
        assert!(tap("127.0.0.1:1", "tok", 800, 0).await.is_err());
        assert!(tap("127.0.0.1:1", "tok", 0, 480).await.is_err());
        assert!(tap("127.0.0.1:1", "tok", -1, 0).await.is_err());
    }

    #[tokio::test]
    async fn root_returns_page_text() {
        let (port, task) = serve_once(b"<h1>deskhub</h1>".to_vec()).await;
        let text = root(&format!("127.0.0.1:{port}")).await.unwrap();
        assert_eq!(text, "<h1>deskhub</h1>");
        assert_eq!(task.await.unwrap(), "GET / HTTP/1.1");
    }

    #[tokio::test]
    async fn pair_sends_name_and_code_and_reads_token() {
        let (port, task) =
            serve_status("200 OK", br#"{"token":"abcd2345efgh6789","host":"deskhub"}"#.to_vec())
                .await;
        let p = pair(&format!("127.0.0.1:{port}"), "My PC", "0427")
            .await
            .unwrap();
        assert_eq!(p.token, "abcd2345efgh6789");
        assert_eq!(p.host, "deskhub");
        assert!(!format!("{p:?}").contains("abcd"));
        assert_eq!(
            task.await.unwrap(),
            "POST /pair?name=My+PC&code=0427 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn pair_explains_each_refusal() {
        for (status, want) in [
            ("403 Forbidden", "Denied on the board"),
            ("404 Not Found", "update it to 0.9.0"),
            ("408 Request Timeout", "Nobody answered"),
            ("429 Too Many Requests", "wait 10 seconds"),
            ("400 Bad Request", "already open"),
        ] {
            let (port, _task) = serve_status(status, b"a pairing prompt is already open".to_vec()).await;
            let err = pair(&format!("127.0.0.1:{port}"), "PC", "1234")
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{status}: {err}");
        }
    }

    #[test]
    fn pair_codes_are_four_digits() {
        for _ in 0..50 {
            let c = pair_code();
            assert_eq!(c.len(), 4, "{c}");
            assert!(c.chars().all(|ch| ch.is_ascii_digit()), "{c}");
        }
    }

    #[test]
    fn firmware_check_takes_app_images_only() {
        let mut img = vec![0u8; 100 * 1024];
        assert!(check_firmware(&img).is_err());
        img[0] = 0xE9;
        assert!(check_firmware(&img).is_err());
        img[32..36].copy_from_slice(&0xABCD_5432u32.to_le_bytes());
        assert!(check_firmware(&img).is_ok());
        assert!(check_firmware(&img[..1024]).is_err());
    }
}
