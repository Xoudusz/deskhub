//! End-to-end check of the Claude waiting alert: reads the session files deskhub-cli's
//! hook wrote, runs them through the alerts tracker and prints the board message.
//! With a board URL (and DESKHUB_TOKEN set) it also sends that message to the board.
//!
//! cargo run -p deskhub-core --example alert_chain -- <home>/.deskhub/claude_sessions [ws://host/ws]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use deskhub_core::collectors::alerts::{load_sessions, Tracker};
use deskhub_core::protocol::hello;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("sessions folder");
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    let sessions = load_sessions(std::path::Path::new(&root), now);
    println!("sessions: {}", sessions.len());
    let msg = Tracker::default()
        .poll(sessions, now, 0, 30.0)
        .expect("a first poll always returns a message");
    println!("board message: {msg}");

    if let Some(url) = args.next() {
        let token = std::env::var("DESKHUB_TOKEN")?;
        let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await?;
        ws.send(Message::Text(hello(&token).to_string().into()))
            .await?;
        let reply = tokio::time::timeout(Duration::from_secs(5), ws.next()).await?;
        match reply {
            Some(Ok(Message::Text(t))) if t.contains("hello_ok") => println!("board: hello_ok"),
            other => anyhow::bail!("no hello_ok: {other:?}"),
        }
        ws.send(Message::Text(msg.to_string().into())).await?;
        println!("alert sent; holding the link 8 s");
        tokio::time::sleep(Duration::from_secs(8)).await;
        let _ = ws.close(None).await;
    }
    Ok(())
}
