//! Install/remove the Desk Hub status line and hooks in Claude Code's settings.json,
//! pointing at deskhub-cli. Port of companion/install_claude.py.
//!
//! Pure functions over explicit paths. `settings_path` is Claude Code's settings.json
//! (normally ~/.claude/settings.json). The Desk Hub state directory (~/.deskhub, where
//! deskhub-cli reads statusline_previous.json) is passed explicitly with the `*_in`
//! functions; the plain ones derive it from the settings file's grandparent folder.
//! `cli` is the deskhub-cli executable.
//!
//! Key order: serde_json is built with `preserve_order`, so the user's key order survives.
//! Writes go to a temp file that is renamed over settings.json.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

/// Events the hook command is registered for.
pub const HOOK_EVENTS: [&str; 7] = [
    "Notification",
    "PermissionRequest",
    "PreToolUse",
    "UserPromptSubmit",
    "PostToolUse",
    "Stop",
    "SessionEnd",
];
const CLI_MARK: &str = "deskhub-cli";
/// Python-era commands, removed when migrating.
const PY_STATUSLINE: &str = "statusline_bridge.py";
const PY_HOOK: &str = "hook_bridge.py";
const STATE_FILE: &str = "statusline_previous.json";
const BACKUP_FILE: &str = "settings.json.bak";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Statusline,
    Hooks,
    All,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Installed {
    pub statusline: bool,
    pub hooks: bool,
}

pub fn install(settings_path: &Path, cli: &Path, what: What) -> Result<()> {
    install_in(settings_path, &state_dir(settings_path), cli, what)
}

/// `install` with the Desk Hub state directory given (deskhub-cli's ~/.deskhub). Use this
/// when settings.json is not at ~/.claude (CLAUDE_CONFIG_DIR).
pub fn install_in(settings_path: &Path, dir: &Path, cli: &Path, what: What) -> Result<()> {
    if what != What::Hooks {
        install_statusline(settings_path, dir, cli)?;
    }
    if what != What::Statusline {
        install_hooks(settings_path, dir, cli)?;
    }
    Ok(())
}

pub fn remove(settings_path: &Path, what: What) -> Result<()> {
    remove_in(settings_path, &state_dir(settings_path), what)
}

/// `remove` with the Desk Hub state directory given.
pub fn remove_in(settings_path: &Path, dir: &Path, what: What) -> Result<()> {
    if what != What::Hooks {
        remove_statusline(settings_path, dir)?;
    }
    if what != What::Statusline {
        remove_hooks(settings_path, dir)?;
    }
    Ok(())
}

/// Whether the status line and every hook event point at this `cli` exactly.
pub fn status(settings_path: &Path, cli: &Path) -> Installed {
    let Ok(settings) = load(settings_path) else {
        return Installed::default();
    };
    let statusline =
        settings.get("statusLine").and_then(cmd_of) == Some(statusline_cmd(cli).as_str());
    let want = hook_cmd(cli);
    let hooks = HOOK_EVENTS.iter().all(|ev| has_hook(&settings, ev, &want));
    Installed { statusline, hooks }
}

fn statusline_cmd(cli: &Path) -> String {
    format!("{} statusline", quoted(cli))
}

fn hook_cmd(cli: &Path) -> String {
    format!("{} hook", quoted(cli))
}

fn quoted(cli: &Path) -> String {
    format!("\"{}\"", cli.to_string_lossy())
}

fn hook_matcher(event: &str) -> &'static str {
    // PreToolUse only for the tools that wait for you, so other tool calls skip the hook.
    if event == "PreToolUse" {
        "AskUserQuestion|ExitPlanMode"
    } else {
        ""
    }
}

/// `~/.deskhub` for a settings file at `~/.claude/settings.json`.
fn state_dir(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from(".deskhub"), |home| home.join(".deskhub"))
}

fn cmd_of(v: &Value) -> Option<&str> {
    v.get("command")?.as_str()
}

fn is_ours_statusline(cmd: &str) -> bool {
    cmd.contains(CLI_MARK) || cmd.contains(PY_STATUSLINE)
}

fn is_ours_hook(cmd: &str) -> bool {
    cmd.contains(CLI_MARK) || cmd.contains(PY_HOOK)
}

fn has_hook(settings: &Map<String, Value>, event: &str, command: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|e| {
                e.get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hs| hs.iter().any(|h| cmd_of(h) == Some(command)))
            })
        })
}

fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    match value {
        Value::Object(m) => Ok(m),
        _ => bail!("{} is not a JSON object", path.display()),
    }
}

fn write_settings(path: &Path, settings: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(settings)?;
    text.push('\n');
    // Temp file + rename: a crash mid-write never leaves a truncated settings.json.
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let tmp = path.with_file_name(format!(".{name}.deskhub.tmp"));
    let res = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, path));
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.with_context(|| format!("write {}", path.display()))
}

/// Copy settings.json to `dir/settings.json.bak`, once. A later change keeps the first backup.
fn backup(settings_path: &Path, dir: &Path) -> Result<()> {
    if !settings_path.is_file() || dir.join(BACKUP_FILE).exists() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    fs::copy(settings_path, dir.join(BACKUP_FILE))?;
    Ok(())
}

/// The saved previous status line, unwrapped from the Python `{"statusLine": ...}` form.
/// None when there is no state file or it cannot be read.
fn read_state(dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(dir.join(STATE_FILE)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(match value {
        Value::Object(mut m) if m.contains_key("statusLine") => {
            m.remove("statusLine").unwrap_or(Value::Null)
        }
        other => other,
    })
}

fn write_state(dir: &Path, previous: &Value) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(STATE_FILE), serde_json::to_string(previous)?)?;
    Ok(())
}

/// Previous status line recovered from a Python-era `--wrap "<cmd>"` command, for users
/// whose state file was lost. Empty object when there is nothing to recover.
fn wrapped_previous(current: &Value) -> Value {
    const MARK: &str = "--wrap \"";
    let cmd = cmd_of(current).unwrap_or("");
    let Some(idx) = cmd.find(MARK) else {
        return json!({});
    };
    let Some(rest) = cmd[idx + MARK.len()..].strip_suffix('"') else {
        return json!({});
    };
    let mut prev = json!({"type": "command", "command": rest.replace("\\\"", "\"")});
    if let Some(padding) = current.get("padding") {
        prev["padding"] = padding.clone();
    }
    prev
}

fn install_statusline(settings_path: &Path, dir: &Path, cli: &Path) -> Result<()> {
    let mut settings = load(settings_path)?;
    let current = settings
        .get("statusLine")
        .filter(|v| v.is_object())
        .cloned();
    let ours = current
        .as_ref()
        .and_then(cmd_of)
        .is_some_and(is_ours_statusline);

    match (&current, ours) {
        // Already ours: keep the saved previous status line, just point at this cli.
        (Some(cur), true) => {
            if !dir.join(STATE_FILE).exists() {
                write_state(dir, &wrapped_previous(cur))?;
            }
        }
        // Foreign (or none): save it so the cli can forward to it and remove can restore it.
        (cur, _) => write_state(dir, &cur.clone().unwrap_or_else(|| json!({})))?,
    }

    let mut obj = Map::new();
    obj.insert("type".into(), json!("command"));
    obj.insert("command".into(), json!(statusline_cmd(cli)));
    if let Some(padding) = current.as_ref().and_then(|c| c.get("padding")) {
        obj.insert("padding".into(), padding.clone());
    }
    settings.insert("statusLine".into(), Value::Object(obj));
    backup(settings_path, dir)?;
    write_settings(settings_path, &settings)
}

fn remove_statusline(settings_path: &Path, dir: &Path) -> Result<()> {
    let mut settings = load(settings_path)?;
    // A status line the user set after installing is theirs: leave it alone.
    let ours = settings
        .get("statusLine")
        .and_then(cmd_of)
        .is_none_or(is_ours_statusline);
    match read_state(dir).filter(|_| ours) {
        Some(prev) => {
            if prev.as_object().is_some_and(|o| !o.is_empty()) {
                settings.insert("statusLine".into(), prev);
            } else {
                settings.remove("statusLine");
            }
            backup(settings_path, dir)?;
            write_settings(settings_path, &settings)?;
            let _ = fs::remove_file(dir.join(STATE_FILE));
        }
        None => {
            let ours = settings
                .get("statusLine")
                .and_then(cmd_of)
                .is_some_and(is_ours_statusline);
            if ours {
                settings.remove("statusLine");
                backup(settings_path, dir)?;
                write_settings(settings_path, &settings)?;
            }
        }
    }
    Ok(())
}

fn install_hooks(settings_path: &Path, dir: &Path, cli: &Path) -> Result<()> {
    let mut settings = load(settings_path)?;
    // Re-running replaces our entries instead of duplicating them.
    strip_hooks(&mut settings);
    let cmd = hook_cmd(cli);
    let hooks = settings
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("settings.json \"hooks\" is not an object")?;
    for event in HOOK_EVENTS {
        let entries = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("settings.json hooks.{event} is not an array"))?;
        entries.push(json!({
            "matcher": hook_matcher(event),
            "hooks": [{"type": "command", "command": cmd}],
        }));
    }
    backup(settings_path, dir)?;
    write_settings(settings_path, &settings)
}

fn remove_hooks(settings_path: &Path, dir: &Path) -> Result<()> {
    let mut settings = load(settings_path)?;
    if strip_hooks(&mut settings) {
        backup(settings_path, dir)?;
        write_settings(settings_path, &settings)?;
    }
    Ok(())
}

/// Remove our hook commands in place. Other hooks in the same group are kept.
/// Returns whether anything was removed.
fn strip_hooks(settings: &mut Map<String, Value>) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(Value::Array(entries)) = hooks.get_mut(&event) else {
            continue;
        };
        let old = std::mem::take(entries);
        let mut kept = Vec::with_capacity(old.len());
        let mut touched = false;
        for mut entry in old {
            let (group_touched, group_empty) = prune_group(&mut entry);
            touched |= group_touched;
            if !(group_touched && group_empty) {
                kept.push(entry);
            }
        }
        if touched {
            changed = true;
        }
        if touched && kept.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, Value::Array(kept));
        }
    }
    let hooks_empty = hooks.is_empty();
    if changed && hooks_empty {
        settings.remove("hooks");
    }
    changed
}

/// Drop our commands from one matcher group. Returns (touched, now empty).
fn prune_group(entry: &mut Value) -> (bool, bool) {
    let Some(inner) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
        return (false, false);
    };
    let before = inner.len();
    inner.retain(|h| !cmd_of(h).is_some_and(is_ours_hook));
    (inner.len() != before, inner.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A temp home with `.claude/settings.json`, removed on drop.
    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "deskhub-install-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(root.join(".claude")).unwrap();
            Self { root }
        }

        fn settings(&self) -> PathBuf {
            self.root.join(".claude").join("settings.json")
        }

        fn deskhub(&self) -> PathBuf {
            self.root.join(".deskhub")
        }

        fn cli(&self) -> PathBuf {
            PathBuf::from("/opt/deskhub/deskhub-cli")
        }

        fn write(&self, v: &Value) {
            fs::write(self.settings(), serde_json::to_string_pretty(v).unwrap()).unwrap();
        }

        fn read(&self) -> Value {
            serde_json::from_str(&fs::read_to_string(self.settings()).unwrap()).unwrap()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn python_hook(event: &str) -> Value {
        json!({"matcher": "", "hooks": [{"type": "command",
            "command": format!("\"C:/py/python.exe\" \"C:/u/.deskhub/hook_bridge.py\" {event}")}]})
    }

    #[test]
    fn fresh_install_adds_statusline_and_all_hooks() {
        let sb = Sandbox::new("fresh");
        install(&sb.settings(), &sb.cli(), What::All).unwrap();
        let s = sb.read();
        assert_eq!(s["statusLine"]["type"], "command");
        assert_eq!(
            s["statusLine"]["command"],
            "\"/opt/deskhub/deskhub-cli\" statusline"
        );
        for event in HOOK_EVENTS {
            assert_eq!(s["hooks"][event].as_array().unwrap().len(), 1, "{event}");
            assert_eq!(
                s["hooks"][event][0]["hooks"][0]["command"],
                "\"/opt/deskhub/deskhub-cli\" hook"
            );
        }
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["matcher"],
            "AskUserQuestion|ExitPlanMode"
        );
        assert_eq!(s["hooks"]["Stop"][0]["matcher"], "");
        assert_eq!(
            status(&sb.settings(), &sb.cli()),
            Installed {
                statusline: true,
                hooks: true
            }
        );
    }

    #[test]
    fn reinstall_is_idempotent() {
        let sb = Sandbox::new("idem");
        install(&sb.settings(), &sb.cli(), What::All).unwrap();
        let first = fs::read_to_string(sb.settings()).unwrap();
        install(&sb.settings(), &sb.cli(), What::All).unwrap();
        assert_eq!(fs::read_to_string(sb.settings()).unwrap(), first);
        assert_eq!(sb.read()["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn migrates_python_era_entries_and_keeps_unrelated_ones() {
        let sb = Sandbox::new("migrate");
        let wrapped_cmd = "\"C:/py/python.exe\" \"C:/u/.deskhub/statusline_bridge.py\" --wrap \"ccusage \\\"x\\\"\"";
        sb.write(&json!({
            "model": "opus",
            "statusLine": {"type": "command", "command": wrapped_cmd, "padding": 1},
            "hooks": {
                "Stop": [
                    {"matcher": "", "hooks": [{"type": "command", "command": "notify-user"}]},
                    python_hook("Stop"),
                ],
                "PreToolUse": [python_hook("PreToolUse")],
            }
        }));
        install(&sb.settings(), &sb.cli(), What::All).unwrap();

        let s = sb.read();
        assert_eq!(s["model"], "opus");
        assert_eq!(
            s["statusLine"]["command"],
            "\"/opt/deskhub/deskhub-cli\" statusline"
        );
        assert_eq!(s["statusLine"]["padding"], 1);
        let stop = s["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user hook kept, python entry replaced");
        assert_eq!(stop[0]["hooks"][0]["command"], "notify-user");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert!(!fs::read_to_string(sb.settings())
            .unwrap()
            .contains("hook_bridge.py"));
        assert!(!fs::read_to_string(sb.settings())
            .unwrap()
            .contains("statusline_bridge.py"));

        // The recovered previous command is what the cli forwards to.
        let prev: Value =
            serde_json::from_str(&fs::read_to_string(sb.deskhub().join(STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(prev["command"], "ccusage \"x\"");
        assert!(sb.deskhub().join(BACKUP_FILE).is_file());

        remove(&sb.settings(), What::All).unwrap();
        let s = sb.read();
        assert_eq!(s["statusLine"]["command"], "ccusage \"x\"");
        assert_eq!(s["model"], "opus");
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(s["hooks"].get("PreToolUse").is_none());
    }

    #[test]
    fn foreign_statusline_is_saved_and_restored() {
        let sb = Sandbox::new("foreign");
        let original = json!({"type": "command", "command": "ccusage", "padding": 0});
        sb.write(&json!({"statusLine": original.clone()}));
        install(&sb.settings(), &sb.cli(), What::Statusline).unwrap();
        assert_eq!(
            sb.read()["statusLine"]["command"],
            "\"/opt/deskhub/deskhub-cli\" statusline"
        );
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(sb.deskhub().join(STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(saved["command"], "ccusage");

        remove(&sb.settings(), What::Statusline).unwrap();
        assert_eq!(sb.read()["statusLine"], original);
        assert!(!sb.deskhub().join(STATE_FILE).exists());
    }

    #[test]
    fn explicit_state_dir_for_claude_config_dir() {
        // CLAUDE_CONFIG_DIR: settings.json is not under the home, the state stays in ~/.deskhub.
        let sb = Sandbox::new("cfgdir");
        let cfg = sb.root.join("elsewhere").join("cfg");
        fs::create_dir_all(&cfg).unwrap();
        let settings = cfg.join("settings.json");
        let original = json!({"type": "command", "command": "ccusage"});
        fs::write(
            &settings,
            json!({"statusLine": original.clone()}).to_string(),
        )
        .unwrap();
        install_in(&settings, &sb.deskhub(), &sb.cli(), What::Statusline).unwrap();
        assert!(sb.deskhub().join(STATE_FILE).exists());
        remove_in(&settings, &sb.deskhub(), What::Statusline).unwrap();
        let back: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(back["statusLine"], original);
        assert!(
            fs::read_dir(&cfg).unwrap().count() == 1,
            "no temp file left behind"
        );
    }

    #[test]
    fn remove_keeps_a_statusline_the_user_set_after_install() {
        let sb = Sandbox::new("userset");
        sb.write(&json!({"statusLine": {"type": "command", "command": "ccusage"}}));
        install(&sb.settings(), &sb.cli(), What::Statusline).unwrap();
        let mut v = sb.read();
        v["statusLine"] = json!({"type": "command", "command": "starship"});
        sb.write(&v);
        remove(&sb.settings(), What::Statusline).unwrap();
        assert_eq!(sb.read()["statusLine"]["command"], "starship");
    }

    #[test]
    fn key_order_is_kept() {
        let sb = Sandbox::new("order");
        fs::write(sb.settings(), r#"{"zeta": 1, "alpha": 2, "model": "opus"}"#).unwrap();
        install(&sb.settings(), &sb.cli(), What::Statusline).unwrap();
        let text = fs::read_to_string(sb.settings()).unwrap();
        assert!(text.find("zeta").unwrap() < text.find("alpha").unwrap());
    }

    #[test]
    fn remove_clears_everything_and_keeps_other_keys() {
        let sb = Sandbox::new("remove");
        sb.write(&json!({"model": "opus"}));
        install(&sb.settings(), &sb.cli(), What::All).unwrap();
        remove(&sb.settings(), What::All).unwrap();
        let s = sb.read();
        assert_eq!(s, json!({"model": "opus"}));
        assert_eq!(
            status(&sb.settings(), &sb.cli()),
            Installed {
                statusline: false,
                hooks: false
            }
        );
    }

    #[test]
    fn invalid_settings_are_an_error_not_overwritten() {
        let sb = Sandbox::new("invalid");
        fs::write(sb.settings(), "{ not json").unwrap();
        assert!(install(&sb.settings(), &sb.cli(), What::All).is_err());
        assert_eq!(fs::read_to_string(sb.settings()).unwrap(), "{ not json");
    }
}
