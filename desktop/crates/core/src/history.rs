//! Chart history: one value per minute for CPU, GPU and ping, kept 7 days in
//! ~/.deskhub/chart_history.json, so the board's Stats and Internet charts survive a restart of
//! the app (and the board: every message carries the whole chart, see session.rs replay).
//!
//! A minute stores the average of its samples (rounded), NONE when nothing was recorded (app
//! off, no GPU) and LOST when every sample was a lost probe. The charts get at most POINTS
//! points: 1 h = 60 x 1 min, 24 h = 144 x 10 min, 7 days = 144 x 70 min.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "chart_history.json";
/// Minutes kept (7 days).
pub const KEEP: usize = 7 * 24 * 60;
/// Most points a chart gets.
pub const POINTS: usize = 144;
/// Chart lengths offered in Settings (hours).
pub const HOURS: [u32; 3] = [1, 24, 168];
pub const DEFAULT_HOURS: u32 = 24;
/// No sample in that minute (or chart point).
pub const NONE: i32 = -1;
/// Every sample in that minute was lost (a ping without an answer).
const LOST: i32 = -2;
/// Written at most this often (seconds); a crash loses at most this much.
const SAVE_EVERY: i64 = 300;

/// The current, still open minute.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Open {
    minute: i64,
    sum: f64,
    n: u32,
    lost: u32,
}

impl Open {
    fn value(&self) -> i32 {
        if self.n > 0 {
            (self.sum / f64::from(self.n)).round() as i32
        } else if self.lost > 0 {
            LOST
        } else {
            NONE
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Series {
    /// The minute (unix seconds / 60) of `vals[0]`.
    start: i64,
    /// Closed minutes, oldest first.
    vals: VecDeque<i32>,
    open: Option<Open>,
}

impl Series {
    /// Close the open minute and fill the minutes up to `minute` with NONE.
    fn roll(&mut self, minute: i64) {
        let Some(o) = self.open else { return };
        if o.minute >= minute {
            return;
        }
        self.open = None;
        self.push(o.minute, o.value());
    }

    fn push(&mut self, minute: i64, v: i32) {
        let end = self.start + self.vals.len() as i64;
        if self.vals.is_empty() || minute - end > KEEP as i64 || minute < self.start {
            self.start = minute;
            self.vals.clear();
        } else {
            for _ in end..minute {
                self.vals.push_back(NONE);
            }
        }
        if minute >= self.start + self.vals.len() as i64 {
            self.vals.push_back(v);
        }
        while self.vals.len() > KEEP {
            self.vals.pop_front();
            self.start += 1;
        }
    }

    fn add(&mut self, minute: i64, value: Option<f64>) {
        self.roll(minute);
        let o = self.open.get_or_insert(Open { minute, sum: 0.0, n: 0, lost: 0 });
        if o.minute != minute {
            return; // an older minute than the open one (clock went back): ignore
        }
        match value {
            Some(v) if v.is_finite() => {
                o.sum += v;
                o.n += 1;
            }
            _ => o.lost += 1,
        }
    }

    /// The value of one minute (closed, open or NONE).
    fn at(&self, minute: i64) -> i32 {
        if let Some(o) = self.open.filter(|o| o.minute == minute) {
            return o.value();
        }
        let i = minute - self.start;
        if i < 0 {
            return NONE;
        }
        self.vals.get(i as usize).copied().unwrap_or(NONE)
    }
}

/// All series, keyed by name ("cpu", "gpu", "ping").
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    series: BTreeMap<String, Series>,
    #[serde(skip)]
    saved_at: i64,
}

/// The settings value, limited to the offered lengths.
pub fn valid_hours(hours: u32) -> u32 {
    if HOURS.contains(&hours) {
        hours
    } else {
        DEFAULT_HOURS
    }
}

/// The caption for a chart length: "LAST HOUR", "LAST 24 HOURS", "LAST 7 DAYS".
pub fn label(hours: u32) -> &'static str {
    match valid_hours(hours) {
        1 => "LAST HOUR",
        168 => "LAST 7 DAYS",
        _ => "LAST 24 HOURS",
    }
}

impl History {
    /// One sample at `now` (unix seconds). None counts as lost (a ping without an answer).
    pub fn add(&mut self, name: &str, value: Option<f64>, now: i64) {
        self.series
            .entry(name.to_string())
            .or_default()
            .add(now.div_euclid(60), value);
    }

    /// The chart for the last `hours`, oldest first: the average per point, NONE where nothing
    /// was recorded (or every sample was lost). The last point holds the current minute.
    pub fn points(&self, name: &str, hours: u32, now: i64) -> Vec<i32> {
        let span = i64::from(valid_hours(hours)) * 60;
        let count = (span as usize).min(POINTS) as i64;
        let step = span / count;
        let end = now.div_euclid(60) + 1;
        let first = end - count * step;
        let Some(s) = self.series.get(name) else {
            return vec![NONE; count as usize];
        };
        (0..count)
            .map(|i| {
                let (mut sum, mut n) = (0i64, 0i64);
                for m in first + i * step..first + (i + 1) * step {
                    let v = s.at(m);
                    if v >= 0 {
                        sum += i64::from(v);
                        n += 1;
                    }
                }
                if n > 0 {
                    ((sum as f64) / (n as f64)).round() as i32
                } else {
                    NONE
                }
            })
            .collect()
    }

    /// Whether a save is due (at most every SAVE_EVERY seconds).
    pub fn save_due(&self, now: i64) -> bool {
        now - self.saved_at >= SAVE_EVERY
    }

    pub fn load(path: &Path, now: i64) -> Self {
        let mut h: History = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let minute = now.div_euclid(60);
        for s in h.series.values_mut() {
            s.roll(minute);
        }
        h.saved_at = now;
        h
    }

    /// Writes the file (temp file + rename, so a crash never leaves half a file).
    pub fn save(&mut self, path: &Path, now: i64) -> std::io::Result<()> {
        self.saved_at = now;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

/// ~/.deskhub/chart_history.json
pub fn path() -> Option<PathBuf> {
    Some(crate::collectors::alerts::deskhub_dir()?.join(FILE))
}

static STORE: Mutex<Option<History>> = Mutex::new(None);

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The shared history (loaded on first use). Collectors record into it and read charts.
fn with<R>(f: impl FnOnce(&mut History, i64) -> R) -> R {
    let now = now_secs();
    let mut g = STORE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let h = g.get_or_insert_with(|| path().map_or_else(History::default, |p| History::load(&p, now)));
    f(h, now)
}

/// Record one sample now; saves the file when due.
pub fn record(name: &str, value: Option<f64>) {
    with(|h, now| {
        h.add(name, value, now);
        if h.save_due(now) {
            if let Some(p) = path() {
                if let Err(e) = h.save(&p, now) {
                    log::debug!("chart history save failed: {e}");
                }
            }
        }
    });
}

/// The chart for `name` over `hours` (see History::points).
pub fn chart(name: &str, hours: u32) -> Vec<i32> {
    with(|h, now| h.points(name, hours, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000; // a 10-minute boundary (divisible by 600)

    #[test]
    fn minute_averages_and_gaps() {
        let mut h = History::default();
        h.add("cpu", Some(10.0), T0);
        h.add("cpu", Some(30.0), T0 + 30);
        h.add("cpu", Some(50.0), T0 + 180); // 3 minutes later: 2 minutes without data
        let p = h.points("cpu", 1, T0 + 180);
        assert_eq!(p.len(), 60);
        assert_eq!(&p[56..], [20, NONE, NONE, 50]);
        assert!(p[..56].iter().all(|v| *v == NONE));
    }

    #[test]
    fn lost_pings_are_gaps_and_do_not_pull_the_average() {
        let mut h = History::default();
        h.add("ping", None, T0);
        h.add("ping", None, T0 + 10);
        h.add("ping", Some(20.0), T0 + 60);
        h.add("ping", None, T0 + 70);
        let p = h.points("ping", 1, T0 + 70);
        assert_eq!(&p[58..], [NONE, 20]);
    }

    #[test]
    fn window_sizes() {
        let mut h = History::default();
        for m in 0..30 {
            h.add("cpu", Some(f64::from(m)), T0 + i64::from(m) * 60);
        }
        let now = T0 + 29 * 60;
        assert_eq!(h.points("cpu", 1, now).len(), 60);
        let day = h.points("cpu", 24, now);
        assert_eq!(day.len(), 144);
        // 10-minute points: minutes 0-9, 10-19, 20-29.
        assert_eq!(&day[141..], [5, 15, 25]);
        assert_eq!(h.points("cpu", 168, now).len(), 144);
        assert_eq!(h.points("cpu", 5, now).len(), 144); // invalid -> 24 h
        assert_eq!(h.points("nothing", 24, now), vec![NONE; 144]);
    }

    #[test]
    fn keeps_seven_days() {
        let mut h = History::default();
        h.add("cpu", Some(1.0), T0);
        h.add("cpu", Some(2.0), T0 + 60);
        let later = T0 + (KEEP as i64 + 5) * 60;
        h.add("cpu", Some(3.0), later);
        h.add("cpu", Some(4.0), later + 60); // closes `later`'s minute
        let s = &h.series["cpu"];
        assert!(s.vals.len() <= KEEP);
        assert_eq!(s.at(T0 / 60), NONE);
        assert_eq!(s.at(later / 60), 3);
    }

    #[test]
    fn saves_and_loads_with_a_gap_for_the_time_off() {
        let dir = std::env::temp_dir().join(format!("deskhub-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join(FILE);
        let mut h = History::default();
        h.add("cpu", Some(40.0), T0);
        h.add("gpu", Some(70.0), T0);
        h.save(&p, T0).unwrap();
        // Back 5 minutes later: the open minute was closed, the time off is a gap.
        let mut back = History::load(&p, T0 + 300);
        back.add("cpu", Some(60.0), T0 + 300);
        let c = back.points("cpu", 1, T0 + 300);
        assert_eq!(&c[54..], [40, NONE, NONE, NONE, NONE, 60]);
        assert_eq!(*back.points("gpu", 1, T0 + 300).last().unwrap(), NONE);
        assert!(!back.save_due(T0 + 300));
        assert!(back.save_due(T0 + 300 + SAVE_EVERY));
        // A broken file starts empty.
        std::fs::write(&p, b"{not json").unwrap();
        assert!(History::load(&p, T0).series.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels() {
        assert_eq!(label(1), "LAST HOUR");
        assert_eq!(label(24), "LAST 24 HOURS");
        assert_eq!(label(168), "LAST 7 DAYS");
        assert_eq!(label(0), "LAST 24 HOURS");
    }
}
