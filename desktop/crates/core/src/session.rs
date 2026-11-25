//! The board link: connect, hello, layout and home, forward queued messages, reconnect.
//! Mirrors companion/deskhub/__main__.py session() and run().

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{interval_at, timeout, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::pages::{home_message, layout_message};
use crate::protocol::{hello, Incoming};
use crate::settings::{Location, Settings, State, Status};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Timing knobs. Tests shorten the backoff.
#[derive(Debug, Clone)]
pub struct Options {
    /// Delay before each reconnect attempt; the last entry repeats.
    pub backoff: Vec<Duration>,
    pub connect_timeout: Duration,
    pub hello_timeout: Duration,
    pub ping_interval: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            backoff: [2, 4, 8, 16, 30].map(Duration::from_secs).to_vec(),
            connect_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(10),
        }
    }
}

/// One outgoing frame: JSON text, or a binary frame (ART1 album art, STM1 game art).
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Json(Value),
    Bin(Vec<u8>),
}

impl Out {
    /// For the simulator in the settings window: `{"json": {...}}` or `{"bin": "<base64>"}`.
    pub fn to_sim_json(&self) -> Value {
        use base64::Engine;
        match self {
            Out::Json(v) => serde_json::json!({ "json": v }),
            Out::Bin(b) => {
                serde_json::json!({ "bin": base64::engine::general_purpose::STANDARD.encode(b) })
            }
        }
    }
}

/// The latest frame per key (e.g. "stats", "media", "art"), in first-publish order.
/// The board keeps this state in RAM only, so all of it is resent after each connect.
#[derive(Default)]
struct Latest(Vec<(String, Out)>);

impl Latest {
    fn set(&mut self, key: &str, out: Out) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = out,
            None => self.0.push((key.to_string(), out)),
        }
    }
}

/// Entry point. Must run inside a tokio runtime.
pub struct Service;

/// Control handle for a running service. Dropping it stops the service.
pub struct ServiceHandle {
    status_rx: watch::Receiver<Status>,
    settings_tx: watch::Sender<Settings>,
    out_tx: mpsc::UnboundedSender<Out>,
    incoming_tx: broadcast::Sender<Incoming>,
    stop_tx: watch::Sender<bool>,
    latest: Arc<Mutex<Latest>>,
    sim_tx: broadcast::Sender<Out>,
}

/// Frames buffered for the simulator's live feed. Small: an art frame is up to ~300 KB, and a
/// reader that falls behind resyncs from `sim_snapshot` anyway.
const SIM_FEED: usize = 32;

impl Service {
    pub fn start(settings: Settings) -> ServiceHandle {
        Self::start_with(settings, Options::default())
    }

    pub fn start_with(settings: Settings, opts: Options) -> ServiceHandle {
        let (status_tx, status_rx) = watch::channel(Status::searching(&settings.board.host));
        let (settings_tx, settings_rx) = watch::channel(settings);
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (incoming_tx, _) = broadcast::channel(64);
        let (stop_tx, stop_rx) = watch::channel(false);
        let latest = Arc::new(Mutex::new(Latest::default()));
        let (sim_tx, _) = broadcast::channel(SIM_FEED);
        let task = Task {
            opts,
            settings_rx,
            out_rx,
            status_tx,
            incoming_tx: incoming_tx.clone(),
            stop_rx,
            latest: latest.clone(),
        };
        tokio::spawn(task.run());
        ServiceHandle {
            status_rx,
            settings_tx,
            out_tx,
            incoming_tx,
            stop_tx,
            latest,
            sim_tx,
        }
    }
}

impl ServiceHandle {
    pub fn status(&self) -> watch::Receiver<Status> {
        self.status_rx.clone()
    }

    /// New settings. Sent to the board right away if they changed the layout or home.
    pub fn update_settings(&self, settings: Settings) {
        let (layout, home) = (layout_message(&settings), home_message(&settings));
        let old = self.settings_tx.send_replace(settings);
        // The simulator gets the same layout/home the board would.
        if layout != layout_message(&old) {
            let _ = self.sim_tx.send(Out::Json(layout));
        }
        if home != home_message(&old) {
            let _ = self.sim_tx.send(Out::Json(home));
        }
    }

    /// For the simulator: every frame the board would get on connect, in the same order:
    /// layout, home, then the published state with binary frames (art) first. The board's own
    /// location (from hello_ok) stands in while the app doesn't set one.
    pub fn sim_snapshot(&self) -> Vec<Out> {
        let settings = self.settings_tx.borrow().clone();
        let mut out = vec![
            Out::Json(layout_message(&settings)),
            Out::Json(home_message(&settings)),
        ];
        let latest = self.latest.lock().unwrap();
        let has_location = latest.0.iter().any(|(k, _)| k == "location");
        let mut state: Vec<Out> = latest.0.iter().map(|(_, o)| o.clone()).collect();
        drop(latest);
        state.sort_by_key(|o| matches!(o, Out::Json(_)));
        if !has_location {
            if let Some(l) = self.status_rx.borrow().location.clone() {
                out.push(Out::Json(serde_json::json!({
                    "type": "location", "city": l.city, "lat": l.lat, "lon": l.lon, "tz": l.tz,
                    "fahrenheit": l.fahrenheit, "h24": l.h24,
                })));
            }
        }
        out.extend(state);
        out
    }

    /// The simulator's live feed: every frame published from now on, and layout/home when
    /// settings change them.
    pub fn sim_frames(&self) -> broadcast::Receiver<Out> {
        self.sim_tx.subscribe()
    }

    /// Queue a one-off message for the board. Dropped while the link is down.
    pub fn send(&self, msg: Value) {
        let _ = self.out_tx.send(Out::Json(msg));
    }

    /// Publish the current state for `key` (e.g. "stats"): sent now if connected, and
    /// resent after every reconnect until replaced.
    pub fn publish(&self, key: &str, msg: Value) {
        self.publish_out(key, Out::Json(msg));
    }

    /// Like `publish`, for a binary frame (album or game art).
    pub fn publish_bin(&self, key: &str, frame: Vec<u8>) {
        self.publish_out(key, Out::Bin(frame));
    }

    fn publish_out(&self, key: &str, out: Out) {
        // Store and queue under one lock, so two publishers of a key can't reorder.
        let mut latest = self.latest.lock().unwrap();
        latest.set(key, out.clone());
        let _ = self.sim_tx.send(out.clone());
        let _ = self.out_tx.send(out);
    }

    /// A cheap clonable publisher for collector tasks.
    pub fn publisher(&self) -> Publisher {
        Publisher {
            out_tx: self.out_tx.clone(),
            latest: self.latest.clone(),
            sim_tx: self.sim_tx.clone(),
        }
    }

    /// The live settings (collectors watch this).
    pub fn settings(&self) -> watch::Receiver<Settings> {
        self.settings_tx.subscribe()
    }

    /// Every message the board sends after hello, parsed.
    pub fn incoming(&self) -> broadcast::Receiver<Incoming> {
        self.incoming_tx.subscribe()
    }

    pub fn stop(&self) {
        let _ = self.stop_tx.send_replace(true);
    }
}

/// Publish side of a `ServiceHandle`, for collector tasks (clone freely).
#[derive(Clone)]
pub struct Publisher {
    out_tx: mpsc::UnboundedSender<Out>,
    latest: Arc<Mutex<Latest>>,
    sim_tx: broadcast::Sender<Out>,
}

impl Publisher {
    pub fn publish(&self, key: &str, msg: Value) {
        self.publish_out(key, Out::Json(msg));
    }

    pub fn publish_bin(&self, key: &str, frame: Vec<u8>) {
        self.publish_out(key, Out::Bin(frame));
    }

    /// One-off message (not resent after reconnect).
    pub fn send(&self, msg: Value) {
        let _ = self.out_tx.send(Out::Json(msg));
    }

    fn publish_out(&self, key: &str, out: Out) {
        // Store and queue under one lock, so two publishers of a key can't reorder.
        let mut latest = self.latest.lock().unwrap();
        latest.set(key, out.clone());
        let _ = self.sim_tx.send(out.clone());
        let _ = self.out_tx.send(out);
    }
}

/// How one connection ended.
enum End {
    /// Stop requested, or the handle is gone.
    Stop,
    /// Board host or token changed: reconnect now.
    Reconnect,
    /// The board closed right after hello: wrong token.
    Refused,
    /// Could not connect, no hello_ok in time, or the link dropped. `was_up` is true
    /// once hello_ok had been received.
    Lost { was_up: bool },
}

/// What the board says about itself in hello_ok.
struct Hello {
    version: String,
    variant: Option<String>,
    creds: Option<String>,
    location: Option<Location>,
}

enum Reply {
    Hello(Hello),
    Closed,
    Other,
}

struct Task {
    opts: Options,
    settings_rx: watch::Receiver<Settings>,
    out_rx: mpsc::UnboundedReceiver<Out>,
    status_tx: watch::Sender<Status>,
    incoming_tx: broadcast::Sender<Incoming>,
    stop_rx: watch::Receiver<bool>,
    latest: Arc<Mutex<Latest>>,
}

impl Task {
    async fn run(mut self) {
        let mut attempt = 0usize;
        loop {
            if *self.stop_rx.borrow() {
                break;
            }
            let settings = self.settings_rx.borrow_and_update().clone();
            let host = settings.board.host.clone();
            match self.session(settings).await {
                End::Stop => break,
                End::Reconnect => {
                    attempt = 0;
                    continue;
                }
                End::Refused => {
                    self.status_tx.send_replace(Status {
                        state: State::Refused,
                        host,
                        detail: "Wrong token?".into(),
                        firmware: None,
                        since: None,
                        variant: None,
                        creds: None,
                        location: None,
                    });
                }
                End::Lost { was_up } => {
                    // A link that came up and then dropped retries from the first delay.
                    if was_up {
                        attempt = 0;
                    }
                    self.status_tx.send_replace(Status::searching(&host));
                }
            }
            let delay = self
                .opts
                .backoff
                .get(attempt)
                .or(self.opts.backoff.last())
                .copied()
                .unwrap_or(Duration::from_secs(30));
            attempt += 1;
            if self.wait(delay).await {
                break;
            }
        }
        self.status_tx.send_modify(|s| {
            s.state = State::Stopped;
            s.detail = "Stopped".into();
            s.firmware = None;
            s.since = None;
        });
    }

    /// Sleep through the backoff. Queued messages are dropped while the link is down.
    /// Returns true when the service should stop.
    async fn wait(&mut self, delay: Duration) -> bool {
        let pause = tokio::time::sleep(delay);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => return false,
                msg = self.out_rx.recv() => {
                    if msg.is_none() {
                        return true;
                    }
                }
                changed = self.stop_rx.changed() => {
                    if changed.is_err() || *self.stop_rx.borrow() {
                        return true;
                    }
                }
            }
        }
    }

    async fn session(&mut self, settings: Settings) -> End {
        let host = settings.board.host.clone();
        let url = format!("ws://{host}/ws");
        let mut ws = match timeout(self.opts.connect_timeout, connect_async(url.as_str())).await {
            Ok(Ok((ws, _))) => ws,
            _ => return End::Lost { was_up: false },
        };

        if ws
            .send(Message::text(hello(&settings.board.token).to_string()))
            .await
            .is_err()
        {
            return End::Lost { was_up: false };
        }
        let info = match timeout(self.opts.hello_timeout, read_hello(&mut ws)).await {
            Ok(Reply::Hello(v)) => v,
            Ok(Reply::Closed) => return End::Refused,
            Ok(Reply::Other) | Err(_) => return End::Lost { was_up: false },
        };

        self.status_tx.send_replace(Status {
            state: State::Connected,
            host: host.clone(),
            detail: format!("connected to {host}"),
            firmware: Some(info.version),
            since: Some(now_secs()),
            variant: info.variant,
            creds: info.creds,
            location: info.location,
        });

        // Everything the board keeps in RAM: resent after each connect.
        let mut current = settings;
        let mut last_layout = layout_message(&current);
        let mut last_home = home_message(&current);
        let (mut sink, mut stream) = ws.split();
        if sink.send(frame(&last_layout)).await.is_err()
            || sink.send(frame(&last_home)).await.is_err()
        {
            return End::Lost { was_up: true };
        }
        // Then every published state (stats, media + art, ...), oldest key first. Frames
        // queued while the link was down are stale copies of it (or one-off messages
        // that are dropped while offline anyway): drop them so nothing goes out twice.
        while self.out_rx.try_recv().is_ok() {}
        // Binary frames (art) first, so a media message never points at art the board
        // hasn't received yet.
        let mut snapshot: Vec<Out> = self
            .latest
            .lock()
            .unwrap()
            .0
            .iter()
            .map(|(_, o)| o.clone())
            .collect();
        snapshot.sort_by_key(|o| matches!(o, Out::Json(_)));
        for out in &snapshot {
            if sink.send(out_frame(out)).await.is_err() {
                return End::Lost { was_up: true };
            }
        }

        let mut ping = interval_at(
            Instant::now() + self.opts.ping_interval,
            self.opts.ping_interval,
        );
        loop {
            tokio::select! {
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Text(text))) => forward(&text, &self.incoming_tx),
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return End::Lost { was_up: true },
                    Some(Ok(_)) => {}
                },
                out = self.out_rx.recv() => match out {
                    Some(msg) => {
                        if sink.send(out_frame(&msg)).await.is_err() {
                            return End::Lost { was_up: true };
                        }
                    }
                    None => {
                        let _ = sink.close().await;
                        return End::Stop;
                    }
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        return End::Lost { was_up: true };
                    }
                }
                changed = self.settings_rx.changed() => {
                    if changed.is_err() {
                        let _ = sink.close().await;
                        return End::Stop;
                    }
                    let next = self.settings_rx.borrow_and_update().clone();
                    if next.board != current.board {
                        let _ = sink.close().await;
                        return End::Reconnect;
                    }
                    let layout = layout_message(&next);
                    if layout != last_layout {
                        if sink.send(frame(&layout)).await.is_err() {
                            return End::Lost { was_up: true };
                        }
                        last_layout = layout;
                    }
                    let home = home_message(&next);
                    if home != last_home {
                        if sink.send(frame(&home)).await.is_err() {
                            return End::Lost { was_up: true };
                        }
                        last_home = home;
                    }
                    current = next;
                }
                changed = self.stop_rx.changed() => {
                    if changed.is_err() || *self.stop_rx.borrow() {
                        let _ = sink.close().await;
                        return End::Stop;
                    }
                }
            }
        }
    }
}

fn out_frame(out: &Out) -> Message {
    match out {
        Out::Json(v) => frame(v),
        Out::Bin(b) => Message::binary(b.clone()),
    }
}

fn parse_hello(v: &Value) -> Hello {
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // Firmware with runtime settings reports its location; older firmware sends only the version.
    let location = v.get("location").and_then(|l| {
        let s = |k: &str| l.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Location {
            enabled: false,
            city: s("city")?,
            lat: s("lat")?,
            lon: s("lon")?,
            tz: s("tz")?,
            fahrenheit: l
                .get("fahrenheit")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            h24: l.get("h24").and_then(Value::as_bool).unwrap_or(false),
        })
    });
    Hello {
        version: text("version").unwrap_or_else(|| "?".into()),
        variant: text("variant"),
        creds: text("creds"),
        location,
    }
}

/// Wait for the reply to hello. A close right after hello means the board refused
/// the token; other socket errors only mean the link is gone.
async fn read_hello(ws: &mut Ws) -> Reply {
    while let Some(frame) = ws.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    return Reply::Other;
                };
                if v.get("type").and_then(Value::as_str) != Some("hello_ok") {
                    return Reply::Other;
                }
                return Reply::Hello(parse_hello(&v));
            }
            // A wrong token makes the board drop the socket: a close frame, or a TCP
            // close without one (ResetWithoutClosingHandshake).
            Ok(Message::Close(_))
            | Err(tokio_tungstenite::tungstenite::Error::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            )) => return Reply::Closed,
            // Any other socket error is a network problem, not a refusal.
            Err(_) => return Reply::Other,
            Ok(_) => {}
        }
    }
    Reply::Closed
}

fn forward(text: &str, tx: &broadcast::Sender<Incoming>) {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => {
            let _ = tx.send(Incoming::from_value(v));
        }
        Err(e) => log::warn!("bad message from board: {e}"),
    }
}

fn frame(v: &Value) -> Message {
    Message::text(v.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::{accept_async, WebSocketStream};

    use super::{Options, Out, Service};
    use crate::pages::{home_message, layout_message};
    use crate::protocol::{hello, Incoming};
    use crate::settings::{Settings, State, Status};

    const WAIT: Duration = Duration::from_secs(5);

    fn fast() -> Options {
        Options {
            backoff: vec![Duration::from_millis(20)],
            connect_timeout: Duration::from_secs(2),
            hello_timeout: Duration::from_secs(2),
            ping_interval: Duration::from_secs(30),
        }
    }

    fn settings_for(port: u16, token: &str) -> Settings {
        let mut s = Settings::default();
        s.board.host = format!("127.0.0.1:{port}");
        s.board.token = token.into();
        s
    }

    async fn bind() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    async fn accept(listener: &TcpListener) -> WebSocketStream<TcpStream> {
        let (stream, _) = timeout(WAIT, listener.accept())
            .await
            .expect("no connection")
            .unwrap();
        timeout(WAIT, accept_async(stream))
            .await
            .expect("handshake timeout")
            .unwrap()
    }

    async fn next_json(ws: &mut WebSocketStream<TcpStream>) -> Value {
        let frame = timeout(WAIT, ws.next())
            .await
            .expect("no message")
            .expect("closed")
            .unwrap();
        match frame {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    async fn send_json(ws: &mut WebSocketStream<TcpStream>, v: Value) {
        ws.send(Message::text(v.to_string())).await.unwrap();
    }

    /// Read the hello, check the token, answer hello_ok.
    async fn handshake(ws: &mut WebSocketStream<TcpStream>, token: &str) {
        assert_eq!(next_json(ws).await, hello(token));
        send_json(ws, json!({"type": "hello_ok", "version": "0.1.0"})).await;
    }

    async fn wait_state(svc: &super::ServiceHandle, want: State) -> Status {
        let mut rx = svc.status();
        let st = timeout(WAIT, rx.wait_for(|s| s.state == want))
            .await
            .expect("state timeout")
            .unwrap();
        (*st).clone()
    }

    #[tokio::test]
    async fn hello_then_layout_then_home() {
        let (listener, port) = bind().await;
        let settings = settings_for(port, "secret");
        let svc = Service::start_with(settings.clone(), fast());
        let mut ws = accept(&listener).await;
        handshake(&mut ws, "secret").await;

        assert_eq!(next_json(&mut ws).await, layout_message(&settings));
        assert_eq!(next_json(&mut ws).await, home_message(&settings));

        let st = wait_state(&svc, State::Connected).await;
        assert_eq!(st.firmware.as_deref(), Some("0.1.0"));
        assert_eq!(st.variant, None);
        assert_eq!(st.creds, None);
        assert!(st.since.is_some());
        assert_eq!(st.detail, format!("connected to 127.0.0.1:{port}"));
        svc.stop();
    }

    #[test]
    fn hello_ok_with_board_info() {
        let h = super::parse_hello(&json!({
            "type": "hello_ok", "version": "firmware-v0.7.0", "variant": "p4rev1", "creds": "nvs",
            "location": {"city": "Vienna", "lat": "48.2082", "lon": "16.3738",
                         "tz": "CET-1CEST,M3.5.0,M10.5.0/3", "fahrenheit": false, "h24": true}
        }));
        assert_eq!(h.version, "firmware-v0.7.0");
        assert_eq!(h.variant.as_deref(), Some("p4rev1"));
        assert_eq!(h.creds.as_deref(), Some("nvs"));
        let l = h.location.unwrap();
        assert_eq!(
            (l.city.as_str(), l.lat.as_str(), l.h24, l.fahrenheit),
            ("Vienna", "48.2082", true, false)
        );
        assert!(!l.enabled);

        let old = super::parse_hello(&json!({"type": "hello_ok", "version": "927b5fd"}));
        assert_eq!((old.variant, old.creds, old.location), (None, None, None));
    }

    #[tokio::test]
    async fn wrong_token_is_refused() {
        let (listener, port) = bind().await;
        let svc = Service::start_with(settings_for(port, "wrong"), fast());
        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, hello("wrong"));
        // The board closes the socket without a hello_ok.
        drop(ws);

        let st = wait_state(&svc, State::Refused).await;
        assert_eq!(st.detail, "Wrong token?");
        assert_eq!(st.firmware, None);
        svc.stop();
    }

    #[tokio::test]
    async fn published_state_is_sent_live_and_resent_after_reconnect() {
        let (listener, port) = bind().await;
        let settings = settings_for(port, "secret");
        let svc = Service::start_with(settings.clone(), fast());
        let publisher = svc.publisher();
        // Published while offline: kept for the first connect.
        publisher.publish_bin("art", vec![b'A', b'R', b'T', b'1']);
        publisher.publish("media", json!({"type": "media", "title": "one"}));

        let mut ws = accept(&listener).await;
        handshake(&mut ws, "secret").await;
        assert_eq!(next_json(&mut ws).await, layout_message(&settings));
        assert_eq!(next_json(&mut ws).await, home_message(&settings));
        let art = timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(art, Message::binary(vec![b'A', b'R', b'T', b'1']));
        assert_eq!(
            next_json(&mut ws).await,
            json!({"type": "media", "title": "one"})
        );

        // Live update replaces the stored state and goes out at once.
        wait_state(&svc, State::Connected).await;
        svc.publish("media", json!({"type": "media", "title": "two"}));
        assert_eq!(
            next_json(&mut ws).await,
            json!({"type": "media", "title": "two"})
        );

        // After a drop, the latest state (not the old one) is resent, art first.
        drop(ws);
        let mut ws = accept(&listener).await;
        handshake(&mut ws, "secret").await;
        next_json(&mut ws).await; // layout
        next_json(&mut ws).await; // home
        let art = timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
        assert!(matches!(art, Message::Binary(_)));
        assert_eq!(
            next_json(&mut ws).await,
            json!({"type": "media", "title": "two"})
        );
        svc.stop();
    }

    #[tokio::test]
    async fn reset_after_hello_is_not_refused() {
        let (listener, port) = bind().await;
        let svc = Service::start_with(settings_for(port, "secret"), fast());
        let mut ws = accept(&listener).await;
        assert_eq!(next_json(&mut ws).await, hello("secret"));
        // A network reset (RST), not a board refusal. Linger 0 makes the close an RST.
        #[allow(deprecated)]
        ws.get_ref().set_linger(Some(Duration::ZERO)).unwrap();
        drop(ws);

        // The service retries, and while it does the status says searching, not refused.
        let _again = accept(&listener).await;
        assert_eq!(svc.status().borrow().state, State::Searching);
        svc.stop();
    }

    #[tokio::test]
    async fn settings_update_resends_changed_layout_only() {
        let (listener, port) = bind().await;
        let settings = settings_for(port, "secret");
        let svc = Service::start_with(settings.clone(), fast());
        let mut ws = accept(&listener).await;
        handshake(&mut ws, "secret").await;
        assert_eq!(next_json(&mut ws).await["type"], "layout");
        assert_eq!(next_json(&mut ws).await["type"], "home");
        wait_state(&svc, State::Connected).await;

        let mut next = settings.clone();
        next.display.brightness = 50;
        svc.update_settings(next.clone());
        let layout = next_json(&mut ws).await;
        assert_eq!(layout, layout_message(&next));
        assert_eq!(layout["brightness"], 50);

        // Home did not change, so the next message on the wire is the probe, not home.
        svc.send(json!({"type": "probe"}));
        assert_eq!(next_json(&mut ws).await, json!({"type": "probe"}));
        svc.stop();
    }

    #[tokio::test]
    async fn reconnects_after_drop() {
        let (listener, port) = bind().await;
        let settings = settings_for(port, "secret");
        let svc = Service::start_with(settings.clone(), fast());

        let mut first = accept(&listener).await;
        handshake(&mut first, "secret").await;
        assert_eq!(next_json(&mut first).await["type"], "layout");
        wait_state(&svc, State::Connected).await;
        drop(first);
        wait_state(&svc, State::Searching).await;

        let mut second = accept(&listener).await;
        handshake(&mut second, "secret").await;
        assert_eq!(next_json(&mut second).await, layout_message(&settings));
        assert_eq!(next_json(&mut second).await, home_message(&settings));
        wait_state(&svc, State::Connected).await;
        svc.stop();
    }

    #[tokio::test]
    async fn hello_timeout_retries() {
        let (listener, port) = bind().await;
        let svc = Service::start_with(
            settings_for(port, "secret"),
            Options {
                hello_timeout: Duration::from_millis(100),
                ..fast()
            },
        );
        // First connection: never answer hello.
        let mut silent = accept(&listener).await;
        assert_eq!(next_json(&mut silent).await, hello("secret"));
        // The service gives up and connects again.
        let mut retry = accept(&listener).await;
        assert_eq!(next_json(&mut retry).await, hello("secret"));
        svc.stop();
    }

    #[tokio::test]
    async fn incoming_messages_are_broadcast() {
        let (listener, port) = bind().await;
        let svc = Service::start_with(settings_for(port, "secret"), fast());
        let mut rx = svc.incoming();
        let mut ws = accept(&listener).await;
        handshake(&mut ws, "secret").await;
        assert_eq!(next_json(&mut ws).await["type"], "layout");
        assert_eq!(next_json(&mut ws).await["type"], "home");

        send_json(
            &mut ws,
            json!({"type": "alert_action", "id": "a1", "action": "deny"}),
        )
        .await;
        send_json(&mut ws, json!({"type": "media", "cmd": "next", "value": 0})).await;
        send_json(&mut ws, json!({"type": "steam_launch", "appid": 730})).await;
        send_json(&mut ws, json!({"type": "speedtest"})).await;
        send_json(&mut ws, json!({"type": "shortcut", "id": 4})).await;
        send_json(&mut ws, json!({"type": "unknown", "x": 1})).await;

        let expected = [
            Incoming::AlertAction {
                id: "a1".into(),
                action: "deny".into(),
            },
            Incoming::Media {
                cmd: "next".into(),
                value: 0,
            },
            Incoming::SteamLaunch { appid: 730 },
            Incoming::Speedtest,
            Incoming::Shortcut { id: 4 },
            Incoming::Other(json!({"type": "unknown", "x": 1})),
        ];
        for want in expected {
            let got = timeout(WAIT, rx.recv())
                .await
                .expect("no incoming")
                .unwrap();
            assert_eq!(got, want);
        }
        svc.stop();
    }

    #[test]
    fn sim_json_shapes() {
        assert_eq!(
            Out::Json(json!({"a": 1})).to_sim_json(),
            json!({"json": {"a": 1}})
        );
        assert_eq!(
            Out::Bin(vec![1, 2, 3]).to_sim_json(),
            json!({"bin": "AQID"})
        );
    }

    #[tokio::test]
    async fn sim_snapshot_and_feed() {
        let mut s = settings_for(1, "t");
        s.board.host = "127.0.0.1:1".into();
        let svc = super::Service::start_with(s.clone(), fast());
        svc.publish("stats", json!({"type": "stats", "cpu": 5}));
        svc.publish_bin("art", vec![b'A', b'R', b'T', b'1']);
        let snap = svc.sim_snapshot();
        assert_eq!(snap.len(), 4);
        match (&snap[0], &snap[1]) {
            (Out::Json(a), Out::Json(b)) => {
                assert_eq!(a["type"], "layout");
                assert_eq!(b["type"], "home");
            }
            _ => panic!("layout and home first"),
        }
        assert!(matches!(&snap[2], Out::Bin(_)), "art before JSON state");
        assert!(matches!(&snap[3], Out::Json(v) if v["type"] == "stats"));

        let mut feed = svc.sim_frames();
        svc.publisher().publish("media", json!({"type": "media"}));
        assert!(matches!(feed.recv().await.unwrap(), Out::Json(v) if v["type"] == "media"));
        // A settings change that only touches home sends home, not layout.
        let mut s2 = s.clone();
        s2.home.header = !s2.home.header;
        svc.update_settings(s2.clone());
        assert!(matches!(feed.recv().await.unwrap(), Out::Json(v) if v["type"] == "home"));
        svc.update_settings(s2);
        assert!(feed.try_recv().is_err(), "unchanged settings send nothing");
    }

    #[tokio::test]
    async fn stop_reports_stopped() {
        let (_listener, port) = bind().await;
        let svc = Service::start_with(settings_for(port, "secret"), fast());
        svc.stop();
        let st = wait_state(&svc, State::Stopped).await;
        assert_eq!(st.detail, "Stopped");
    }
}
