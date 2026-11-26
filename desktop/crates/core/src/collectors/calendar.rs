//! Calendar page: the next events of the user's .ics feeds (e.g. Outlook's published
//! calendar link), as the "events" message (calendar_feed.py + __main__.py events_message).
//! Feed links often hold a secret: logs and errors name the feed, never the link.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::media::ascii_fold;
use super::Ctx;
use crate::ics;
use crate::settings::CalendarFeed;

/// How often the feeds are read again (CALENDAR_REFRESH_S).
const REFRESH: Duration = Duration::from_secs(600);
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 4 * 1024 * 1024;
/// The board keeps 16 (HUB_MAX_EVENTS).
pub const MAX_EVENTS: usize = 16;
pub const DEFAULT_COLOR: &str = "#7c9cff";

/// Per feed, for Settings > Calendar.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FeedStatus {
    pub name: String,
    pub ok: bool,
    pub error: String,
    /// Events of this feed in the window.
    pub events: usize,
}

static STATUS: Mutex<Vec<FeedStatus>> = Mutex::new(Vec::new());

/// The last read of every feed (empty before the first).
pub fn status() -> Vec<FeedStatus> {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

/// One board event (calendar_feed.py _collect).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardEvent {
    pub title: String,
    pub loc: String,
    pub cal: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub color: String,
}

/// A feed's display name: its own, else "Calendar N".
pub fn feed_name(f: &CalendarFeed, i: usize) -> String {
    let n = f.name.trim();
    if n.is_empty() { format!("Calendar {}", i + 1) } else { n.to_string() }
}

/// `#RRGGBB` or the default blue.
fn color_of(f: &CalendarFeed) -> String {
    let c = f.color.trim();
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
    if ok { c.to_string() } else { DEFAULT_COLOR.to_string() }
}

/// The events of one feed's text that haven't ended, window now-1 day .. now+days.
pub fn feed_events(text: &str, name: &str, color: &str, now: i64, days: i64) -> Vec<BoardEvent> {
    ics::events_between(text, now - 86_400, now + days * 86_400)
        .into_iter()
        .filter(|e| e.end > now)
        .map(|e| BoardEvent {
            title: ascii_fold(&e.title, 78),
            loc: ascii_fold(&e.loc, 62),
            cal: ascii_fold(name, 30),
            start: e.start,
            end: e.end,
            all_day: e.all_day,
            color: color.to_string(),
        })
        .collect()
}

/// Sorted by start (all-day first on the same start), at most MAX_EVENTS.
pub fn events_message(mut events: Vec<BoardEvent>, days: i64) -> Value {
    events.sort_by_key(|e| (e.start, !e.all_day));
    events.truncate(MAX_EVENTS);
    let list: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "title": e.title, "loc": e.loc, "cal": e.cal, "start": e.start,
                "end": e.end, "all_day": e.all_day, "color": e.color,
            })
        })
        .collect();
    json!({"type": "events", "events": list, "days": days})
}

/// webcal:// is https:// (Outlook and Apple hand those out).
pub fn fetch_url(url: &str) -> String {
    let u = url.trim();
    match u.get(..9) {
        Some(p) if p.eq_ignore_ascii_case("webcal://") => format!("https://{}", &u[9..]),
        _ => u.to_string(),
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let resp = client.get(fetch_url(url)).send().await.map_err(crate::plex::send_err)?;
    let body = crate::plex::read_limited(resp, MAX_BODY).await?;
    let text = String::from_utf8_lossy(&body).into_owned();
    anyhow::ensure!(text.contains("BEGIN:VCALENDAR"), "the link doesn't give a calendar (.ics)");
    Ok(text)
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

pub async fn run(mut ctx: Ctx) {
    let client = match reqwest::Client::builder().timeout(TIMEOUT).user_agent("deskhub").build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("calendar: {e}");
            return;
        }
    };
    let mut last: Option<Value> = None;
    loop {
        let (feeds, days) = {
            let s = ctx.settings.borrow_and_update();
            (s.calendar.feeds.clone(), s.calendar.days.clamp(1, 60))
        };
        let now = now_secs();
        let mut all = Vec::new();
        let mut status = Vec::new();
        for (i, f) in feeds.iter().enumerate().filter(|(_, f)| !f.url.trim().is_empty()) {
            let name = feed_name(f, i);
            let mut st = FeedStatus { name: name.clone(), ..Default::default() };
            match fetch(&client, &f.url).await {
                Ok(text) => {
                    let ev = feed_events(&text, &name, &color_of(f), now, days);
                    st.ok = true;
                    st.events = ev.len();
                    all.extend(ev);
                }
                Err(e) => {
                    log::warn!("calendar {name:?}: {e:#}");
                    st.error = format!("{e:#}");
                }
            }
            status.push(st);
        }
        if let Ok(mut s) = STATUS.lock() {
            *s = status;
        }
        // No feeds: leave the board alone (it may still get events from the Python companion).
        if !feeds.iter().all(|f| f.url.trim().is_empty()) {
            let msg = events_message(all, days);
            if last.as_ref() != Some(&msg) {
                ctx.out.publish("events", msg.clone());
                last = Some(msg);
            }
        }
        // Wait for the refresh, or for a change to the calendar settings (others don't count).
        let deadline = tokio::time::Instant::now() + REFRESH;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                r = ctx.settings.changed() => {
                    if r.is_err() { return }
                    let s = ctx.settings.borrow();
                    if s.calendar.feeds != feeds || s.calendar.days.clamp(1, 60) != days {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICS: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nSUMMARY:\u{201C}Kick-off\u{201D} with M\u{00FC}ller\r\nLOCATION:Caf\u{00E9}\r\nDTSTART:20261201T100000Z\r\nDTEND:20261201T110000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:2\r\nSUMMARY:Over\r\nDTSTART:20261130T080000Z\r\nDTEND:20261130T090000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:3\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261201\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn at(h: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(2026, 11, 30, h, 0, 0).unwrap().timestamp()
    }

    #[test]
    fn folds_text_and_drops_ended() {
        let ev = feed_events(ICS, "Work", "#ff0000", at(12), 7);
        let titles: Vec<&str> = ev.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"\u{201C}Kick-off\u{201D} with M\u{00FC}ller"));
        assert!(!titles.contains(&"Over"));
        let k = ev.iter().find(|e| e.title.starts_with('\u{201C}')).unwrap();
        assert_eq!((k.loc.as_str(), k.cal.as_str(), k.color.as_str()), ("Caf\u{00E9}", "Work", "#ff0000"));
    }

    #[test]
    fn message_sorted_capped_and_shaped() {
        let mk = |start: i64, all_day: bool| BoardEvent {
            title: "t".into(), loc: String::new(), cal: "c".into(), start, end: start + 60,
            all_day, color: DEFAULT_COLOR.into(),
        };
        let mut events: Vec<BoardEvent> = (0..20).rev().map(|i| mk(1000 + i, false)).collect();
        events.push(mk(1000, true));
        let m = events_message(events, 7);
        let list = m["events"].as_array().unwrap();
        assert_eq!(list.len(), MAX_EVENTS);
        assert_eq!(list[0]["all_day"], true);
        assert_eq!(list[1]["start"], 1000);
        assert_eq!(m["days"], 7);
        let keys: Vec<&String> = list[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["title", "loc", "cal", "start", "end", "all_day", "color"]);
    }

    #[test]
    fn names_colors_and_webcal() {
        let f = CalendarFeed { name: " ".into(), url: "x".into(), color: "red".into() };
        assert_eq!(feed_name(&f, 1), "Calendar 2");
        assert_eq!(color_of(&f), DEFAULT_COLOR);
        assert_eq!(fetch_url("webcal://outlook.office365.com/owa/calendar/x/calendar.ics"),
            "https://outlook.office365.com/owa/calendar/x/calendar.ics");
        assert_eq!(fetch_url(" https://a/b.ics "), "https://a/b.ics");
    }
}
