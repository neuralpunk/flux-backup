//! Retention: which snapshots to keep, independent of storage engine.
//!
//! A snapshot is kept if **any** rule keeps it:
//!
//! * pinned snapshots, and named ones when `keep_named` is set
//! * the newest snapshot, always (a prune must never leave you with nothing recent)
//! * the `keep_last` newest
//! * every snapshot younger than `keep_within`
//! * bucket rules: for `keep_hourly`/`daily`/`weekly`/`monthly`/`yearly` = N, the newest
//!   snapshot of each of the N most recent hours/days/ISO weeks/months/years that contain a
//!   snapshot, preferring one that completed without errors
//!
//! Then `max_age` removes anything older than it that isn't pinned, named-and-protected or
//! the newest. Free-space and free-inode floors are enforced separately by the engine,
//! using [`space_candidates`] (oldest prunable first).

use std::collections::HashSet;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::profile::Retention;

/// What retention needs to know about a snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub id: String,
    pub time: Timestamp,
    pub named: bool,
    pub pinned: bool,
    /// Completed without per-file errors.
    pub healthy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Pinned,
    Named,
    Newest,
    Last,
    Within,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Pinned => "pinned",
            Reason::Named => "named",
            Reason::Newest => "newest",
            Reason::Last => "last-n",
            Reason::Within => "recent",
            Reason::Hourly => "hourly",
            Reason::Daily => "daily",
            Reason::Weekly => "weekly",
            Reason::Monthly => "monthly",
            Reason::Yearly => "yearly",
        }
    }
}

/// Maps a local time to the id of its hour/day/week/month/year bucket.
type BucketKey = fn(&jiff::Zoned) -> i64;

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub id: String,
    pub time: Timestamp,
    pub keep: bool,
    /// Rules that keep this snapshot (empty when it is pruned).
    pub reasons: Vec<Reason>,
    /// True if `max_age` overrode bucket rules.
    pub expired: bool,
}

/// Decide for every snapshot. Output is newest first.
pub fn evaluate(
    snaps: &[SnapshotInfo],
    policy: &Retention,
    now: Timestamp,
    tz: &TimeZone,
) -> Vec<Decision> {
    let mut order: Vec<&SnapshotInfo> = snaps.iter().collect();
    order.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.id.cmp(&a.id)));

    let mut reasons: Vec<Vec<Reason>> = vec![Vec::new(); order.len()];
    for (i, s) in order.iter().enumerate() {
        if s.pinned {
            reasons[i].push(Reason::Pinned);
        }
        if s.named && policy.keep_named {
            reasons[i].push(Reason::Named);
        }
    }
    if !order.is_empty() {
        reasons[0].push(Reason::Newest);
    }

    if !policy.enabled {
        return order
            .iter()
            .zip(reasons)
            .map(|(s, r)| Decision {
                id: s.id.clone(),
                time: s.time,
                keep: true,
                reasons: r,
                expired: false,
            })
            .collect();
    }

    for r in reasons.iter_mut().take(policy.keep_last as usize) {
        r.push(Reason::Last);
    }
    if let Some(within) = policy.keep_within {
        let cutoff = now
            .saturating_sub(within.as_duration())
            .unwrap_or(Timestamp::MIN);
        for (i, s) in order.iter().enumerate() {
            if s.time >= cutoff {
                reasons[i].push(Reason::Within);
            }
        }
    }

    let buckets: [(u32, Reason, BucketKey); 5] = [
        (policy.keep_hourly, Reason::Hourly, |z| {
            i64::from(z.year()) * 1_000_000 + i64::from(z.day_of_year()) * 100 + i64::from(z.hour())
        }),
        (policy.keep_daily, Reason::Daily, |z| {
            i64::from(z.year()) * 1000 + i64::from(z.day_of_year())
        }),
        (policy.keep_weekly, Reason::Weekly, |z| {
            let w = z.date().iso_week_date();
            i64::from(w.year()) * 100 + i64::from(w.week())
        }),
        (policy.keep_monthly, Reason::Monthly, |z| {
            i64::from(z.year()) * 100 + i64::from(z.month())
        }),
        (policy.keep_yearly, Reason::Yearly, |z| i64::from(z.year())),
    ];
    let zoned: Vec<jiff::Zoned> = order.iter().map(|s| s.time.to_zoned(tz.clone())).collect();
    for (count, reason, key) in buckets {
        if count == 0 {
            continue;
        }
        // Group consecutive (time-ordered) snapshots by bucket; pick one per bucket.
        let mut seen = 0u32;
        let mut i = 0;
        while i < order.len() && seen < count {
            let k = key(&zoned[i]);
            let mut j = i;
            while j < order.len() && key(&zoned[j]) == k {
                j += 1;
            }
            let pick = (i..j).find(|&x| order[x].healthy).unwrap_or(i);
            reasons[pick].push(reason);
            seen += 1;
            i = j;
        }
    }

    let expiry = policy.max_age.map(|age| {
        now.saturating_sub(age.as_duration())
            .unwrap_or(Timestamp::MIN)
    });
    order
        .iter()
        .zip(reasons)
        .map(|(s, mut r)| {
            let mut expired = false;
            if let Some(cut) = expiry
                && s.time < cut
            {
                let protected = r
                    .iter()
                    .any(|x| matches!(x, Reason::Pinned | Reason::Named | Reason::Newest));
                if !protected && !r.is_empty() {
                    expired = true;
                    r.clear();
                }
            }
            r.sort();
            r.dedup();
            Decision {
                id: s.id.clone(),
                time: s.time,
                keep: !r.is_empty(),
                reasons: r,
                expired,
            }
        })
        .collect()
}

/// Snapshots that may be deleted to free space, oldest first: everything except pinned,
/// protected-named, and the newest. Ids in `already` are excluded.
pub fn space_candidates(
    snaps: &[SnapshotInfo],
    policy: &Retention,
    already: &HashSet<String>,
) -> Vec<String> {
    let newest = snaps
        .iter()
        .max_by(|a, b| a.time.cmp(&b.time).then_with(|| a.id.cmp(&b.id)))
        .map(|s| s.id.clone());
    let mut v: Vec<&SnapshotInfo> = snaps
        .iter()
        .filter(|s| !s.pinned && !(s.named && policy.keep_named))
        .filter(|s| Some(&s.id) != newest.as_ref() && !already.contains(&s.id))
        .collect();
    v.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.id.cmp(&b.id)));
    v.into_iter().map(|s| s.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::HumanDuration;
    use jiff::ToSpan;

    fn ago(now: Timestamp, hours: i64) -> Timestamp {
        now.checked_sub(hours.hours()).unwrap()
    }

    fn none() -> Retention {
        Retention {
            enabled: true,
            keep_last: 0,
            keep_within: None,
            keep_hourly: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: 0,
            max_age: None,
            keep_named: true,
            min_free_space: None,
            min_free_inodes_percent: None,
        }
    }

    fn snap(id: &str, t: Timestamp) -> SnapshotInfo {
        SnapshotInfo {
            id: id.into(),
            time: t,
            named: false,
            pinned: false,
            healthy: true,
        }
    }

    fn kept(d: &[Decision]) -> Vec<&str> {
        let mut v: Vec<&str> = d.iter().filter(|d| d.keep).map(|d| d.id.as_str()).collect();
        v.sort();
        v
    }

    fn now() -> Timestamp {
        "2026-06-15T12:00:00Z".parse().unwrap()
    }

    /// One snapshot every 6 hours for 400 days.
    fn series() -> Vec<SnapshotInfo> {
        (0..1600)
            .map(|i| snap(&format!("s{i:04}"), ago(now(), i * 6)))
            .collect()
    }

    #[test]
    fn newest_is_always_kept() {
        let d = evaluate(&series(), &none(), now(), &TimeZone::UTC);
        assert_eq!(kept(&d), ["s0000"]);
        assert_eq!(d[0].reasons, [Reason::Newest]);
    }

    #[test]
    fn disabled_keeps_everything() {
        let mut p = none();
        p.enabled = false;
        let d = evaluate(&series(), &p, now(), &TimeZone::UTC);
        assert!(d.iter().all(|d| d.keep));
    }

    #[test]
    fn last_and_within() {
        let mut p = none();
        p.keep_last = 3;
        p.keep_within = Some(HumanDuration::from_secs(25 * 3600)); // covers 0,6,12,18,24 h
        let d = evaluate(&series(), &p, now(), &TimeZone::UTC);
        assert_eq!(kept(&d), ["s0000", "s0001", "s0002", "s0003", "s0004"]);
    }

    #[test]
    fn buckets() {
        let mut p = none();
        p.keep_daily = 7;
        p.keep_weekly = 4;
        p.keep_monthly = 12;
        p.keep_yearly = 3;
        let d = evaluate(&series(), &p, now(), &TimeZone::UTC);
        let k = d.iter().filter(|d| d.keep).count();
        // 2026-06-15 is a Monday. 7 daily (Jun 9-15); weekly adds Jun 7 and May 31 (the
        // other two overlap daily); monthly adds Jul 2025-Apr 2026 (10); yearly adds nothing
        // (the series spans 2025-2026 only and both picks overlap). 7 + 2 + 10 = 19.
        assert_eq!(k, 19);
        // Daily picks are the newest snapshot of each of the 7 newest days.
        let daily: Vec<_> = d
            .iter()
            .filter(|d| d.reasons.contains(&Reason::Daily))
            .map(|d| d.time)
            .collect();
        assert_eq!(daily.len(), 7);
        assert!(daily.windows(2).all(|w| w[0] > w[1]));
        // 12 monthly picks from 12 distinct months.
        let months: HashSet<_> = d
            .iter()
            .filter(|d| d.reasons.contains(&Reason::Monthly))
            .map(|d| d.time.to_zoned(TimeZone::UTC).strftime("%Y-%m").to_string())
            .collect();
        assert_eq!(months.len(), 12);
    }

    #[test]
    fn prefers_healthy_in_bucket() {
        let mut p = none();
        p.keep_daily = 2;
        let mut s = vec![
            snap("today-new", ago(now(), 1)),
            snap("today-old", ago(now(), 2)),
            snap("yesterday-bad", ago(now(), 25)),
            snap("yesterday-good", ago(now(), 26)),
        ];
        s[2].healthy = false;
        let d = evaluate(&s, &p, now(), &TimeZone::UTC);
        assert_eq!(kept(&d), ["today-new", "yesterday-good"]);
    }

    #[test]
    fn named_pinned_and_max_age() {
        let mut p = none();
        p.keep_yearly = 10;
        p.max_age = Some(HumanDuration::from_secs(90 * 86_400));
        let mut s = series();
        s[1500].named = true;
        s[1400].pinned = true;
        let d = evaluate(&s, &p, now(), &TimeZone::UTC);
        let by_id = |id: &str| d.iter().find(|x| x.id == id).unwrap();
        assert!(by_id("s1500").keep && by_id("s1500").reasons.contains(&Reason::Named));
        assert!(by_id("s1400").keep);
        // Yearly picks older than 90 days are expired.
        assert!(d.iter().any(|x| x.expired && !x.keep));
        let oldest_kept = d
            .iter()
            .filter(|x| x.keep && x.reasons.iter().all(|r| *r == Reason::Yearly))
            .map(|x| x.time)
            .min();
        assert!(oldest_kept.is_none_or(|t| t >= ago(now(), 90 * 24)));

        p.keep_named = false;
        let d = evaluate(&s, &p, now(), &TimeZone::UTC);
        assert!(!d.iter().find(|x| x.id == "s1500").unwrap().keep);
    }

    #[test]
    fn space_candidate_order() {
        let mut s = vec![
            snap("a", ago(now(), 30)),
            snap("b", ago(now(), 20)),
            snap("c", ago(now(), 10)),
        ];
        s[0].pinned = true;
        let c = space_candidates(&s, &none(), &HashSet::new());
        assert_eq!(c, ["b"], "pinned and newest are never candidates");
    }
}
