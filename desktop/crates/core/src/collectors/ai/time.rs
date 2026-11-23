//! Timestamps for the AI usage scan. Stamps are ISO 8601 as Claude Code and Codex write
//! them. Days are local calendar days, like the Python (date.today()).

use std::time::{SystemTime, UNIX_EPOCH};

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;

/// Epoch milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Epoch milliseconds at the start of the hour containing `ms`.
pub fn floor_hour(ms: i64) -> i64 {
    ms.div_euclid(HOUR_MS) * HOUR_MS
}

/// Local day number of `ms`: days since 1970-01-01 on this PC's clock (so "today"
/// rolls over at local midnight, with daylight saving applied per timestamp).
pub fn day_of(ms: i64) -> i64 {
    day_with_offset(ms, local_offset_ms(ms))
}

/// Day number of `ms` for a zone `offset_ms` ahead of UTC.
pub fn day_with_offset(ms: i64, offset_ms: i64) -> i64 {
    (ms + offset_ms).div_euclid(DAY_MS)
}

/// The local zone's offset from UTC at `ms`, in milliseconds.
fn local_offset_ms(ms: i64) -> i64 {
    use chrono::{Local, Offset, TimeZone};
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            i64::from(t.offset().fix().local_minus_utc()) * 1000
        }
        chrono::LocalResult::None => 0,
    }
}

/// Parses "2026-11-23T10:30:00.123Z", "...+01:00" or "...+0100" to epoch milliseconds.
/// A stamp without an offset is read as UTC.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (y, m, d) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    if !matches!(b.get(10), Some(b'T' | b' ')) {
        return None;
    }
    let (hh, mm) = (digits(11, 2)?, digits(14, 2)?);
    if b.get(13) != Some(&b':') || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    if hh > 23 || mm > 59 {
        return None;
    }

    let mut i = 16;
    let mut sec = 0;
    if b.get(i) == Some(&b':') {
        sec = digits(i + 1, 2)?;
        if sec > 60 {
            return None;
        }
        i += 3;
    }
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        let mut frac: String = s[start..i].chars().take(3).collect();
        while frac.len() < 3 {
            frac.push('0');
        }
        ms = frac.parse::<i64>().ok()?;
    }
    let mut offset_min = 0;
    if let Some(sign_byte @ (b'+' | b'-')) = b.get(i).copied() {
        let sign = if sign_byte == b'+' { 1 } else { -1 };
        let oh = digits(i + 1, 2)?;
        let mut j = i + 3;
        if b.get(j) == Some(&b':') {
            j += 1;
        }
        let om = digits(j, 2)?;
        offset_min = sign * (oh * 60 + om);
    }

    let days = days_from_civil(y, m, d);
    let secs = days * 86_400 + hh * 3_600 + mm * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_stamps() {
        // 2026-01-01T00:00:00Z is 1767225600; 2026-11-23T12:00:00Z is 326 days later plus 12 h.
        assert_eq!(
            parse_iso_ms("2026-01-01T00:00:00Z"),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            parse_iso_ms("2026-11-23T12:00:00Z"),
            Some(1_795_435_200_000)
        );
        assert_eq!(
            parse_iso_ms("2026-11-23T12:00:00.250Z"),
            Some(1_795_435_200_250)
        );
        assert_eq!(
            parse_iso_ms("2026-11-23T13:00:00+01:00"),
            Some(1_795_435_200_000)
        );
        assert_eq!(
            parse_iso_ms("2026-11-23T12:00:00+0000"),
            Some(1_795_435_200_000)
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_iso_ms(""), None);
        assert_eq!(parse_iso_ms("yesterday"), None);
        assert_eq!(parse_iso_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso_ms("2026-11-23T25:00:00Z"), None);
    }

    #[test]
    fn hour_and_day_buckets() {
        let t = 1_795_435_200_000 + 90 * 60_000; // 13:30 UTC
        assert_eq!(floor_hour(t), 1_795_435_200_000 + HOUR_MS);
        assert_eq!(day_with_offset(t, 0), 1_795_435_200_000 / DAY_MS);
        // 23:30 UTC is already the next day in Vienna (UTC+1).
        let late = 1_795_435_200_000 + 11 * HOUR_MS + 30 * 60_000;
        assert_eq!(day_with_offset(late, 0), 1_795_435_200_000 / DAY_MS);
        assert_eq!(
            day_with_offset(late, HOUR_MS),
            1_795_435_200_000 / DAY_MS + 1
        );
    }
}
