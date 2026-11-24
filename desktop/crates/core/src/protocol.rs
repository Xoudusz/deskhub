//! Board protocol: the hello handshake and the messages the board sends back
//! (firmware/main/hub_server.c and the ui_*.c senders).

use serde_json::{json, Value};

/// The first message of every connection.
pub fn hello(token: &str) -> Value {
    json!({"type": "hello", "token": token})
}

/// A message from the board, parsed. Anything unknown or malformed is `Other`.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Reply to hello. `version` is the firmware version.
    HelloOk { version: String },
    /// A pressed alert button (ui_alert.c).
    AlertAction { id: String, action: String },
    /// A media control (ui.c): cmd "toggle", "next", "prev", "seek", ...
    Media { cmd: String, value: i64 },
    /// Launch a Steam game from the board (ui_games.c).
    SteamLaunch { appid: u64 },
    /// Run a speed test (ui_net.c).
    Speedtest,
    /// A shortcut tile pressed (ui_shortcuts.c).
    Shortcut { id: i64 },
    Other(Value),
}

impl Incoming {
    /// Parse a board message. Unknown types, and known types with missing or wrong-typed
    /// fields, come back as `Other` with the original value.
    pub fn from_value(v: Value) -> Self {
        match parse(&v) {
            Some(m) => m,
            None => Self::Other(v),
        }
    }
}

fn parse(v: &Value) -> Option<Incoming> {
    Some(match v.get("type")?.as_str()? {
        "hello_ok" => Incoming::HelloOk { version: text(v, "version").unwrap_or_else(|| "?".into()) },
        "alert_action" => Incoming::AlertAction { id: text(v, "id")?, action: text(v, "action")? },
        "media" => Incoming::Media { cmd: text(v, "cmd")?, value: v.get("value")?.as_i64()? },
        "steam_launch" => Incoming::SteamLaunch { appid: v.get("appid")?.as_u64()? },
        "speedtest" => Incoming::Speedtest,
        "shortcut" => Incoming::Shortcut { id: v.get("id")?.as_i64()? },
        _ => return None,
    })
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Incoming {
        Incoming::from_value(serde_json::from_str(s).unwrap())
    }

    #[test]
    fn hello_shape() {
        assert_eq!(hello("secret"), json!({"type": "hello", "token": "secret"}));
    }

    #[test]
    fn hello_ok_with_and_without_version() {
        assert_eq!(parse_str(r#"{"type":"hello_ok","version":"0.1.0"}"#), Incoming::HelloOk { version: "0.1.0".into() });
        assert_eq!(parse_str(r#"{"type":"hello_ok"}"#), Incoming::HelloOk { version: "?".into() });
    }

    #[test]
    fn alert_action() {
        assert_eq!(
            parse_str(r#"{"type":"alert_action","id":"perm-7","action":"allow"}"#),
            Incoming::AlertAction { id: "perm-7".into(), action: "allow".into() }
        );
    }

    #[test]
    fn media_command() {
        assert_eq!(parse_str(r#"{"type":"media","cmd":"seek","value":30}"#), Incoming::Media { cmd: "seek".into(), value: 30 });
        assert_eq!(parse_str(r#"{"type":"media","cmd":"toggle","value":0}"#), Incoming::Media { cmd: "toggle".into(), value: 0 });
    }

    #[test]
    fn steam_launch() {
        assert_eq!(parse_str(r#"{"type":"steam_launch","appid":570}"#), Incoming::SteamLaunch { appid: 570 });
    }

    #[test]
    fn speedtest() {
        assert_eq!(parse_str(r#"{"type":"speedtest"}"#), Incoming::Speedtest);
    }

    #[test]
    fn shortcut() {
        assert_eq!(parse_str(r#"{"type":"shortcut","id":3}"#), Incoming::Shortcut { id: 3 });
    }

    #[test]
    fn unknown_and_malformed_are_other() {
        let unknown = json!({"type": "stats", "cpu": 12});
        assert_eq!(Incoming::from_value(unknown.clone()), Incoming::Other(unknown));
        // Known type, missing field.
        assert!(matches!(parse_str(r#"{"type":"steam_launch"}"#), Incoming::Other(_)));
        // Known type, wrong field type.
        assert!(matches!(parse_str(r#"{"type":"shortcut","id":"x"}"#), Incoming::Other(_)));
        assert!(matches!(parse_str(r#"{"type":"media","cmd":"seek","value":"30"}"#), Incoming::Other(_)));
        // No type at all.
        assert!(matches!(parse_str(r#"{"id":1}"#), Incoming::Other(_)));
    }
}
