//! Internet page: latency probe, jitter, loss, uptime, data used and a speed test, as the
//! "net" message every 2 s (port of companion/deskhub/netstats.py and its net_loop).
//! Data used is kept in ~/.deskhub/net_usage.json, the same file the companion wrote.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use sysinfo::Networks;
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::Ctx;
use crate::protocol::Incoming;

const TARGETS: [&str; 2] = ["1.1.1.1:443", "8.8.8.8:443"];
const TICK: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const WINDOW: usize = 60;
const SAVE_EVERY_S: i64 = 60;
const IP_REFRESH: Duration = Duration::from_secs(30 * 60);
/// At most one speed test per two minutes (SPEEDTEST_MIN_GAP_S).
const SPEEDTEST_GAP: Duration = Duration::from_secs(120);
const CF: &str = "https://speed.cloudflare.com";
const DOWN_BYTES: usize = 25_000_000;
const UP_BYTES: usize = 10_000_000;

/// The numbers behind the "net" message; time comes in from outside so it can be tested.
#[derive(Debug, Default)]
pub struct NetState {
    samples: VecDeque<Option<f64>>,
    online: bool,
    up_since: Option<i64>,
    last_drop: Option<Value>,
    went_offline: Option<i64>,
    day: String,
    month: String,
    today_bytes: u64,
    month_bytes: u64,
    saved_at: i64,
    pub speed: Option<Value>,
    pub testing: bool,
    pub ip: String,
}

impl NetState {
    /// Saved usage and the last speed test (the companion's format).
    pub fn from_saved(v: &Value) -> Self {
        Self {
            day: v["day"].as_str().unwrap_or("").into(),
            month: v["month"].as_str().unwrap_or("").into(),
            today_bytes: v["today"].as_u64().unwrap_or(0),
            month_bytes: v["month_bytes"].as_u64().unwrap_or(0),
            speed: v.get("speed").filter(|s| s.is_object()).cloned(),
            ..Default::default()
        }
    }

    pub fn saved(&self) -> Value {
        json!({"day": self.day, "month": self.month, "today": self.today_bytes,
            "month_bytes": self.month_bytes, "speed": self.speed})
    }

    /// One probe result (ms, None = no answer) at `now`.
    pub fn probe(&mut self, now: i64, ms: Option<f64>) {
        self.samples.push_back(ms);
        while self.samples.len() > WINDOW {
            self.samples.pop_front();
        }
        let online = self.samples.iter().rev().take(3).any(Option::is_some);
        if online && !self.online {
            if let Some(off) = self.went_offline.take() {
                self.last_drop = Some(json!({"at": off, "dur_s": now - off}));
            }
            self.up_since = Some(now);
        } else if !online && self.online {
            self.went_offline = Some(now);
        }
        self.online = online;
    }

    /// Adds `bytes` used at `now`; the day and month totals start over at local midnight.
    /// True when it's time to save.
    pub fn usage(&mut self, now: i64, bytes: u64) -> bool {
        let local = Local.timestamp_opt(now, 0).single().unwrap_or_else(Local::now);
        let (day, month) = (local.format("%Y-%m-%d").to_string(), local.format("%Y-%m").to_string());
        let mut changed = false;
        if day != self.day {
            (self.day, self.today_bytes, changed) = (day, 0, true);
        }
        if month != self.month {
            (self.month, self.month_bytes, changed) = (month, 0, true);
        }
        self.today_bytes += bytes;
        self.month_bytes += bytes;
        if changed || now - self.saved_at >= SAVE_EVERY_S {
            self.saved_at = now;
            return true;
        }
        false
    }

    fn ok(&self) -> Vec<f64> {
        self.samples.iter().flatten().copied().collect()
    }

    /// Median of the last 5 answers.
    pub fn latency_ms(&self) -> Option<f64> {
        let ok = self.ok();
        let mut recent: Vec<f64> = ok[ok.len().saturating_sub(5)..].to_vec();
        if recent.is_empty() {
            return None;
        }
        recent.sort_by(f64::total_cmp);
        let n = recent.len();
        Some(if n % 2 == 1 { recent[n / 2] } else { (recent[n / 2 - 1] + recent[n / 2]) / 2.0 })
    }

    /// Mean change between answers in a row.
    pub fn jitter_ms(&self) -> Option<f64> {
        let ok = self.ok();
        (ok.len() >= 2).then(|| ok.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (ok.len() - 1) as f64)
    }

    pub fn loss_pct(&self) -> i64 {
        if self.samples.is_empty() {
            return 0;
        }
        let lost = self.samples.iter().filter(|s| s.is_none()).count();
        (100.0 * lost as f64 / self.samples.len() as f64).round() as i64
    }

    pub fn message(&self, show_ip: bool) -> Value {
        let r1 = |v: Option<f64>| v.map(|x| (x * 10.0).round() / 10.0);
        json!({
            "type": "net",
            "online": self.online,
            "latency_ms": r1(self.latency_ms()),
            "jitter_ms": r1(self.jitter_ms()),
            "loss_pct": self.loss_pct(),
            "hist": self.samples.iter().map(|s| s.map_or(-1, |x| x.round() as i64)).collect::<Vec<_>>(),
            "up_since": self.up_since,
            "last_drop": self.last_drop,
            "today_bytes": self.today_bytes,
            "month_bytes": self.month_bytes,
            "speed": self.speed,
            "testing": self.testing,
            "ip": if show_ip { self.ip.as_str() } else { "" },
        })
    }
}

/// TCP connect time in ms to the first target that answers.
async fn tcp_probe() -> Option<f64> {
    for t in TARGETS {
        let start = Instant::now();
        if let Ok(Ok(_)) = tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(t)).await {
            return Some(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    None
}

/// The board's message: the chart is the saved history (history.rs), not the 2-minute window.
fn board_message(st: &NetState, s: &crate::settings::Settings) -> Value {
    let mut m = st.message(s.internet.show_ip);
    m["hist"] = json!(crate::history::chart("ping", s.display.chart_hours));
    m["hist_label"] = json!(crate::history::label(s.display.chart_hours));
    m
}

fn state_file() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".deskhub").join("net_usage.json"))
}

fn load() -> NetState {
    state_file()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|v| NetState::from_saved(&v))
        .unwrap_or_default()
}

fn save(s: &NetState) {
    let Some(p) = state_file() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Temp file + rename: a crash never leaves half a file.
    let tmp = p.with_extension("json.tmp");
    let res = std::fs::write(&tmp, s.saved().to_string()).and_then(|()| std::fs::rename(&tmp, &p));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        log::debug!("net usage save failed: {e}");
    }
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder().timeout(Duration::from_secs(60)).user_agent("deskhub").build().ok()
}

/// Cloudflare's speed test: 5 small requests for the ping, 25 MB down, 10 MB up.
async fn speedtest() -> anyhow::Result<Value> {
    let http = client().ok_or_else(|| anyhow::anyhow!("no HTTP client"))?;
    let mut pings = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        http.get(format!("{CF}/__down?bytes=0")).send().await?.error_for_status()?.bytes().await?;
        pings.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let t = Instant::now();
    let mut resp = http.get(format!("{CF}/__down?bytes={DOWN_BYTES}")).send().await?.error_for_status()?;
    let mut got = 0usize;
    while let Some(chunk) = resp.chunk().await? {
        got += chunk.len();
    }
    let down_s = t.elapsed().as_secs_f64().max(1e-6);
    let t = Instant::now();
    http.post(format!("{CF}/__up")).body(vec![0u8; UP_BYTES]).send().await?.error_for_status()?;
    let up_s = t.elapsed().as_secs_f64().max(1e-6);
    pings.sort_by(f64::total_cmp);
    let r1 = |x: f64| (x * 10.0).round() / 10.0;
    Ok(json!({
        "down_mbps": r1(got as f64 * 8.0 / down_s / 1e6),
        "up_mbps": r1(UP_BYTES as f64 * 8.0 / up_s / 1e6),
        "ping_ms": r1(pings[2]),
        "at": chrono::Utc::now().timestamp(),
    }))
}

async fn public_ip() -> Option<String> {
    let text = client()?.get(format!("{CF}/cdn-cgi/trace")).send().await.ok()?.text().await.ok()?;
    text.lines().find_map(|l| l.strip_prefix("ip=")).map(|ip| ip.trim().to_string())
}

enum Done {
    Speed(Option<Value>),
    Ip(Option<String>),
}

pub async fn run(mut ctx: Ctx) {
    let mut st = load();
    let mut nets = Networks::new_with_refreshed_list();
    let mut tick = tokio::time::interval(TICK);
    let (tx, mut rx) = mpsc::channel::<Done>(4);
    let mut last_test: Option<Instant> = None;
    let mut last_ip: Option<Instant> = None;
    let mut auto_at = Instant::now();
    loop {
        let start_test = tokio::select! {
            _ = tick.tick() => {
                let ms = tcp_probe().await;
                nets.refresh(true);
                // sysinfo gives bytes since the previous refresh.
                let bytes: u64 = nets.list().values().map(|n| n.received() + n.transmitted()).sum();
                let now = chrono::Utc::now().timestamp();
                st.probe(now, ms);
                crate::history::record("ping", ms);
                if st.usage(now, bytes) {
                    save(&st);
                }
                let (show_ip, hours) = {
                    let s = ctx.settings.borrow();
                    (s.internet.show_ip, s.internet.speedtest_hours)
                };
                if show_ip && last_ip.is_none_or(|t| t.elapsed() >= IP_REFRESH) {
                    last_ip = Some(Instant::now());
                    let tx = tx.clone();
                    tokio::spawn(async move { let _ = tx.send(Done::Ip(public_ip().await)).await; });
                }
                ctx.out.publish("net", board_message(&st, &ctx.settings.borrow()));
                hours > 0.0 && auto_at.elapsed() >= Duration::from_secs_f64(hours * 3600.0)
            }
            msg = ctx.incoming.recv() => match msg {
                Ok(Incoming::Speedtest) => true,
                Ok(_) | Err(RecvError::Lagged(_)) => false,
                Err(RecvError::Closed) => return,
            },
            Some(done) = rx.recv() => {
                match done {
                    Done::Speed(result) => {
                        st.testing = false;
                        if let Some(v) = result {
                            st.speed = Some(v);
                            save(&st);
                        }
                    }
                    Done::Ip(ip) => if let Some(ip) = ip { st.ip = ip },
                }
                ctx.out.publish("net", board_message(&st, &ctx.settings.borrow()));
                false
            }
        };
        if start_test && !st.testing && last_test.is_none_or(|t| t.elapsed() >= SPEEDTEST_GAP) {
            last_test = Some(Instant::now());
            auto_at = Instant::now();
            st.testing = true;
            ctx.out.publish("net", board_message(&st, &ctx.settings.borrow()));
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = match speedtest().await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::warn!("speed test failed: {e:#}");
                        None
                    }
                };
                let _ = tx.send(Done::Speed(result)).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_drops_and_numbers() {
        let mut s = NetState::default();
        s.probe(100, Some(10.0));
        assert!(s.online);
        assert_eq!(s.up_since, Some(100));
        s.probe(102, Some(20.0));
        s.probe(104, None);
        s.probe(106, None);
        assert!(s.online, "one answer in the last three keeps it online");
        s.probe(108, None);
        assert!(!s.online);
        s.probe(110, Some(14.0));
        assert!(s.online);
        assert_eq!(s.last_drop, Some(json!({"at": 108, "dur_s": 2})));
        assert_eq!(s.up_since, Some(110));
        assert_eq!(s.latency_ms(), Some(14.0)); // median of 10, 20, 14
        assert_eq!(s.jitter_ms(), Some(8.0)); // |20-10| + |14-20| over 2
        assert_eq!(s.loss_pct(), 50);
        let m = s.message(false);
        assert_eq!(m["hist"], json!([10, 20, -1, -1, -1, 14]));
        assert_eq!(m["ip"], "");
        assert_eq!(m["type"], "net");
    }

    #[test]
    fn window_and_empty() {
        let mut s = NetState::default();
        assert_eq!((s.latency_ms(), s.jitter_ms(), s.loss_pct()), (None, None, 0));
        for i in 0..100 {
            s.probe(i, Some(i as f64));
        }
        assert_eq!(s.message(false)["hist"].as_array().unwrap().len(), WINDOW);
        assert_eq!(s.latency_ms(), Some(97.0));
    }

    #[test]
    fn usage_resets_by_day_and_saves() {
        let mut s = NetState::default();
        let t0 = Local.with_ymd_and_hms(2026, 11, 30, 23, 59, 0).unwrap().timestamp();
        assert!(s.usage(t0, 100), "first call sets the day: save");
        assert!(!s.usage(t0 + 10, 50));
        assert_eq!((s.today_bytes, s.month_bytes), (150, 150));
        assert!(s.usage(t0 + 120, 7), "new day and month");
        assert_eq!((s.today_bytes, s.month_bytes), (7, 7));
        let back = NetState::from_saved(&s.saved());
        assert_eq!((back.today_bytes, back.month_bytes, back.day.as_str()), (7, 7, "2026-12-01"));
    }

    #[test]
    fn reads_the_companions_file() {
        let v = json!({"day": "2026-11-26", "month": "2026-11", "today": 5, "month_bytes": 9,
            "speed": {"down_mbps": 100.0, "up_mbps": 20.0, "ping_ms": 9.0, "at": 1}});
        let s = NetState::from_saved(&v);
        assert_eq!(s.message(true)["speed"]["down_mbps"], 100.0);
        assert_eq!(NetState::from_saved(&json!({"speed": null})).speed, None);
    }
}
