//! The board's media-server section: what plays on the user's Plex server (everyone on it,
//! one session shown, "(+N more)"), or the server's version and library while idle. Port of
//! upstream companion/deskhub/media_server.py (MediaServers). Plex HTTP is in crate::plex.
//!
//! Messages: {"type":"media_server", ...} JSON plus an "ARS1" poster frame (128x192),
//! published before the JSON that names its art_id.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::media::{ascii_fold, crc32, extrapolated_pos, rgb_to_rgb565_le, art_frame_magic};
use super::Ctx;
use crate::plex::{self, Conn, ServerInfo, Session};
use crate::secrets;
use crate::settings::MediaServer;

const POLL_ACTIVE: Duration = Duration::from_secs(3);
const POLL_IDLE: Duration = Duration::from_secs(15);
const BACKOFF: Duration = Duration::from_secs(60);
const ART_RETRY: Duration = Duration::from_secs(30);
const INFO_EVERY: Duration = Duration::from_secs(10 * 60);
pub const POSTER_W: u32 = 128;
pub const POSTER_H: u32 = 192;

/// Signed out (or no server picked): the board shows its "not set up" state.
pub fn not_configured() -> Value {
    json!({"type": "media_server", "configured": false})
}

/// Fits a cover inside a 2:3 poster on black (square album art gets bands), as an ARS1
/// frame: the server's own cover slot, so it doesn't replace the PC's.
pub fn poster_frame(art_id: u32, raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(raw)?.to_rgb8();
    let (w, h) = (img.width(), img.height());
    anyhow::ensure!(w > 0 && h > 0, "empty cover image");
    // Shrink to fit, never enlarge (PIL thumbnail()).
    let scale = (POSTER_W as f64 / w as f64).min(POSTER_H as f64 / h as f64).min(1.0);
    let tw = ((w as f64 * scale).round() as u32).clamp(1, POSTER_W);
    let th = ((h as f64 * scale).round() as u32).clamp(1, POSTER_H);
    let sized = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Lanczos3);
    let mut canvas = image::RgbImage::new(POSTER_W, POSTER_H);
    image::imageops::replace(
        &mut canvas,
        &sized,
        ((POSTER_W - tw) / 2) as i64,
        ((POSTER_H - th) / 2) as i64,
    );
    Ok(art_frame_magic(
        b"ARS1",
        art_id,
        POSTER_W as u16,
        POSTER_H as u16,
        &rgb_to_rgb565_le(canvas.as_raw()),
    ))
}

/// The session to show: the current one stays while it plays (or while nothing plays),
/// else the first playing one, else the first.
pub fn pick(sessions: &[Session], current: Option<&str>) -> Option<usize> {
    if sessions.is_empty() {
        return None;
    }
    let any_playing = sessions.iter().any(|s| s.playing);
    if let Some(cur) = current {
        if let Some(i) = sessions.iter().position(|s| s.id == cur && (s.playing || !any_playing)) {
            return Some(i);
        }
    }
    Some(sessions.iter().position(|s| s.playing).unwrap_or(0))
}

/// The server and what it plays, as last polled.
#[derive(Debug, Default)]
pub struct Poller {
    pub sessions: Vec<Session>,
    /// Index into `sessions`.
    pub current: Option<usize>,
    /// Unix seconds of the sessions reply (positions are as of then).
    pub seen: f64,
    pub online: bool,
    pub info: Option<ServerInfo>,
    info_at: Option<Instant>,
    /// Cover key of the frame last made (None = make one).
    art_key: Option<String>,
    art_failed: Option<(String, Instant)>,
    pub art_id: u32,
}

impl Poller {
    /// Takes a sessions reply and picks the one to show.
    pub fn update(&mut self, sessions: Vec<Session>, now_unix: f64) {
        let cur = self.current.and_then(|i| self.sessions.get(i)).map(|s| s.id.clone());
        self.current = pick(&sessions, cur.as_deref());
        self.sessions = sessions;
        self.seen = now_unix;
        self.online = true;
    }

    /// The server didn't answer: nothing shown until it does.
    pub fn failed(&mut self) {
        self.sessions.clear();
        self.current = None;
        self.online = false;
    }

    pub fn shown(&self) -> Option<&Session> {
        self.current.and_then(|i| self.sessions.get(i))
    }

    pub fn interval(&self) -> Duration {
        if self.sessions.is_empty() {
            POLL_IDLE
        } else {
            POLL_ACTIVE
        }
    }

    fn info_due(&self, now: Instant) -> bool {
        self.info_at.is_none_or(|t| now.duration_since(t) >= INFO_EVERY)
    }

    /// The cover to fetch now: the shown session's, when it changed and didn't just fail.
    /// A session without a cover clears the art.
    pub fn art_wanted(&mut self, now: Instant) -> Option<String> {
        let key = self.shown()?.art_key.clone();
        if self.art_key.as_deref() == Some(key.as_str()) {
            return None;
        }
        if let Some((k, at)) = &self.art_failed {
            if *k == key && now.duration_since(*at) < ART_RETRY {
                return None;
            }
        }
        self.art_id = 0;
        if key.is_empty() {
            self.art_key = Some(key);
            return None;
        }
        Some(key)
    }

    /// The fetched cover for `key`: the frame to publish, or None (failure retried later).
    pub fn art_fetched(&mut self, key: String, raw: anyhow::Result<Vec<u8>>, now: Instant) -> Option<Vec<u8>> {
        let frame = raw.and_then(|raw| {
            let id = (crc32(&raw) & 0x7FFF_FFFF).max(1);
            poster_frame(id, &raw).map(|f| (id, f))
        });
        match frame {
            Ok((id, f)) => {
                self.art_id = id;
                self.art_key = Some(key);
                self.art_failed = None;
                Some(f)
            }
            Err(e) => {
                log::info!("plex cover: {e:#}");
                self.art_key = None;
                self.art_failed = Some((key, now));
                None
            }
        }
    }

    /// The board's server section.
    pub fn message(&self, name: &str, now_unix: f64) -> Value {
        let version = match self.info.as_ref().map(|i| i.version.as_str()).filter(|v| !v.is_empty()) {
            Some(v) => format!("Plex Media Server {v}"),
            None => "Plex Media Server".to_string(),
        };
        let mut msg = json!({
            "type": "media_server",
            "configured": true,
            "name": ascii_fold(name, 31),
            "kind": "plex",
            "online": self.online,
            "version": ascii_fold(&version, 47),
            "library": ascii_fold(&self.info.as_ref().map(|i| plex::describe(&i.counts)).unwrap_or_default(), 95),
            "streams": self.sessions.len(),
            "transcoding": self.sessions.iter().filter(|s| s.transcoding).count(),
            "active": false,
        });
        let Some(s) = self.shown() else {
            return msg;
        };
        let mut who = [s.device.as_str(), s.user.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" - ");
        let others = self.sessions.len() - 1;
        if others > 0 {
            who = if who.is_empty() {
                format!("+{others} more")
            } else {
                format!("{who} (+{others} more)")
            };
        }
        let source = if s.transcoding {
            format!("{name} - transcoding")
        } else {
            name.to_string()
        };
        let pos = extrapolated_pos(s.pos, s.dur, s.playing, Some(self.seen), now_unix);
        let m = msg.as_object_mut().expect("object");
        m.insert("active".into(), json!(true));
        m.insert("playing".into(), json!(s.playing));
        m.insert("title".into(), json!(ascii_fold(&s.title, 120)));
        m.insert("artist".into(), json!(ascii_fold(&s.subtitle, 90)));
        m.insert("album".into(), json!(ascii_fold(&who, 90)));
        m.insert("source".into(), json!(ascii_fold(&source, 31)));
        m.insert("pos".into(), json!(pos as i64));
        m.insert("dur".into(), json!(s.dur as i64));
        // Read-only: no seeking or play/pause from the board.
        m.insert("can_seek".into(), json!(false));
        m.insert("controls".into(), json!(false));
        m.insert("art_id".into(), json!(self.art_id));
        msg
    }

    /// One round against the server: sessions, info when due, the cover when it changed.
    /// Returns the poster frame to publish (before the message), if any. Err = offline.
    pub async fn round(&mut self, conn: &Conn, now: Instant) -> anyhow::Result<Option<Vec<u8>>> {
        let sessions = match conn.sessions().await {
            Ok(s) => s,
            Err(e) => {
                self.failed();
                return Err(e);
            }
        };
        self.update(sessions, now_unix());
        if self.info_due(now) {
            self.info_at = Some(now);
            match conn.info().await {
                Ok(i) => self.info = Some(i),
                Err(e) => log::info!("plex info: {e:#}"),
            }
        }
        Ok(match self.art_wanted(now) {
            Some(key) => {
                let raw = conn.cover(&key).await;
                self.art_fetched(key, raw, now)
            }
            None => None,
        })
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// How to reach the chosen server, through plex.tv.
async fn connect(cfg: &MediaServer, token: &str) -> anyhow::Result<Conn> {
    let all = plex::resources(plex::TV, &cfg.client_id, token).await?;
    let res = all
        .iter()
        .find(|r| r.id == cfg.server_id)
        .ok_or_else(|| anyhow::anyhow!("the server is no longer on this Plex account"))?;
    plex::pick_connection(res, token, &cfg.client_id, cfg!(windows))
}

pub async fn run(mut ctx: Ctx) {
    let mut poller = Poller::default();
    let mut conn: Option<Conn> = None;
    let mut last_cfg: Option<MediaServer> = None;
    loop {
        let cfg = ctx.settings.borrow().media_server.clone();
        if last_cfg.as_ref() != Some(&cfg) {
            poller = Poller::default();
            conn = None;
            last_cfg = Some(cfg.clone());
        }
        let token = if cfg.ready() {
            match secrets::system_store_for(secrets::PLEX_TOKEN).get() {
                Ok(t) => t.filter(|t| !t.is_empty()),
                Err(e) => {
                    log::warn!("plex token: {e:#}");
                    None
                }
            }
        } else {
            None
        };
        let Some(token) = token else {
            ctx.out.publish("media_server", not_configured());
            if ctx.settings.changed().await.is_err() {
                return;
            }
            continue;
        };

        let wait = match &conn {
            Some(c) => match poller.round(c, Instant::now()).await {
                Ok(frame) => {
                    if let Some(f) = frame {
                        ctx.out.publish_bin("server_art", f);
                    }
                    poller.interval()
                }
                Err(e) => {
                    log::info!("plex {}: {e:#}", cfg.server_name);
                    // The address may have changed: ask plex.tv again next time.
                    conn = None;
                    BACKOFF
                }
            },
            None => match connect(&cfg, &token).await {
                Ok(c) => {
                    conn = Some(c);
                    continue;
                }
                Err(e) => {
                    log::info!("plex {}: {e:#}", cfg.server_name);
                    poller.failed();
                    BACKOFF
                }
            },
        };
        ctx.out.publish("media_server", poller.message(&cfg.server_name, now_unix()));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            r = ctx.settings.changed() => if r.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::tests::{conn, fake, sessions_json};
    use crate::plex::{parse_sessions, Counts};

    fn sessions() -> Vec<Session> {
        parse_sessions(&sessions_json())
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn pick_prefers_playing_and_sticks() {
        let mut s = sessions(); // [paused episode, playing movie, playing track]
        assert_eq!(pick(&s, None), Some(1));
        assert_eq!(pick(&s, Some("99")), Some(2)); // still playing: stays
        assert_eq!(pick(&s, Some("abc")), Some(1)); // paused while others play: moves
        for x in &mut s {
            x.playing = false;
        }
        assert_eq!(pick(&s, Some("abc")), Some(0)); // nothing plays: stays
        assert_eq!(pick(&s, Some("gone")), Some(0));
        assert_eq!(pick(&[], None), None);
    }

    #[test]
    fn idle_and_active_messages() {
        let mut p = Poller {
            info: Some(ServerInfo { version: "1.41.2".into(), counts: Counts([312, 45, 2104, 0]) }),
            ..Default::default()
        };
        p.update(Vec::new(), 1000.0);
        let m = p.message("Home", 1000.0);
        assert_eq!(m["configured"], true);
        assert_eq!(m["online"], true);
        assert_eq!(m["active"], false);
        assert_eq!(m["version"], "Plex Media Server 1.41.2");
        assert_eq!(m["library"], "312 movies, 45 shows, 2,104 episodes");
        assert_eq!(m["streams"], 0);
        assert!(m.get("title").is_none());

        p.update(sessions(), 1000.0);
        let m = p.message("Home", 1010.0);
        assert_eq!(m["active"], true);
        assert_eq!(m["title"], "Film");
        assert_eq!(m["artist"], "2020");
        assert_eq!(m["album"], "Plex Web - sam (+2 more)");
        assert_eq!(m["source"], "Home - transcoding");
        assert_eq!(m["pos"], 10); // playing: extrapolated from the reply
        assert_eq!(m["streams"], 3);
        assert_eq!(m["transcoding"], 1);
        assert_eq!((m["can_seek"].clone(), m["controls"].clone()), (json!(false), json!(false)));

        p.failed();
        let m = p.message("Home", 1010.0);
        assert_eq!((m["online"].clone(), m["active"].clone()), (json!(false), json!(false)));
        assert_eq!(not_configured(), json!({"type": "media_server", "configured": false}));
    }

    #[test]
    fn poster_is_letterboxed_on_black() {
        let f = poster_frame(7, &png(100, 100)).unwrap();
        assert_eq!(&f[..4], b"ARS1");
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(f[8..10].try_into().unwrap()), 128);
        assert_eq!(u16::from_le_bytes(f[10..12].try_into().unwrap()), 192);
        let px = |x: usize, y: usize| u16::from_le_bytes([f[12 + (y * 128 + x) * 2], f[13 + (y * 128 + x) * 2]]);
        // 100x100 is not enlarged: black bands all round, white in the middle.
        assert_eq!(px(0, 0), 0);
        assert_eq!(px(64, 96), 0xFFFF);
        assert_eq!(px(64, 20), 0);
        assert!(poster_frame(1, b"not an image").is_err());
    }

    #[test]
    fn art_is_fetched_once_and_retried_after_failure() {
        let t0 = Instant::now();
        let mut p = Poller::default();
        p.update(sessions(), 0.0);
        let key = p.art_wanted(t0).unwrap();
        assert_eq!(key, "/m");
        assert!(p.art_fetched(key.clone(), Err(anyhow::anyhow!("down")), t0).is_none());
        assert_eq!(p.art_wanted(t0 + Duration::from_secs(5)), None);
        let key = p.art_wanted(t0 + ART_RETRY).unwrap();
        assert!(p.art_fetched(key, Ok(png(20, 30)), t0).is_some());
        assert_ne!(p.art_id, 0);
        assert_eq!(p.art_wanted(t0), None);
        // A session without a cover clears the art.
        let mut s = sessions();
        s[1].art_key.clear();
        p.update(s, 0.0);
        assert_eq!(p.art_wanted(t0), None);
        assert_eq!(p.art_id, 0);
    }

    #[tokio::test]
    async fn a_round_against_a_fake_server() {
        let f = fake(vec![
            ("/status/sessions", 200, sessions_json().to_string().into_bytes()),
            ("/photo/:/transcode", 200, png(256, 384)),
            ("/", 200, br#"{"MediaContainer": {"version": "1.41.2.1-x"}}"#.to_vec()),
            ("/library/sections", 200, br#"{"MediaContainer": {}}"#.to_vec()),
        ])
        .await;
        let c = conn(f.port);
        let mut p = Poller::default();
        let t0 = Instant::now();
        let frame = p.round(&c, t0).await.unwrap().expect("poster");
        assert_eq!(&frame[..4], b"ARS1");
        assert_eq!(p.info.as_ref().unwrap().version, "1.41.2");
        assert_eq!(p.interval(), POLL_ACTIVE);
        // Same cover, info not due: one request this time.
        let before = f.requests.lock().unwrap().len();
        assert!(p.round(&c, t0).await.unwrap().is_none());
        assert_eq!(f.requests.lock().unwrap().len(), before + 1);

        let mut down = Poller::default();
        assert!(down.round(&conn(1), t0).await.is_err());
        assert!(!down.online);
    }
}
