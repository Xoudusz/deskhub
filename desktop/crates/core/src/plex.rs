//! Plex: "Sign in with Plex" through plex.tv (a PIN the user approves in the browser), the
//! account's servers, and one server's sessions, covers and library sizes. Port of upstream
//! companion/deskhub/media_server.py (Plex part). The collector is collectors/plex.rs.
//!
//! Connection: plex.tv lists each server's local HTTPS address under its own
//! `*.plex.direct` name, which the server's certificate is made for. The client pins that
//! name to the LAN address plex.tv gives (`ClientBuilder::resolve`), so certificates are
//! checked normally and no DNS lookup happens (routers with DNS-rebind protection drop
//! plex.direct answers). Without such an address: plain HTTP on the LAN address.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

pub const TV: &str = "https://plex.tv/api/v2";
pub const PRODUCT: &str = "Desk Hub";
const TIMEOUT: Duration = Duration::from_secs(8);
const TV_TIMEOUT: Duration = Duration::from_secs(15);
/// Trying one of a server's addresses.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Largest reply read (session lists and posters are far smaller).
const MAX_BODY: usize = 8 * 1024 * 1024;
/// Poster size asked from the server: twice the board's 128x192.
const COVER_W: u32 = 256;
const COVER_H: u32 = 384;

/// A new per-install client identifier (X-Plex-Client-Identifier), random.
pub fn new_client_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut parts = [0u64; 2];
    for (i, p) in parts.iter_mut().enumerate() {
        // RandomState is seeded from the OS's random source.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        *p = h.finish();
    }
    format!("deskhub-{:016x}{:016x}", parts[0], parts[1])
}

fn tv_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(TV_TIMEOUT)
        .user_agent("deskhub")
        .build()
        .context("http client")
}

fn plex_headers(rb: reqwest::RequestBuilder, client_id: &str) -> reqwest::RequestBuilder {
    rb.header("Accept", "application/json")
        .header("X-Plex-Product", PRODUCT)
        .header("X-Plex-Client-Identifier", client_id)
}

/// Body of a successful reply, capped; errors carry the status but never the URL (it can
/// hold a token).
async fn read_ok(mut resp: reqwest::Response) -> Result<Vec<u8>> {
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if resp.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        bail!("reply too large");
    }
    // Read in chunks and stop at the cap (a missing or wrong length can't grow memory).
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow!("reading the reply: {}", e.without_url()))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            bail!("reply too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn json_ok(resp: reqwest::Response) -> Result<Value> {
    let body = read_ok(resp).await?;
    serde_json::from_slice(&body).context("the reply isn't JSON")
}

fn send_err(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow!("no answer in time")
    } else if e.is_connect() {
        anyhow!("can't connect: {}", e.without_url())
    } else {
        anyhow!("request failed: {}", e.without_url())
    }
}

// ---- sign-in -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub id: i64,
    pub code: String,
}

/// Where the user approves `code`.
pub fn auth_url(client_id: &str, code: &str) -> String {
    let q = reqwest::Url::parse_with_params(
        "https://app.plex.tv/auth",
        &[
            ("clientID", client_id),
            ("code", code),
            ("context[device][product]", PRODUCT),
        ],
    )
    .ok()
    .and_then(|u| u.query().map(str::to_string))
    .unwrap_or_default();
    format!("https://app.plex.tv/auth#?{q}")
}

/// Starts a sign-in: a PIN to approve at `auth_url`.
pub async fn pin_start(tv: &str, client_id: &str) -> Result<Pin> {
    let rb = tv_client()?.post(format!("{tv}/pins")).query(&[("strong", "true")]);
    let v = json_ok(plex_headers(rb, client_id).send().await.map_err(send_err)?).await?;
    let id = v["id"].as_i64().context("plex.tv gave no PIN id")?;
    let code = v["code"].as_str().unwrap_or_default().to_string();
    if code.is_empty() {
        bail!("plex.tv gave no PIN code");
    }
    Ok(Pin { id, code })
}

/// The account token once the PIN is approved, None while waiting. An expired PIN is an
/// error.
pub async fn pin_poll(tv: &str, client_id: &str, id: i64) -> Result<Option<String>> {
    let rb = tv_client()?.get(format!("{tv}/pins/{id}"));
    let resp = plex_headers(rb, client_id).send().await.map_err(send_err)?;
    if resp.status().as_u16() == 404 {
        bail!("the sign-in code expired");
    }
    let v = json_ok(resp).await?;
    Ok(v["authToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub uri: String,
    pub local: bool,
    pub relay: bool,
}

/// A server on the account. Debug leaves the token out.
#[derive(Clone, PartialEq)]
pub struct Resource {
    pub name: String,
    pub id: String,
    pub owned: bool,
    pub access_token: String,
    pub connections: Vec<Connection>,
}

impl std::fmt::Debug for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resource")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("owned", &self.owned)
            .field("access_token", &format_args!("<{} chars>", self.access_token.len()))
            .field("connections", &self.connections)
            .finish()
    }
}

/// The servers in a plex.tv /resources reply (players and other devices left out).
pub fn parse_resources(v: &Value) -> Vec<Resource> {
    let Some(list) = v.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter(|r| {
            r["provides"]
                .as_str()
                .unwrap_or_default()
                .split(',')
                .any(|p| p.trim() == "server")
        })
        .map(|r| Resource {
            name: r["name"].as_str().unwrap_or_default().to_string(),
            id: r["clientIdentifier"].as_str().unwrap_or_default().to_string(),
            owned: r["owned"].as_bool().unwrap_or(false),
            access_token: r["accessToken"].as_str().unwrap_or_default().to_string(),
            connections: r["connections"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| Connection {
                            protocol: c["protocol"].as_str().unwrap_or_default().to_string(),
                            address: c["address"].as_str().unwrap_or_default().to_string(),
                            port: c["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(32400),
                            uri: c["uri"].as_str().unwrap_or_default().to_string(),
                            local: c["local"].as_bool().unwrap_or(false),
                            relay: c["relay"].as_bool().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The account's servers from plex.tv.
pub async fn resources(tv: &str, client_id: &str, token: &str) -> Result<Vec<Resource>> {
    let rb = tv_client()?
        .get(format!("{tv}/resources"))
        .query(&[("includeHttps", "1"), ("includeRelay", "0")])
        .header("X-Plex-Token", token);
    let v = json_ok(plex_headers(rb, client_id).send().await.map_err(send_err)?).await?;
    Ok(parse_resources(&v))
}

// ---- one server ----------------------------------------------------------------

/// How to reach one server. Debug leaves the token out.
#[derive(Clone, PartialEq)]
pub struct Conn {
    /// "https://192-168-1-5.<hash>.plex.direct:32400" or "http://192.168.1.5:32400".
    pub base: String,
    /// plex.direct name pinned to the LAN address.
    pub resolve: Option<(String, SocketAddr)>,
    pub token: String,
    pub client_id: String,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("base", &self.base)
            .field("resolve", &self.resolve)
            .field("token", &format_args!("<{} chars>", self.token.len()))
            .finish()
    }
}

/// Every way to reach `res` from this PC, best first. Builds with TLS (Windows): its
/// HTTPS plex.direct addresses, local ones first, then remote ones (a router that loops
/// back). Never plain HTTP there. Builds without TLS (Linux, dev only): plain HTTP on its
/// addresses. Relays are never used. Only the server's own token is sent, never the
/// account's. A server on Docker's host network also lists its bridge addresses
/// (172.x) as local, so the caller tries them in order (`Conn::identity`).
pub fn candidates(res: &Resource, client_id: &str, https: bool) -> Result<Vec<Conn>> {
    if res.access_token.is_empty() {
        bail!("plex.tv gave no access token for {}", res.name);
    }
    let mut ordered: Vec<&Connection> = res.connections.iter().filter(|c| !c.relay).collect();
    ordered.sort_by_key(|c| !c.local); // stable: local first, plex.tv's order kept
    let conn = |base: String, resolve| Conn {
        base,
        resolve,
        token: res.access_token.clone(),
        client_id: client_id.to_string(),
    };
    let mut out = Vec::new();
    for c in ordered {
        let Ok(ip) = c.address.parse::<IpAddr>() else {
            continue;
        };
        if https {
            if c.protocol != "https" {
                continue;
            }
            let Ok(url) = reqwest::Url::parse(&c.uri) else {
                continue;
            };
            let Some(host) = url.host_str().filter(|h| h.ends_with(".plex.direct")) else {
                continue;
            };
            let port = url.port().unwrap_or(c.port);
            let base = format!("https://{host}:{port}");
            if !out.iter().any(|o: &Conn| o.base == base) {
                out.push(conn(base, Some((host.to_string(), SocketAddr::new(ip, port)))));
            }
        } else {
            let host = match ip {
                IpAddr::V6(ip) => format!("[{ip}]"),
                IpAddr::V4(_) => c.address.clone(),
            };
            let base = format!("http://{host}:{}", c.port);
            if !out.iter().any(|o: &Conn| o.base == base) {
                out.push(conn(base, None));
            }
        }
    }
    if out.is_empty() {
        if https {
            bail!("{} has no secure address (turn on Secure connections in Plex)", res.name);
        }
        bail!("plex.tv lists no address for {}", res.name);
    }
    Ok(out)
}

impl Conn {
    /// Where this goes, for messages: the address it is pinned to, else the base.
    pub fn target(&self) -> String {
        match &self.resolve {
            Some((_, addr)) => addr.to_string(),
            None => self.base.trim_start_matches("http://").to_string(),
        }
    }

    /// The server's machine identifier (plex.tv's clientIdentifier), quickly: used to
    /// find which listed address really is this server.
    pub async fn identity(&self) -> Result<String> {
        let client = self.client_with(PROBE_TIMEOUT)?;
        let rb = client.get(format!("{}/identity", self.base));
        let v = json_ok(plex_headers(rb, &self.client_id).send().await.map_err(send_err)?).await?;
        Ok(v["MediaContainer"]["machineIdentifier"].as_str().unwrap_or_default().to_string())
    }

    fn client(&self) -> Result<reqwest::Client> {
        self.client_with(TIMEOUT)
    }

    fn client_with(&self, timeout: Duration) -> Result<reqwest::Client> {
        let mut b = reqwest::Client::builder().timeout(timeout).user_agent("deskhub");
        if let Some((host, addr)) = &self.resolve {
            b = b.resolve(host, *addr);
        }
        b.build().context("http client")
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response> {
        let rb = self
            .client()?
            .get(format!("{}{path}", self.base))
            .query(query)
            .header("X-Plex-Token", &self.token);
        plex_headers(rb, &self.client_id).send().await.map_err(send_err)
    }

    async fn get_json(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        json_ok(self.get(path, query).await?).await
    }

    /// What plays on the server now.
    pub async fn sessions(&self) -> Result<Vec<Session>> {
        Ok(parse_sessions(&self.get_json("/status/sessions", &[]).await?))
    }

    /// The poster (or album cover) for `art_key`, image bytes.
    pub async fn cover(&self, art_key: &str) -> Result<Vec<u8>> {
        let (w, h) = (COVER_W.to_string(), COVER_H.to_string());
        let q = [("url", art_key), ("width", w.as_str()), ("height", h.as_str()), ("minSize", "1")];
        read_ok(self.get("/photo/:/transcode", &q).await?).await
    }

    /// Version and library sizes, for the board while nothing plays.
    pub async fn info(&self) -> Result<ServerInfo> {
        let root = self.get_json("/", &[]).await?;
        let sections = self.get_json("/library/sections", &[]).await?;
        let mut counts = Counts::default();
        for sec in sections["MediaContainer"]["Directory"].as_array().into_iter().flatten() {
            let key = sec["key"].as_str().unwrap_or_default();
            // Plex item types: 1 movie, 2 show, 4 episode, 10 track.
            let wanted: &[(&str, usize)] = match sec["type"].as_str().unwrap_or_default() {
                "movie" => &[("1", 0)],
                "show" => &[("2", 1), ("4", 2)],
                "artist" => &[("10", 3)],
                _ => &[],
            };
            for (kind, slot) in wanted {
                let q = [("type", *kind), ("X-Plex-Container-Start", "0"), ("X-Plex-Container-Size", "0")];
                let mc = self.get_json(&format!("/library/sections/{key}/all"), &q).await?;
                let mc = &mc["MediaContainer"];
                let n = mc["totalSize"].as_u64().or_else(|| mc["size"].as_u64()).unwrap_or(0);
                counts.0[*slot] += n;
            }
        }
        Ok(ServerInfo {
            version: short_version(root["MediaContainer"]["version"].as_str().unwrap_or_default()),
            counts,
        })
    }
}

/// "1.41.2.9200-c6bbc1b53" -> "1.41.2".
pub fn short_version(v: &str) -> String {
    let base = v.split('-').next().unwrap_or_default();
    base.split('.').take(3).collect::<Vec<_>>().join(".")
}

/// Movies, shows, episodes, songs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counts(pub [u64; 4]);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerInfo {
    pub version: String,
    pub counts: Counts,
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2,104 episodes" / "1 movie".
pub fn plural(n: u64, word: &str) -> String {
    let w = if n == 1 { &word[..word.len() - 1] } else { word };
    format!("{} {w}", thousands(n))
}

/// "312 movies, 45 shows, 2,104 episodes".
pub fn describe(c: &Counts) -> String {
    ["movies", "shows", "episodes", "songs"]
        .iter()
        .zip(c.0)
        .filter(|(_, n)| *n > 0)
        .map(|(w, n)| plural(n, w))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One session on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// Show and episode, artist and album, or the year.
    pub subtitle: String,
    pub user: String,
    pub device: String,
    pub playing: bool,
    /// Seconds, as of the reply.
    pub pos: f64,
    pub dur: f64,
    /// What to fetch for the cover ("" = none).
    pub art_key: String,
    pub transcoding: bool,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or_default()
}

fn first<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().map(|k| s(v, k)).find(|x| !x.is_empty()).unwrap_or_default()
}

fn episode_line(show: &str, season: Option<i64>, episode: Option<i64>) -> String {
    match (season, episode) {
        (Some(se), Some(ep)) if show.is_empty() => format!("S{se} E{ep}"),
        (Some(se), Some(ep)) => format!("{show} - S{se} E{ep}"),
        _ => show.to_string(),
    }
}

fn id_text(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// The sessions in a /status/sessions reply.
pub fn parse_sessions(v: &Value) -> Vec<Session> {
    let Some(list) = v["MediaContainer"]["Metadata"].as_array() else {
        return Vec::new();
    };
    list.iter()
        .map(|m| {
            let (subtitle, art) = match s(m, "type") {
                "episode" => (
                    episode_line(s(m, "grandparentTitle"), m["parentIndex"].as_i64(), m["index"].as_i64()),
                    first(m, &["grandparentThumb", "parentThumb", "thumb"]),
                ),
                "track" => {
                    let artist = first(m, &["grandparentTitle", "originalTitle"]);
                    let album = s(m, "parentTitle");
                    let sub = match (artist.is_empty(), album.is_empty()) {
                        (false, false) => format!("{artist} - {album}"),
                        (false, true) => artist.to_string(),
                        _ => album.to_string(),
                    };
                    (sub, first(m, &["parentThumb", "thumb"]))
                }
                _ => (
                    m["year"].as_i64().filter(|y| *y != 0).map(|y| y.to_string()).unwrap_or_default(),
                    s(m, "thumb"),
                ),
            };
            let player = &m["Player"];
            let id = [&m["Session"]["id"], &m["sessionKey"], &m["ratingKey"]]
                .into_iter()
                .map(id_text)
                .find(|x| !x.is_empty())
                .unwrap_or_default();
            Session {
                id,
                title: s(m, "title").to_string(),
                subtitle,
                user: s(&m["User"], "title").to_string(),
                device: first(player, &["title", "product"]).to_string(),
                playing: s(player, "state") == "playing",
                pos: m["viewOffset"].as_f64().unwrap_or(0.0) / 1000.0,
                dur: m["duration"].as_f64().unwrap_or(0.0) / 1000.0,
                art_key: art.to_string(),
                transcoding: s(&m["TranscodeSession"], "videoDecision") == "transcode",
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A tiny HTTP server: answers by path (without the query) and records each request head.
    pub(crate) struct Fake {
        pub port: u16,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    pub(crate) async fn fake(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: HashMap<&str, (u16, Vec<u8>)> =
            routes.into_iter().map(|(p, st, b)| (p, (st, b))).collect();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut req = Vec::new();
                let mut buf = [0u8; 2048];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                let head = String::from_utf8_lossy(&req).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let path = target.split('?').next().unwrap_or("/").to_string();
                log.lock().unwrap().push(head);
                let (status, body) = routes.get(path.as_str()).cloned().unwrap_or((404, b"{}".to_vec()));
                let line = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                sock.write_all(line.as_bytes()).await.ok();
                sock.write_all(&body).await.ok();
                sock.shutdown().await.ok();
            }
        });
        Fake { port, requests }
    }

    pub(crate) fn conn(port: u16) -> Conn {
        Conn {
            base: format!("http://127.0.0.1:{port}"),
            resolve: None,
            token: "server-token".into(),
            client_id: "deskhub-test".into(),
        }
    }

    pub(crate) fn sessions_json() -> Value {
        json!({"MediaContainer": {"size": 3, "Metadata": [
            {"type": "episode", "title": "Pilot", "grandparentTitle": "Show", "parentIndex": 1, "index": 2,
             "grandparentThumb": "/library/metadata/1/thumb/9", "thumb": "/ep", "viewOffset": 61000,
             "duration": 1800000, "sessionKey": "12", "User": {"title": "alex"},
             "Player": {"title": "Living Room TV", "state": "paused"}, "Session": {"id": "abc"}},
            {"type": "movie", "title": "Film", "year": 2020, "thumb": "/m", "viewOffset": 0, "duration": 6000000,
             "sessionKey": 13, "User": {"title": "sam"}, "Player": {"product": "Plex Web", "state": "playing"},
             "TranscodeSession": {"videoDecision": "transcode"}},
            {"type": "track", "title": "Song", "grandparentTitle": "Band", "parentTitle": "Album",
             "parentThumb": "/a", "thumb": "/t", "ratingKey": "99", "Player": {"state": "playing"}}
        ]}})
    }

    #[test]
    fn parses_episode_movie_and_track() {
        let v = parse_sessions(&sessions_json());
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].id, "abc");
        assert_eq!(v[0].subtitle, "Show - S1 E2");
        assert_eq!(v[0].art_key, "/library/metadata/1/thumb/9");
        assert_eq!((v[0].pos, v[0].dur), (61.0, 1800.0));
        assert!(!v[0].playing);
        assert_eq!((v[0].device.as_str(), v[0].user.as_str()), ("Living Room TV", "alex"));
        assert_eq!(v[1].id, "13");
        assert_eq!(v[1].subtitle, "2020");
        assert_eq!(v[1].device, "Plex Web");
        assert!(v[1].transcoding && v[1].playing);
        assert_eq!(v[2].id, "99");
        assert_eq!(v[2].subtitle, "Band - Album");
        assert_eq!(v[2].art_key, "/a");
        assert!(parse_sessions(&json!({"MediaContainer": {"size": 0}})).is_empty());
        assert!(parse_sessions(&json!([])).is_empty());
    }

    #[test]
    fn episode_lines_and_versions() {
        assert_eq!(episode_line("", Some(2), Some(3)), "S2 E3");
        assert_eq!(episode_line("Show", None, Some(3)), "Show");
        assert_eq!(short_version("1.41.2.9200-c6bbc1b53"), "1.41.2");
        assert_eq!(short_version(""), "");
    }

    #[test]
    fn library_description() {
        assert_eq!(describe(&Counts([312, 45, 2104, 0])), "312 movies, 45 shows, 2,104 episodes");
        assert_eq!(describe(&Counts([1, 0, 0, 1_234_567])), "1 movie, 1,234,567 songs");
        assert_eq!(describe(&Counts::default()), "");
        assert_eq!(plural(1, "shows"), "1 show");
    }

    fn resource(conns: Value) -> Resource {
        parse_resources(&json!([
            {"name": "Phone", "provides": "client,player", "clientIdentifier": "p"},
            {"name": "Home", "provides": "server", "clientIdentifier": "srv1", "owned": true,
             "accessToken": "srv-token", "connections": conns}
        ]))
        .remove(0)
    }

    #[test]
    fn resources_keep_servers_only_and_hide_tokens() {
        let r = resource(json!([]));
        assert_eq!((r.name.as_str(), r.id.as_str(), r.owned), ("Home", "srv1", true));
        assert!(!format!("{r:?}").contains("srv-token"));
    }

    #[test]
    fn candidates_are_secure_local_first_and_skip_relays() {
        let r = resource(json!([
            {"protocol": "https", "address": "203.0.113.9", "port": 32400,
             "uri": "https://203-0-113-9.abc.plex.direct:32400", "local": false},
            {"protocol": "https", "address": "10.0.0.1", "port": 443, "uri": "https://relay.plex.direct:443",
             "local": true, "relay": true},
            {"protocol": "https", "address": "172.17.0.1", "port": 32400,
             "uri": "https://172-17-0-1.abc.plex.direct:32400", "local": true},
            {"protocol": "https", "address": "192.168.68.103", "port": 32400,
             "uri": "https://192-168-68-103.abc.plex.direct:32400", "local": true},
            {"protocol": "http", "address": "192.168.68.103", "port": 32400,
             "uri": "http://192.168.68.103:32400", "local": true}
        ]));
        let c = candidates(&r, "cid", true).unwrap();
        let bases: Vec<&str> = c.iter().map(|c| c.base.as_str()).collect();
        assert_eq!(bases, [
            "https://172-17-0-1.abc.plex.direct:32400",
            "https://192-168-68-103.abc.plex.direct:32400",
            "https://203-0-113-9.abc.plex.direct:32400",
        ]);
        assert_eq!(
            c[1].resolve,
            Some(("192-168-68-103.abc.plex.direct".into(), "192.168.68.103:32400".parse().unwrap()))
        );
        assert_eq!(c[1].target(), "192.168.68.103:32400");
        assert_eq!(c[1].token, "srv-token");
        assert!(!format!("{:?}", c[1]).contains("srv-token"));
        // Without TLS (Linux builds): plain HTTP on each address.
        let c = candidates(&r, "cid", false).unwrap();
        assert_eq!(c[0].base, "http://172.17.0.1:32400");
        assert_eq!(c[1].base, "http://192.168.68.103:32400");
        assert_eq!(c[1].resolve, None);
    }

    #[test]
    fn candidates_need_https_and_the_server_token() {
        let mut r = resource(json!([{"protocol": "http", "address": "192.168.1.5", "port": 32400,
            "uri": "http://192.168.1.5:32400", "local": true}]));
        // TLS builds never fall back to plain HTTP.
        assert!(candidates(&r, "cid", true).is_err());
        let c = candidates(&r, "cid", false).unwrap();
        assert_eq!((c[0].base.as_str(), c[0].token.as_str()), ("http://192.168.1.5:32400", "srv-token"));
        // Never the account token: no server token, no connection.
        r.access_token.clear();
        assert!(candidates(&r, "cid", false).is_err());
        assert!(candidates(&resource(json!([])), "cid", false).is_err());
    }

    #[tokio::test]
    async fn identity_names_the_server() {
        let f = fake(vec![("/identity", 200, br#"{"MediaContainer": {"machineIdentifier": "srv1"}}"#.to_vec())]).await;
        assert_eq!(conn(f.port).identity().await.unwrap(), "srv1");
        assert!(conn(1).identity().await.is_err());
    }

    #[test]
    fn auth_url_carries_client_and_code() {
        let u = auth_url("deskhub-1", "ab12");
        assert!(u.starts_with("https://app.plex.tv/auth#?clientID=deskhub-1&code=ab12&"));
        assert!(u.contains("context%5Bdevice%5D%5Bproduct%5D=Desk+Hub"));
    }

    #[test]
    fn client_ids_are_random() {
        let (a, b) = (new_client_id(), new_client_id());
        assert!(a.starts_with("deskhub-") && a.len() == 40);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn pin_flow_against_a_fake_plex_tv() {
        let f = fake(vec![
            ("/pins", 201, br#"{"id": 77, "code": "zx9"}"#.to_vec()),
            ("/pins/77", 200, br#"{"id": 77, "authToken": "acct-token"}"#.to_vec()),
            ("/pins/78", 200, br#"{"id": 78, "authToken": null}"#.to_vec()),
            ("/resources", 200, br#"[{"name":"Home","provides":"server","clientIdentifier":"s"}]"#.to_vec()),
        ])
        .await;
        let tv = format!("http://127.0.0.1:{}", f.port);
        let pin = pin_start(&tv, "cid").await.unwrap();
        assert_eq!(pin, Pin { id: 77, code: "zx9".into() });
        assert_eq!(pin_poll(&tv, "cid", 77).await.unwrap().as_deref(), Some("acct-token"));
        assert_eq!(pin_poll(&tv, "cid", 78).await.unwrap(), None);
        assert!(pin_poll(&tv, "cid", 79).await.is_err()); // 404: expired
        let res = resources(&tv, "cid", "acct-token").await.unwrap();
        assert_eq!(res.len(), 1);
        let reqs = f.requests.lock().unwrap();
        assert!(reqs[0].starts_with("POST /pins?strong=true "));
        let lower = reqs[0].to_lowercase();
        assert!(lower.contains("x-plex-client-identifier: cid") && lower.contains("x-plex-product: desk hub"));
        let last = reqs.last().unwrap().to_lowercase();
        assert!(last.contains("includehttps=1") && last.contains("x-plex-token: acct-token"));
    }

    #[tokio::test]
    async fn server_sessions_cover_and_info() {
        let f = fake(vec![
            ("/status/sessions", 200, sessions_json().to_string().into_bytes()),
            ("/photo/:/transcode", 200, b"IMG".to_vec()),
            ("/", 200, br#"{"MediaContainer": {"version": "1.41.2.9200-abc"}}"#.to_vec()),
            ("/library/sections", 200,
             br#"{"MediaContainer": {"Directory": [{"key": "1", "type": "movie"}, {"key": "2", "type": "show"}, {"key": "3", "type": "photo"}]}}"#.to_vec()),
            ("/library/sections/1/all", 200, br#"{"MediaContainer": {"totalSize": 312}}"#.to_vec()),
            ("/library/sections/2/all", 200, br#"{"MediaContainer": {"totalSize": 45, "size": 0}}"#.to_vec()),
        ])
        .await;
        let c = conn(f.port);
        assert_eq!(c.sessions().await.unwrap().len(), 3);
        assert_eq!(c.cover("/library/metadata/1/thumb/9").await.unwrap(), b"IMG");
        let info = c.info().await.unwrap();
        assert_eq!(info.version, "1.41.2");
        // The fake answers both show queries (shows and episodes) with 45.
        assert_eq!(info.counts, Counts([312, 45, 45, 0]));
        let reqs = f.requests.lock().unwrap();
        let first = reqs[0].to_lowercase();
        assert!(first.contains("x-plex-token: server-token") && first.contains("accept: application/json"));
        assert!(reqs[1].contains("url=%2Flibrary%2Fmetadata%2F1%2Fthumb%2F9") && reqs[1].contains("width=256"));
    }

    #[tokio::test]
    async fn errors_name_the_status_not_the_url() {
        let f = fake(vec![("/status/sessions", 401, b"{}".to_vec())]).await;
        let e = conn(f.port).sessions().await.unwrap_err().to_string();
        assert_eq!(e, "HTTP 401");
        let e = conn(1).sessions().await.unwrap_err().to_string();
        assert!(!e.contains("server-token"), "{e}");
    }
}
