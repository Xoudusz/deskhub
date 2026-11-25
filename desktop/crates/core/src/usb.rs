//! First setup over the USB cable (firmware usb_setup.c): the app writes lines
//! `@deskhub {json}` to the board's console port and picks replies of the same shape out of
//! the log lines. Commands: `info` (version, host, token, Wi-Fi), `scan` (networks the board
//! sees), `wifi` (join and save a network).
//!
//! The port must be opened with DTR and RTS off and left that way: on the P4 board a port
//! opened with them on hung the board until it was replugged.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PREFIX: &str = "@deskhub ";
/// Longest line kept; longer ones (never a reply) are dropped.
const LINE_MAX: usize = 4096;

pub const INFO_WAIT: Duration = Duration::from_secs(5);
/// The board waits up to 10 s for its scan.
pub const SCAN_WAIT: Duration = Duration::from_secs(15);
/// The board waits up to 40 s for the join.
pub const WIFI_WAIT: Duration = Duration::from_secs(50);

/// USB vendor ids of the boards' USB ports: Espressif (S3 built-in), WCH CH34x and
/// Silicon Labs CP210x (USB-to-UART bridges).
pub const VIDS: [u16; 3] = [0x303A, 0x1A86, 0x10C4];

/// What `info` answers. Debug leaves the token out.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct BoardInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub ssid: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub saved: u32,
}

impl std::fmt::Debug for BoardInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardInfo")
            .field("version", &self.version)
            .field("variant", &self.variant)
            .field("host", &self.host)
            .field("token", &format_args!("<{} chars>", self.token.len()))
            .field("ssid", &self.ssid)
            .field("ip", &self.ip)
            .field("saved", &self.saved)
            .finish()
    }
}

/// A network the board sees.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub ssid: String,
    pub rssi: i32,
    pub secure: bool,
}

/// A joined network: the board's address on it and its name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Joined {
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub host: String,
}

/// The protocol over a port (a serial port, or a fake one in tests). Reads must time out
/// (return `TimedOut`/`WouldBlock` or 0 bytes) instead of blocking forever.
pub struct Link<P: Read + Write> {
    port: P,
    buf: Vec<u8>,
}

impl<P: Read + Write> Link<P> {
    pub fn new(port: P) -> Self {
        Self {
            port,
            buf: Vec::new(),
        }
    }

    /// Send one command; return the reply of the same type (or the board's error reply).
    /// Lines that aren't replies (logs) are skipped.
    pub fn request(&mut self, msg: &Value, wait: Duration) -> Result<Value> {
        let want = msg
            .get("type")
            .and_then(Value::as_str)
            .context("command without type")?
            .to_string();
        let line = format!("{PREFIX}{msg}\n");
        self.port
            .write_all(line.as_bytes())
            .and_then(|_| self.port.flush())
            .context("writing to the board's port")?;
        let end = Instant::now() + wait;
        while Instant::now() < end {
            while let Some(line) = self.next_line() {
                let Some(json) = line.strip_prefix(PREFIX) else {
                    continue;
                };
                let Ok(reply) = serde_json::from_str::<Value>(json.trim()) else {
                    continue;
                };
                match reply.get("type").and_then(Value::as_str) {
                    Some(t) if t == want => return Ok(reply),
                    Some("error") => bail!(
                        "the board said: {}",
                        reply["error"].as_str().unwrap_or("error")
                    ),
                    _ => continue,
                }
            }
            let mut chunk = [0u8; 512];
            match self.port.read(&mut chunk) {
                Ok(0) => std::thread::sleep(Duration::from_millis(20)),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(anyhow!("reading the board's port: {e}")),
            }
            if self.buf.len() > LINE_MAX && !self.buf.contains(&b'\n') {
                self.buf.clear();
            }
        }
        bail!("the board did not answer \"{want}\" (is it a Desk Hub with firmware 0.9.0 or newer?)")
    }

    fn next_line(&mut self) -> Option<String> {
        let at = self.buf.iter().position(|&b| b == b'\n')?;
        let raw: Vec<u8> = self.buf.drain(..=at).collect();
        Some(
            String::from_utf8_lossy(&raw)
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        )
    }

    pub fn info(&mut self) -> Result<BoardInfo> {
        let reply = self.request(&json!({"type": "info"}), INFO_WAIT)?;
        serde_json::from_value(reply).context("the board's info reply")
    }

    pub fn scan(&mut self) -> Result<Vec<Network>> {
        let reply = self.request(&json!({"type": "scan"}), SCAN_WAIT)?;
        if let Some(e) = reply.get("error").and_then(Value::as_str) {
            bail!("scan: {e}");
        }
        serde_json::from_value(reply["networks"].clone()).context("the board's scan reply")
    }

    /// Join and save a network. The password stays out of every error text.
    pub fn wifi(&mut self, ssid: &str, pass: &str) -> Result<Joined> {
        check_wifi(ssid, pass)?;
        let msg = json!({"type": "wifi", "ssid": ssid, "pass": pass});
        let reply = self.request(&msg, WIFI_WAIT)?;
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!(
                "{}",
                reply["error"].as_str().unwrap_or("the board could not join")
            );
        }
        serde_json::from_value(reply).context("the board's wifi reply")
    }
}

/// The board's own rule (usb_setup.c do_wifi): names 1-32 bytes, passwords 8-64 or none.
pub fn check_wifi(ssid: &str, pass: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 {
        bail!("Wi-Fi names have 1 to 32 characters");
    }
    if !pass.is_empty() && !(8..=64).contains(&pass.len()) {
        bail!("Wi-Fi passwords have 8 to 64 characters (or none for an open network)");
    }
    Ok(())
}

/// A link over a real serial port.
pub type BoardLink = Link<Box<dyn Port>>;

/// A port the link runs over.
pub trait Port: Read + Write + Send {}
impl<T: Read + Write + Send> Port for T {}

/// A serial port that may be a board.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,
    pub label: String,
}

#[cfg(windows)]
mod port {
    use super::*;

    /// USB serial ports with a board's vendor id.
    pub fn find() -> Result<Vec<PortInfo>> {
        let ports = serialport::available_ports().context("listing serial ports")?;
        Ok(ports
            .into_iter()
            .filter_map(|p| match p.port_type {
                serialport::SerialPortType::UsbPort(u) if VIDS.contains(&u.vid) => {
                    Some(PortInfo {
                        label: u.product.unwrap_or_else(|| "USB serial".into()),
                        name: p.port_name,
                    })
                }
                _ => None,
            })
            .collect())
    }

    /// Open at 115200 8N1 with no flow control: serialport sets DTR and RTS to disabled
    /// in the port settings on Windows, so neither line is raised (see the module doc).
    /// Don't call write_data_terminal_ready / write_request_to_send or dtr_on_open.
    pub fn open(name: &str) -> Result<BoardLink> {
        let port = serialport::new(name, 115_200)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(200))
            .open()
            .with_context(|| format!("opening {name} (is another program using it?)"))?;
        Ok(Link::new(Box::new(port)))
    }
}

#[cfg(windows)]
pub use port::{find, open};

/// USB setup is Windows only; elsewhere no board is ever found.
#[cfg(not(windows))]
pub fn find() -> Result<Vec<PortInfo>> {
    Ok(Vec::new())
}

#[cfg(not(windows))]
pub fn open(name: &str) -> Result<BoardLink> {
    bail!("USB setup works on Windows only ({name})")
}

/// The first port that answers `info`: its name, the link (still open) and the info.
pub fn find_board() -> Result<(String, BoardLink, BoardInfo)> {
    let ports = find()?;
    if ports.is_empty() {
        bail!("No board on USB. Plug it into this PC with a data cable (not a charge-only one).");
    }
    let mut last = None;
    for p in ports {
        match open(&p.name).and_then(|mut link| link.info().map(|i| (link, i))) {
            Ok((link, info)) => return Ok((p.name, link, info)),
            Err(e) => last = Some(format!("{}: {e:#}", p.name)),
        }
    }
    bail!("{}", last.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A port that answers each written command with scripted chunks.
    struct Fake {
        written: Vec<u8>,
        /// Per command: the chunks the board sends back.
        replies: VecDeque<Vec<Vec<u8>>>,
        pending: VecDeque<Vec<u8>>,
    }

    impl Fake {
        fn new(replies: Vec<Vec<&str>>) -> Self {
            Self {
                written: Vec::new(),
                replies: replies
                    .into_iter()
                    .map(|r| r.into_iter().map(|c| c.as_bytes().to_vec()).collect())
                    .collect(),
                pending: VecDeque::new(),
            }
        }
    }

    impl Write for Fake {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(b);
            if b.ends_with(b"\n") {
                if let Some(r) = self.replies.pop_front() {
                    self.pending.extend(r);
                }
            }
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for Fake {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            match self.pending.pop_front() {
                Some(c) => {
                    out[..c.len()].copy_from_slice(&c);
                    Ok(c.len())
                }
                None => Err(std::io::Error::new(ErrorKind::TimedOut, "timeout")),
            }
        }
    }

    const INFO: &str = r#"@deskhub {"type":"info","version":"firmware-v0.9.0","variant":"p4rev1","host":"deskhub","token":"abcd2345efgh6789","ssid":"Home","ip":"10.0.0.9","saved":1}"#;

    #[test]
    fn info_skips_logs_and_split_lines() {
        let reply = format!("{INFO}\r\n");
        let (a, b) = reply.split_at(30);
        let mut link = Link::new(Fake::new(vec![vec![
            "I (123) net: got ip\r\n@deskhub not json\n",
            a,
            b,
        ]]));
        let info = link.info().unwrap();
        assert_eq!(info.host, "deskhub");
        assert_eq!(info.token, "abcd2345efgh6789");
        assert_eq!(info.saved, 1);
        assert_eq!(
            String::from_utf8(link.port.written.clone()).unwrap(),
            "@deskhub {\"type\":\"info\"}\n"
        );
        let shown = format!("{info:?}");
        assert!(!shown.contains("abcd2345"), "{shown}");
        assert!(shown.contains("<16 chars>"), "{shown}");
    }

    #[test]
    fn scan_lists_networks_and_reports_board_errors() {
        let mut link = Link::new(Fake::new(vec![
            vec![
                "@deskhub {\"type\":\"scan\",\"networks\":[{\"ssid\":\"Home\",\"rssi\":-40,\"secure\":true},{\"ssid\":\"Cafe\",\"rssi\":-70,\"secure\":false}]}\n",
            ],
            vec!["@deskhub {\"type\":\"scan\",\"error\":\"the scan did not finish\",\"networks\":[]}\n"],
        ]));
        let nets = link.scan().unwrap();
        assert_eq!(nets.len(), 2);
        assert_eq!(
            nets[1],
            Network {
                ssid: "Cafe".into(),
                rssi: -70,
                secure: false
            }
        );
        let err = link.scan().unwrap_err().to_string();
        assert!(err.contains("did not finish"), "{err}");
    }

    #[test]
    fn wifi_sends_credentials_and_reads_result() {
        let mut link = Link::new(Fake::new(vec![
            vec!["@deskhub {\"type\":\"wifi\",\"ok\":true,\"ip\":\"10.0.0.9\",\"host\":\"deskhub\"}\n"],
            vec!["@deskhub {\"type\":\"wifi\",\"ok\":false,\"error\":\"couldn't join\"}\n"],
        ]));
        let j = link.wifi("Home", "password1").unwrap();
        assert_eq!(j.ip, "10.0.0.9");
        let sent = String::from_utf8(link.port.written.clone()).unwrap();
        assert_eq!(
            sent,
            "@deskhub {\"type\":\"wifi\",\"ssid\":\"Home\",\"pass\":\"password1\"}\n"
        );
        let err = link.wifi("Home", "wrongpass").unwrap_err().to_string();
        assert!(err.contains("couldn't join"), "{err}");
        assert!(!err.contains("wrongpass"), "{err}");
    }

    #[test]
    fn wifi_rules_match_the_board() {
        assert!(check_wifi("Home", "").is_ok());
        assert!(check_wifi("Home", "12345678").is_ok());
        assert!(check_wifi("", "12345678").is_err());
        assert!(check_wifi(&"x".repeat(33), "").is_err());
        assert!(check_wifi("Home", "short").is_err());
        assert!(check_wifi("Home", &"p".repeat(65)).is_err());
        // Nothing is written for a refused request.
        let mut link = Link::new(Fake::new(vec![]));
        assert!(link.wifi("Home", "short").is_err());
        assert!(link.port.written.is_empty());
    }

    #[test]
    fn unknown_command_and_silence_are_errors() {
        let mut link = Link::new(Fake::new(vec![vec![
            "@deskhub {\"type\":\"error\",\"error\":\"unknown command\"}\n",
        ]]));
        let err = link
            .request(&json!({"type": "nope"}), Duration::from_millis(300))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown command"), "{err}");

        let mut quiet = Link::new(Fake::new(vec![vec!["I (5) boot: just logs\n"]]));
        let err = quiet
            .request(&json!({"type": "info"}), Duration::from_millis(200))
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not answer"), "{err}");
    }
}
