//! iCalendar (.ics) feeds for the board's Calendar page: the VEVENTs of one feed,
//! recurrences expanded, for a time window (calendar_feed.py _collect).
//!
//! Written for Outlook's published calendars too: TZIDs are Windows names ("W. Europe
//! Standard Time"), moved or edited occurrences come as extra VEVENTs with a
//! RECURRENCE-ID, and cancelled ones as EXDATE or STATUS:CANCELLED.

use std::collections::HashSet;
use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};

/// One occurrence, times as Unix seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub title: String,
    pub loc: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
}

/// At most this many occurrences of one recurring event in a window.
const MAX_OCCURRENCES: u16 = 500;

/// The occurrences that overlap `from..to` (Unix seconds), cancelled ones left out.
/// Unknown or broken parts are skipped; never fails.
pub fn events_between(text: &str, from: i64, to: i64) -> Vec<Event> {
    let vevents = components(text);
    // Occurrences replaced by a RECURRENCE-ID override (uid, original start).
    let mut replaced: HashSet<(String, i64)> = HashSet::new();
    for v in &vevents {
        if let (Some(rid), Some(uid)) = (v.get("RECURRENCE-ID"), v.get("UID")) {
            if let Some(t) = parse_time(rid) {
                replaced.insert((uid.value.clone(), t.epoch()));
            }
        }
    }
    let mut out = Vec::new();
    for v in &vevents {
        let cancelled = v.get("STATUS").is_some_and(|s| s.value.eq_ignore_ascii_case("CANCELLED"));
        let Some(start) = v.get("DTSTART").and_then(parse_time) else { continue };
        let length = length_of(v, &start);
        let title = v.get("SUMMARY").map(|p| unescape(&p.value)).unwrap_or_else(|| "(no title)".into());
        let loc = v.get("LOCATION").map(|p| unescape(&p.value)).unwrap_or_default();
        let mk = |s: i64| Event { title: title.clone(), loc: loc.clone(), start: s, end: s + length, all_day: start.all_day };
        let rrule = v.get("RRULE").filter(|_| v.get("RECURRENCE-ID").is_none());
        let starts = match rrule {
            Some(rule) => {
                let uid = v.get("UID").map(|p| p.value.clone()).unwrap_or_default();
                expand(&rule.value, &start, &exdates(v), from - length, to)
                    .into_iter()
                    .filter(|s| !replaced.contains(&(uid.clone(), *s)))
                    .collect()
            }
            None => vec![start.epoch()],
        };
        if cancelled {
            continue;
        }
        out.extend(starts.into_iter().map(mk).filter(|e| e.end > from && e.start < to));
    }
    out
}

// ---- content lines ---------------------------------------------------------------

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Default)]
struct VEvent {
    props: Vec<Prop>,
}

impl VEvent {
    fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
}

/// The VEVENTs of the feed (their own properties; VALARMs inside are skipped).
fn components(text: &str) -> Vec<VEvent> {
    // Unfold: a line starting with a space or tab continues the previous one.
    let text = text.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "");
    let mut out = Vec::new();
    let mut cur: Option<VEvent> = None;
    let mut nested = 0;
    for line in text.lines() {
        let Some(p) = parse_line(line) else { continue };
        match (p.name.as_str(), p.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => {
                cur = Some(VEvent::default());
                nested = 0;
            }
            ("END", "VEVENT") => out.extend(cur.take()),
            ("BEGIN", _) if cur.is_some() => nested += 1,
            ("END", _) if cur.is_some() => nested -= 1,
            _ => {
                if let Some(ev) = cur.as_mut().filter(|_| nested == 0) {
                    ev.props.push(p);
                }
            }
        }
    }
    out
}

/// `NAME;KEY=VAL;KEY="V:AL":value`.
fn parse_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?.0;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string()))
        .collect();
    Some(Prop { name, params, value: value.trim_end().to_string() })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

// ---- times -----------------------------------------------------------------------

/// A DTSTART-like value in its own zone.
#[derive(Debug, Clone, Copy)]
struct When {
    at: DateTime<Tz>,
    all_day: bool,
}

impl When {
    fn epoch(&self) -> i64 {
        self.at.timestamp()
    }
}

fn parse_time(p: &Prop) -> Option<When> {
    parse_value(&p.value, p.param("TZID"), p.param("VALUE") == Some("DATE"))
}

fn parse_value(value: &str, tzid: Option<&str>, date_only: bool) -> Option<When> {
    let v = value.trim();
    if date_only || v.len() == 8 {
        let d = NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d").ok()?;
        return Some(When { at: local_at(Tz::LOCAL, d.and_hms_opt(0, 0, 0)?)?, all_day: true });
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(When { at: Tz::UTC.from_utc_datetime(&n), all_day: false });
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz = tzid.map(zone).unwrap_or(Tz::LOCAL);
    Some(When { at: local_at(tz, n)?, all_day: false })
}

/// A wall-clock time in `tz` (the earlier one when the clock goes back; skipped hours move on).
fn local_at(tz: Tz, n: NaiveDateTime) -> Option<DateTime<Tz>> {
    tz.from_local_datetime(&n)
        .earliest()
        .or_else(|| tz.from_local_datetime(&(n + Duration::hours(1))).earliest())
}

/// IANA names directly; Windows names (Outlook) by table; anything else is the PC's zone,
/// which is right for most calendars on this PC.
fn zone(tzid: &str) -> Tz {
    let id = tzid.trim().trim_start_matches('/');
    if let Ok(tz) = chrono_tz::Tz::from_str(id) {
        return Tz::Tz(tz);
    }
    const WINDOWS: &[(&str, &str)] = &[
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("Central European Standard Time", "Europe/Warsaw"),
        ("Romance Standard Time", "Europe/Paris"),
        ("GMT Standard Time", "Europe/London"),
        ("Greenwich Standard Time", "Atlantic/Reykjavik"),
        ("GTB Standard Time", "Europe/Bucharest"),
        ("E. Europe Standard Time", "Europe/Chisinau"),
        ("FLE Standard Time", "Europe/Kiev"),
        ("Russian Standard Time", "Europe/Moscow"),
        ("Turkey Standard Time", "Europe/Istanbul"),
        ("UTC", "UTC"),
        ("Coordinated Universal Time", "UTC"),
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("India Standard Time", "Asia/Kolkata"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
    ];
    WINDOWS
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(id))
        .and_then(|(_, iana)| chrono_tz::Tz::from_str(iana).ok())
        .map(Tz::Tz)
        .unwrap_or(Tz::LOCAL)
}

/// Seconds from start to end: DTEND, else DURATION, else a day (all-day) or an hour.
fn length_of(v: &VEvent, start: &When) -> i64 {
    let fallback = if start.all_day { 86_400 } else { 3_600 };
    let len = if let Some(end) = v.get("DTEND").and_then(parse_time) {
        end.epoch() - start.epoch()
    } else if let Some(d) = v.get("DURATION") {
        parse_duration(&d.value).unwrap_or(fallback)
    } else {
        fallback
    };
    if len > 0 { len } else { fallback }
}

/// `P1W`, `PT1H30M`, `-P1D`, `P1DT12H` in seconds.
fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut total, mut num) = (0_i64, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            'T' => {}
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match c {
                    'W' => 604_800,
                    'D' => 86_400,
                    'H' => 3_600,
                    'M' => 60,
                    _ => 1,
                };
            }
            _ => return None,
        }
    }
    Some(sign * total)
}

fn exdates(v: &VEvent) -> Vec<DateTime<Tz>> {
    v.props
        .iter()
        .filter(|p| p.name == "EXDATE")
        .flat_map(|p| {
            let tzid = p.param("TZID");
            let date_only = p.param("VALUE") == Some("DATE");
            p.value.split(',').filter_map(move |x| parse_value(x, tzid, date_only)).map(|w| w.at)
        })
        .collect()
}

// ---- recurrence ------------------------------------------------------------------

/// Starts (Unix seconds) of `rule` from `start`, in `from..to`, minus `exdates`.
/// UNTIL is applied here (the rrule crate wants it in DTSTART's zone or UTC, which
/// Outlook and all-day events don't always give). A rule it can't read gives the
/// first occurrence only.
fn expand(rule: &str, start: &When, exdates: &[DateTime<Tz>], from: i64, to: i64) -> Vec<i64> {
    let mut until = None;
    let rest: Vec<&str> = rule
        .split(';')
        .filter(|part| match part.split_once('=') {
            Some((k, v)) if k.eq_ignore_ascii_case("UNTIL") => {
                until = parse_value(v, None, false).map(|w| {
                    // A date-only UNTIL includes that whole day.
                    if w.all_day { w.epoch() + 86_399 } else { w.epoch() }
                });
                false
            }
            _ => !part.is_empty(),
        })
        .collect();
    let parsed = RRule::<Unvalidated>::from_str(&rest.join(";")).and_then(|r| r.validate(start.at));
    let Ok(rule) = parsed else {
        log::debug!("calendar: unreadable RRULE, showing the first occurrence only");
        return vec![start.epoch()];
    };
    let mut set = RRuleSet::new(start.at)
        .rrule(rule)
        .after(Tz::UTC.from_utc_datetime(&utc_naive(from)))
        .before(Tz::UTC.from_utc_datetime(&utc_naive(to)));
    for x in exdates {
        set = set.exdate(*x);
    }
    set.all(MAX_OCCURRENCES)
        .dates
        .into_iter()
        .map(|d| d.timestamp())
        .filter(|&s| until.is_none_or(|u| s <= u))
        .collect()
}

fn utc_naive(secs: i64) -> NaiveDateTime {
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_default().naive_utc()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap().timestamp()
    }

    fn wrap(body: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{body}END:VCALENDAR\r\n")
    }

    /// Outlook-style: Windows TZID, weekly series with an EXDATE, one moved occurrence
    /// (RECURRENCE-ID), a VTIMEZONE and a VALARM, folded lines.
    const OUTLOOK: &str = "BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nSUMMARY:Team \r\n meeting\r\nLOCATION:Room 4\\, 2nd floor\r\n\
DTSTART;TZID=W. Europe Standard Time:20261102T100000\r\nDTEND;TZID=W. Europe Standard Time:20261102T110000\r\n\
RRULE:FREQ=WEEKLY;UNTIL=20261130T090000Z;INTERVAL=1;BYDAY=MO;WKST=SU\r\n\
EXDATE;TZID=W. Europe Standard Time:20261109T100000\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nRECURRENCE-ID;TZID=W. Europe Standard Time:20261116T100000\r\nSUMMARY:Team meeting (moved)\r\n\
DTSTART;TZID=W. Europe Standard Time:20261116T140000\r\nDTEND;TZID=W. Europe Standard Time:20261116T150000\r\nEND:VEVENT\r\n";

    #[test]
    fn outlook_series_with_exdate_override_and_until() {
        let ev = events_between(&wrap(OUTLOOK), utc(2026, 11, 1, 0, 0), utc(2026, 12, 31, 0, 0));
        let mut got: Vec<(String, i64)> = ev.iter().map(|e| (e.title.clone(), e.start)).collect();
        got.sort_by_key(|g| g.1);
        assert_eq!(
            got,
            vec![
                ("Team meeting".into(), utc(2026, 11, 2, 9, 0)),
                // 9 Nov excluded, 16 Nov moved to 14:00 (13:00 UTC).
                ("Team meeting (moved)".into(), utc(2026, 11, 16, 13, 0)),
                ("Team meeting".into(), utc(2026, 11, 23, 9, 0)),
                ("Team meeting".into(), utc(2026, 11, 30, 9, 0)),
            ]
        );
        assert_eq!(ev[0].loc, "Room 4, 2nd floor");
        assert!(ev.iter().all(|e| e.end - e.start == 3600 && !e.all_day));
    }

    #[test]
    fn window_limits_series() {
        let ev = events_between(&wrap(OUTLOOK), utc(2026, 11, 20, 0, 0), utc(2026, 11, 25, 0, 0));
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].start, utc(2026, 11, 23, 9, 0));
    }

    #[test]
    fn utc_iana_and_floating_times() {
        let body = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:UTC\r\nDTSTART:20261201T080000Z\r\nDTEND:20261201T083000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:NY\r\nDTSTART;TZID=America/New_York:20261201T090000\r\nDURATION:PT2H\r\nEND:VEVENT\r\n";
        let ev = events_between(&wrap(body), utc(2026, 11, 30, 0, 0), utc(2026, 12, 3, 0, 0));
        assert_eq!(ev[0].start, utc(2026, 12, 1, 8, 0));
        assert_eq!(ev[0].end, utc(2026, 12, 1, 8, 30));
        assert_eq!(ev[1].start, utc(2026, 12, 1, 14, 0));
        assert_eq!(ev[1].end - ev[1].start, 7200);
    }

    #[test]
    fn all_day_and_default_lengths() {
        let body = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261208\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Trip\r\nDTSTART;VALUE=DATE:20261210\r\nDTEND;VALUE=DATE:20261213\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Call\r\nDTSTART:20261209T100000Z\r\nEND:VEVENT\r\n";
        let ev = events_between(&wrap(body), utc(2026, 12, 1, 0, 0), utc(2026, 12, 31, 0, 0));
        assert!(ev[0].all_day && ev[0].end - ev[0].start == 86_400);
        assert!(ev[1].all_day && ev[1].end - ev[1].start == 3 * 86_400);
        assert!(!ev[2].all_day && ev[2].end - ev[2].start == 3_600);
    }

    #[test]
    fn cancelled_events_are_left_out() {
        let body = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Gone\r\nSTATUS:CANCELLED\r\nDTSTART:20261201T080000Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Daily\r\nDTSTART:20261201T080000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID:20261202T080000Z\r\nSTATUS:CANCELLED\r\nSUMMARY:Daily\r\nDTSTART:20261202T080000Z\r\nEND:VEVENT\r\n";
        let ev = events_between(&wrap(body), utc(2026, 11, 30, 0, 0), utc(2026, 12, 31, 0, 0));
        let starts: Vec<i64> = ev.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 12, 1, 8, 0), utc(2026, 12, 3, 8, 0)]);
    }

    #[test]
    fn old_series_still_reaches_today_and_bad_rule_keeps_first() {
        let body = "BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Standup\r\nDTSTART:20150105T080000Z\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Odd\r\nDTSTART:20261201T080000Z\r\nRRULE:FREQ=SOMETIMES\r\nEND:VEVENT\r\n";
        let ev = events_between(&wrap(body), utc(2026, 11, 30, 0, 0), utc(2026, 12, 8, 0, 0));
        let starts: Vec<i64> = ev.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 11, 30, 8, 0), utc(2026, 12, 7, 8, 0), utc(2026, 12, 1, 8, 0)]);
    }

    #[test]
    fn durations_and_zones() {
        assert_eq!(parse_duration("PT1H30M"), Some(5400));
        assert_eq!(parse_duration("P1W"), Some(604_800));
        assert_eq!(parse_duration("P1DT12H"), Some(129_600));
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        assert_eq!(parse_duration("1H"), None);
        assert_eq!(zone("W. Europe Standard Time").name(), "Europe/Berlin");
        assert_eq!(zone("Europe/Vienna").name(), "Europe/Vienna");
        assert!(zone("Some Custom Zone").is_local());
    }
}
