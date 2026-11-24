//! Now playing via the Windows media session, album art as ART1. Port of companion/deskhub/media.py.
//!
//! The pure helpers (fold, names, crc, RGB565 frame, message builder) build everywhere and are
//! unit-tested on Linux. Only the Windows path uses them, hence the dead_code allowance.
#![cfg_attr(not(windows), allow(dead_code))]

use serde_json::{json, Value};

use super::Ctx;
#[cfg(windows)]
use crate::protocol::Incoming;
#[cfg(windows)]
use tokio::sync::broadcast;

/// Album art edge length in pixels (media.py ART_SIZE).
pub const ART_SIZE: u32 = 280;

/// Seconds between 1601-01-01 (WinRT DateTime epoch) and 1970-01-01 (unix epoch).
const WINRT_EPOCH_OFFSET_SECS: f64 = 11_644_473_600.0;

const SOURCES: &[(&str, &str)] = &[
    ("spotify", "Spotify"),
    ("chrome", "Chrome"),
    ("msedge", "Edge"),
    ("firefox", "Firefox"),
    ("308046b0af4a39cb", "Firefox"),
    ("opera", "Opera"),
    ("brave", "Brave"),
    ("vlc", "VLC"),
    ("applemusic", "Apple Music"),
    ("zunemusic", "Media Player"),
    ("tidal", "TIDAL"),
    ("discord", "Discord"),
];

/// Board commands that reach the media session (media.py command()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Next,
    Prev,
    /// Seek to an absolute position in seconds.
    Seek(i64),
}

pub fn parse_command(cmd: &str, value: i64) -> Option<Command> {
    match cmd {
        "toggle" => Some(Command::Toggle),
        "next" => Some(Command::Next),
        "prev" => Some(Command::Prev),
        "seek" => Some(Command::Seek(value)),
        _ => None,
    }
}

/// Seek target in 100 ns ticks.
pub fn seek_ticks(secs: i64) -> i64 {
    secs.saturating_mul(10_000_000)
}

/// Converts a WinRT DateTime (100 ns ticks since 1601 UTC) to unix seconds.
pub fn windows_time_to_unix(universal_100ns: i64) -> f64 {
    universal_100ns as f64 / 1e7 - WINRT_EPOCH_OFFSET_SECS
}

/// Python str.title(): upper-case after an uncased char, lower-case inside a word.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_alphabetic();
    }
    out
}

/// Accented Latin letters and the ASCII base that NFKD would leave behind.
const LATIN_BASE: &[(&str, char)] = &[
    ("ÀÁÂÃÄÅ", 'A'),
    ("àáâãäå", 'a'),
    ("Ç", 'C'),
    ("ç", 'c'),
    ("ÈÉÊË", 'E'),
    ("èéêë", 'e'),
    ("ÌÍÎÏ", 'I'),
    ("ìíîï", 'i'),
    ("Ñ", 'N'),
    ("ñ", 'n'),
    ("ÒÓÔÕÖ", 'O'),
    ("òóôõö", 'o'),
    ("ÙÚÛÜ", 'U'),
    ("ùúûü", 'u'),
    ("Ý", 'Y'),
    ("ýÿ", 'y'),
];

/// Port of companion/deskhub/text.py ascii_fold: the hub's fonts are ASCII only.
pub fn ascii_fold(s: &str, limit: usize) -> String {
    let mut mapped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2018}' | '\u{2019}' => mapped.push('\''),
            '\u{201C}' | '\u{201D}' => mapped.push('"'),
            '\u{2013}' | '\u{2014}' | '\u{00B7}' | '\u{2022}' => mapped.push('-'),
            '\u{2026}' => mapped.push_str("..."),
            c if c.is_ascii() => mapped.push(c),
            c => {
                if let Some((_, base)) = LATIN_BASE.iter().find(|(set, _)| set.contains(c)) {
                    mapped.push(*base);
                }
            }
        }
    }
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(limit).collect()
}

/// Friendly source name from a session's app id (media.py _source_name).
pub fn source_name(app_id: &str) -> String {
    let low = app_id.to_lowercase();
    for (key, name) in SOURCES {
        if low.contains(key) {
            return (*name).to_string();
        }
    }
    let base = low.rsplit('!').next().unwrap_or("");
    let base = base.rsplit('\\').next().unwrap_or("");
    let base = base.strip_suffix(".exe").unwrap_or(base);
    let folded = ascii_fold(&title_case(base), 24);
    if folded.is_empty() {
        "Media".to_string()
    } else {
        folded
    }
}

/// zlib-compatible CRC-32 (IEEE, reflected), same as Python zlib.crc32.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Top-left x, y and side of the largest centered square (media.py crop box).
pub fn square_crop(w: u32, h: u32) -> (u32, u32, u32) {
    let side = w.min(h);
    ((w - side) / 2, (h - side) / 2, side)
}

/// Packs interleaved RGB8 pixels into RGB565 little-endian bytes.
pub fn rgb_to_rgb565_le(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 2);
    for p in rgb.as_chunks::<3>().0 {
        let v = ((u16::from(p[0]) & 0xF8) << 8)
            | ((u16::from(p[1]) & 0xFC) << 3)
            | (u16::from(p[2]) >> 3);
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// ART1 frame: "ART1" | u32 art_id | u16 w | u16 h | RGB565 pixels (all little endian).
pub fn art_frame(art_id: u32, w: u16, h: u16, rgb565: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + rgb565.len());
    out.extend_from_slice(b"ART1");
    out.extend_from_slice(&art_id.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(rgb565);
    out
}

/// Decodes a cover image, center-crops it to a square, resizes to ART_SIZE and frames it.
pub fn thumbnail_frame(art_id: u32, raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(raw)?.to_rgb8();
    let (x, y, side) = square_crop(img.width(), img.height());
    anyhow::ensure!(side > 0, "empty cover image");
    let square = image::imageops::crop_imm(&img, x, y, side, side).to_image();
    let sized = image::imageops::resize(
        &square,
        ART_SIZE,
        ART_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    let size = ART_SIZE as u16;
    Ok(art_frame(
        art_id,
        size,
        size,
        &rgb_to_rgb565_le(sized.as_raw()),
    ))
}

/// Playback facts read from the session, before any formatting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub app_id: String,
    pub playing: bool,
    pub position_enabled: bool,
    /// Seconds into the track, as last reported by the player.
    pub pos: f64,
    pub dur: f64,
    /// Unix seconds of the last timeline update.
    pub last_updated_unix: Option<f64>,
}

/// Timeline position, extrapolated while playing (media.py snapshot()).
pub fn extrapolated_pos(
    pos: f64,
    dur: f64,
    playing: bool,
    last_updated_unix: Option<f64>,
    now_unix: f64,
) -> f64 {
    let mut p = pos;
    if playing {
        if let Some(updated) = last_updated_unix {
            let age = now_unix - updated;
            if age > 0.0 && age < 24.0 * 3600.0 {
                p += age;
            }
        }
    }
    if dur > 0.0 {
        p = p.min(dur);
    }
    p
}

/// The "media" message for an active session.
pub fn media_message(np: &NowPlaying, now_unix: f64, art_id: u32) -> Value {
    let pos = extrapolated_pos(np.pos, np.dur, np.playing, np.last_updated_unix, now_unix);
    let artist = if np.artist.is_empty() {
        &np.album_artist
    } else {
        &np.artist
    };
    json!({
        "type": "media",
        "active": !np.title.is_empty(),
        "playing": np.playing,
        "title": ascii_fold(&np.title, 120),
        "artist": ascii_fold(artist, 90),
        "album": ascii_fold(&np.album, 90),
        "source": source_name(&np.app_id),
        "pos": pos as i64,
        "dur": np.dur as i64,
        "can_seek": np.position_enabled && np.dur > 0.0,
        "art_id": art_id,
    })
}

/// What the board gets when nothing is playing (no session, or props unreadable).
pub fn inactive_message() -> Value {
    json!({"type": "media", "active": false})
}

/// One sampling result from the WinRT thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Sample {
    Inactive,
    Active {
        now_playing: NowPlaying,
        /// Title, artist or album changed since the last sample (art must be re-read).
        key_changed: bool,
        /// Raw cover bytes, present only when the key changed and the cover was read.
        thumb: Option<Vec<u8>>,
    },
}

#[cfg(windows)]
fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Publishes "media" every second and "art" before any message carrying new art.
#[cfg(windows)]
pub async fn run(ctx: Ctx) {
    let Ctx {
        out, mut incoming, ..
    } = ctx;
    let (sample_tx, mut samples) = tokio::sync::mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("media-winrt".into())
        .spawn(move || win::worker(sample_tx, cmd_rx));
    if let Err(e) = spawned {
        log::warn!("media thread failed: {e}");
        out.publish("media", inactive_message());
        return;
    }

    let mut art_id: u32 = 0;
    let mut sessions_open = true;
    let mut incoming_open = true;
    loop {
        tokio::select! {
            sample = samples.recv(), if sessions_open => match sample {
                None => {
                    // Session manager unavailable: stay inactive.
                    sessions_open = false;
                    out.publish("media", inactive_message());
                }
                Some(sample) => handle_sample(&out, &mut art_id, sample).await,
            },
            cmd = incoming.recv(), if incoming_open => match cmd {
                Ok(Incoming::Media { cmd, value }) => {
                    if let Some(c) = parse_command(&cmd, value) {
                        let _ = cmd_tx.send(c);
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => incoming_open = false,
            },
            else => break,
        }
    }
}

#[cfg(windows)]
async fn handle_sample(out: &crate::session::Publisher, art_id: &mut u32, sample: Sample) {
    match sample {
        Sample::Inactive => out.publish("media", inactive_message()),
        Sample::Active {
            now_playing,
            key_changed,
            thumb,
        } => {
            if key_changed {
                *art_id = 0;
            }
            if let Some(raw) = thumb {
                let id = (crc32(&raw) & 0x7FFF_FFFF).max(1);
                match tokio::task::spawn_blocking(move || thumbnail_frame(id, &raw)).await {
                    Ok(Ok(frame)) => {
                        out.publish_bin("art", frame);
                        *art_id = id;
                    }
                    Ok(Err(e)) => log::info!("could not read cover art: {e}"),
                    Err(e) => log::info!("cover art task failed: {e}"),
                }
            }
            out.publish("media", media_message(&now_playing, now_unix(), *art_id));
        }
    }
}

/// Off Windows there is no session manager: publish the inactive message once and idle.
#[cfg(not(windows))]
pub async fn run(ctx: Ctx) {
    ctx.out.publish("media", inactive_message());
    std::future::pending::<()>().await;
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use tokio::sync::mpsc::UnboundedSender;
    use windows::core::{Interface, Result as WinResult};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    };
    use windows::Storage::Streams::{DataReader, IInputStream, IRandomAccessStreamReference};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::{seek_ticks, windows_time_to_unix, Command, NowPlaying, Sample};

    /// Owns every WinRT object. Samples once a second; runs commands as they arrive.
    pub fn worker(samples: UnboundedSender<Sample>, cmds: Receiver<Command>) {
        // SAFETY: plain COM init on this thread; a failure (already initialized) is harmless.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mgr = match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("media session manager unavailable: {e}");
                return;
            }
        };
        let mut last_key: Option<(String, String, String)> = None;
        let mut next = Instant::now();
        loop {
            loop {
                let wait = next.saturating_duration_since(Instant::now());
                match cmds.recv_timeout(wait) {
                    Ok(c) => exec(&mgr, c),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            next = Instant::now() + Duration::from_secs(1);
            match sample(&mgr, &mut last_key) {
                Ok(s) => {
                    if samples.send(s).is_err() {
                        return;
                    }
                }
                Err(e) => log::debug!("media poll failed: {e}"),
            }
        }
    }

    fn exec(mgr: &Manager, cmd: Command) {
        let Ok(session) = mgr.GetCurrentSession() else {
            return;
        };
        let r: WinResult<bool> = match cmd {
            Command::Toggle => session.TryTogglePlayPauseAsync().and_then(|op| op.join()),
            Command::Next => session.TrySkipNextAsync().and_then(|op| op.join()),
            Command::Prev => session.TrySkipPreviousAsync().and_then(|op| op.join()),
            Command::Seek(secs) => session
                .TryChangePlaybackPositionAsync(seek_ticks(secs))
                .and_then(|op| op.join()),
        };
        if let Err(e) = r {
            log::debug!("media command failed: {e}");
        }
    }

    fn sample(mgr: &Manager, last_key: &mut Option<(String, String, String)>) -> WinResult<Sample> {
        let Ok(session) = mgr.GetCurrentSession() else {
            return Ok(Sample::Inactive);
        };
        let props = match session
            .TryGetMediaPropertiesAsync()
            .and_then(|op| op.join())
        {
            Ok(p) => p,
            Err(e) => {
                log::debug!("media properties failed: {e}");
                return Ok(Sample::Inactive);
            }
        };
        let info = session.GetPlaybackInfo()?;
        let timeline = session.GetTimelineProperties()?;
        let playing = info.PlaybackStatus()? == PlaybackStatus::Playing;
        let position_enabled = info
            .Controls()
            .and_then(|c| c.IsPlaybackPositionEnabled())
            .unwrap_or(false);

        let title = props.Title()?.to_string();
        let artist = props.Artist()?.to_string();
        let album_artist = props.AlbumArtist()?.to_string();
        let album = props.AlbumTitle()?.to_string();

        let key = (title.clone(), artist.clone(), album.clone());
        let key_changed = last_key.as_ref() != Some(&key);
        let mut thumb = None;
        if key_changed {
            *last_key = Some(key);
            if let Ok(reference) = props.Thumbnail() {
                match read_thumbnail(&reference) {
                    Ok(raw) => thumb = Some(raw),
                    Err(e) => log::info!("could not read cover art: {e}"),
                }
            }
        }

        let now_playing = NowPlaying {
            title,
            artist,
            album_artist,
            album,
            app_id: session.SourceAppUserModelId()?.to_string(),
            playing,
            position_enabled,
            pos: timeline.Position()?.Duration as f64 / 1e7,
            dur: timeline.EndTime()?.Duration as f64 / 1e7,
            last_updated_unix: timeline
                .LastUpdatedTime()
                .ok()
                .map(|t| windows_time_to_unix(t.UniversalTime)),
        };
        Ok(Sample::Active {
            now_playing,
            key_changed,
            thumb,
        })
    }

    fn read_thumbnail(reference: &IRandomAccessStreamReference) -> WinResult<Vec<u8>> {
        let stream = reference.OpenReadAsync()?.join()?;
        let size = stream.Size()? as u32;
        let input: IInputStream = stream.cast()?;
        let reader = DataReader::CreateDataReader(&input)?;
        let loaded = reader.LoadAsync(size)?.join()? as usize;
        let mut buf = vec![0u8; loaded];
        reader.ReadBytes(&mut buf)?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn rgb565_pixels_little_endian() {
        assert_eq!(rgb_to_rgb565_le(&[255, 0, 0]), vec![0x00, 0xF8]);
        assert_eq!(rgb_to_rgb565_le(&[0, 255, 0]), vec![0xE0, 0x07]);
        assert_eq!(rgb_to_rgb565_le(&[0, 0, 255]), vec![0x1F, 0x00]);
        assert_eq!(
            rgb_to_rgb565_le(&[255, 255, 255, 0, 0, 0]),
            vec![0xFF, 0xFF, 0x00, 0x00]
        );
    }

    #[test]
    fn art_frame_layout() {
        let frame = art_frame(0x0102_0304, 2, 1, &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(
            frame,
            vec![
                b'A', b'R', b'T', b'1', 0x04, 0x03, 0x02, 0x01, 0x02, 0x00, 0x01, 0x00, 0xAA, 0xBB,
                0xCC, 0xDD
            ]
        );
    }

    #[test]
    fn square_crop_is_centered() {
        assert_eq!(square_crop(4, 2), (1, 0, 2));
        assert_eq!(square_crop(2, 5), (0, 1, 2));
        assert_eq!(square_crop(5, 5), (0, 0, 5));
    }

    #[test]
    fn thumbnail_becomes_280_square_frame() {
        let img = image::RgbImage::from_pixel(4, 2, image::Rgb([255, 0, 0]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let frame = thumbnail_frame(7, png.get_ref()).unwrap();
        assert_eq!(&frame[..4], b"ART1");
        assert_eq!(&frame[4..8], &7u32.to_le_bytes());
        assert_eq!(&frame[8..10], &280u16.to_le_bytes());
        assert_eq!(&frame[10..12], &280u16.to_le_bytes());
        assert_eq!(frame.len(), 12 + 280 * 280 * 2);
        // Solid red stays solid red after crop and resize: RGB565 0xF800, little endian.
        assert!(frame[12..]
            .as_chunks::<2>()
            .0
            .iter()
            .all(|px| px == &[0x00, 0xF8]));
    }

    #[test]
    fn thumbnail_garbage_is_error() {
        assert!(thumbnail_frame(1, b"not an image").is_err());
    }

    #[test]
    fn ascii_fold_maps_punctuation_and_latin() {
        let s = "Caf\u{e9} \u{2026}\u{2019}s \u{201c}x\u{201d} \u{2013} y \u{4e2d}";
        assert_eq!(ascii_fold(s, 120), "Cafe ...'s \"x\" - y");
        assert_eq!(ascii_fold("abcdef", 3), "abc");
        assert_eq!(ascii_fold("", 3), "");
    }

    #[test]
    fn source_names() {
        assert_eq!(source_name("Spotify.exe"), "Spotify");
        assert_eq!(
            source_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(source_name(r"C:\tools\my-tool.exe"), "My-Tool");
        assert_eq!(source_name(""), "Media");
    }

    #[test]
    fn position_extrapolates_only_while_playing_and_clamps() {
        assert_eq!(
            extrapolated_pos(10.0, 200.0, true, Some(1000.0), 1003.0),
            13.0
        );
        assert_eq!(
            extrapolated_pos(10.0, 200.0, false, Some(1000.0), 1003.0),
            10.0
        );
        assert_eq!(
            extrapolated_pos(10.0, 200.0, true, Some(0.0), 100_000.0),
            10.0
        );
        assert_eq!(
            extrapolated_pos(199.0, 200.0, true, Some(1000.0), 1010.0),
            200.0
        );
        assert_eq!(extrapolated_pos(5.0, 0.0, true, None, 1000.0), 5.0);
    }

    #[test]
    fn media_message_active_fields() {
        let np = NowPlaying {
            title: "Song".into(),
            artist: String::new(),
            album_artist: "Band".into(),
            album: "Alb".into(),
            app_id: "spotify.exe".into(),
            playing: true,
            position_enabled: true,
            pos: 10.5,
            dur: 200.0,
            last_updated_unix: Some(998.0),
        };
        let msg = media_message(&np, 1000.0, 7);
        assert_eq!(msg["type"], "media");
        assert_eq!(msg["active"], true);
        assert_eq!(msg["playing"], true);
        assert_eq!(msg["title"], "Song");
        assert_eq!(msg["artist"], "Band");
        assert_eq!(msg["album"], "Alb");
        assert_eq!(msg["source"], "Spotify");
        assert_eq!(msg["pos"], 12);
        assert_eq!(msg["dur"], 200);
        assert_eq!(msg["can_seek"], true);
        assert_eq!(msg["art_id"], 7);
    }

    #[test]
    fn media_message_empty_title_is_inactive_and_no_seek_without_duration() {
        let np = NowPlaying {
            position_enabled: true,
            dur: 0.0,
            ..Default::default()
        };
        let msg = media_message(&np, 0.0, 0);
        assert_eq!(msg["active"], false);
        assert_eq!(msg["can_seek"], false);
    }

    #[test]
    fn inactive_message_shape() {
        assert_eq!(
            inactive_message(),
            json!({"type": "media", "active": false})
        );
    }

    #[test]
    fn commands_parse() {
        assert_eq!(parse_command("toggle", 0), Some(Command::Toggle));
        assert_eq!(parse_command("next", 0), Some(Command::Next));
        assert_eq!(parse_command("prev", 0), Some(Command::Prev));
        assert_eq!(parse_command("seek", 30), Some(Command::Seek(30)));
        assert_eq!(parse_command("bogus", 0), None);
        assert_eq!(seek_ticks(30), 300_000_000);
    }

    #[test]
    fn winrt_time_converts_to_unix() {
        assert!((windows_time_to_unix(116_444_736_000_000_000) - 0.0).abs() < 1e-6);
        assert!((windows_time_to_unix(116_444_736_000_000_000 + 25_000_000) - 2.5).abs() < 1e-6);
    }
}
