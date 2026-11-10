//! Retention for compatibility-mode repositories: the same policy as every engine
//! (pins, names, buckets, free-space floors), applied by deleting snapshots with the tool.

use std::collections::HashSet;

use jiff::Timestamp;
use serde::Serialize;

use flux_core::profile::Retention;
use flux_core::progress::{Event, Reporter};
use flux_core::retention::{self, Decision, SnapshotInfo};
use flux_fs::snapshot::{SnapshotMeta, SnapshotStatus};

use crate::CompatError;

#[derive(Debug, Serialize)]
pub struct PruneReport {
    pub decisions: Vec<Decision>,
    pub removed: Vec<String>,
    pub removed_for_space: Vec<String>,
    pub space_floor_unmet: bool,
    pub dry_run: bool,
}

pub fn info(m: &SnapshotMeta) -> SnapshotInfo {
    SnapshotInfo {
        id: m.id.clone(),
        time: m.started,
        named: m.name.is_some(),
        pinned: m.pinned,
        healthy: m.status == SnapshotStatus::Complete,
    }
}

/// Free space and inodes meet the policy's floors (`free`: bytes and inode percentage).
pub fn floors_met(free: Option<(u64, f64)>, policy: &Retention) -> bool {
    let Some((bytes, inodes)) = free else {
        return true;
    };
    policy.min_free_space.is_none_or(|m| bytes >= m.bytes())
        && policy
            .min_free_inodes_percent
            .is_none_or(|m| inodes >= f64::from(m))
}

/// Apply `policy` to `snaps`: delete what it doesn't keep (oldest first), then more old
/// snapshots while the free-space floors aren't met. `free` reports the current free
/// space; `delete` removes one snapshot.
pub fn apply(
    snaps: &[SnapshotMeta],
    policy: &Retention,
    dry_run: bool,
    free: &dyn Fn() -> Option<(u64, f64)>,
    delete: &mut dyn FnMut(&str) -> Result<(), CompatError>,
    report: Reporter<'_>,
) -> Result<PruneReport, CompatError> {
    let infos: Vec<SnapshotInfo> = snaps.iter().map(info).collect();
    let decisions = retention::evaluate(
        &infos,
        policy,
        Timestamp::now(),
        &jiff::tz::TimeZone::system(),
    );
    let mut removed = Vec::new();
    for d in decisions.iter().rev().filter(|d| !d.keep) {
        if !dry_run {
            delete(&d.id)?;
            report(Event::Info {
                message: format!("pruned snapshot {}", d.id),
            });
        }
        removed.push(d.id.clone());
    }
    let mut removed_for_space = Vec::new();
    let mut space_floor_unmet = false;
    if policy.enabled
        && (policy.min_free_space.is_some() || policy.min_free_inodes_percent.is_some())
    {
        if dry_run {
            space_floor_unmet = !floors_met(free(), policy);
        } else {
            let gone: HashSet<String> = removed.iter().cloned().collect();
            let mut it = retention::space_candidates(&infos, policy, &gone).into_iter();
            while !floors_met(free(), policy) {
                let Some(id) = it.next() else {
                    space_floor_unmet = true;
                    break;
                };
                delete(&id)?;
                report(Event::Info {
                    message: format!("removed snapshot {id} to free space"),
                });
                removed_for_space.push(id);
            }
        }
    }
    Ok(PruneReport {
        decisions,
        removed,
        removed_for_space,
        space_floor_unmet,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use flux_core::units::ByteSize;

    fn snap(id: &str, hours_ago: i64) -> SnapshotMeta {
        let t = Timestamp::now() - jiff::SignedDuration::from_hours(hours_ago);
        SnapshotMeta {
            format: 1,
            id: id.into(),
            profile: "p".into(),
            host: "h".into(),
            user: "u".into(),
            started: t,
            finished: t,
            status: SnapshotStatus::Complete,
            name: None,
            pinned: false,
            sources: Vec::new(),
            stats: Default::default(),
            errors: Vec::new(),
            errors_total: 0,
            flux_version: "test".into(),
            sources_missing: Vec::new(),
            note: None,
            tags: Vec::new(),
        }
    }

    /// A free-space minimum that deleting can't reach takes what the rules leave, down to
    /// the safety floor and no further (it used to take all but the newest).
    #[test]
    fn a_space_floor_that_cannot_be_met_stops_at_the_safety_floor() {
        let snaps: Vec<SnapshotMeta> = (0..6).map(|i| snap(&format!("s{i}"), i * 2)).collect();
        let policy = Retention {
            min_free_space: Some(ByteSize(1 << 40)),
            safety_floor: 3,
            ..Retention::default()
        };
        let deleted = RefCell::new(Vec::new());
        let r = apply(
            &snaps,
            &policy,
            false,
            &|| Some((0, 100.0)),
            &mut |id| {
                deleted.borrow_mut().push(id.to_owned());
                Ok(())
            },
            &|_| {},
        )
        .unwrap();
        // Everything is within `keep_within`, so the rules remove nothing; for space, the
        // oldest go, the three newest complete ones stay.
        assert!(r.removed.is_empty());
        assert_eq!(r.removed_for_space, ["s5", "s4", "s3"]);
        assert_eq!(*deleted.borrow(), ["s5", "s4", "s3"]);
        assert!(r.space_floor_unmet);
    }

    /// Deleting stops as soon as there is room.
    #[test]
    fn space_pruning_stops_once_the_floor_is_met() {
        let snaps: Vec<SnapshotMeta> = (0..6).map(|i| snap(&format!("s{i}"), i * 2)).collect();
        let policy = Retention {
            min_free_space: Some(ByteSize(100)),
            ..Retention::default()
        };
        let deleted = RefCell::new(Vec::new());
        let r = apply(
            &snaps,
            &policy,
            false,
            &|| Some((deleted.borrow().len() as u64 * 60, 100.0)),
            &mut |id| {
                deleted.borrow_mut().push(id.to_owned());
                Ok(())
            },
            &|_| {},
        )
        .unwrap();
        assert_eq!(r.removed_for_space, ["s5", "s4"]);
        assert!(!r.space_floor_unmet);
    }
}
