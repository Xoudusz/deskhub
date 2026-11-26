//! GitHub contribution chart (the green squares of your profile) for the GitHub page and
//! Home widget, as the "github" message (port of companion/deskhub/github.py).
//!
//! Needs a fine-grained personal access token (no extra permissions), kept in the
//! credential store (secrets::GITHUB_TOKEN). Requests go only to api.github.com.

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::media::ascii_fold;
use super::Ctx;
use crate::secrets;

const API: &str = "https://api.github.com/graphql";
const POLL: Duration = Duration::from_secs(15 * 60);
const BACKOFF: Duration = Duration::from_secs(10 * 60);
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_REPOS: usize = 4;
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const QUERY: &str = "query { viewer { id login contributionsCollection {
  totalCommitContributions totalPullRequestContributions totalPullRequestReviewContributions
  totalIssueContributions restrictedContributionsCount
  commitContributionsByRepository(maxRepositories: 5) { repository { name } contributions { totalCount } }
  contributionCalendar {
  totalContributions weeks { contributionDays { date weekday contributionCount contributionLevel } } } } } }";

/// GitHub leaves private repositories out of commitContributionsByRepository for a
/// fine-grained token, so count your commits on each private repository's default branch.
const PRIVATE_QUERY: &str = "query($id: ID!, $since: GitTimestamp!) { viewer { repositories(first: 50, privacy: PRIVATE,
  ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], orderBy: {field: PUSHED_AT, direction: DESC}) {
  nodes { name pushedAt defaultBranchRef { target { ... on Commit {
    history(since: $since, author: {id: $id}) { totalCount } } } } } } } }";

/// For Settings: whether the last fetch worked, and as whom.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GithubStatus {
    pub ok: bool,
    pub error: String,
    pub login: String,
    pub total: i64,
}

static STATUS: Mutex<Option<GithubStatus>> = Mutex::new(None);
/// Poke the collector (a new token was saved).
pub static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

/// None before the first try (or without a token).
pub fn status() -> Option<GithubStatus> {
    STATUS.lock().ok().and_then(|s| s.clone())
}

fn set_status(s: Option<GithubStatus>) {
    if let Ok(mut cur) = STATUS.lock() {
        *cur = s;
    }
}

/// One day of the contribution calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: String,
    pub weekday: i64,
    pub count: i64,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contributions {
    pub login: String,
    pub total: i64,
    pub days: Vec<Day>,
    pub kinds: Value,
    pub repos: Vec<(String, i64)>,
}

fn level(s: &str) -> u8 {
    match s {
        "FIRST_QUARTILE" => 1,
        "SECOND_QUARTILE" => 2,
        "THIRD_QUARTILE" => 3,
        "FOURTH_QUARTILE" => 4,
        _ => 0,
    }
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

/// The viewer's part of the first query's `data`.
pub fn parse(data: &Value) -> Result<(String, Contributions)> {
    let v = &data["viewer"];
    let login = v["login"].as_str().context("no login in the reply")?.to_string();
    let cc = &v["contributionsCollection"];
    let cal = &cc["contributionCalendar"];
    let days = cal["weeks"]
        .as_array()
        .context("no contribution calendar")?
        .iter()
        .flat_map(|w| w["contributionDays"].as_array().cloned().unwrap_or_default())
        .map(|d| Day {
            date: d["date"].as_str().unwrap_or("").to_string(),
            weekday: int(&d["weekday"]),
            count: int(&d["contributionCount"]),
            level: level(d["contributionLevel"].as_str().unwrap_or("")),
        })
        .collect();
    let kinds = json!({
        "commits": int(&cc["totalCommitContributions"]),
        "prs": int(&cc["totalPullRequestContributions"]),
        "reviews": int(&cc["totalPullRequestReviewContributions"]),
        "issues": int(&cc["totalIssueContributions"]),
        "private": int(&cc["restrictedContributionsCount"]),
    });
    let repos = cc["commitContributionsByRepository"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| (ascii_fold(r["repository"]["name"].as_str().unwrap_or(""), 32), int(&r["contributions"]["totalCount"])))
                .collect()
        })
        .unwrap_or_default();
    let id = v["id"].as_str().unwrap_or("").to_string();
    Ok((id, Contributions { login: ascii_fold(&login, 39), total: int(&cal["totalContributions"]), days, kinds, repos }))
}

/// Your commits in the last year per private repository (second query's `data`).
pub fn parse_private(data: &Value) -> Vec<(String, i64)> {
    data["viewer"]["repositories"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|n| (ascii_fold(n["name"].as_str().unwrap_or(""), 32), int(&n["defaultBranchRef"]["target"]["history"]["totalCount"])))
                .filter(|(_, c)| *c > 0)
                .collect()
        })
        .unwrap_or_default()
}

/// Days in a row with contributions up to today, or up to yesterday while today is empty.
pub fn streak(counts: &[i64]) -> i64 {
    let n = counts.len() as i64;
    let end = if counts.last().is_some_and(|&c| c > 0) { n - 1 } else { n - 2 };
    let mut run = 0;
    while end - run >= 0 && counts[(end - run) as usize] > 0 {
        run += 1;
    }
    run
}

pub fn longest_streak(counts: &[i64]) -> i64 {
    let (mut best, mut run) = (0, 0);
    for &c in counts {
        run = if c > 0 { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// The board message: one digit per day (oldest first, today last), the weekday of the
/// first day (0 = Sunday), month labels by week column, and totals (github.py message).
pub fn message(c: &Contributions) -> Value {
    let days = &c.days;
    let counts: Vec<i64> = days.iter().map(|d| d.count).collect();
    let n = counts.len();
    let levels: String = days.iter().map(|d| char::from(b'0' + d.level.min(4))).collect();
    let start_wday = days.first().map(|d| d.weekday).unwrap_or(0);
    let mut months = Vec::new();
    for (i, d) in days.iter().enumerate() {
        let Ok(date) = NaiveDate::parse_from_str(&d.date, "%Y-%m-%d") else { continue };
        let pos = i as i64 + start_wday;
        // A label goes on the first full week of each month, like GitHub's chart.
        if date.day() <= 7 && pos % 7 == 0 {
            months.push(json!([pos / 7, MONTHS[date.month0() as usize]]));
        }
    }
    let today_wday = if n > 0 { ((n as i64 - 1 + start_wday) % 7) as usize } else { 0 };
    let this_from = n.saturating_sub(1 + today_wday);
    let last_from = n.saturating_sub(8 + today_wday);
    let week_days: Vec<i64> = if n > 0 { counts[this_from..].to_vec() } else { vec![] };
    let last_week_days: Vec<i64> = if n > 0 { counts[last_from..this_from].to_vec() } else { vec![] };
    let best = (0..n).max_by_key(|&i| (counts[i], std::cmp::Reverse(i)));
    let best_date = match best {
        Some(i) if counts[i] > 0 => NaiveDate::parse_from_str(&days[i].date, "%Y-%m-%d")
            .map(|b| format!("{} {}", MONTHS[b.month0() as usize], b.day()))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let mut repos = c.repos.clone();
    repos.sort_by_key(|r| -r.1);
    repos.truncate(MAX_REPOS);
    json!({
        "type": "github", "login": c.login, "total": c.total, "levels": levels,
        "start_wday": start_wday, "months": months, "today": counts.last().copied().unwrap_or(0),
        "week": week_days.iter().sum::<i64>(), "last_week": last_week_days.iter().sum::<i64>(),
        "streak": streak(&counts), "longest": longest_streak(&counts),
        "best": best.map(|i| counts[i]).unwrap_or(0), "best_date": best_date,
        "week_days": week_days, "last_week_days": last_week_days, "kinds": c.kinds,
        "repos": repos.iter().map(|(name, count)| json!({"name": name, "count": count})).collect::<Vec<_>>(),
        "accent": false,
    })
}

async fn post(client: &reqwest::Client, token: &str, query: &str, vars: Value) -> Result<Value> {
    let resp = client
        .post(API)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(json!({"query": query, "variables": vars}).to_string())
        .send()
        .await
        .map_err(crate::plex::send_err)?;
    if resp.status().as_u16() == 401 {
        bail!("GitHub refused the token");
    }
    let body = crate::plex::read_limited(resp, MAX_BODY).await?;
    let v: Value = serde_json::from_slice(&body).context("GitHub's reply isn't JSON")?;
    if let Some(e) = v["errors"].as_array().and_then(|a| a.first()) {
        bail!("GitHub: {}", e["message"].as_str().unwrap_or("query failed"));
    }
    Ok(v["data"].clone())
}

pub async fn fetch(client: &reqwest::Client, token: &str) -> Result<Contributions> {
    let (id, mut c) = parse(&post(client, token, QUERY, json!({})).await?)?;
    if int(&c.kinds["private"]) > 0 && !id.is_empty() {
        let since = (chrono::Utc::now() - chrono::Duration::days(365)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        // A token that can't read them still shows the public ones.
        match post(client, token, PRIVATE_QUERY, json!({"id": id, "since": since})).await {
            Ok(data) => c.repos.extend(parse_private(&data)),
            Err(e) => log::debug!("github private repos: {e:#}"),
        }
    }
    Ok(c)
}

pub fn token() -> Option<String> {
    secrets::system_store_for(secrets::GITHUB_TOKEN).get().ok().flatten().filter(|t| !t.trim().is_empty())
}

pub async fn run(ctx: Ctx) {
    let client = match reqwest::Client::builder().timeout(TIMEOUT).user_agent("deskhub").build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("github: {e}");
            return;
        }
    };
    let mut last: Option<Value> = None;
    loop {
        let wait = match token() {
            None => {
                set_status(None);
                None
            }
            Some(t) => match fetch(&client, t.trim()).await {
                Ok(c) => {
                    let msg = message(&c);
                    set_status(Some(GithubStatus { ok: true, error: String::new(), login: c.login.clone(), total: c.total }));
                    if last.as_ref() != Some(&msg) {
                        log::info!("GitHub: {} contributions in the last year", c.total);
                        ctx.out.publish("github", msg.clone());
                        last = Some(msg);
                    }
                    Some(POLL)
                }
                Err(e) => {
                    log::warn!("GitHub contributions failed: {e:#}; trying again in 10 minutes");
                    set_status(Some(GithubStatus { error: format!("{e:#}"), ..Default::default() }));
                    Some(BACKOFF)
                }
            },
        };
        match wait {
            Some(d) => {
                tokio::select! {
                    _ = tokio::time::sleep(d) => {}
                    _ = WAKE.notified() => {}
                }
            }
            None => WAKE.notified().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 days, first a Sunday (2026-11-01), today = Tuesday 2026-11-10.
    fn sample() -> Value {
        let counts = [0, 2, 0, 1, 3, 0, 0, 1, 4, 2];
        let days: Vec<Value> = counts
            .iter()
            .enumerate()
            .map(|(i, &c)| json!({"date": format!("2026-11-{:02}", i + 1), "weekday": i % 7, "contributionCount": c,
                "contributionLevel": if c == 0 { "NONE" } else if c > 3 { "FOURTH_QUARTILE" } else { "FIRST_QUARTILE" }}))
            .collect();
        json!({"viewer": {"id": "U1", "login": "octo", "contributionsCollection": {
            "totalCommitContributions": 9, "totalPullRequestContributions": 2, "totalPullRequestReviewContributions": 1,
            "totalIssueContributions": 0, "restrictedContributionsCount": 3,
            "commitContributionsByRepository": [{"repository": {"name": "a"}, "contributions": {"totalCount": 2}},
                {"repository": {"name": "b"}, "contributions": {"totalCount": 7}}],
            "contributionCalendar": {"totalContributions": 13, "weeks": [{"contributionDays": days[..7]}, {"contributionDays": days[7..]}]}}}})
    }

    #[test]
    fn message_matches_github_py() {
        let (id, mut c) = parse(&sample()).unwrap();
        assert_eq!(id, "U1");
        c.repos.extend(parse_private(&json!({"viewer": {"repositories": {"nodes": [
            {"name": "secret", "defaultBranchRef": {"target": {"history": {"totalCount": 5}}}},
            {"name": "idle", "defaultBranchRef": {"target": {"history": {"totalCount": 0}}}},
            {"name": "empty", "defaultBranchRef": null}]}}})));
        let m = message(&c);
        assert_eq!(m["levels"], "0101100141");
        assert_eq!(m["start_wday"], 0);
        assert_eq!(m["months"], json!([[0, "Nov"]]));
        assert_eq!(m["today"], 2);
        assert_eq!(m["week"], 7); // Sun 8 Nov..Tue 10 Nov: 1 + 4 + 2
        assert_eq!(m["last_week"], 6);
        assert_eq!(m["week_days"], json!([1, 4, 2]));
        assert_eq!(m["last_week_days"], json!([0, 2, 0, 1, 3, 0, 0]));
        assert_eq!(m["streak"], 3);
        assert_eq!(m["longest"], 3);
        assert_eq!((m["best"].clone(), m["best_date"].clone()), (json!(4), json!("Nov 9")));
        assert_eq!(m["kinds"]["private"], 3);
        assert_eq!(m["repos"], json!([{"name": "b", "count": 7}, {"name": "secret", "count": 5}, {"name": "a", "count": 2}]));
        assert_eq!(m["login"], "octo");
    }

    #[test]
    fn streaks() {
        assert_eq!(streak(&[1, 1, 0]), 2); // empty today keeps yesterday's run
        assert_eq!(streak(&[1, 0, 0]), 0);
        assert_eq!(streak(&[]), 0);
        assert_eq!(streak(&[3]), 1);
        assert_eq!(longest_streak(&[1, 1, 0, 1, 1, 1, 0]), 3);
        assert_eq!(longest_streak(&[]), 0);
    }

    #[test]
    fn empty_calendar_is_safe() {
        let c = Contributions { login: "x".into(), total: 0, days: vec![], kinds: json!({}), repos: vec![] };
        let m = message(&c);
        assert_eq!(m["levels"], "");
        assert_eq!(m["week_days"], json!([]));
        assert_eq!(m["best_date"], "");
    }
}
