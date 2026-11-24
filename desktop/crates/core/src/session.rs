//! The board link: connect, hello, layout and home, forward queued messages, reconnect.
//! Mirrors companion/deskhub/__main__.py session() and run().

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
use crate::settings::{Settings, State, Status};

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

/// Entry point. Must run inside a tokio runtime.
pub struct Service;

/// Control handle for a running service. Dropping it stops the service.
pub struct ServiceHandle {
    status_rx: watch::Receiver<Status>,
    settings_tx: watch::Sender<Settings>,
    out_tx: mpsc::UnboundedSender<Value>,
    incoming_tx: broadcast::Sender<Incoming>,
    stop_tx: watch::Sender<bool>,
}

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
        let task = Task {
            opts,
            settings_rx,
            out_rx,
            status_tx,
            incoming_tx: incoming_tx.clone(),
            stop_rx,
        };
        tokio::spawn(task.run());
        ServiceHandle { status_rx, settings_tx, out_tx, incoming_tx, stop_tx }
    }
}

impl ServiceHandle {
    pub fn status(&self) -> watch::Receiver<Status> {
        self.status_rx.clone()
    }

    /// New settings. Sent to the board right away if they changed the layout or home.
    pub fn update_settings(&self, settings: Settings) {
        let _ = self.settings_tx.send_replace(settings);
    }

    /// Queue a message for the board. Dropped while the link is down.
    pub fn send(&self, msg: Value) {
        let _ = self.out_tx.send(msg);
    }

    /// Every message the board sends after hello, parsed.
    pub fn incoming(&self) -> broadcast::Receiver<Incoming> {
        self.incoming_tx.subscribe()
    }

    pub fn stop(&self) {
        let _ = self.stop_tx.send_replace(true);
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

enum Reply {
    Hello(String),
    Closed,
    Other,
}

struct Task {
    opts: Options,
    settings_rx: watch::Receiver<Settings>,
    out_rx: mpsc::UnboundedReceiver<Value>,
    status_tx: watch::Sender<Status>,
    incoming_tx: broadcast::Sender<Incoming>,
    stop_rx: watch::Receiver<bool>,
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

        if ws.send(Message::text(hello(&settings.board.token).to_string())).await.is_err() {
            return End::Lost { was_up: false };
        }
        let version = match timeout(self.opts.hello_timeout, read_hello(&mut ws)).await {
            Ok(Reply::Hello(v)) => v,
            Ok(Reply::Closed) => return End::Refused,
            Ok(Reply::Other) | Err(_) => return End::Lost { was_up: false },
        };

        self.status_tx.send_replace(Status {
            state: State::Connected,
            host: host.clone(),
            detail: format!("connected to {host}"),
            firmware: Some(version),
            since: Some(now_secs()),
        });

        // Everything the board keeps in RAM: resent after each connect.
        let mut current = settings;
        let mut last_layout = layout_message(&current);
        let mut last_home = home_message(&current);
        let (mut sink, mut stream) = ws.split();
        if sink.send(frame(&last_layout)).await.is_err() || sink.send(frame(&last_home)).await.is_err() {
            return End::Lost { was_up: true };
        }

        let mut ping = interval_at(Instant::now() + self.opts.ping_interval, self.opts.ping_interval);
        loop {
            tokio::select! {
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Text(text))) => forward(&text, &self.incoming_tx),
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return End::Lost { was_up: true },
                    Some(Ok(_)) => {}
                },
                out = self.out_rx.recv() => match out {
                    Some(msg) => {
                        if sink.send(frame(&msg)).await.is_err() {
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

/// Wait for the reply to hello. A close right after hello means the board refused
/// the token; other socket errors only mean the link is gone.
async fn read_hello(ws: &mut Ws) -> Reply {
    while let Some(frame) = ws.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else { return Reply::Other };
                if v.get("type").and_then(Value::as_str) != Some("hello_ok") {
                    return Reply::Other;
                }
                return Reply::Hello(v.get("version").and_then(Value::as_str).unwrap_or("?").to_string());
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

    use super::{Options, Service};
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
        let (stream, _) = timeout(WAIT, listener.accept()).await.expect("no connection").unwrap();
        timeout(WAIT, accept_async(stream)).await.expect("handshake timeout").unwrap()
    }

    async fn next_json(ws: &mut WebSocketStream<TcpStream>) -> Value {
        let frame = timeout(WAIT, ws.next()).await.expect("no message").expect("closed").unwrap();
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
        let st = timeout(WAIT, rx.wait_for(|s| s.state == want)).await.expect("state timeout").unwrap();
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
        assert!(st.since.is_some());
        assert_eq!(st.detail, format!("connected to 127.0.0.1:{port}"));
        svc.stop();
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
            Options { hello_timeout: Duration::from_millis(100), ..fast() },
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

        send_json(&mut ws, json!({"type": "alert_action", "id": "a1", "action": "deny"})).await;
        send_json(&mut ws, json!({"type": "media", "cmd": "next", "value": 0})).await;
        send_json(&mut ws, json!({"type": "steam_launch", "appid": 730})).await;
        send_json(&mut ws, json!({"type": "speedtest"})).await;
        send_json(&mut ws, json!({"type": "shortcut", "id": 4})).await;
        send_json(&mut ws, json!({"type": "unknown", "x": 1})).await;

        let expected = [
            Incoming::AlertAction { id: "a1".into(), action: "deny".into() },
            Incoming::Media { cmd: "next".into(), value: 0 },
            Incoming::SteamLaunch { appid: 730 },
            Incoming::Speedtest,
            Incoming::Shortcut { id: 4 },
            Incoming::Other(json!({"type": "unknown", "x": 1})),
        ];
        for want in expected {
            let got = timeout(WAIT, rx.recv()).await.expect("no incoming").unwrap();
            assert_eq!(got, want);
        }
        svc.stop();
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
