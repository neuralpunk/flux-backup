//! Space: how much a profile would back up (estimated while it's being set up), and when
//! the backup drive fills up at the current rate (from the free space after each run).

use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;

use flux_core::profile::{ChangeDetection, Target};
use flux_core::progress::{Progress, silent};
use flux_runner::runlog::RunRecord;

use crate::{ApiError, Result, Service};

/// What a profile's sources and exclusions add up to now.
#[derive(Debug, Serialize)]
pub struct Estimate {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    /// Items that couldn't be read (permissions).
    pub unreadable: u64,
    /// Sources that don't exist.
    pub missing: Vec<String>,
    /// Free space where the backups go (local targets).
    pub free: Option<u64>,
}

/// Count what a profile (being edited, so passed whole) would back up. Only the newest
/// request runs to the end: starting another stops the previous one (`cancelled`).
pub fn estimate_impl(state: &Service, profile: serde_json::Value) -> Result<Estimate> {
    let mut p = crate::edit::parse(profile).map_err(|issues| {
        ApiError::new(
            "error",
            issues
                .into_iter()
                .next()
                .map_or_else(String::new, |i| i.message),
        )
    })?;
    if p.sources.paths.is_empty() {
        return Err(ApiError::new("error", "Choose what to back up first."));
    }
    // Counting only: no hashing, no extended attributes.
    p.options.change_detection = ChangeDetection::Metadata;
    p.options.preserve_acl = false;
    p.options.preserve_xattr = false;
    // A drive inside the sources isn't backed up into itself.
    let never = match &p.target {
        Target::Local { path, .. } if !path.as_os_str().is_empty() => vec![path.clone()],
        _ => Vec::new(),
    };

    let me = state.estimate_gen.fetch_add(1, SeqCst) + 1;
    let progress = Progress::new();
    let done = std::sync::atomic::AtomicBool::new(false);
    let scanned = std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(SeqCst) {
                if state.estimate_gen.load(SeqCst) != me {
                    progress.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let r = flux_fs::scan::thread_pool(&p)
            .map_err(|e| ApiError::new("error", e.to_string()))
            .and_then(|pool| {
                pool.install(|| flux_fs::scan::count_profile(&p, &never, &progress, &silent))
                    .map_err(|e| ApiError::new("error", e.to_string()))
            });
        done.store(true, SeqCst);
        r
    });
    if progress.is_cancelled() {
        return Err(ApiError::new(
            "cancelled",
            "a newer estimate replaced this one",
        ));
    }
    let scanned = scanned?;
    let c = scanned.counts;
    let out = Estimate {
        files: c.files,
        folders: c.folders,
        bytes: c.bytes,
        unreadable: scanned.errors.len() as u64,
        missing: scanned
            .missing_sources
            .iter()
            .map(|m| m.display().to_string())
            .collect(),
        free: flux_runner::free_space(&p.target),
    };
    Ok(out)
}

/// Free space at one moment.
#[derive(Debug, Clone, Serialize)]
pub struct SpacePoint {
    pub t: Timestamp,
    pub free: u64,
}

/// Free space where the backups go, its recent history, and where it's heading.
#[derive(Debug, Serialize)]
pub struct Space {
    pub free: u64,
    /// Size of the drive or filesystem.
    pub size: Option<u64>,
    /// Oldest first, ending with now.
    pub points: Vec<SpacePoint>,
    /// Change in free space per day over the history (negative: filling up). `None`
    /// without enough history.
    pub per_day: Option<i64>,
    /// Days until full at that rate, when filling up (and within ten years).
    pub days_left: Option<u32>,
}

/// History used for the forecast.
const HISTORY_DAYS: i64 = 90;

/// The forecast, from the free space recorded after recent runs (newest first, as
/// `recent_runs` returns them) and the free space now. A trend needs at least three
/// points over three days.
pub fn forecast(
    runs: &[RunRecord],
    free: Option<u64>,
    size: Option<u64>,
    now: Timestamp,
) -> Option<Space> {
    let free = free?;
    let since = now - jiff::SignedDuration::from_hours(24 * HISTORY_DAYS);
    let mut points: Vec<SpacePoint> = runs
        .iter()
        .filter(|r| r.finished >= since && r.finished < now)
        .filter_map(|r| {
            r.free_after.map(|f| SpacePoint {
                t: r.finished,
                free: f,
            })
        })
        .collect();
    points.reverse();
    points.push(SpacePoint { t: now, free });

    let day = |t: Timestamp| t.duration_since(now).as_secs_f64() / 86_400.0;
    let span = day(now) - day(points[0].t);
    let per_day = (points.len() >= 3 && span >= 3.0).then(|| {
        // Least squares: free = a + b·day.
        let n = points.len() as f64;
        let mx = points.iter().map(|p| day(p.t)).sum::<f64>() / n;
        let my = points.iter().map(|p| p.free as f64).sum::<f64>() / n;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for p in &points {
            let dx = day(p.t) - mx;
            sxy += dx * (p.free as f64 - my);
            sxx += dx * dx;
        }
        if sxx > 0.0 { sxy / sxx } else { 0.0 }
    });
    let days_left = per_day
        .filter(|&b| b < 0.0)
        .map(|b| free as f64 / -b)
        .filter(|&d| d < 3650.0)
        .map(|d| d.round() as u32);
    Some(Space {
        free,
        size,
        points,
        per_day: per_day.map(|b| b.round() as i64),
        days_left,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_runner::Trigger;
    use flux_runner::runlog::RunStatus;

    fn run(now: Timestamp, days_ago: i64, free: u64) -> RunRecord {
        let t = now - jiff::SignedDuration::from_hours(24 * days_ago);
        RunRecord {
            run_id: "r".into(),
            profile: "p".into(),
            trigger: Trigger::Scheduled,
            started: t,
            finished: t,
            status: RunStatus::Completed,
            snapshot: None,
            pruned: vec![],
            error: None,
            stats: None,
            warnings: 0,
            hooks: vec![],
            free_after: Some(free),
        }
    }

    #[test]
    fn forecasts() {
        let now = Timestamp::now();
        const G: u64 = 1 << 30;
        // Losing 2 GiB a day, 40 GiB left: about 20 days.
        let runs: Vec<_> = (1..=10)
            .map(|d| run(now, d, 40 * G + 2 * G * d as u64))
            .collect();
        let s = forecast(&runs, Some(40 * G), Some(500 * G), now).unwrap();
        assert_eq!(s.points.len(), 11);
        assert!(s.points[0].t < s.points[10].t, "oldest first");
        assert_eq!(s.per_day, Some(-2 * G as i64));
        assert_eq!(s.days_left, Some(20));

        // Steady (pruning keeps up): no date.
        let runs: Vec<_> = (1..=10).map(|d| run(now, d, 40 * G)).collect();
        let s = forecast(&runs, Some(40 * G), None, now).unwrap();
        assert_eq!(s.per_day, Some(0));
        assert_eq!(s.days_left, None);

        // Too little history for a trend.
        let runs = vec![run(now, 1, 41 * G)];
        let s = forecast(&runs, Some(40 * G), None, now).unwrap();
        assert_eq!(s.per_day, None);
        // Old runs and runs without a measurement don't count.
        let mut old = run(now, 200, 90 * G);
        old.free_after = Some(90 * G);
        let mut none = run(now, 2, 0);
        none.free_after = None;
        let s = forecast(&[none, old], Some(40 * G), None, now).unwrap();
        assert_eq!(s.points.len(), 1);
        assert!(forecast(&[], None, None, now).is_none());
    }
}
